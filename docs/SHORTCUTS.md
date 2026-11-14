# Shortcut reference

The default profile is **normal**: nonmodal, familiar editing. Type directly;
no command is needed to begin. `F1` opens this information inside Raster and
`Ctrl+Shift+P` opens a searchable command palette. Optional Vim-inspired and
selection-first profiles are partial; see
[LIMITATIONS.md](LIMITATIONS.md).

## Default key map

These are the bindings in `default_keybindings()`. Config `[keys]` entries
override them; unknown chords or commands are rejected.

### Files and lifecycle

| Chord | Command |
| --- | --- |
| `Ctrl+S` | Save |
| `Ctrl+Q` | Quit (confirm dirty buffers) |
| `Ctrl+N` | New buffer |
| `Ctrl+O` | Open file |
| `Ctrl+Shift+R` | Recover |
| `Ctrl+X Ctrl+Z` | Suspend (Unix only) |

### Editing and undo

| Chord | Command |
| --- | --- |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` / `Ctrl+Shift+Z` | Redo |
| `Tab` / `Shift+Tab` | Indent / outdent |
| `Alt+Up` / `Alt+Down` | Move line up / down |
| `Ctrl+/` | Toggle comment |
| `Ctrl+K` | Delete line |
| `Ctrl+D` | Add cursor at next match |
| `Ctrl+Alt+Up` / `Ctrl+Alt+Down` | Add cursor above / below |
| `Ctrl+Space` | Completion |

### Internal register and system clipboard

| Chord | Command |
| --- | --- |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste using the internal register |
| `Ctrl+Shift+C` / `Ctrl+Shift+X` / `Ctrl+Shift+V` | System clipboard copy / cut / paste |

Internal registers stay inside Raster. System clipboard and OSC 52 are off by
default and require explicit configuration; remote sessions additionally
require `clipboard.allow_remote`. See [CONFIG.md](CONFIG.md).

### Navigation and selection

| Chord | Command |
| --- | --- |
| arrows | Move by grapheme / visual row |
| `Shift`+arrows | Extend selection |
| `Home` / `End` | Line start / end |
| `Ctrl+Home` / `Ctrl+End` | Document start / end |
| `PageUp` / `PageDown` | Logical-line paging |
| `Ctrl+Left` / `Ctrl+Right` | Word left / right |
| `Ctrl+Shift+Left` / `Ctrl+Shift+Right` | Select word left / right |
| `Ctrl+Shift+Home` / `Ctrl+Shift+End` | Select to document start / end |
| `Ctrl+A` | Select all |
| `Ctrl+G` | Go to line |
| `Esc` | Cancel prompt / collapse selection |

Up/Down follow soft-wrapped rows when soft wrap is enabled. Horizontal
scrolling is bounded in unwrapped mode. Wide and combining characters map to
grapheme starts for cursor placement.

### Search and project

| Chord | Command |
| --- | --- |
| `Ctrl+F` | Find |
| `Ctrl+R` | Replace |
| `F3` / `Shift+F3` | Next / previous match |
| `Ctrl+P` | Fuzzy files |
| `Ctrl+Shift+F` | Project search |
| `Ctrl+E` | Toggle file browser |

Project scans honor ignore rules and can be canceled. Search results carry
generations and document versions so obsolete results are not applied.

### Buffers, views, and UI

| Chord | Command |
| --- | --- |
| `Ctrl+B` | Buffer list |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous buffer |
| `Ctrl+W v` / `Ctrl+W s` | Split vertical / horizontal |
| `Ctrl+W h/j/k/l` | Focus left / down / up / right |
| `Ctrl+W q` | Close split |
| `Ctrl+W =` | Equalize splits |
| `Ctrl+W Ctrl+W` | Next buffer |
| `F1` | Help |
| `Ctrl+Shift+P` | Command palette |
| `Enter` | Confirm |
| `Esc` / `Ctrl+C` / `Ctrl+G` in a prompt | Cancel |

Multiple views share one buffer but keep independent viewport and focus state.
Small terminals show a graceful "too small" message and refuse additional
splits when the focused rectangle cannot meet the configured minimum size.

### LSP and Git

| Chord | Command |
| --- | --- |
| `F2` | Rename symbol |
| `F12` | Go to definition |
| `Shift+F12` | References |
| `Ctrl+Shift+I` | Format buffer |
| `Alt+Enter` | Code action |
| `Ctrl+Shift+G` | Git status |

These features require explicit configuration and never prevent ordinary
editing when the optional service is unavailable.

## Named commands and `:`

In the normal profile, typing `:` inserts a colon. `Alt+:` opens the
named-command prompt; in Vim normal mode, `:` opens it. `Ctrl+Shift+P`
searches the same command vocabulary. Arguments are split without a shell. Common aliases:

| Command | Alias / example |
| --- | --- |
| Save | `:w` |
| Save all | `:wa` |
| Save as | `:write-as path` |
| Quit | `:q` |
| Force quit | `:q!` |
| Open | `:open path`, `:o path` |
| Reload | `:reload`, `:e` |
| Overwrite external change | `:overwrite`, `:w!` |
| Go to line | `:goto 42` |
| Next / previous buffer | `:bn`, `:bp` |
| Close buffer | `:close`, `:bd` |
| Undo / redo | `:u`, `:r` |
| Restart LSP | `:lsp-restart` |
| Stop LSP | `:lsp-stop` |
| Config reload | `:config-reload` |

Use `:w` / `:q` if a terminal, multiplexer, or SSH layer consumes `Ctrl+S` /
`Ctrl+Q`. Raster disables XON/XOFF flow control in raw mode on Unix, but some
outer layers can still intercept those bytes. `Ctrl+Tab` and modified arrow
sequences vary by terminal; `:bn`, `:bp`, and palette commands are portable
alternatives.

## Mouse and paste

SGR mouse reporting is enabled when `ui.mouse = true` and can be disabled with
`--no-mouse` or `:toggle-mouse`; all actions remain reachable by keyboard.
Bracketed paste (`ESC [ 200 ~ ... ESC [ 201 ~`) inserts text literally and is
never interpreted as commands. Untrusted text is sanitized before display, so
pasted terminal control sequences cannot inject raw escape sequences into the
renderer's output.
