<div align="center">
  <img src="assets/raster-logo.png" alt="Raster logo" width="480">
  <h1>Raster</h1>
  <p>A minimal command prompt and terminal text editor.</p>
  <p>
    <a href="https://github.com/TheronRadley/raster/actions/workflows/ci.yml"><img src="https://github.com/TheronRadley/raster/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
    <a href="https://github.com/TheronRadley/raster/releases/tag/windows-preview-brand-20261114"><img src="https://img.shields.io/badge/Windows%20x64-preview-555555" alt="Windows x64 preview"></a>
  </p>
</div>

## Download

**[Download the Windows x64 installer (.exe)](https://github.com/TheronRadley/raster/releases/download/windows-preview-brand-20261114/Raster-Setup-0.1.0-x64.exe)**

The installer targets 64-bit Windows 10 and 11 and installs per user. This is
an **unsigned preview**; SmartScreen may warn, and interactive use on Windows
10/11 desktops still needs testing. [Installation and build details](docs/PACKAGING.md).

## Get started

On Windows, open **Raster** from the Start menu for a clean command prompt.
Type `help` to list the built-in commands, or `edit notes.txt` to open a file in
the editor. Close the editor to return to the prompt. The **Raster Editor**
shortcut opens the editor directly.

From an existing terminal:

```sh
raster --shell        # command prompt; type help to list commands
raster notes.txt      # open a file in the editor
raster --help         # command-line options
```

Raster runs inside your terminal; it is not a terminal emulator. The prompt
passes other commands to the system shell. `cd` persists, but environment
changes made by external commands do not. See the [shell guide](docs/SHELL.md).

## Build from source

Install a Rust toolchain with Cargo (minimum declared version: 1.88). On
Windows, use the MSVC toolchain and Microsoft C++ Build Tools.

```sh
git clone https://github.com/TheronRadley/raster.git
cd raster
cargo build --release --locked --offline
```

The executable is `target/release/raster` on Unix and
`target\release\raster.exe` on Windows. Runtime Rust dependencies are vendored;
Cargo does not need registry access for this build. See
[packaging](docs/PACKAGING.md) for installer builds.

## Editor shortcuts

| Action | Keys |
| --- | --- |
| Save / quit | `Ctrl+S` / `Ctrl+Q` |
| Open / new buffer | `Ctrl+O` / `Ctrl+N` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y` |
| Find / replace | `Ctrl+F` / `Ctrl+R` |
| Files / commands | `Ctrl+P` / `Ctrl+Shift+P` |
| Help / cancel | `F1` / `Esc` |

See the [full shortcut reference](docs/SHORTCUTS.md) and
[configuration guide](docs/CONFIG.md). Language servers and system clipboard
integration are opt-in.

## Status

Raster is under development. Full Arabic shaping and bidirectional rendering,
real language-server testing, and interactive Windows, macOS, and Termux
validation remain open. See [known limitations](docs/LIMITATIONS.md) and the
[platform matrix](docs/PLATFORMS.md).

## Documentation

[Shell](docs/SHELL.md) · [Configuration](docs/CONFIG.md) ·
[Shortcuts](docs/SHORTCUTS.md) · [Architecture](docs/ARCHITECTURE.md) ·
[Packaging](docs/PACKAGING.md) · [Contributing](CONTRIBUTING.md) ·
[Security](SECURITY.md)

## License

[MIT](LICENSE). Third-party notices are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
