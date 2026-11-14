# Packaging and installation

No release has been published. Windows x64 installer builds are available as
unsigned CI artifacts on successful runs; they are not signed release assets.
Source builds remain available on all platforms.

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

## Windows installer (x64)

The Inno Setup installer targets **64-bit Windows 10 and 11**. By default it
installs per-user into `%LOCALAPPDATA%\Programs\Raster` without administrator
rights, adds Start menu shortcuts, and provides an uninstaller. It does **not**
modify PATH, register file associations, or delete editor configuration,
sessions, or recovery data on uninstall. The installed app is a terminal
editor, not a graphical window. Start it from the Start menu or run
`& "$env:LOCALAPPDATA\Programs\Raster\raster.exe"` in PowerShell.

To build the installer on x64 Windows, install Rust's MSVC toolchain, Microsoft
C++ Build Tools and [Inno Setup 6](https://jrsoftware.org/isinfo.php), then run:

```powershell
.\packaging\windows\build-installer.ps1
```

The script compiles only the editor for `x86_64-pc-windows-msvc`, then produces
`target\installer\Raster-Setup-<version>-x64.exe`. If Inno Setup is installed
elsewhere, supply `-InnoCompiler "C:\path\to\ISCC.exe"`. No .NET, Python,
C/C++ helper library, or Rust toolchain is required on the machine running the
installer. The build itself requires the Rust/MSVC and Inno tools above.

Windows CI builds the installer and tests silent installation, upgrade,
`--version`, `--health`, and uninstallation, including preservation of an
untracked file. Download `raster-windows-x64-installer` from a successful
GitHub Actions run; artifacts may expire and may require GitHub sign-in. The
executable is **unsigned**, so Windows SmartScreen may warn. Verify its source
before running it; do not assume it is an official signed release. CI uses
Windows Server, not Windows 10 or 11 desktops. Interactive use and installer
behavior on actual Windows 10/11 devices, accessibility, ARM64 emulation, and
code signing still require validation. 32-bit Windows is not supported by this
installer.

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
benchmark/PTY/native-check/replay artifacts where available. These are CI build
outputs, not signed or published releases. On Windows it also uploads the
`raster-windows-x64-installer` artifact. The workflow runs in hosted GitHub
Actions, not inside this sandbox; see the PR checks for current results.

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
