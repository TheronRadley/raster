# Native verification helpers

The Raster editing engine stays Rust. This directory contains a small,
deliberately bounded C/C++ addition used by verification tooling:

- `raster_checksum.h` / `raster_checksum.c` — a C99 shared library exporting a
  stable C ABI (`raster_fnv1a64`, `raster_checksum_abi_version`). It computes the
  same FNV-1a 64-bit digest used by the Linux PTY saved-byte check.
- `raster_checksum_cli.cpp` — a C++17 command-line tool that links the C ABI,
  hashes a file, and prints a zero-padded lowercase hex digest. It is useful for
  manual verification without Python.
- `../scripts/native_checks.py` — Python builds the two artifacts with the
  platform compiler, loads the C library with `ctypes`, and checks the C
  library, the C++ CLI, a pure-Python reference, and the committed PTY artifact
  agree. No network or third-party packages are needed.

Build and check locally:

```sh
python3 scripts/native_checks.py --build-dir target/native --output native-checks.json
```

On Windows, run the script after an MSVC developer command prompt has put
`cl.exe` on `PATH`; CI does this with `ilammy/msvc-dev-cmd`. On Linux and macOS,
the script uses `cc`/`c++` (or GCC/Clang equivalents). Generated binaries live
under `target/native/`, which is ignored by Git.

The C ABI is versioned. Any change to the exported function signatures must
bump `raster_checksum_abi_version()` and update the Python/C++ callers together.
