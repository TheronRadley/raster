# Third-party notices

Raster is MIT licensed. The `third_party/` directory contains vendored source
snapshots used at build time. Their licenses are retained in each crate
directory:

- `ropey` v1.6.1 — MIT (`third_party/ropey/LICENSE.md`).
- `smallvec` v1.13.2 — MIT OR Apache-2.0 (`third_party/smallvec/LICENSE-*`).
- `str_indices` v0.4.4 — MIT OR Apache-2.0 (`third_party/str_indices/LICENSE-*`).
- `unicode-segmentation` v1.12.0 — MIT OR Apache-2.0 (`third_party/unicode-segmentation/LICENSE-*`).
- `unicode-width` v0.2.0 — MIT OR Apache-2.0 (`third_party/unicode-width/LICENSE-*`).
- `regex-lite` 0.1.9 — MIT OR Apache-2.0 (`third_party/regex-lite/LICENSE-*`).

The Unicode data used by `unicode-segmentation` and `unicode-width` is derived
from the Unicode Character Database under the Unicode license; upstream
notices are retained with those crates. No vendored code is loaded dynamically
at runtime, and no network download occurs during a normal build.
