# Platform matrix

This file states which checks ran locally, which are defined in CI, and which
remain unverified. It does not claim compatibility from source inspection
alone.

| Platform | Local validation in this sandbox | CI definition | Status |
| --- | --- | --- | --- |
| Linux x86_64 | `cargo fmt`, `cargo clippy -D warnings`, debug/release tests, release build, `--help`/`--version`/`--health`, headless replay, Linux PTY edit/save/resize/cleanup, mock-LSP tests, property/reference-model tests, mutation fuzz, native C/C++/Python checks, startup/bench measurements. | Ubuntu job in [.github/workflows/ci.yml](../.github/workflows/ci.yml) runs fmt, clippy, native checks, debug/release tests, release build, fuzz target check, headless replay, PTY smoke with the C checksum library, and a small benchmark. | **Locally validated.** Hosted Ubuntu push/PR jobs passed at `e37b0f8`; see PR checks for the latest commit, which adds native checks. |
| macOS | Not run locally. | macOS job runs fmt, clippy, native checks, tests, release build, and headless replay. | **Not locally validated.** Hosted macOS push/PR jobs passed at `e37b0f8`; Unix `stty` raw mode, signal numbers, and suspend/resume still need device validation. |
| Windows | Not run locally. `#[cfg(windows)]` console FFI was fixed after the first hosted run. | Windows job enables MSVC and runs fmt, clippy, native checks, tests, release build, and headless replay. | **Not locally validated.** Hosted Windows push/PR jobs passed at `e37b0f8`; interactive console mode save/restore and resize behavior still need device validation. |
| Android / Termux | Not run. | No CI job; separate manual validation required. | **Unverified.** Linux desktop results do not prove Termux behavior. |

## Local environment

- Date: 2026-11-13 UTC.
- OS: `Linux e2b.local 6.1.158+ #1 SMP PREEMPT_DYNAMIC ... x86_64 GNU/Linux`.
- CPU: Intel(R) Xeon(R) Processor @ 2.60GHz, 2 visible CPUs.
- Memory: about 4 GiB total.
- Toolchains: Rust/Cargo 1.99.0 (primary); `cargo check` also passed with Rust
  1.95.0. The declared `rust-version` is 1.88; that exact toolchain was not
  installed locally.
- Sandbox-only build setting: `RUSTFLAGS='-C link-arg=-fuse-ld=bfd'` because
  the default linker path failed here. CI does not use this workaround.
- Hosted CI uses current stable Rust (1.100.0 on the first passing run); the
  local primary toolchain was 1.99.0. A Clippy 1.100 lint and Windows-specific
  compile/test failures were fixed on this branch. Hosted jobs are not executed
  inside this sandbox; see the PR checks for per-commit results.
- Terminal for interactive PTY test: `xterm-256color`, 100x30, resized to
  40x10 and back, with `NO_COLOR=1`. Headless tests used no TTY and `TERM=dumb`.

## Terminal, SSH, and tmux behavior

- Unix raw mode uses `stty` with `-ixon` so `Ctrl+S` and `Ctrl+Q` reach Raster.
  Some outer terminal or multiplexer layers can still consume those bytes;
  `:w` and `:q` are documented alternatives.
- Terminal size comes from `stty size` on Unix, Windows console APIs on
  Windows, or `COLUMNS`/`LINES` as a fallback. Resize is polled and also
  signaled by `SIGWINCH` on Unix (signal 28 on Linux/macOS/Android; not yet
  validated on macOS/Termux).
- Bracketed paste is decoded as text. Mouse reporting is optional and has a
  keyboard-only fallback.
- Styling is monochrome/restrained, respects `NO_COLOR`, and degrades to plain
  text in `--dump-screen`. Color capability is not probed deeply.
- The default minimum usable terminal is 24x6; smaller terminals show a clear
  message rather than corrupting layout. Additional splits are refused when
  the focused rectangle is too small.
- OSC 52 is off by default and remote use requires `clipboard.allow_remote`.
  System clipboard integration is also opt-in. Internal registers never leave
  the process.
- Terminal state is restored on normal exit, recoverable errors, and panics
  where possible. Forced termination (`SIGKILL`, power loss) cannot run
  cleanup. Suspension/resume is Unix-only; Windows reports it as unsupported.

## Termux validation plan (not yet performed)

1. On a Termux device: `pkg install rust` (or an equivalent current Rust
   toolchain), then `git clone` this branch.
2. `cargo build --release --locked --offline`.
3. Run `raster --help`, `raster --health`, and `raster --no-config --replay
   examples/replay.txt --dump-screen screen.txt`.
4. Launch interactively in Termux, exercise typing, `Ctrl+S`, `Ctrl+Q`,
   resize, and terminal restoration; record `TERM`, Android version, and any
   key-sequence differences.
5. Report results separately; do not infer Termux support from Ubuntu CI.
