#!/usr/bin/env python3
"""Exercise prompt -> help -> editor -> prompt in a Linux pseudo-terminal."""
import os
import pty
import shutil
import signal
import sys
import tempfile
import time

from pty_smoke import read_available, set_winsize, wait_for_text


def main(binary):
    work = tempfile.mkdtemp(prefix="raster-shell-")
    pid = master = None
    reaped = False
    try:
        path = os.path.join(work, "sample.txt")
        with open(path, "w", encoding="utf-8") as output:
            output.write("hello from editor\n")
        env = dict(os.environ, HOME=work, XDG_CONFIG_HOME=os.path.join(work, "config"),
                   XDG_STATE_HOME=os.path.join(work, "state"), TERM="xterm-256color")
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(work)
            os.execvpe(binary, [binary, "--shell"], env)
        os.set_blocking(master, False)
        set_winsize(master, 24, 80)
        captured = bytearray()
        assert wait_for_text(master, captured, "> ", 8), "initial prompt missing"
        assert b"\x1b[" not in captured, "prompt must not draw editor UI"
        captured.clear()
        os.write(master, b"help\n")
        assert wait_for_text(master, captured, "Raster commands:", 5), "help missing"
        assert wait_for_text(master, captured, "> ", 5), "prompt did not return after help"
        captured.clear()
        os.write(master, b"edit sample.txt\n")
        assert wait_for_text(master, captured, "hello from editor", 8), "editor did not open"
        captured.clear()
        os.write(master, b"\x11")  # Ctrl+Q
        assert wait_for_text(master, captured, "> ", 8), "prompt did not return after editor"
        os.write(master, b"exit\n")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            read_available(master, captured, 0.05)
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                reaped = True
                assert os.waitstatus_to_exitcode(status) == 0, "prompt exited with an error"
                print("interactive shell/editor smoke passed")
                return
        raise RuntimeError("prompt did not exit")
    finally:
        if pid and not reaped:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
        if master is not None:
            os.close(master)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main(os.path.abspath(sys.argv[1]))
