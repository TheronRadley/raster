# Architecture

Raster is a single Rust crate with a library (`src/lib.rs`) and four binaries.
The editing engine is testable without a terminal: `Editor` accepts parsed
`Event`s and renders into a pure `Frame`; `src/main.rs` owns the real terminal,
input thread, and process loop.

## Module boundaries

| Area | Modules | Responsibility |
| --- | --- | --- |
| Text storage and edit transactions | `core/document.rs`, `core/transaction.rs`, `core/history.rs` | Rope-backed text, byte/char/grapheme/display conversions, one transaction path, exact inverse edits and selection restoration. |
| Selections, cursors, navigation | `core/selection.rs`, `app/view.rs`, `app/layout.rs` | Selection normalization and multicursor rules; independent viewport state and deterministic edit transforms for view anchors. |
| Buffers and file metadata | `app/buffer.rs`, `fs.rs` | Shared document + disk state, dirty tracking, save/save-as/reload, external modification checks, atomic/journaled writes, recovery job creation. |
| Views, splits, layout | `app/view.rs`, `app/split.rs`, `app/layout.rs` | Multiple views share a buffer by id; each view owns scroll, wrap, and cursor state. Splits form a binary tree with bounded rectangles. |
| Commands and keybindings | `core/commands.rs`, `term/input.rs`, `config.rs` | One command vocabulary, chord parser, palette/`:` commands, configurable key overrides. |
| Terminal input/rendering | `term/input.rs`, `term/renderer.rs`, `term/terminal.rs` | Key/paste/mouse/focus decoding, pure frame construction, sanitized ANSI/plain output, raw-mode setup and cleanup. |
| Filesystem/recovery/session | `fs.rs`, `session.rs`, `util.rs` | Strict UTF-8 load, newline preservation, private local recovery, session metadata without document contents. |
| Search, syntax, language services | `core/search.rs`, `core/syntax.rs`, `core/project.rs`, `app/search_worker.rs`, `lsp.rs`, `git.rs` | Bounded literal/regex search, lexical syntax worker, ignore-aware project workers, optional LSP client, read-only Git indicators. |
| Configuration | `config.rs`, `cli.rs` | Strict TOML subset parser, CLI overrides, key map construction, path resolution. |

## Why a rope

`ropey` 1.6.1 is a balanced rope with logarithmic edits, cheap shared clones,
and chunked iteration. A piece table was considered, but the rope already
provides the needed properties without a second index implementation, and its
`Rope::clone` is a small constant-size clone of a shared tree rather than a
full-file copy. Ordinary typing, undo, redo, syntax requests, recovery jobs,
and save snapshots use cheap rope clones. Display, save, and recovery operate
on bounded chunks; no keystroke copies the whole document. `Document::clone()`
preserves version and newline metadata, so expensive whole-document newline
detection stays off the input and render paths.

## Coordinate systems

Raster deliberately distinguishes:

- `BytePos`: absolute UTF-8 byte offsets used by transactions, selections, and
  diagnostics;
- `GraphemeIndex`: UAX #29 grapheme boundaries for cursor movement;
- `DisplayColumn`: terminal cells after tab expansion and wide/combining
  character width;
- `LineIndex` and `LineCol`: logical line numbers and display columns.

`Document` confines ropey's character indices to its module. LSP positions are
converted to and from bytes using the negotiated UTF-8 or UTF-16 encoding; UTF-32
server capability is rejected explicitly rather than guessed.

## Transactions and history

Every mutation goes through `Transaction`: typing, replacement, multicursor
edits, formatting, undo, and redo. A transaction produces forward and inverse
edits only; the inverse edits rebuild prior text and explicit after-selection
positions. `History` stores inverse transactions, never document copies, and
exposes a monotonic `position()` so `Buffer` can track clean/dirty state in
O(1) without hashing the whole document on every frame. Undo/redo restore exact
text and cursors; returning to the last saved history point clears the dirty
indicator.

`transform_pos` and explicit edit-coordinate mapping transform selections,
diagnostic ranges, and viewport anchors after each transaction. Multiple views
sharing one document therefore keep independent viewport state.

## Expensive work and async results

Syntax highlighting, project scanning/project search, buffer search, Git
status, and recovery writing run on bounded worker channels. Requests carry
document versions and search generations; obsolete results are rejected or
reconciled rather than applied. Project search can be canceled. The LSP client
uses a bounded reader queue, request timeouts, stale-version checks, and
explicit shutdown; a hung, crashed, or flooding server must not stop ordinary
editing. Diagnostics are versioned and their byte ranges are transformed after
edits.

## Terminal safety and data safety

- The renderer owns every escape sequence. File contents, filenames,
  diagnostics, and external output are sanitized before reaching display text;
  control and bidi override characters are replaced or escaped.
- Bracketed paste is decoded as text, not as commands.
- Raw mode is set with `stty` on Unix and console APIs on Windows; Ctrl+S and
  Ctrl+Q flow control is disabled on Unix. A panic hook and `Drop` restore
  terminal state on normal exit, recoverable errors, and panics where possible.
  Forced termination cannot run cleanup. Suspend/resume is Unix-only.
- Saves use a temporary file, flush, directory sync where available, rename,
  and a journal for interrupted-save recovery. Permissions, symlink/hard-link
  targets, and original read bytes are respected. Permission and full-disk
  failures leave the original intact.
- External changes never silently reload a dirty buffer. Invalid UTF-8 and NUL
  bytes are refused rather than replaced. Recovery files are private (`0600`)
  inside a private state directory (`0700`); session JSON stores paths and
  viewport metadata, not document contents.
- System clipboard and OSC 52 are distinct from internal registers and are off
  by default. Remote OSC 52 requires explicit opt-in.

## Dependencies

Six permissively licensed crates are vendored as Cargo path dependencies under
`third_party/` to keep offline builds reproducible; see
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) and
[third_party/README.md](../third_party/README.md).

- `ropey` + `smallvec` + `str_indices`: balanced rope and index conversion.
- `unicode-segmentation`: UAX #29 grapheme boundaries.
- `unicode-width`: display columns for wide/combining characters.
- `regex-lite`: bounded-time regex search with no transitive dependencies. It
  lacks Unicode case folding and `\p{...}` classes; literal search and data
  integrity do not depend on it.

No heavy terminal, TOML, JSON, or LSP framework is used: the input decoder,
strict config/JSON subsets, renderer, and JSON-RPC client are small local
modules. This keeps platform behavior auditable and avoids pulling in
unreviewed runtime behavior.

## Explicitly out of scope for this branch

Full Arabic shaping/bidi rendering and selection, complete Vim/Helix
compatibility, a debugger, an embedded terminal, destructive Git operations,
real-server LSP integration tests, release assets, and validated
macOS/Windows/Termux support are not delivered here. They are documented in
[LIMITATIONS.md](LIMITATIONS.md).
