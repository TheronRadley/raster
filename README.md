# Raster

Raster is a predictable, nonmodal terminal text editor written in Rust. It is
designed for everyday coding over SSH as well as locally: a quiet monochrome
UI, familiar shortcuts, safe saves, and optional coding features that never
block ordinary editing.

> **Status:** this branch contains a working editor implementation and a large
> test suite, but it is **not release-ready**. See
> [docs/LIMITATIONS.md](docs/LIMITATIONS.md) for exact gaps and the prioritized
> roadmap. Do not treat unimplemented or unverified items as shipped.

## First edit

```sh
cargo build --release --locked --offline
./target/release/raster notes.txt        # or just ./target/release/raster
# type text, then Ctrl+S to save and Ctrl+Q to quit
./target/release/raster --help           # CLI options and version
./target/release/raster --health         # config/state paths as JSON
```

With no path, Raster opens a small welcome buffer showing the essential
shortcuts. Type immediately; there is no modal command gate. `Ctrl+O` opens a
file prompt, `Ctrl+S` on a scratch buffer opens Save As, `F1` opens the full
in-editor shortcut help, and `Ctrl+Shift+P` opens a searchable command palette.
Dirty buffers ask for confirmation before quitting or reloading. Raster does
not modify your shell configuration.

A deterministic headless frame is available without a TTY:

```sh
./target/release/raster --no-config --no-session --no-lsp --no-mouse \
  --replay examples/replay.txt --dump-screen screen.txt
```

## Shortcut essentials

| Action | Keys |
| --- | --- |
| Save / quit / new / open | `Ctrl+S`, `Ctrl+Q`, `Ctrl+N`, `Ctrl+O` |
| Undo / redo | `Ctrl+Z`, `Ctrl+Y` or `Ctrl+Shift+Z` |
| Find / replace / next match | `Ctrl+F`, `Ctrl+R`, `F3`, `Shift+F3` |
| Files / project search / commands | `Ctrl+P`, `Ctrl+Shift+F`, `Ctrl+Shift+P` |
| Buffers / splits | `Ctrl+B`, `Ctrl+Tab`, `Ctrl+W v`, `Ctrl+W s`, `Ctrl+W h/j/k/l`, `Ctrl+W q` |
| Selection / navigation | arrows, `Shift`+arrows, `Home`, `End`, `Ctrl+Home`, `Ctrl+End`, `Ctrl+A` |
| Internal register | `Ctrl+C`, `Ctrl+X`, `Ctrl+V` |
| System clipboard (opt-in) | `Ctrl+Shift+C`, `Ctrl+Shift+X`, `Ctrl+Shift+V` |
| Help / cancel | `F1`, `Esc` |

`Ctrl+S` and `Ctrl+Q` require XON/XOFF flow control to be disabled; Raster does
that in raw mode. If a terminal or multiplexer consumes them, use `:w` and
`:q` in the palette or `:` command prompt. See
[docs/SHORTCUTS.md](docs/SHORTCUTS.md) for the full reference and terminal
ambiguities.

## Configuration

Configuration is TOML. `raster --health` prints the active config and state
paths. A commented, valid example is in
[examples/config.toml](examples/config.toml). Notable defaults:

- familiar `normal` profile; optional Vim-inspired and selection-first
  profiles are partial and documented as such;
- LSP disabled until the user explicitly installs and configures a server;
  Raster never auto-installs servers, runs project scripts, or enables
  project-local executable configuration;
- internal registers are always available; system clipboard and OSC 52 are
  off by default, and remote OSC 52 requires explicit `allow_remote`;
- private local recovery and session metadata (no document contents in the
  session file);
- LF, CRLF, mixed newlines, and a missing final newline are preserved unless
  the user explicitly requests conversion.

See [docs/CONFIG.md](docs/CONFIG.md) for all keys and validation rules.

## What is implemented

- Rope-backed shared buffers with exact transactions shared by typing,
  replacement, multicursor edits, format, undo, and redo; no full-file copy per
  keystroke.
- Independent views/splits, soft wrap, horizontal scrolling, visual movement,
  and edit transforms for selections, diagnostics, and viewport anchors.
- Multiple buffers, command palette, fuzzy files, project search, lexical
  syntax highlighting for Rust/C/C++/Go/Python, mouse reporting with disabled
  fallback, and a compact tab/status UI.
- Safe save/reopen, external-modification detection, read-only handling,
  interrupted-save recovery, private recovery files, and session restoration.
- Optional LSP client with negotiated UTF-8/UTF-16 position encoding, bounded
  reads, timeouts, stale-response rejection, crash/flood handling, and
  validated WorkspaceEdits. Basic editing continues if a server or grammar is
  unavailable.
- Lightweight Git branch/status/diff indicators; no destructive Git actions.
- Headless replay, deterministic mutation fuzzing, reproducible engine and
  frame benchmarks, mock-LSP tests, and a Linux PTY smoke test.

Full Arabic shaping and bidi rendering are **not** implemented. Storage,
grapheme boundaries, display widths, and cursor placement for Arabic and mixed
scripts are tested; see [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

## Checks and documentation

Local checks on Linux are reported in
[docs/PERFORMANCE.md](docs/PERFORMANCE.md) and
[docs/PLATFORMS.md](docs/PLATFORMS.md). CI for Linux, macOS, and Windows is
defined in [.github/workflows/ci.yml](.github/workflows/ci.yml) but had not run
in this sandbox at the time of writing. Termux is a separate validation task.

Other documents: [architecture](docs/ARCHITECTURE.md),
[shortcut reference](docs/SHORTCUTS.md), [configuration](docs/CONFIG.md),
[performance](docs/PERFORMANCE.md), [platforms](docs/PLATFORMS.md),
[limitations and roadmap](docs/LIMITATIONS.md), [packaging](docs/PACKAGING.md),
[contributing](CONTRIBUTING.md), and [security policy](SECURITY.md).

## License

MIT. The original repository license is preserved. Vendored dependency notices
are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
