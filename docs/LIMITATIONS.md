# Known limitations and prioritized roadmap

Raster on this branch is a working editor, **not a finished release**. Stage 4
release hardening is incomplete. This file lists exact gaps; nothing here is
implied to be shipped.

## Release blockers / correctness gaps

1. **Full Arabic shaping and bidi rendering are not implemented.** Exact byte
   storage, round-trip, grapheme boundaries, display widths, cursor placement,
   combining marks, emoji, wide characters, LF/CRLF/mixed newlines, and missing
   final newline are tested. The renderer does not perform contextual Arabic
   shaping or full Unicode bidi reordering; bidi control characters are
   sanitized rather than shaping text. Selection and visual movement across
   shaped/bidi text are therefore incomplete. Raster does not manually reverse
   strings or equate UTF-8 storage with complete Arabic support.
2. **macOS, Windows, and Android/Termux are not locally validated.** Linux PTY,
   headless, unit, property, mock-LSP, fuzz, benchmark, formatter, linter, and
   release builds passed locally. Hosted Linux/macOS/Windows CI in
   [.github/workflows/ci.yml](../.github/workflows/ci.yml) has run on this PR;
   all three OS jobs passed at commit `e37b0f8` after a Clippy 1.100 lint and
   Windows-specific compile/test fixes. The later commit adds C/C++/Python
   native checks; see the PR checks for its result. Windows console FFI and
   macOS Unix signal numbers are compiled/tested in CI but not exercised on a
   local device. Termux requires separate device validation.
3. **Real language servers are not integration-tested.** The LSP client passes
   mock-server tests for initialize, diagnostics, completion, hover,
   definition/references/rename/format/code actions, UTF-8/UTF-16 encodings,
   stale responses, hangs, crashes, flooding, and shutdown. It has not been
   validated against `rust-analyzer`, `clangd`, `gopls`, or a Python server.
4. **WorkspaceEdit review needs more real-world validation.** Versions and
   paths are validated, broad/multi-file edits require user visibility and
   `trust_workspace_edits = false` by default, and server commands are not
   executed automatically. More tests with real servers and multiple open
   buffers are needed before calling this workflow release-grade.

## Deliberate scope limits

- Optional `vim` and `selection-first` profiles are **partial**. The Vim subset
  covers `i`, `a`, `v`, `h/j/k/l`, `x`, `u`, `dd`, `yy`, and `:`; selection-first
  extends movement selection. They are not complete Vim or Helix
  compatibility.
- Git integration is read-only indicators: branch, changed files, and diff
  lines. No commit, stage, checkout, reset, or other destructive action.
- Debugger integration and an embedded terminal are **planned later**, not
  implemented.
- `regex-lite` does not provide Unicode-aware case folding or `\p{...}`
  property classes. Literal search, storage, and editing boundaries do not
  depend on it.
- Syntax highlighting is lexical and built-in for Rust, C, C++, Go, and
  Python. Tree-sitter grammars, grammar downloads, and grammar-package
  management are not implemented; a missing grammar never blocks editing.
- Large-file mode reduces optional work (notably syntax requests and LSP) above
  the configured threshold. A 100 MiB benchmark with `--large-file` passed, but
  a full CLI large-file editing/save/recovery workflow was not exercised.
- System clipboard integration depends on external programs (`pbcopy`/
  `pbpaste`, PowerShell, `wl-copy`/`wl-paste`, or `xclip`); failures are
  reported. OSC 52 is capped and off by default, and remote use requires
  `allow_remote`. Internal registers are always available.
- Session JSON stores paths and viewport metadata, not document contents.
  Recovery files are private on Unix (`0600` in a `0700` directory). On Windows,
  `std::fs::Permissions` exposes only `readonly()`, so the code relies on the OS
  default ACL and the tests do not claim Unix permission-bit equivalence. CI
  compiles and tests the Windows path; ACL equivalence remains unverified.
- Forced termination (`SIGKILL`, power loss) cannot restore terminal state.
  Normal exit, recoverable errors, and panics use a cleanup hook where
  possible. Suspend/resume is Unix-only.
- Some terminal chords are inherently ambiguous: `Ctrl+S`/`Ctrl+Q` can be
  consumed by a multiplexer or SSH layer, and `Ctrl+Tab`/modified arrows vary
  by terminal. `:w`, `:q`, `:bn`, `:bp`, and palette commands are documented
  alternatives.
- No release assets, distro packages, Homebrew formula, Scoop manifest, or
  published crates.io release exist. CI uploads build artifacts only; no
  release was published.

## Verification state

- **Local Linux, passed:** `cargo fmt -p raster -- --check`; `cargo clippy
  --offline -p raster --all-targets -- -D warnings`; debug and release
  `cargo test --offline --locked`; `cargo build --release --offline --locked`;
  `cargo check --offline --manifest-path fuzz/Cargo.toml`; CLI help/version/
  health; headless replay; Linux PTY edit/save/resize/cleanup; mock-LSP,
  property/reference-model, terminal layout, filesystem, and session tests;
  50,000 deterministic fuzz operations; reproducible benchmarks.
- **Local, partially checked:** `cargo check` with Rust 1.95.0 passed; the
  declared MSRV 1.88 exact toolchain was not installed.
- **Hosted CI, run on this PR:** Linux/macOS/Windows fmt, clippy, tests,
  release build, headless replay; Linux-only release tests, fuzz-target check,
  PTY smoke, and small benchmark. All six push/PR checks passed at commit
  `e37b0f8`; a later commit adds native C/C++/Python checks on all three OSes.
  See the PR checks for the current head.
- **Unverified:** local macOS/Windows interactive behavior, Termux, real LSP
  servers, terminal-emulator rendering cost, SSH/tmux latency, Arabic
  shaping/bidi, Windows ACL equivalence, release packaging installs.

## Prioritized next improvements

1. Keep hosted Linux/macOS/Windows CI green including native checks, and
   perform separate Termux device validation.
2. Add real-server LSP integration tests (explicitly installed by the test
   harness) and harden multi-file WorkspaceEdit review before release.
3. Decide and implement full Arabic shaping/bidi rendering and selection, or
   keep it an explicit permanent limitation with expanded display/selection
   tests. Do not claim complete support until this is solved.
4. Exercise large-file mode end-to-end (open, edit, save, recovery, resize,
   quit) and measure long-session memory separately from benchmark fixtures.
5. Complete optional profile behavior tables and tests, then document exactly
   what is supported; keep debugger/embedded terminal as later milestones.
6. Add packaging automation and installation tests for each supported
   platform, then prepare a release only after the above blockers are closed.

See [PERFORMANCE.md](PERFORMANCE.md) for measured results and misses, and
[PLATFORMS.md](PLATFORMS.md) for the platform matrix.
