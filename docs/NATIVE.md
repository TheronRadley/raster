# Language boundaries and native checks

Raster's interactive editor is written in Rust. The C, C++, and Python code in
this repository is deliberately limited to verification tooling; it is not
linked into the editor binary and does not handle user documents at runtime.
This keeps the editing path auditable while using each language where it has a
clear advantage.

| Language | Role | Why it is here |
| --- | --- | --- |
| Rust | Editor core, terminal controller, renderer, LSP client, saves, tests, benchmarks | Memory-safe data structures, one audited runtime, offline vendored dependencies |
| C99 | `native/raster_checksum.c` shared library with a versioned C ABI | A tiny, stable byte-level checksum ABI that Python and C++ can call without duplicating the algorithm |
| C++17 | `native/raster_checksum_cli.cpp` command-line file hasher | Exercises C/C++ interop and provides a standalone manual verification tool |
| Python 3 | `scripts/pty_smoke.py`, `scripts/native_checks.py` | PTY orchestration, compiler discovery, `ctypes` ABI validation, and cross-checking committed artifacts |

The C ABI is version 2: the original one-shot function remains available, and
`raster_fnv1a64_init` / `raster_fnv1a64_update` support incremental hashing.
The C++ CLI reads fixed-size chunks instead of loading an entire file, checks
for I/O errors, and has memory use independent of file size.

The checksum is FNV-1a 64-bit, the same non-cryptographic digest used by the
Linux PTY smoke test to prove that saved bytes are exact. It is a verification
aid, **not** a security primitive and not a tamper-proof hash. See
[native/README.md](../native/README.md) for the ABI and build details.

## What is checked

`python3 scripts/native_checks.py --build-dir target/native --output native-checks.json`:

1. Builds the C shared library and C++ CLI with the platform compiler (`cc`/`c++`
   on Linux/macOS, MSVC `cl.exe` on Windows; CI enables the MSVC environment).
2. Loads the C library with Python `ctypes` and checks the exported ABI version.
3. Compares the C library against a pure-Python FNV-1a reference for empty,
   ASCII, Unicode, and exact PTY saved-byte inputs, and tests streaming updates.
4. Runs the C++ CLI on temporary files, including inputs around the 64 KiB
   chunk boundary, and compares its digest to Python and C. It also tests error
   exits for missing files and invalid usage.
5. Cross-checks `docs/performance/pty_smoke.json` against the C library.

`scripts/pty_smoke.py` can optionally use the C library with `--native-lib`;
without that flag it falls back to its pure-Python reference. CI passes the
library on Linux. Generated native binaries and reports live under `target/` or
in CI artifact files and are not committed.

## Platform notes

- Linux: `cc`/`c++` build a `.so`; CI uses it for the PTY smoke test.
- macOS: `cc`/`c++` build a `.dylib`; `DYLD_LIBRARY_PATH` is set only for the
  test CLI invocation.
- Windows: `ilammy/msvc-dev-cmd` puts `cl.exe` on `PATH` after the Rust steps;
  the script builds a DLL and import library and sets `PATH` for the test CLI
  invocation. Enabling MSVC earlier lets Git Bash's `/usr/bin/link.exe` shadow
  Cargo's MSVC linker, so CI deliberately orders the native step last.
- Termux/Android: not validated. The Rust editor core and Python scripts are
  the supported local path; native C/C++ checks are not part of that validation
  yet.

The native checks do not measure editor performance, do not test real language
servers, and do not replace the Rust test suite. They verify that the small
cross-language helper is built and behaves identically on each hosted CI
platform.
