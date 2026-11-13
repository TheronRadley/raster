# Contributing

Raster is a small, auditable Rust codebase. Contributions should keep ordinary
editing predictable and preserve data safety.

## Before changing code

1. Inspect `README.md`, `docs/ARCHITECTURE.md`, and `docs/LIMITATIONS.md`.
2. Keep the editing engine testable without a terminal. Put terminal behavior
   in `src/term/`, controller behavior in `src/app/editor.rs`, and pure text
   rules in `src/core/`.
3. Do not add a second implementation language without a specific technical
   need. Established Rust crates are acceptable when they materially improve
   correctness or maintenance; document licensing, platform support, and why
   the dependency is needed.
4. Do not introduce full-file copies on ordinary keystrokes, undo, redo, or
   render paths. Use rope clones and bounded chunks.
5. Keep expensive work off the interactive input path. Worker results must
   carry versions/generations and obsolete results must be rejected.
6. Never emit untrusted file content, filenames, diagnostics, or external
   output as raw terminal control sequences. Keep escape-sequence ownership in
   the renderer.
7. Do not silently normalize text. Preserve LF, CRLF, mixed line endings, and
   missing final newline; refuse invalid UTF-8/NUL bytes rather than replacing
   them. Do not silently reload a dirty buffer or place document contents in
   normal logs.
8. Do not auto-install language servers, execute project scripts, or enable
   project-local executable configuration. LSP WorkspaceEdits must validate
   versions and keep broad changes visible to the user.

## Local checks

From the repository root:

```sh
cargo fmt -p raster
cargo fmt -p raster -- --check
cargo clippy --offline -p raster --all-targets -- -D warnings
cargo test --offline --locked
cargo test --offline --locked --release
cargo build --release --offline --locked
cargo check --offline --locked --manifest-path fuzz/Cargo.toml
cargo run --offline --locked --bin raster-fuzz -- --iterations 100 --max-ops 40
```

In restricted sandboxes, the local Rust toolchain may need
`RUSTFLAGS='-C link-arg=-fuse-ld=bfd'`. CI does not use that workaround.

Useful extra validations:

```sh
cargo run --offline --locked --bin raster -- --no-config --health
cargo run --offline --locked --bin raster -- --no-config --no-session --no-lsp \
  --replay examples/replay.txt --dump-screen screen.txt
python3 scripts/pty_smoke.py --binary target/debug/raster --output pty-smoke.json  # Linux only
cargo run --offline --locked --bin raster-bench -- --size-mib 1 --iterations 20 --output bench.json
```

CI runs fmt, clippy, tests, release build, and headless replay on Linux,
macOS, and Windows; Linux additionally runs release tests, the fuzz-target
check, PTY smoke, and a small benchmark. See
[.github/workflows/ci.yml](.github/workflows/ci.yml).

## Tests to add with changes

- Text/storage: byte round-trip, Unicode/Arabic/combining/emoji/wide
  characters, CRLF/mixed newlines, missing final newline.
- Editing: exact transaction inverse, undo/redo text and cursors, multicursor
  rules, selection and diagnostic transformations, viewport independence.
- Filesystem: external modification, read-only, permission/full-disk failure,
  interrupted-save journal, symlink/hard-link preservation, private recovery.
- Terminal: key chords, bracketed paste, resize during prompts/splits,
  sanitized untrusted text, cleanup/restoration.
- Async: stale LSP/search responses, cancellation, crash/hang/flood handling.
- Property/reference-model tests for new edit transforms; deterministic fuzz
  cases for new random text fragments.
- Mark UI/profile features as partial when they are not complete; do not claim
  Vim/Helix compatibility.

## Documentation

Update `README.md`, `docs/CONFIG.md`, `docs/SHORTCUTS.md`,
`docs/PLATFORMS.md`, `docs/PERFORMANCE.md`, and `docs/LIMITATIONS.md` when
behavior changes. Label planned features clearly. Do not fabricate
screenshots, benchmark numbers, platform support, release assets, or
compatibility claims.

## Pull requests

Work on the current session branch. Keep commits focused, include the exact
checks you ran, and state which platforms remain unverified. Do not publish
releases or change remote settings without explicit authorization.
