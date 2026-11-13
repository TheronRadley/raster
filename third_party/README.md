# Vendored dependencies

This directory contains trimmed, pinned source snapshots of six permissively
licensed Rust crates. They are ordinary Cargo path dependencies, not a second
implementation language and not a separate workspace crate set. Vendoring keeps
`cargo build --offline` reproducible in restricted environments and lets CI
build without a package registry.

| Crate | Pin | License | Why it is here |
| --- | --- | --- | --- |
| `ropey` | `v1.6.1` | MIT | Balanced rope: logarithmic edits, cheap clones, chunked save/recovery. |
| `smallvec` | `v1.13.2` | MIT OR Apache-2.0 | Required by ropey. |
| `str_indices` | `v0.4.4` | MIT OR Apache-2.0 | Byte/char/line index conversion for ropey. |
| `unicode-segmentation` | `v1.12.0` | MIT OR Apache-2.0 | UAX #29 grapheme boundaries. |
| `unicode-width` | `v0.2.0` | MIT OR Apache-2.0 | Terminal display columns. |
| `regex-lite` | `0.1.9` | MIT OR Apache-2.0 | Bounded-time regex matching with no transitive dependencies. |

Only `src/`, manifests, READMEs, and license files were retained. Manifests
were adjusted to use local paths and to drop dev-only dependencies and
workspace inheritance; a lint allow for newer Rust lifetime-syntax warnings was
added at the top of vendored `lib.rs` files. No algorithmic source changes were
made. See `THIRD_PARTY_NOTICES.md` at the repository root.

`regex-lite` matches Unicode scalar values but does not provide Unicode-aware
case folding or `\p{...}` classes. Raster documents this limitation. Literal
search, storage, editing boundaries, and display widths do not depend on it.
