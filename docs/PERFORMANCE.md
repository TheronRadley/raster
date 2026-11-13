# Measured performance

Measurements were taken on 2026-11-13 UTC in this sandbox. Raw JSON artifacts
are committed under [docs/performance/](performance/). They are reproducible
with the commands below. This report separates editor processing from frame
construction and editor resources from LSP resources; it does **not** claim
terminal-emulator or real-server LSP performance.

## Environment and method

- OS: `Linux e2b.local 6.1.158+ x86_64 GNU/Linux`.
- CPU: Intel(R) Xeon(R) Processor @ 2.60GHz, 2 visible CPUs.
- Memory: about 4 GiB total.
- Rust: `rustc 1.99.0 (b940084d7 2026-09-28)`; `cargo check` also passed with
  Rust 1.95.0. Declared `rust-version = "1.88"` was not installed locally.
- Build: `cargo build --release --locked --offline`; release profile uses thin
  LTO, one codegen unit, `strip = true`, and unwinding panics.
- Sandbox-only linker flag: `RUSTFLAGS='-C link-arg=-fuse-ld=bfd'`.
- Terminal for the PTY measurement: `xterm-256color`, 100x30 then 40x10 then
  100x30, `NO_COLOR=1`. Benchmarks/headless runs used `TERM=xterm-256color`
  and no TTY. No SSH or tmux was active.
- Fixture: deterministic UTF-8 generated in memory with seed
  `0x526173746572`, exact target byte length, ASCII code-like lines with LF and
  CRLF terminators, Arabic, combining marks, emoji, and CJK/wide text.
- `raster-bench` measures: fixture generation, document load, insert/undo/redo
  cycles, navigation (`next_grapheme`, `prev_grapheme`, line lookup, display
  info, line/col), bounded literal search (`needle`, max 1000 results),
  visible-line syntax spans, and pure 80x24 frame construction. No terminal
  writes, LSP, Git, recovery, or network work is included in those timings.
- `--bench-startup` measures headless `Editor` construction plus one 80x24
  frame, seven iterations; no file I/O, terminal, LSP, Git, or recovery.

Reproduce:

```sh
export PATH="$HOME/.local/rust199/bin:$PATH"
export TERM=xterm-256color
cargo build --release --locked --offline
./target/release/raster --no-config --bench-startup
./target/release/raster-bench --size-mib 1 --iterations 120 --seed 0x526173746572 --output bench-1mib.json
./target/release/raster-bench --size-mib 10 --iterations 20 --seed 0x526173746572 --output bench-10mib.json
./target/release/raster-bench --size-mib 100 --iterations 3 --seed 0x526173746572 --output bench-100mib.json
./target/release/raster-bench --size-mib 100 --iterations 3 --seed 0x526173746572 --large-file --output bench-100mib-large-mode.json
python3 scripts/pty_smoke.py --binary target/release/raster --output pty-smoke.json
```

## Provisional targets

| Target | Result | Assessment |
| --- | --- | --- |
| Empty release startup under 100 ms | median **0.579 ms** (min 0.523 ms, max 0.754 ms, 7 iterations) | **Met** locally. This is headless construction, not process/`exec` or terminal attach time. |
| Idle editor RSS under 50 MiB without LSP | **3,888 KiB** (about 3.80 MiB) from the interactive PTY smoke process | **Met** locally. LSP was disabled; no server RSS included. |
| p95 ordinary edit/navigation under 16 ms on a 1 MiB fixture | edit cycle p95 **21.636 µs**, navigation p95 **40.038 µs**, pure frame p95 **0.581 ms** | **Met** locally. |
| Comfortable around 10 MiB | edit p95 **13.569 µs**, navigation p95 **34.542 µs**, search p95 **4.144 ms**, frame p95 **0.507 ms** | **Met** locally. |
| Deliberate ~100 MiB large-file mode | load **293.240 ms**, edit p95 **63.697 µs**, frame p95 **0.679 ms**, benchmark-process RSS **224,220 KiB** | **Partially measured.** The reduced-feature mode was enabled with `--large-file`; full CLI large-file workflow and long-session memory behavior were not end-to-end validated. |

## Engine, search, syntax, and frame results

Times are from the committed JSON files. `µs` = microseconds, `ms` =
milliseconds. `search` is bounded literal search within one document, not a
project-wide recursive grep. `frame` is pure frame construction, not writing
bytes to a terminal.

| Fixture | Mode | Load ms | Edit p50/p95 µs | Navigation p50/p95 µs | Search p95 ms | Visible syntax p95 ms | Frame p50/p95 ms | Process RSS KiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 MiB, 120 iterations | normal | 3.212 | 9.551 / 21.636 | 21.531 / 40.038 | 3.775 | 0.061 | 0.559 / 0.581 | 6,056 |
| 10 MiB, 20 iterations | normal | 27.797 | 10.318 / 13.569 | 20.398 / 34.542 | 4.144 | 0.043 | 0.465 / 0.507 | 25,748 |
| 100 MiB, 3 iterations | normal | 295.860 | 15.024 / 65.415 | 27.563 / 51.158 | 3.699 | 0.061 | 0.555 / 0.649 | 224,228 |
| 100 MiB, 3 iterations | `--large-file` | 293.240 | 12.678 / 63.697 | 23.486 / 43.419 | 4.155 | 0.063 | 0.543 / 0.679 | 224,220 |

The 100 MiB benchmark-process RSS includes the generated `String` fixture, the
rope, snapshot clones, allocator retention, and worker state. It is **not**
editor idle RSS. The JSON files explicitly record
`editor_idle_rss_without_lsp_kib: null` and `lsp_rss_kib: null` for the
benchmark; idle RSS was measured separately by the PTY smoke test. LSP memory
was not measured because no real language server was started.

## Fuzz and startup artifacts

- `startup.json`: median **0.579170 ms**, release build, 7 iterations.
- `fuzz.json`: **500** deterministic cases, **50,000** operations, **0**
  failures against a `String` reference model.
- `pty_smoke.json`: all interactive checks passed, idle RSS **3,888 KiB**,
  terminal flags restored, cleanup sequences present, exact save content
  verified by byte length and FNV-1a hash (no document contents in the report).

## Major architectural fix during measurement

An earlier 10 MiB frame measurement had p95 about **37.6 ms**. Two whole-file
operations were on the render path: `Buffer::dirty()` recomputed a document
fingerprint per frame, and the syntax worker rebuilt `Document` from rope bytes
(which scans for newline style) on each request. Dirty state is now tracked by
a monotonic history position, and syntax requests send a cheap
`Document::clone()` that preserves metadata. The current 10 MiB frame p95 is
**0.507 ms**. This was a structural fix, not a micro-optimization.

## Misses, exclusions, and unverified items

- LSP resource usage, real `rust-analyzer`/`clangd`/`gopls`/Python server
  latency, and grammar-package behavior are **not measured**. Mock-LSP
  correctness tests passed locally.
- Terminal output, terminal-emulator rendering, SSH latency, and tmux overhead
  are **not measured**; frame construction is in-memory only.
- Startup is headless editor construction, not `exec`, dynamic linking, or
  terminal attach. The 100 ms target is a product-level provisional target, so
  this result is necessary but not sufficient.
- The 100 MiB large-mode number is a controlled benchmark with syntax requests
  reduced, not a full user workflow with recovery, project scan, LSP, Git, and
  many buffers.
- macOS, Windows, and Termux performance are unverified; CI will run a small
  1 MiB benchmark on hosted runners but does not gate on these numbers.
- Benchmark results vary with allocator state and host load. The committed
  JSON files record the exact run; do not substitute rounded table values for
  raw data.
