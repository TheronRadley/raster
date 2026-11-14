//! The single command vocabulary used by keybindings, the palette, the
//! optional editing profiles, LSP actions, and tests. Commands carry no
//! terminal state; `Editor::execute` maps them onto transactions.

use std::collections::HashMap;

use crate::term::input::KeyChord;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum CommandId {
    Noop,
    // Files and lifecycle
    NewBuffer,
    OpenFile,
    Save,
    SaveAs,
    SaveAll,
    CloseBuffer,
    Reload,
    Overwrite,
    Recover,
    RecoverSave,
    CleanRecovery,
    Quit,
    ForceQuit,
    Suspend,
    // Editing and undo
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SystemCopy,
    SystemCut,
    SystemPaste,
    DeleteSelection,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteLine,
    DuplicateLine,
    MoveLineUp,
    MoveLineDown,
    Indent,
    Outdent,
    ToggleComment,
    InsertTab,
    InsertNewline,
    // Navigation and selection
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    VisualUp,
    VisualDown,
    MoveWordLeft,
    MoveWordRight,
    MoveLineStart,
    MoveLineEnd,
    MoveDocStart,
    MoveDocEnd,
    PageUp,
    PageDown,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
    SelectDocStart,
    SelectDocEnd,
    SelectAll,
    CollapseSelection,
    SwapSelectionEnds,
    GoToLine,
    // Search and project
    Find,
    FindRegex,
    FindNext,
    FindPrevious,
    Replace,
    ReplaceAll,
    ProjectSearch,
    ToggleSearchMode,
    ToggleCaseSensitive,
    ToggleHidden,
    ToggleIgnored,
    FuzzyFiles,
    RefreshFiles,
    ToggleFileBrowser,
    // Views and UI
    NextBuffer,
    PreviousBuffer,
    BufferList,
    SplitVertical,
    SplitHorizontal,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CloseSplit,
    EqualizeSplits,
    Help,
    CommandPalette,
    Cancel,
    Confirm,
    ConfigReload,
    ToggleReadOnly,
    ToggleSoftWrap,
    ToggleLineNumbers,
    ToggleMouse,
    // Multiple cursors and profiles
    AddCursorAbove,
    AddCursorBelow,
    AddCursorNextMatch,
    RemoveSecondaryCursors,
    ProfileNormal,
    ProfileVim,
    ProfileSelectionFirst,
    VimInsert,
    VimAppend,
    VimNormal,
    VimVisual,
    VimDeleteLine,
    VimYankLine,
    // LSP and Git
    Completion,
    Hover,
    GoToDefinition,
    References,
    RenameSymbol,
    FormatBuffer,
    CodeAction,
    LspRestart,
    LspStatus,
    LspStop,
    GitStatus,
    GitRefresh,
}

impl CommandId {
    pub fn name(self) -> &'static str {
        use CommandId::*;
        match self {
            Noop => "noop",
            NewBuffer => "new-buffer",
            OpenFile => "open-file",
            Save => "save",
            SaveAs => "save-as",
            SaveAll => "save-all",
            CloseBuffer => "close-buffer",
            Reload => "reload",
            Overwrite => "overwrite",
            Recover => "recover",
            RecoverSave => "recover-save",
            CleanRecovery => "clean-recovery",
            Quit => "quit",
            ForceQuit => "force-quit",
            Suspend => "suspend",
            Undo => "undo",
            Redo => "redo",
            Cut => "cut",
            Copy => "copy",
            Paste => "paste",
            SystemCopy => "system-copy",
            SystemCut => "system-cut",
            SystemPaste => "system-paste",
            DeleteSelection => "delete-selection",
            Backspace => "backspace",
            Delete => "delete",
            DeleteWordLeft => "delete-word-left",
            DeleteWordRight => "delete-word-right",
            DeleteLine => "delete-line",
            DuplicateLine => "duplicate-line",
            MoveLineUp => "move-line-up",
            MoveLineDown => "move-line-down",
            Indent => "indent",
            Outdent => "outdent",
            ToggleComment => "toggle-comment",
            InsertTab => "insert-tab",
            InsertNewline => "insert-newline",
            MoveLeft => "move-left",
            MoveRight => "move-right",
            MoveUp => "move-up",
            MoveDown => "move-down",
            VisualUp => "visual-up",
            VisualDown => "visual-down",
            MoveWordLeft => "move-word-left",
            MoveWordRight => "move-word-right",
            MoveLineStart => "move-line-start",
            MoveLineEnd => "move-line-end",
            MoveDocStart => "move-doc-start",
            MoveDocEnd => "move-doc-end",
            PageUp => "page-up",
            PageDown => "page-down",
            SelectLeft => "select-left",
            SelectRight => "select-right",
            SelectUp => "select-up",
            SelectDown => "select-down",
            SelectWordLeft => "select-word-left",
            SelectWordRight => "select-word-right",
            SelectLineStart => "select-line-start",
            SelectLineEnd => "select-line-end",
            SelectDocStart => "select-doc-start",
            SelectDocEnd => "select-doc-end",
            SelectAll => "select-all",
            CollapseSelection => "collapse-selection",
            SwapSelectionEnds => "swap-selection-ends",
            GoToLine => "goto-line",
            Find => "find",
            FindRegex => "find-regex",
            FindNext => "find-next",
            FindPrevious => "find-previous",
            Replace => "replace",
            ReplaceAll => "replace-all",
            ProjectSearch => "project-search",
            ToggleSearchMode => "toggle-search-mode",
            ToggleCaseSensitive => "toggle-case-sensitive",
            ToggleHidden => "toggle-hidden",
            ToggleIgnored => "toggle-ignored",
            FuzzyFiles => "fuzzy-files",
            RefreshFiles => "refresh-files",
            ToggleFileBrowser => "toggle-file-browser",
            NextBuffer => "next-buffer",
            PreviousBuffer => "previous-buffer",
            BufferList => "buffer-list",
            SplitVertical => "split-vertical",
            SplitHorizontal => "split-horizontal",
            FocusLeft => "focus-left",
            FocusRight => "focus-right",
            FocusUp => "focus-up",
            FocusDown => "focus-down",
            CloseSplit => "close-split",
            EqualizeSplits => "equalize-splits",
            Help => "help",
            CommandPalette => "command-palette",
            Cancel => "cancel",
            Confirm => "confirm",
            ConfigReload => "config-reload",
            ToggleReadOnly => "toggle-read-only",
            ToggleSoftWrap => "toggle-soft-wrap",
            ToggleLineNumbers => "toggle-line-numbers",
            ToggleMouse => "toggle-mouse",
            AddCursorAbove => "add-cursor-above",
            AddCursorBelow => "add-cursor-below",
            AddCursorNextMatch => "add-cursor-next-match",
            RemoveSecondaryCursors => "remove-secondary-cursors",
            ProfileNormal => "profile-normal",
            ProfileVim => "profile-vim",
            ProfileSelectionFirst => "profile-selection-first",
            VimInsert => "vim-insert",
            VimAppend => "vim-append",
            VimNormal => "vim-normal",
            VimVisual => "vim-visual",
            VimDeleteLine => "vim-delete-line",
            VimYankLine => "vim-yank-line",
            Completion => "completion",
            Hover => "hover",
            GoToDefinition => "go-to-definition",
            References => "references",
            RenameSymbol => "rename-symbol",
            FormatBuffer => "format-buffer",
            CodeAction => "code-action",
            LspRestart => "lsp-restart",
            LspStatus => "lsp-status",
            LspStop => "lsp-stop",
            GitStatus => "git-status",
            GitRefresh => "git-refresh",
        }
    }

    pub fn description(self) -> &'static str {
        use CommandId::*;
        match self {
            Save => "Save the active buffer safely",
            SaveAs => "Save the active buffer under a new path",
            Quit => "Quit, confirming unsaved work",
            Undo => "Undo one transaction",
            Redo => "Redo one transaction",
            Find => "Find in the active buffer (literal mode)",
            FindRegex => "Find in the active buffer (regex mode)",
            Replace => "Replace with explicit buffer/selection scope",
            ReplaceAll => "Replace all matches in the current scope",
            ProjectSearch => "Search project files with ignore rules",
            FuzzyFiles => "Fuzzy search project files",
            CommandPalette => "Search all commands",
            BufferList => "Switch buffers",
            SplitVertical => "Create a vertical split",
            SplitHorizontal => "Create a horizontal split",
            Help => "Show shortcut discovery and help",
            Recover => "Restore unsaved recovery files",
            Suspend => "Suspend on Unix and restore the terminal on resume",
            GoToDefinition => "Go to definition via the configured language server",
            FormatBuffer => "Format via the configured language server",
            GitStatus => "Show lightweight Git branch and changed files",
            _ => "Raster command",
        }
    }

    pub fn category(self) -> &'static str {
        use CommandId::*;
        match self {
            NewBuffer | OpenFile | Save | SaveAs | SaveAll | CloseBuffer | Reload | Overwrite
            | Recover | RecoverSave | CleanRecovery | Quit | ForceQuit | Suspend => "File",
            Undo | Redo | Cut | Copy | Paste | SystemCopy | SystemCut | SystemPaste
            | DeleteSelection | Backspace | Delete | DeleteWordLeft | DeleteWordRight
            | DeleteLine | DuplicateLine | MoveLineUp | MoveLineDown | Indent | Outdent
            | ToggleComment | InsertTab | InsertNewline => "Edit",
            MoveLeft | MoveRight | MoveUp | MoveDown | VisualUp | VisualDown | MoveWordLeft
            | MoveWordRight | MoveLineStart | MoveLineEnd | MoveDocStart | MoveDocEnd | PageUp
            | PageDown | SelectLeft | SelectRight | SelectUp | SelectDown | SelectWordLeft
            | SelectWordRight | SelectLineStart | SelectLineEnd | SelectDocStart | SelectDocEnd
            | SelectAll | CollapseSelection | SwapSelectionEnds | GoToLine => "Navigation",
            Find | FindRegex | FindNext | FindPrevious | Replace | ReplaceAll | ProjectSearch
            | ToggleSearchMode | ToggleCaseSensitive | ToggleHidden | ToggleIgnored
            | FuzzyFiles | RefreshFiles | ToggleFileBrowser => "Search",
            NextBuffer | PreviousBuffer | BufferList | SplitVertical | SplitHorizontal
            | FocusLeft | FocusRight | FocusUp | FocusDown | CloseSplit | EqualizeSplits | Help
            | CommandPalette | Cancel | Confirm | ConfigReload | ToggleReadOnly
            | ToggleSoftWrap | ToggleLineNumbers | ToggleMouse => "View",
            AddCursorAbove
            | AddCursorBelow
            | AddCursorNextMatch
            | RemoveSecondaryCursors
            | ProfileNormal
            | ProfileVim
            | ProfileSelectionFirst
            | VimInsert
            | VimAppend
            | VimNormal
            | VimVisual
            | VimDeleteLine
            | VimYankLine => "Profiles",
            Completion | Hover | GoToDefinition | References | RenameSymbol | FormatBuffer
            | CodeAction | LspRestart | LspStatus | LspStop | GitStatus | GitRefresh => "Coding",
            Noop => "Internal",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        ALL.iter()
            .find(|def| def.id.name() == name || def.aliases.contains(&name))
            .map(|d| d.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub args: Vec<String>,
}

impl Command {
    pub fn new(id: CommandId) -> Self {
        Self {
            id,
            args: Vec::new(),
        }
    }
    pub fn with_args(id: CommandId, args: Vec<String>) -> Self {
        Self { id, args }
    }

    /// Parse a palette or `:` command line. Arguments are split respecting
    /// simple double quotes; no shell is involved.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let line = line.strip_prefix(':').unwrap_or(line);
        let parts = split_args(line);
        let name = parts.first()?.to_ascii_lowercase();
        let id = CommandId::from_name(&name)?;
        Some(Self {
            id,
            args: parts[1..].to_vec(),
        })
    }
}

fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct CommandDef {
    pub id: CommandId,
    pub aliases: &'static [&'static str],
}

macro_rules! def {
    ($id:ident, [$($alias:literal),*]) => {
        CommandDef { id: CommandId::$id, aliases: &[$($alias),*] }
    };
}

pub static ALL: &[CommandDef] = &[
    def!(Noop, []),
    def!(NewBuffer, ["new"]),
    def!(OpenFile, ["open", "o"]),
    def!(Save, ["w"]),
    def!(SaveAs, ["write-as"]),
    def!(SaveAll, ["wa"]),
    def!(CloseBuffer, ["close", "bd"]),
    def!(Reload, ["e"]),
    def!(Overwrite, ["w!"]),
    def!(Recover, ["recovery"]),
    def!(RecoverSave, ["recover-interrupted-save"]),
    def!(CleanRecovery, ["cleanup"]),
    def!(Quit, ["q"]),
    def!(ForceQuit, ["q!"]),
    def!(Suspend, ["z"]),
    def!(Undo, ["u"]),
    def!(Redo, ["r"]),
    def!(Cut, ["x"]),
    def!(Copy, ["y"]),
    def!(Paste, ["p"]),
    def!(SystemCopy, []),
    def!(SystemCut, []),
    def!(SystemPaste, []),
    def!(DeleteSelection, []),
    def!(Backspace, []),
    def!(Delete, []),
    def!(DeleteWordLeft, []),
    def!(DeleteWordRight, []),
    def!(DeleteLine, []),
    def!(DuplicateLine, []),
    def!(MoveLineUp, []),
    def!(MoveLineDown, []),
    def!(Indent, []),
    def!(Outdent, []),
    def!(ToggleComment, []),
    def!(InsertTab, []),
    def!(InsertNewline, []),
    def!(MoveLeft, []),
    def!(MoveRight, []),
    def!(MoveUp, []),
    def!(MoveDown, []),
    def!(VisualUp, []),
    def!(VisualDown, []),
    def!(MoveWordLeft, []),
    def!(MoveWordRight, []),
    def!(MoveLineStart, []),
    def!(MoveLineEnd, []),
    def!(MoveDocStart, []),
    def!(MoveDocEnd, []),
    def!(PageUp, []),
    def!(PageDown, []),
    def!(SelectLeft, []),
    def!(SelectRight, []),
    def!(SelectUp, []),
    def!(SelectDown, []),
    def!(SelectWordLeft, []),
    def!(SelectWordRight, []),
    def!(SelectLineStart, []),
    def!(SelectLineEnd, []),
    def!(SelectDocStart, []),
    def!(SelectDocEnd, []),
    def!(SelectAll, []),
    def!(CollapseSelection, []),
    def!(SwapSelectionEnds, []),
    def!(GoToLine, ["goto", "g"]),
    def!(Find, ["search"]),
    def!(FindRegex, ["regex-find"]),
    def!(FindNext, ["n"]),
    def!(FindPrevious, ["N"]),
    def!(Replace, ["s"]),
    def!(ReplaceAll, ["replace-all"]),
    def!(ProjectSearch, ["grep"]),
    def!(ToggleSearchMode, []),
    def!(ToggleCaseSensitive, []),
    def!(ToggleHidden, []),
    def!(ToggleIgnored, []),
    def!(FuzzyFiles, ["files"]),
    def!(RefreshFiles, []),
    def!(ToggleFileBrowser, ["explore"]),
    def!(NextBuffer, ["bn"]),
    def!(PreviousBuffer, ["bp"]),
    def!(BufferList, ["buffers", "ls"]),
    def!(SplitVertical, ["vsplit", "vs"]),
    def!(SplitHorizontal, ["split", "hs"]),
    def!(FocusLeft, []),
    def!(FocusRight, []),
    def!(FocusUp, []),
    def!(FocusDown, []),
    def!(CloseSplit, []),
    def!(EqualizeSplits, []),
    def!(Help, ["shortcuts", "?"]),
    def!(CommandPalette, ["cmds"]),
    def!(Cancel, []),
    def!(Confirm, []),
    def!(ConfigReload, []),
    def!(ToggleReadOnly, []),
    def!(ToggleSoftWrap, []),
    def!(ToggleLineNumbers, []),
    def!(ToggleMouse, []),
    def!(AddCursorAbove, []),
    def!(AddCursorBelow, []),
    def!(AddCursorNextMatch, []),
    def!(RemoveSecondaryCursors, []),
    def!(ProfileNormal, []),
    def!(ProfileVim, []),
    def!(ProfileSelectionFirst, ["profile-helix"]),
    def!(VimInsert, []),
    def!(VimAppend, []),
    def!(VimNormal, []),
    def!(VimVisual, []),
    def!(VimDeleteLine, []),
    def!(VimYankLine, []),
    def!(Completion, ["complete"]),
    def!(Hover, ["K"]),
    def!(GoToDefinition, ["gd"]),
    def!(References, ["gr"]),
    def!(RenameSymbol, ["rename"]),
    def!(FormatBuffer, ["fmt"]),
    def!(CodeAction, ["ca"]),
    def!(LspRestart, []),
    def!(LspStatus, []),
    def!(LspStop, []),
    def!(GitStatus, ["git"]),
    def!(GitRefresh, []),
];

/// Default shortcut scheme. Terminal conflicts are documented in
/// `docs/SHORTCUTS.md`; every unreliable shortcut has a palette alias.
pub fn default_keybindings() -> Vec<(&'static str, CommandId)> {
    use CommandId::*;
    vec![
        ("ctrl+s", Save),
        ("ctrl+q", Quit),
        ("ctrl+z", Undo),
        ("ctrl+y", Redo),
        ("ctrl+shift+z", Redo),
        ("ctrl+n", NewBuffer),
        ("ctrl+o", OpenFile),
        ("ctrl+p", FuzzyFiles),
        ("ctrl+shift+p", CommandPalette),
        ("ctrl+b", BufferList),
        ("ctrl+tab", NextBuffer),
        ("ctrl+shift+tab", PreviousBuffer),
        ("ctrl+f", Find),
        ("ctrl+r", Replace),
        ("f3", FindNext),
        ("shift+f3", FindPrevious),
        ("ctrl+shift+f", ProjectSearch),
        ("f1", Help),
        ("ctrl+g", GoToLine),
        ("ctrl+a", SelectAll),
        ("ctrl+c", Copy),
        ("ctrl+x", Cut),
        ("ctrl+v", Paste),
        ("ctrl+shift+c", SystemCopy),
        ("ctrl+shift+x", SystemCut),
        ("ctrl+shift+v", SystemPaste),
        ("ctrl+d", AddCursorNextMatch),
        ("ctrl+alt+up", AddCursorAbove),
        ("ctrl+alt+down", AddCursorBelow),
        ("ctrl+w v", SplitVertical),
        ("ctrl+w s", SplitHorizontal),
        ("ctrl+w h", FocusLeft),
        ("ctrl+w j", FocusDown),
        ("ctrl+w k", FocusUp),
        ("ctrl+w l", FocusRight),
        ("ctrl+w q", CloseSplit),
        ("ctrl+w =", EqualizeSplits),
        ("ctrl+w ctrl+w", NextBuffer),
        ("ctrl+x ctrl+z", Suspend),
        ("ctrl+e", ToggleFileBrowser),
        ("ctrl+shift+r", Recover),
        ("left", MoveLeft),
        ("right", MoveRight),
        ("up", MoveUp),
        ("down", MoveDown),
        ("home", MoveLineStart),
        ("end", MoveLineEnd),
        ("ctrl+home", MoveDocStart),
        ("ctrl+end", MoveDocEnd),
        ("pageup", PageUp),
        ("pagedown", PageDown),
        ("ctrl+left", MoveWordLeft),
        ("ctrl+right", MoveWordRight),
        ("shift+left", SelectLeft),
        ("shift+right", SelectRight),
        ("shift+up", SelectUp),
        ("shift+down", SelectDown),
        ("shift+home", SelectLineStart),
        ("shift+end", SelectLineEnd),
        ("ctrl+shift+left", SelectWordLeft),
        ("ctrl+shift+right", SelectWordRight),
        ("ctrl+shift+home", SelectDocStart),
        ("ctrl+shift+end", SelectDocEnd),
        ("tab", Indent),
        ("shift+tab", Outdent),
        ("alt+up", MoveLineUp),
        ("alt+down", MoveLineDown),
        ("ctrl+/", ToggleComment),
        ("ctrl+k", DeleteLine),
        ("ctrl+space", Completion),
        ("f2", RenameSymbol),
        ("f12", GoToDefinition),
        ("shift+f12", References),
        ("ctrl+shift+i", FormatBuffer),
        ("alt+enter", CodeAction),
        ("ctrl+shift+g", GitStatus),
        ("esc", Cancel),
        ("enter", Confirm),
    ]
}

pub type KeyMap = HashMap<KeyChord, CommandId>;

pub fn default_key_map() -> Result<KeyMap, crate::term::input::ChordError> {
    let mut map = HashMap::new();
    for (text, id) in default_keybindings() {
        let chord = KeyChord::parse(text)?;
        if map.insert(chord.clone(), id).is_some() {
            return Err(crate::term::input::ChordError(format!(
                "duplicate default binding: {}",
                chord.as_string()
            )));
        }
    }
    Ok(map)
}

pub fn command_defs() -> impl Iterator<Item = &'static CommandDef> {
    ALL.iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_unique_name() {
        let mut names = std::collections::HashSet::new();
        for def in ALL {
            assert!(
                names.insert(def.id.name()),
                "duplicate command {}",
                def.id.name()
            );
        }
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn default_bindings_parse_without_conflicts() {
        let map = default_key_map().unwrap();
        assert_eq!(map.len(), default_keybindings().len());
        assert_eq!(map[&KeyChord::parse("ctrl+s").unwrap()], CommandId::Save);
        assert_eq!(
            map[&KeyChord::parse("ctrl+w v").unwrap()],
            CommandId::SplitVertical
        );
    }

    #[test]
    fn command_line_parses_quoted_arguments() {
        let cmd = Command::parse(":save-as \"/tmp/my file.txt\"").unwrap();
        assert_eq!(cmd.id, CommandId::SaveAs);
        assert_eq!(cmd.args, vec!["/tmp/my file.txt".to_string()]);
        assert_eq!(Command::parse("w").unwrap().id, CommandId::Save);
    }
}
