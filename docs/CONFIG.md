# Configuration

Raster uses a strict, small TOML subset parser. Configuration is optional:
defaults work on first launch. `raster --health` prints the resolved config
path, state path, and whether the config is valid. A complete, commented and
tested example is [examples/config.toml](../examples/config.toml).

## Paths

The config file is resolved in this order:

1. `--config PATH`;
2. `RASTER_CONFIG_DIR/config.toml` when `RASTER_CONFIG_DIR` is set;
3. `%APPDATA%/raster/config.toml` on Windows;
4. `$XDG_CONFIG_HOME/raster/config.toml`, otherwise `$HOME/.config/raster/config.toml`.

State (recovery and session metadata) uses:

1. `RASTER_STATE_DIR` when set;
2. `%LOCALAPPDATA%/raster` on Windows;
3. `$XDG_STATE_HOME/raster`, otherwise `$HOME/.local/state/raster`.

`--no-config` ignores any user config file. CLI flags override config values
for that process only. `NO_COLOR=1` also disables styling. Raster does not edit
shell startup files.

## Supported TOML subset

The local parser supports `[section]`, `[[array.of.tables]]`, `key = value`,
strings, integers, booleans, arrays of strings, comments, and quoted keys such
as `"ctrl+s"`. Unknown sections and keys are rejected with a line/column error
instead of being ignored. This is not a full TOML 1.0 implementation: dotted
key forms, multiline strings, datetimes, and heterogeneous arrays are not
supported.

## Keys

Defaults match `Config::default()` in `src/config.rs`.

### `[editor]`

| Key | Default | Meaning |
| --- | --- | --- |
| `tab_width` | `4` | Display/indent width, validated `1..=16`. |
| `soft_wrap` | `true` | Wrap long lines into visual rows; horizontal scroll is used when off. |
| `auto_indent` | `true` | Preserve indentation on Enter. |
| `profile` | `"normal"` | `"normal"`, `"vim"`, or `"selection-first"`. Optional profiles are partial. |
| `large_file_threshold` | `10485760` | Byte threshold for reduced-feature large-file mode; `0` forces it. |
| `cjk_wide` | `false` | Treat East Asian wide characters as two cells. |
| `newline` | `"preserve"` | `"preserve"`, `"lf"`, or `"crlf"` for explicit save conversion. |

### `[ui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `color` | `"auto"` | `"auto"`, `"never"`, `"always"`; `NO_COLOR` and `--no-color` override. |
| `mouse` | `true` | SGR mouse reporting; `--no-mouse` disables it. |
| `tab_strip` | `true` | Compact tab strip. |
| `line_numbers` | `"absolute"` | `"off"`, `"absolute"`, `"relative"`. |
| `min_columns` | `24` | Minimum usable columns; must be at least 8. |
| `min_rows` | `6` | Minimum usable rows; must be at least 3. |
| `alt_screen` | `true` | Use the terminal alternate screen; `--no-alt-screen` disables it. |

### `[clipboard]`

| Key | Default | Meaning |
| --- | --- | --- |
| `system` | `false` | Enable platform system clipboard integration. |
| `osc52` | `false` | Enable OSC 52 clipboard escape sequences. |
| `osc52_max_bytes` | `65536` | Maximum OSC 52 payload, validated `1..=1000000`. |
| `allow_remote` | `false` | Allow system clipboard/OSC 52 behavior in detected SSH/remote sessions. |

Internal registers (`Ctrl+C`, `Ctrl+X`, `Ctrl+V`) are always available and
never leave the process. System clipboard and OSC 52 require explicit opt-in;
remote sessions additionally require `allow_remote`. The renderer sanitizes
clipboard payloads and caps OSC 52 output.

### `[search]`

| Key | Default | Meaning |
| --- | --- | --- |
| `hidden` | `false` | Include hidden files in project scans. |
| `ignored` | `false` | Include files ignored by `.gitignore`-style rules. |
| `max_results` | `500` | Result cap, validated `1..=100000`. |
| `context_lines` | `0` | Context rows around results. |

`regex-lite` does not provide Unicode-aware case folding or Unicode property
classes. Literal search and exact byte round-trips do not depend on it.

### `[session]`

| Key | Default | Meaning |
| --- | --- | --- |
| `restore` | `true` | Restore open files and viewport metadata on startup. `--no-session` disables it. |
| `recovery` | `true` | Periodically write private recovery snapshots for dirty buffers. |
| `recovery_interval_ms` | `1000` | Minimum 100 ms. |
| `recovery_retention_days` | `7` | Expired recovery data can be removed with `--clean-recovery`. |

Recovery files are local and private. Session JSON contains paths and view
state, not document contents. Dirty buffers are never silently reloaded.

### `[lsp]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | Start configured servers for matching languages. `--no-lsp` disables it. |
| `request_timeout_ms` | `5000` | Validated `100..=120000`. |
| `trust_workspace_edits` | `false` | When false, multi-file/broad WorkspaceEdits require review. |
| `servers` | `[]` | Array of explicit server tables; see below. |

Each `[[lsp.servers]]` table requires:

```toml
[[lsp.servers]]
language = "rust"          # rust, c, cpp, go, or python
command = "rust-analyzer" # already installed by the user
args = []
root_markers = ["Cargo.toml"]
```

Raster never installs or downloads a language server, executes project scripts,
or reads project-local executable configuration. Servers are started only from
this user configuration. Position encoding is negotiated; UTF-8 and UTF-16 are
supported, UTF-32 is rejected. Requests are versioned and stale responses are
discarded. A missing, hanging, crashed, or flooding server must not stop basic
editing.

### `[git]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Read-only branch/status/diff indicators. |
| `refresh_ms` | `5000` | Background refresh interval. |

Raster performs no destructive Git actions.

### `[keys]`

Map chord strings to command names, for example:

```toml
[keys]
"ctrl+s" = "save"
"ctrl+shift+p" = "command-palette"
```

Unknown chords or command names are rejected when the key map is built. The
full default keymap and named command aliases are documented in
[SHORTCUTS.md](SHORTCUTS.md). `:config-reload` reloads the config file from
the resolved path.
