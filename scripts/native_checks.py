#!/usr/bin/env python3
"""Build and validate Raster's C/C++/Python verification helpers.

Rust remains the editor implementation. This script compiles a small C99 shared
library and a C++17 CLI, then uses Python/ctypes to prove that the C ABI, the
C++ CLI, a pure-Python reference, and the committed PTY artifact agree. It is
used by CI on Linux, macOS, and Windows and needs no third-party packages.
"""
import argparse
import ctypes
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / "native"
C_SOURCE = NATIVE / "raster_checksum.c"
CPP_SOURCE = NATIVE / "raster_checksum_cli.cpp"
EXPECTED_PTY_BYTES = "abcPasted مرحبا 😀start\n".encode("utf-8")


def py_fnv1a(data):
    value = 0xCBF29CE484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


def first_tool(names):
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


def run_command(command):
    print("+ " + " ".join(command), file=sys.stderr)
    subprocess.run(command, check=True)


def compiler_plan(build):
    system = platform.system()
    if system == "Windows":
        compiler = first_tool(["cl", "clang-cl"])
        if not compiler:
            raise RuntimeError(
                "MSVC cl.exe or clang-cl.exe is required on Windows; "
                "run ilammy/msvc-dev-cmd first"
            )
        return {
            "kind": "msvc",
            "c_compiler": compiler,
            "cpp_compiler": compiler,
            "library": build / "raster_checksum.dll",
            "import_library": build / "raster_checksum.lib",
            "cli": build / "raster-checksum.exe",
        }

    c_compiler = first_tool(["cc", "gcc", "clang"])
    cpp_compiler = first_tool(["c++", "g++", "clang++"])
    if not c_compiler or not cpp_compiler:
        raise RuntimeError("a C99 compiler and a C++17 compiler are required")
    library_name = (
        "libraster_checksum.dylib" if system == "Darwin" else "libraster_checksum.so"
    )
    return {
        "kind": "unix",
        "c_compiler": c_compiler,
        "cpp_compiler": cpp_compiler,
        "library": build / library_name,
        "cli": build / "raster-checksum",
    }


def build_artifacts(plan, build):
    source_include = str(NATIVE)
    if plan["kind"] == "msvc":
        c_object = build / "raster_checksum.obj"
        cpp_object = build / "raster_checksum_cli.obj"
        run_command([
            plan["c_compiler"],
            "/nologo",
            "/W3",
            "/WX",
            "/O2",
            "/DRASTER_CHECKSUM_BUILD",
            "/LD",
            f"/I{source_include}",
            f"/Fe:{plan['library']}",
            f"/Fo:{c_object}",
            str(C_SOURCE),
            "/link",
            f"/IMPLIB:{plan['import_library']}",
        ])
        run_command([
            plan["cpp_compiler"],
            "/nologo",
            "/W3",
            "/WX",
            "/std:c++17",
            "/EHsc",
            "/O2",
            f"/I{source_include}",
            f"/Fe:{plan['cli']}",
            f"/Fo:{cpp_object}",
            str(CPP_SOURCE),
            str(plan["import_library"]),
        ])
        return

    shared_flag = "-dynamiclib" if platform.system() == "Darwin" else "-shared"
    c_flags = [
        "-std=c99",
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fPIC",
        "-DRASTER_CHECKSUM_BUILD",
        f"-I{source_include}",
    ]
    cpp_flags = [
        "-std=c++17",
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        f"-I{source_include}",
    ]
    run_command([
        plan["c_compiler"],
        *c_flags,
        shared_flag,
        "-o",
        str(plan["library"]),
        str(C_SOURCE),
    ])
    run_command([
        plan["cpp_compiler"],
        *cpp_flags,
        "-o",
        str(plan["cli"]),
        str(CPP_SOURCE),
        f"-L{build}",
        "-lraster_checksum",
    ])


def load_native(plan):
    library = ctypes.CDLL(str(plan["library"]))
    checksum = library.raster_fnv1a64
    checksum.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    checksum.restype = ctypes.c_uint64
    init = library.raster_fnv1a64_init
    init.argtypes = []
    init.restype = ctypes.c_uint64
    update = library.raster_fnv1a64_update
    update.argtypes = [ctypes.c_uint64, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    update.restype = ctypes.c_uint64
    abi = library.raster_checksum_abi_version
    abi.argtypes = []
    abi.restype = ctypes.c_uint32

    def native_hash(data):
        if not data:
            return int(checksum(None, 0))
        buffer = (ctypes.c_uint8 * len(data)).from_buffer_copy(data)
        return int(checksum(buffer, len(data)))

    def native_stream(chunks):
        state = init()
        for chunk in chunks:
            if chunk:
                buffer = (ctypes.c_uint8 * len(chunk)).from_buffer_copy(chunk)
                state = update(state, buffer, len(chunk))
            else:
                state = update(state, None, 0)
        return int(state)

    return native_hash, native_stream, int(abi())


def cli_env(build):
    env = os.environ.copy()
    separator = os.pathsep
    for key in ("PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
        env[key] = str(build) + separator + env.get(key, "")
    return env


def cli_digest(plan, build, data):
    with tempfile.TemporaryDirectory(prefix="raster-native-") as temp:
        sample = Path(temp) / "sample.bin"
        sample.write_bytes(data)
        output = subprocess.check_output(
            [str(plan["cli"]), str(sample)],
            env=cli_env(build),
            text=True,
        ).strip()
        return int(output, 16)


def artifact_hash_checks(native_hash):
    path = ROOT / "docs" / "performance" / "pty_smoke.json"
    if not path.is_file():
        return False, False
    report = json.loads(path.read_text(encoding="utf-8"))
    expected = report.get("expected_fnv1a")
    saved = report.get("saved_fnv1a")
    reference = py_fnv1a(EXPECTED_PTY_BYTES)
    native = native_hash(EXPECTED_PTY_BYTES)
    return True, expected == saved == reference == native


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--build-dir", default="target/native")
    parser.add_argument("--output")
    args = parser.parse_args()

    build = Path(args.build_dir)
    if not build.is_absolute():
        build = ROOT / build
    build.mkdir(parents=True, exist_ok=True)

    plan = compiler_plan(build)
    build_artifacts(plan, build)
    native_hash, native_stream, abi_version = load_native(plan)

    vectors = {
        "empty": b"",
        "ascii": b"abc",
        "unicode": "مرحبا 😀\n".encode("utf-8"),
        "pty_saved_bytes": EXPECTED_PTY_BYTES,
    }
    checks = {"c_abi_version": abi_version == 2}
    for name, data in vectors.items():
        checks[f"c_matches_python_{name}"] = native_hash(data) == py_fnv1a(data)

    empty_digest = cli_digest(plan, build, b"")
    sample_digest = cli_digest(plan, build, EXPECTED_PTY_BYTES)
    checks["cpp_cli_matches_python_empty"] = empty_digest == py_fnv1a(b"")
    checks["cpp_cli_matches_python_sample"] = sample_digest == py_fnv1a(EXPECTED_PTY_BYTES)
    checks["cpp_cli_matches_c_sample"] = sample_digest == native_hash(EXPECTED_PTY_BYTES)

    # Exercise incremental updates, including zero-length chunks and the CLI's
    # exact 64 KiB boundary and multiple reads.
    for size in (0, 1, 65535, 65536, 65537, 131072, 131073):
        data = bytes(range(256)) * (size // 256) + bytes(range(size % 256))
        reference = py_fnv1a(data)
        chunks = [b"", data[:17], b"", data[17:65536], data[65536:], b""]
        checks[f"c_stream_{size}"] = native_stream(chunks) == reference == native_hash(data)
        checks[f"cpp_cli_stream_{size}"] = cli_digest(plan, build, data) == reference

    with tempfile.TemporaryDirectory(prefix="raster-native-errors-") as temp:
        missing = Path(temp) / "missing.bin"
        result = subprocess.run([str(plan["cli"]), str(missing)], env=cli_env(build), capture_output=True)
        checks["cpp_cli_missing_file_fails"] = result.returncode != 0 and not result.stdout
        result = subprocess.run([str(plan["cli"])], env=cli_env(build), capture_output=True)
        checks["cpp_cli_invalid_usage_fails"] = result.returncode == 2 and not result.stdout

    artifact_present, artifact_matches = artifact_hash_checks(native_hash)
    checks["pty_artifact_present"] = artifact_present
    checks["pty_artifact_hashes_match_c"] = artifact_matches

    report = {
        "tool": "scripts/native_checks.py",
        "platform": platform.platform(),
        "python": platform.python_version(),
        "compiler": {
            "kind": plan["kind"],
            "c": plan["c_compiler"],
            "cpp": plan["cpp_compiler"],
        },
        "library": str(plan["library"].relative_to(ROOT)),
        "cli": str(plan["cli"].relative_to(ROOT)),
        "checks": checks,
        "passed": all(checks.values()),
    }
    text = json.dumps(report, sort_keys=True, indent=2) + "\n"
    if args.output:
        output_path = Path(args.output)
        if not output_path.is_absolute():
            output_path = ROOT / output_path
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(text, encoding="utf-8")
    sys.stdout.write(text)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
