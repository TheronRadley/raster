#!/usr/bin/env python3
"""Linux PTY smoke test for Raster's interactive editing loop.

It validates raw-mode setup/cleanup, ordinary typing, exact undo/redo,
bracketed paste, help overlay, resize, save, quit, and idle RSS. It never
prints document contents; only booleans and measured process memory.

With --native-lib it uses the C checksum ABI built by scripts/native_checks.py;
otherwise it falls back to an equivalent pure-Python reference.
"""
import argparse
import ctypes
import errno
import fcntl
import json
import os
import pty
import select
import shutil
import signal
import struct
import sys
import tempfile
import termios
import time


def set_winsize(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def fnv1a(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return value


def native_fnv1a(library_path):
    """Load the C ABI from scripts/native_checks.py's shared library."""
    library = ctypes.CDLL(os.path.abspath(library_path))
    checksum = library.raster_fnv1a64
    checksum.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    checksum.restype = ctypes.c_uint64
    abi = library.raster_checksum_abi_version
    abi.argtypes = []
    abi.restype = ctypes.c_uint32
    if abi() != 2:
        raise RuntimeError("unsupported raster checksum ABI version")

    def digest(data):
        if not data:
            return int(checksum(None, 0))
        buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data)
        return int(checksum(buffer, len(data)))

    return digest


def rss_kib(pid):
    try:
        with open(f"/proc/{pid}/status", "r", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1])
    except OSError:
        pass
    return None


def read_available(fd, output, timeout=0.05):
    end = time.monotonic() + timeout
    while True:
        remaining = end - time.monotonic()
        if remaining <= 0:
            return
        ready, _, _ = select.select([fd], [], [], remaining)
        if not ready:
            return
        try:
            chunk = os.read(fd, 65536)
        except OSError as exc:
            if exc.errno == errno.EIO:
                return
            raise
        if not chunk:
            return
        output.extend(chunk)


def wait_for_text(fd, output, marker, timeout=5.0):
    if isinstance(marker, str):
        marker = marker.encode()
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        read_available(fd, output, 0.05)
        if marker in bytes(output):
            return True
    return False


def run(binary, output_path, keep_work=False, native_lib=None):
    work = tempfile.mkdtemp(prefix="raster-pty-")
    checks = {}
    hash_fn = native_fnv1a(native_lib) if native_lib else fnv1a
    hash_backend = "c" if native_lib else "python"
    idle_rss = None
    exit_code = None
    saved_bytes = None
    saved_hash = None
    # A CLI-opened file starts its caret at line 1, column 1. Typing and
    # bracketed paste therefore precede the original text exactly.
    expected_bytes = "abcPasted مرحبا 😀start\n".encode("utf-8")
    expected_hash = hash_fn(expected_bytes)
    initial_lflag = None
    final_lflag = None
    pid = master = None
    captured = bytearray()
    try:
        sample = os.path.join(work, "sample.txt")
        with open(sample, "wb") as handle:
            handle.write("start\n".encode("utf-8"))
        env = dict(os.environ)
        env.update({
            "HOME": work,
            "XDG_CONFIG_HOME": os.path.join(work, "config"),
            "XDG_STATE_HOME": os.path.join(work, "state"),
            "TERM": "xterm-256color",
            "COLUMNS": "100",
            "LINES": "30",
            "NO_COLOR": "1",
        })
        args = [binary, "--no-config", "--no-session", "--no-lsp",
                "--no-mouse", "--no-color", "--no-alt-screen", sample]
        pid, master = pty.fork()
        if pid == 0:
            try:
                os.execvpe(args[0], args, env)
            except Exception:
                os._exit(127)
        os.set_blocking(master, False)
        set_winsize(master, 30, 100)
        initial_lflag = termios.tcgetattr(master)[3]
        checks["first_frame"] = wait_for_text(master, captured, b"sample.txt", 8.0)
        if not checks["first_frame"]:
            raise RuntimeError("first frame did not appear")
        idle_rss = rss_kib(pid)
        # Split a UTF-8 character across reads, then remove it. Exact saved
        # bytes below catch either corruption or a missed backspace.
        os.write(master, b"\xc3")
        time.sleep(0.02)
        os.write(master, b"\xa9")
        time.sleep(0.15)
        os.write(master, b"\x7f")
        time.sleep(0.15)
        # Ordinary typing + exact undo/redo.
        os.write(master, b"abc")
        time.sleep(0.15)
        os.write(master, b"\x1a")  # Ctrl+Z
        time.sleep(0.15)
        os.write(master, b"\x19")  # Ctrl+Y
        time.sleep(0.15)
        # Bracketed paste is inserted as text, not interpreted as commands.
        os.write(master, "\x1b[200~Pasted مرحبا 😀".encode("utf-8"))
        # The closing marker may be split across terminal reads.
        os.write(master, b"\x1b[20")
        time.sleep(0.15)
        os.write(master, b"1~")
        checks["paste_frame"] = wait_for_text(master, captured, "Pasted".encode("utf-8"), 5.0)
        # Help overlay and dismiss; then resize while still in a dialog.
        os.write(master, b"\x1bOP")  # F1
        checks["help_overlay"] = wait_for_text(master, captured, b"Raster help", 5.0)
        set_winsize(master, 10, 40)
        try:
            os.kill(pid, signal.SIGWINCH)
        except ProcessLookupError:
            pass
        time.sleep(0.25)
        os.write(master, b"\x1b")  # Escape
        time.sleep(0.25)
        set_winsize(master, 30, 100)
        try:
            os.kill(pid, signal.SIGWINCH)
        except ProcessLookupError:
            pass
        checks["resize_frame"] = wait_for_text(master, captured, b"F1 help", 5.0)
        # Save (Ctrl+S) and quit (Ctrl+Q). Raw mode disables XON/XOFF so
        # Ctrl+S reaches the editor instead of freezing the terminal.
        os.write(master, b"\x13")
        deadline = time.monotonic() + 8.0
        checks["saved_exact"] = False
        while time.monotonic() < deadline:
            read_available(master, captured, 0.05)
            try:
                with open(sample, "rb") as handle:
                    data = handle.read()
                saved_bytes = len(data)
                saved_hash = hash_fn(data)
                if data == expected_bytes:
                    checks["saved_exact"] = True
                    break
            except OSError:
                pass
        os.write(master, b"\x11")
        deadline = time.monotonic() + 8.0
        while time.monotonic() < deadline:
            read_available(master, captured, 0.05)
            done, status = os.waitpid(pid, os.WNOHANG)
            if done == pid:
                exit_code = os.waitstatus_to_exitcode(status)
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("editor did not quit")
        # Give the terminal restore writes a chance to arrive.
        read_available(master, captured, 0.3)
        final_lflag = termios.tcgetattr(master)[3]
        checks["terminal_flags_restored"] = initial_lflag == final_lflag
        checks["cleanup_sequences"] = (b"\x1b[?2004l" in captured and b"\x1b[?25h" in captured)
        checks["exit_clean"] = exit_code == 0
        checks["idle_rss_under_50_mib"] = idle_rss is not None and idle_rss < 50 * 1024
        checks["no_untrusted_raw_escape"] = b"\x1b[31m" not in captured and b"\x1b]52" not in captured
    except Exception as exc:
        checks["error"] = str(exc)
        if pid:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
    finally:
        if master is not None:
            try:
                os.close(master)
            except OSError:
                pass
        if not keep_work:
            shutil.rmtree(work, ignore_errors=True)
    report = {
        "tool": "scripts/pty_smoke.py",
        "binary": os.path.basename(binary),
        "hash_backend": hash_backend,
        "platform": "Linux PTY (xterm-256color, 100x30 then 40x10 then 100x30)",
        "idle_rss_kib": idle_rss,
        "expected_bytes": len(expected_bytes),
        "saved_bytes": saved_bytes,
        "expected_fnv1a": expected_hash,
        "saved_fnv1a": saved_hash,
        "work_dir": work if keep_work else None,
        "exit_code": exit_code,
        "terminal_lflag_before": initial_lflag,
        "terminal_lflag_after": final_lflag,
        "checks": checks,
        "passed": checks.get("error") is None and all(
            value for key, value in checks.items() if key not in {"error"}
        ),
    }
    text = json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n"
    if output_path:
        with open(output_path, "w", encoding="utf-8") as handle:
            handle.write(text)
    sys.stdout.write(text)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/raster")
    parser.add_argument("--output")
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument(
        "--native-lib",
        help="optional C shared library built by scripts/native_checks.py",
    )
    args = parser.parse_args()
    return run(
        os.path.abspath(args.binary),
        args.output,
        args.keep_work,
        args.native_lib,
    )


if __name__ == "__main__":
    raise SystemExit(main())
