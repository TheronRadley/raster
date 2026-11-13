# Packaging and installation

No release has been published and no prebuilt release assets exist yet. The
reproducible source build is the supported installation path on this branch.

## Prerequisites

- A current Rust toolchain with Cargo. `Cargo.toml` declares
  `rust-version = "1.88"`; local checks used Rust 1.99.0 and `cargo check`
  also passed with 1.95.0. The exact 1.88 toolchain was not validated here.
- A C linker and standard build tools. In this sandbox only, the default
  linker path failed, so builds used
  `RUSTFLAGS='-C link-arg=-fuse-ld=bfd'`. Normal CI runners do not need that
  workaround.
- No crates.io access is required: all runtime dependencies are vendored as
  Cargo path dependencies under `third_party/`.

## Build from source

```sh
git clone https://github.com/TheronRadley/raster.git
cd raster
cargo build --release --locked --offline
./target/release/raster --version
./target/release/raster --health
```

The release build produces:

| Binary | Purpose |
| --- | --- |
| `raster` | The editor. |
| `raster-bench` | Reproducible engine/frame benchmark. |
| `raster-fuzz` | Deterministic mutation fuzz harness. |
| `mock-lsp` | Test-only JSON-RPC server used by integration tests. |

On Windows, each binary has a `.exe` suffix.

## Install with Cargo

Install only the editor:

```sh
cargo install --path . --locked --offline --bin raster
```

Optional developer tools:

```sh
cargo install --path . --locked --offline --bin raster-bench
cargo install --path . --locked --offline --bin raster-fuzz
```

`mock-lsp` is test-only and should not normally be installed. `cargo install`
writes into Cargo's bin directory; it does not alter shell startup files. Add
that directory to `PATH` yourself if it is not already present.

## Manual install

Copy `target/release/raster` (or `raster.exe`) to a directory already on
`PATH`, for example `/usr/local/bin/raster` on Unix. Keep the MIT license and
third-party notices with any redistributed binary. A minimal redistribution
should include:

- `raster`;
- `LICENSE`;
- `THIRD_PARTY_NOTICES.md`;
- `README.md` and, ideally, the `docs/` directory.

## CI artifacts

[.github/workflows/ci.yml](../.github/workflows/ci.yml) builds release binaries
on Linux, macOS, and Windows and uploads them as workflow artifacts named
`raster-binaries-<runner OS>`. It also uploads generated health/startup/
benchmark/PTY/replay artifacts where available. These are CI build outputs,
not signed or published releases. CI had not run in this sandbox at the time of
writing.

## Cross-compilation and Termux

Cross-compilation is not validated locally. In principle, add the target with
`rustup target add <target>` and build with
`cargo build --release --locked --offline --target <target>`, but the Windows
console and macOS/Termux behavior must be confirmed on those platforms before
calling a binary supported.

Termux is a separate validation task, not part of Ubuntu CI. Suggested manual
steps (not yet performed):

```sh
pkg install rust
git clone https://github.com/TheronRadley/raster.git
cd raster
cargo build --release --locked --offline
./target/release/raster --help
```

Record the Android/Termux version, `TERM`, and any key-sequence differences in
[PLATFORMS.md](PLATFORMS.md) after testing. Do not infer Termux support from a
Linux desktop build.

## Not yet automated

- Signed release archives or checksums.
- crates.io publication.
- Debian/Fedora/Arch packages, Homebrew, Scoop, or winget manifests.
- Installation smoke tests on real macOS, Windows, and Termux systems.

These are release-hardening tasks, not current deliverables.
