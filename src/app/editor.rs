//! Application controller: buffers, views, commands, prompts, asynchronous
//! services, and rendering. It is testable without a terminal: callers feed
//! `Event`s and inspect `render_frame`, which returns a plain `Frame`.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::buffer::{Buffer, BufferError, EditOutcome};
use crate::app::layout::{self, Viewport};
use crate::app::search_worker::{self, BufferSearchHandle};
use crate::app::split::{direction_matches, Axis, SplitNode};
use crate::app::view::{View, ViewId};
use crate::cli::CliOptions;
use crate::clipboard::ClipboardManager;
use crate::config::{Config, LineNumberMode, ProfileName};
use crate::core::commands::{self, Command, CommandId, KeyMap};
use crate::core::document::Document;
use crate::core::project::{self, ProjectIndex, ProjectMatch, ProjectSearchHandle};
use crate::core::search::{self, Match, SearchMode, SearchOptions, SearchScope};
use crate::core::selection::Selection;
use crate::core::syntax::{Language, SyntaxEngine, SyntaxKind};
use crate::core::transaction::{transform_pos, Replacement, Transaction};
use crate::fs as raster_fs;
use crate::fs::{InterruptedSave, SaveOptions};
use crate::git::GitStatus;
use crate::json::Json;
use crate::lsp::{
    self, LspEvent, LspManager, LspRequestKind, PositionEncoding, WorkspaceEditReview,
    WorkspaceTarget,
};
use crate::position::{BytePos, LineIndex, Size};
use crate::session::{
    BufferId, RecoveryJob, RecoveryManager, RecoveryRecord, SessionFile, SessionState,
};
use crate::term::input::{Event, Key, KeyEvent, MouseEvent, MouseKind, SignalKind};
use crate::term::renderer::{Frame, Rect, Style};
use crate::util::{config_dir, display_width, ellipsize, recovery_dir, session_file, short_path};

const WELCOME: &str = "Raster — start typing immediately\n\nEssential shortcuts\n  Ctrl+S save       Ctrl+O open       Ctrl+Q quit\n  Ctrl+F find       Ctrl+R replace    Ctrl+P files\n  Ctrl+Z undo       Ctrl+Y redo       Ctrl+B buffers\n  Ctrl+Shift+P commands             F1 help\n\nFiles are opened without a mode. Press ':' for a named command.\nThis welcome text is an editable scratch buffer; use Ctrl+O to open a file.\n";

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum EditorAction {
    None,
    Quit,
    Suspend,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PromptKind {
    Open,
    SaveAs,
    Find,
    FindRegex,
    ReplacePattern,
    ReplaceTemplate,
    ProjectSearch,
    GoToLine,
    CommandLine,
    RenameSymbol,
}

#[derive(Clone, Debug)]
pub struct PromptState {
    pub kind: PromptKind,
    pub label: String,
    pub input: String,
    /// Character index, not a byte offset.
    pub cursor: usize,
    pub hint: String,
}

impl PromptState {
    pub fn new(kind: PromptKind, label: &str, input: &str, hint: &str) -> Self {
        Self {
            kind,
            label: label.into(),
            input: input.into(),
            cursor: input.chars().count(),
            hint: hint.into(),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum ListKind {
    CommandPalette,
    Files,
    FileBrowser,
    Buffers,
    ProjectResults,
    Completion,
    LspLocations,
    Recovery,
    InterruptedSaves,
    GitStatus,
    CodeActions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub label: String,
    pub payload: String,
}

#[derive(Clone, Debug)]
pub struct ListState {
    pub kind: ListKind,
    pub title: String,
    pub query: String,
    pub cursor: usize,
    pub items: Vec<ListItem>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
}

impl ListState {
    pub fn new(kind: ListKind, title: &str, query: &str, items: Vec<ListItem>) -> Self {
        let n = items.len();
        Self {
            kind,
            title: title.into(),
            query: query.into(),
            cursor: query.chars().count(),
            items,
            filtered: (0..n).collect(),
            selected: 0,
            scroll: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    Quit,
    CloseBuffer(BufferId),
    Reload(BufferId),
    SaveAsOverwrite(PathBuf),
    ApplyWorkspaceEdit,
    RecoverRecord(usize),
    InterruptedSave(usize),
}

#[derive(Clone, Debug)]
pub enum UiMode {
    Normal,
    Prompt(PromptState),
    List(ListState),
    Help {
        lines: Vec<String>,
        offset: usize,
    },
    Confirm {
        question: String,
        action: ConfirmAction,
    },
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub start: BytePos,
    pub end: BytePos,
    pub message: String,
    pub severity: u8,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct EditorLocation {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
struct SearchState {
    pattern: String,
    template: String,
    options: SearchOptions,
    generation: u64,
    version: u64,
    buffer_id: BufferId,
    matches: Vec<Match>,
    index: usize,
    auto_jump: bool,
}

pub struct Editor {
    pub config: Config,
    pub config_path: Option<PathBuf>,
    pub cli: CliOptions,
    pub root: PathBuf,
    pub size: Size,
    key_map: KeyMap,
    buffers: BTreeMap<BufferId, Buffer>,
    views: BTreeMap<ViewId, View>,
    split: SplitNode,
    focus: ViewId,
    next_buffer_id: BufferId,
    next_view_id: ViewId,
    ui: UiMode,
    pending_chord: Vec<KeyEvent>,
    pending_vim: Option<char>,
    vim_mode: VimMode,
    status: String,
    status_style: Style,
    status_until: Instant,
    output: Vec<Vec<u8>>,
    should_quit: bool,
    search: SearchState,
    buffer_search: Option<BufferSearchHandle>,
    project: ProjectIndex,
    project_search: Option<ProjectSearchHandle>,
    project_generation: u64,
    project_results: Vec<ProjectMatch>,
    syntax: SyntaxEngine,
    recovery: Option<RecoveryManager>,
    recovery_records: Vec<RecoveryRecord>,
    recovery_files: HashMap<BufferId, PathBuf>,
    interrupted_saves: Vec<InterruptedSave>,
    interrupted_journals: HashMap<BufferId, PathBuf>,
    session_path: PathBuf,
    lsp: LspManager,
    lsp_event_log: Vec<String>,
    diagnostics: HashMap<BufferId, Vec<Diagnostic>>,
    completions: Vec<String>,
    locations: Vec<EditorLocation>,
    code_actions: Vec<Json>,
    pending_workspace_review: Option<WorkspaceEditReview>,
    git: Option<GitStatus>,
    clipboard: ClipboardManager,
    last_external_check: Instant,
    last_recovery_submit: Instant,
    last_session_save: Instant,
    last_git_refresh: Instant,
    external_warning: bool,
    headless: bool,
}

impl Editor {
    pub fn new(config: Config, cli: CliOptions, size: Size) -> Result<Self, String> {
        Self::build(config, cli, size, false)
    }

    pub fn headless(config: Config, cli: CliOptions, size: Size) -> Result<Self, String> {
        Self::build(config, cli, size, true)
    }

    fn build(
        mut config: Config,
        cli: CliOptions,
        size: Size,
        headless: bool,
    ) -> Result<Self, String> {
        config.apply_cli(&cli);
        config.validate().map_err(|e| e.to_string())?;
        let key_map = config.key_map().map_err(|e| e.to_string())?;
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut root = cli.project.clone().unwrap_or_else(|| cwd.clone());
        if !root.is_dir() {
            root = cwd.clone();
        }
        if cli.project.is_none() {
            let mut markers = vec![
                ".git".into(),
                "Cargo.toml".into(),
                "go.mod".into(),
                "pyproject.toml".into(),
            ];
            for server in &config.lsp.servers {
                markers.extend(server.root_markers.clone());
            }
            root = lsp::detect_project_root(&root, &markers);
        }
        let config_path = cli
            .config
            .clone()
            .unwrap_or_else(|| config_dir().join("config.toml"));
        let session_path = session_file();
        let recovery_path = recovery_dir();
        let mut project = ProjectIndex::new(root.clone());
        if !headless {
            project.start_scan(config.search.hidden, config.search.ignored);
        }
        let syntax = SyntaxEngine::new();
        let lsp = LspManager::new(config.lsp.clone(), root.clone());
        let git = if config.git.enabled && !headless {
            let mut status = GitStatus::new(root.clone());
            status.start_refresh();
            Some(status)
        } else {
            None
        };
        let recovery = if config.session.recovery && !headless {
            match RecoveryManager::start(recovery_path.clone()) {
                Ok(manager) => {
                    let _ = manager.cleanup(Duration::from_secs(
                        config.session.recovery_retention_days.saturating_mul(86400),
                    ));
                    let _ = raster_fs::cleanup_save_journals(
                        &recovery_path,
                        Duration::from_secs(
                            config.session.recovery_retention_days.saturating_mul(86400),
                        ),
                    );
                    Some(manager)
                }
                Err(e) => {
                    eprintln!("raster: recovery disabled: {e}");
                    None
                }
            }
        } else {
            None
        };
        let clipboard = ClipboardManager::new(config.clipboard.clone(), config.is_remote());
        let now = Instant::now();
        let mut editor = Self {
            config,
            config_path: Some(config_path),
            cli,
            root,
            size,
            key_map,
            buffers: BTreeMap::new(),
            views: BTreeMap::new(),
            split: SplitNode::leaf(1),
            focus: 1,
            next_buffer_id: 1,
            next_view_id: 1,
            ui: UiMode::Normal,
            pending_chord: Vec::new(),
            pending_vim: None,
            vim_mode: VimMode::Insert,
            status: String::new(),
            status_style: Style::Plain,
            status_until: now,
            output: Vec::new(),
            should_quit: false,
            search: SearchState::default(),
            buffer_search: None,
            project,
            project_search: None,
            project_generation: 0,
            project_results: Vec::new(),
            syntax,
            recovery,
            recovery_records: Vec::new(),
            recovery_files: HashMap::new(),
            interrupted_saves: Vec::new(),
            interrupted_journals: HashMap::new(),
            session_path,
            lsp,
            diagnostics: HashMap::new(),
            completions: Vec::new(),
            locations: Vec::new(),
            code_actions: Vec::new(),
            pending_workspace_review: None,
            git,
            clipboard,
            last_external_check: now,
            last_recovery_submit: now,
            last_session_save: now,
            last_git_refresh: now,
            external_warning: false,
            headless,
            lsp_event_log: Vec::new(),
        };
        let welcome = Buffer::scratch(editor.next_buffer_id, WELCOME);
        editor.next_buffer_id += 1;
        let id = welcome.id;
        editor.buffers.insert(id, welcome);
        let view = View::new(editor.next_view_id, id);
        editor.next_view_id += 1;
        editor.views.insert(view.id, view);
        let mut restored = false;
        if editor.config.session.restore && !editor.cli.no_session {
            if let Some(session) = SessionState::load(&editor.session_path) {
                editor.restore_session(&session);
                restored = true;
            }
        }
        let cli_files = editor.cli.files.clone();
        for file in cli_files {
            editor.open_path(&file.path, file.line, file.col);
        }
        if !restored && editor.cli.files.is_empty() {
            editor.set_status("Welcome: type, Ctrl+S save, Ctrl+O open, F1 help");
        }
        if editor.cli.recover {
            editor.restore_recovery_records();
        }
        if let Some(warning) = editor.clipboard.warning() {
            editor.set_status(warning);
        }
        Ok(editor)
    }

    pub fn for_test(config: Config, text: &str) -> Self {
        let cli = CliOptions {
            no_config: true,
            no_session: true,
            ..Default::default()
        };
        let mut editor = Self::headless(config, cli, Size::new(80, 24)).expect("test editor");
        let document = Document::from_text(text);
        if let Some(buffer) = editor.buffers.get_mut(&1) {
            buffer.document = document;
            buffer.mark_clean();
            buffer.selections = vec![Selection::caret(BytePos(0))];
        }
        editor.set_status(String::new());
        editor
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
    pub fn mouse_enabled(&self) -> bool {
        self.config.ui.mouse
    }
    pub fn take_output(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.output)
    }
    pub fn ui_mode(&self) -> &UiMode {
        &self.ui
    }
    pub fn active_buffer_id(&self) -> BufferId {
        self.views
            .get(&self.focus)
            .map(|v| v.buffer_id)
            .or_else(|| self.buffers.keys().next().copied())
            .unwrap_or(1)
    }
    pub fn buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }
    pub fn active_buffer(&self) -> &Buffer {
        let id = self.active_buffer_id();
        self.buffers.get(&id).expect("active buffer exists")
    }
    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        let id = self.active_buffer_id();
        self.buffers.get_mut(&id).expect("active buffer exists")
    }
    pub fn document_text(&self) -> String {
        self.active_buffer().document.text()
    }
    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }
    pub fn view_count(&self) -> usize {
        self.views.len()
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn lsp_ready(&self) -> bool {
        self.lsp.has_active_client()
    }
    pub fn lsp_status(&self) -> String {
        self.lsp.status()
    }
    pub fn diagnostic_count(&self) -> usize {
        self.diagnostics.values().map(Vec::len).sum()
    }
    pub fn completion_items(&self) -> Vec<String> {
        self.completions.clone()
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.set_status_styled(text, Style::Plain, Duration::from_secs(6));
    }

    pub fn set_status_styled(&mut self, text: impl Into<String>, style: Style, duration: Duration) {
        self.status = text.into();
        self.status_style = style;
        self.status_until = Instant::now() + duration;
    }

    fn set_viewport_for_buffer(&mut self, id: BufferId, line: usize) {
        let line = line.min(
            self.buffers
                .get(&id)
                .map(|b| b.document.len_lines().saturating_sub(1))
                .unwrap_or(0),
        );
        for view in self.views.values_mut() {
            if view.buffer_id == id {
                view.viewport = Viewport {
                    top_line: line,
                    ..Viewport::default()
                };
            }
        }
    }

    pub fn open_path(&mut self, path: &Path, line: Option<usize>, col: Option<usize>) -> bool {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        for (id, buffer) in &self.buffers {
            if buffer.path() == Some(absolute.as_path()) {
                self.switch_buffer(*id);
                if let Some(line) = line {
                    self.goto_line_col(line, col.unwrap_or(1));
                }
                self.set_status(format!("Opened {}", short_path(&absolute, &self.root)));
                return true;
            }
        }
        let large_mode = fs::metadata(&absolute)
            .map(|m| m.len() > self.config.editor.large_file_threshold)
            .unwrap_or(false);
        let result = if absolute.exists() {
            Buffer::open(
                self.next_buffer_id,
                &absolute,
                self.cli.read_only,
                large_mode,
            )
        } else {
            let label = short_path(&absolute, &self.root);
            Ok(Buffer::from_document(
                self.next_buffer_id,
                label,
                Document::empty(),
                Some(absolute.clone()),
            ))
        };
        match result {
            Ok(buffer) => {
                let id = self.add_buffer(buffer);
                self.switch_buffer(id);
                if let Some(line) = line {
                    self.goto_line_col(line, col.unwrap_or(1));
                }
                let note = if large_mode { " (large-file mode)" } else { "" };
                self.set_status(format!(
                    "Opened {}{note}",
                    short_path(&absolute, &self.root)
                ));
                true
            }
            Err(e) => {
                self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(10));
                false
            }
        }
    }

    fn add_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.buffers.insert(id, buffer);
        id
    }

    fn switch_buffer(&mut self, id: BufferId) {
        if !self.buffers.contains_key(&id) {
            return;
        }
        if let Some(view) = self.views.get_mut(&self.focus) {
            view.buffer_id = id;
        }
    }

    fn goto_line_col(&mut self, line: usize, col: usize) {
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get_mut(&id) else {
            return;
        };
        let line_idx = LineIndex(
            line.saturating_sub(1)
                .min(buffer.document.len_lines().saturating_sub(1)),
        );
        let start = buffer.document.line_to_byte(line_idx);
        let offset = layout::byte_offset_for_display_col(
            &buffer.document,
            line_idx,
            col.saturating_sub(1),
            self.config.editor.tab_width,
            self.config.editor.cjk_wide,
        );
        let pos = BytePos(
            (start.get() + offset.get())
                .min(buffer.document.line_content_range(line_idx).end.get()),
        );
        buffer.set_carets(vec![pos]);
        self.set_viewport_for_buffer(id, line_idx.get().saturating_sub(6));
    }

    pub fn handle_event(&mut self, event: Event) -> EditorAction {
        match event {
            Event::Resize(size) => {
                self.size = size;
                self.set_status("resized");
                EditorAction::None
            }
            Event::Tick => {
                self.tick();
                EditorAction::None
            }
            Event::Key(key) => self.handle_key(key),
            Event::Paste(text) => {
                self.handle_paste(&text);
                EditorAction::None
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse);
                EditorAction::None
            }
            Event::Focus(_) => EditorAction::None,
            Event::Signal(SignalKind::Continue) => {
                self.set_status("resumed");
                EditorAction::None
            }
            Event::Signal(SignalKind::Suspend) => EditorAction::Suspend,
            Event::Signal(SignalKind::Terminate) | Event::Signal(SignalKind::Interrupt) => {
                self.should_quit = true;
                EditorAction::Quit
            }
            Event::Signal(SignalKind::Resize) => EditorAction::None,
            Event::EscapeTimeout => EditorAction::None,
        }
    }

    fn handle_paste(&mut self, text: &str) {
        match &mut self.ui {
            UiMode::Prompt(prompt) => {
                prompt
                    .input
                    .insert_str(byte_at_char(&prompt.input, prompt.cursor), text);
                prompt.cursor += text.chars().count();
            }
            UiMode::List(list) => {
                list.query.push_str(text);
                list.cursor += text.chars().count();
                let mut taken = std::mem::replace(&mut self.ui, UiMode::Normal);
                if let UiMode::List(ref mut state) = taken {
                    self.refresh_list(state);
                }
                self.ui = taken;
            }
            _ => {
                self.apply_edit(|buffer| buffer.insert_text(text));
                self.set_status("bracketed paste inserted as text");
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> EditorAction {
        match &self.ui {
            UiMode::Prompt(_) => {
                self.handle_prompt_key(key);
                return EditorAction::None;
            }
            UiMode::List(_) => {
                self.handle_list_key(key);
                return EditorAction::None;
            }
            UiMode::Help { .. } => {
                self.handle_help_key(key);
                return EditorAction::None;
            }
            UiMode::Confirm { .. } => {
                return self.handle_confirm_key(key);
            }
            UiMode::Normal => {}
        }
        if self.config.editor.profile == ProfileName::Vim && self.vim_mode != VimMode::Insert {
            return self.handle_vim_key(key);
        }
        let mut prefix = self.pending_chord.clone();
        prefix.push(key);
        let mut exact = None;
        let mut has_prefix = false;
        for (chord, id) in &self.key_map {
            if chord.keys.as_slice() == prefix.as_slice() {
                exact = Some(*id);
                break;
            }
            if chord.starts_with(&prefix) {
                has_prefix = true;
            }
        }
        if let Some(id) = exact {
            self.pending_chord.clear();
            return self.execute(Command::new(id));
        }
        if has_prefix {
            self.pending_chord = prefix;
            let text = self
                .pending_chord
                .iter()
                .map(KeyEvent::canonical)
                .collect::<Vec<_>>()
                .join(" ");
            self.set_status(format!("{text} …"));
            return EditorAction::None;
        }
        self.pending_chord.clear();
        self.handle_unbound_key(key)
    }

    fn handle_unbound_key(&mut self, key: KeyEvent) -> EditorAction {
        if key.mods.ctrl || key.mods.alt {
            if key.key == Key::Char('s') && key.mods.ctrl {
                return self.execute(Command::new(CommandId::Save));
            }
            if key.key == Key::Char('q') && key.mods.ctrl {
                return self.execute(Command::new(CommandId::Quit));
            }
            return EditorAction::None;
        }
        match key.key {
            Key::Enter => self.execute(Command::new(CommandId::InsertNewline)),
            Key::Backspace => self.execute(Command::new(CommandId::Backspace)),
            Key::Delete => self.execute(Command::new(CommandId::Delete)),
            Key::Tab if key.mods.shift => self.execute(Command::new(CommandId::Outdent)),
            Key::Tab => self.execute(Command::new(CommandId::InsertTab)),
            Key::Char(':') => {
                self.open_prompt(PromptKind::CommandLine, ":", "", "named command");
                EditorAction::None
            }
            Key::Char(ch) if !ch.is_control() => {
                let s = ch.to_string();
                self.apply_edit(|b| b.insert_text(&s));
                EditorAction::None
            }
            Key::Space => {
                self.apply_edit(|b| b.insert_text(" "));
                EditorAction::None
            }
            _ => EditorAction::None,
        }
    }

    fn handle_vim_key(&mut self, key: KeyEvent) -> EditorAction {
        if key.mods.ctrl {
            if key.key == Key::Char('s') {
                return self.execute(Command::new(CommandId::Save));
            }
            if key.key == Key::Char('q') {
                return self.execute(Command::new(CommandId::Quit));
            }
            if key.key == Key::Char('r') {
                return self.execute(Command::new(CommandId::Redo));
            }
        }
        match key.key {
            Key::Escape => {
                self.vim_mode = VimMode::Normal;
                self.execute(Command::new(CommandId::CollapseSelection))
            }
            Key::Char('i') => {
                self.vim_mode = VimMode::Insert;
                self.set_status("-- INSERT --");
                EditorAction::None
            }
            Key::Char('a') => {
                self.navigate(CommandId::MoveRight, false);
                self.vim_mode = VimMode::Insert;
                self.set_status("-- INSERT --");
                EditorAction::None
            }
            Key::Char('v') => {
                self.vim_mode = VimMode::Visual;
                self.set_status("-- VISUAL --");
                EditorAction::None
            }
            Key::Char('h') => {
                self.navigate(CommandId::MoveLeft, self.vim_mode == VimMode::Visual);
                EditorAction::None
            }
            Key::Char('l') => {
                self.navigate(CommandId::MoveRight, self.vim_mode == VimMode::Visual);
                EditorAction::None
            }
            Key::Char('j') => {
                self.navigate(CommandId::MoveDown, self.vim_mode == VimMode::Visual);
                EditorAction::None
            }
            Key::Char('k') => {
                self.navigate(CommandId::MoveUp, self.vim_mode == VimMode::Visual);
                EditorAction::None
            }
            Key::Char('x') => self.execute(Command::new(CommandId::Delete)),
            Key::Char('u') => self.execute(Command::new(CommandId::Undo)),
            Key::Char('d') => {
                if self.pending_vim == Some('d') {
                    self.pending_vim = None;
                    self.execute(Command::new(CommandId::VimDeleteLine))
                } else {
                    self.pending_vim = Some('d');
                    EditorAction::None
                }
            }
            Key::Char('y') => {
                if self.pending_vim == Some('y') {
                    self.pending_vim = None;
                    self.execute(Command::new(CommandId::VimYankLine))
                } else {
                    self.pending_vim = Some('y');
                    EditorAction::None
                }
            }
            Key::Char(':') => {
                self.open_prompt(PromptKind::CommandLine, ":", "", "named command");
                EditorAction::None
            }
            Key::Char('G') => {
                self.navigate(CommandId::MoveDocEnd, false);
                EditorAction::None
            }
            _ => EditorAction::None,
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        let UiMode::Prompt(mut prompt) = std::mem::replace(&mut self.ui, UiMode::Normal) else {
            return;
        };
        if (key.mods.ctrl || key.mods.alt)
            && (key.key == Key::Char('c') || key.key == Key::Char('g'))
        {
            self.ui = UiMode::Prompt(prompt);
            self.cancel_ui();
            return;
        }
        match key.key {
            Key::Escape => {
                self.cancel_ui();
                return;
            }
            Key::Enter => {
                self.submit_prompt(prompt);
                return;
            }
            Key::Backspace => {
                if prompt.cursor > 0 {
                    let (text, cursor) = remove_char_before(&prompt.input, prompt.cursor);
                    prompt.input = text;
                    prompt.cursor = cursor;
                }
            }
            Key::Delete => {
                if prompt.cursor < prompt.input.chars().count() {
                    let (text, _) = remove_char_before(&prompt.input, prompt.cursor + 1);
                    prompt.input = text;
                }
            }
            Key::Left => prompt.cursor = prompt.cursor.saturating_sub(1),
            Key::Right => prompt.cursor = (prompt.cursor + 1).min(prompt.input.chars().count()),
            Key::Home => prompt.cursor = 0,
            Key::End => prompt.cursor = prompt.input.chars().count(),
            Key::Tab => {
                prompt
                    .input
                    .insert(byte_at_char(&prompt.input, prompt.cursor), '\t');
                prompt.cursor += 1;
            }
            Key::Char(ch) if !ch.is_control() => {
                prompt
                    .input
                    .insert(byte_at_char(&prompt.input, prompt.cursor), ch);
                prompt.cursor += 1;
            }
            Key::Space => {
                prompt
                    .input
                    .insert(byte_at_char(&prompt.input, prompt.cursor), ' ');
                prompt.cursor += 1;
            }
            _ => {}
        }
        self.ui = UiMode::Prompt(prompt);
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        let mut list = match std::mem::replace(&mut self.ui, UiMode::Normal) {
            UiMode::List(list) => list,
            other => {
                self.ui = other;
                return;
            }
        };
        if (key.mods.ctrl || key.mods.alt)
            && (key.key == Key::Char('g') || key.key == Key::Char('c'))
        {
            self.cancel_ui();
            return;
        }
        let page = 10usize;
        match key.key {
            Key::Escape => {
                self.cancel_ui();
                return;
            }
            Key::Enter => {
                self.activate_list_item(&list);
                return;
            }
            Key::Up => {
                list.selected = list.selected.saturating_sub(1);
            }
            Key::Down => {
                if !list.filtered.is_empty() {
                    list.selected = (list.selected + 1).min(list.filtered.len() - 1);
                }
            }
            Key::PageUp => list.selected = list.selected.saturating_sub(page),
            Key::PageDown => {
                list.selected = (list.selected + page).min(list.filtered.len().saturating_sub(1))
            }
            Key::Home => list.selected = 0,
            Key::End => list.selected = list.filtered.len().saturating_sub(1),
            Key::Backspace => {
                if list.cursor > 0 {
                    let (text, cursor) = remove_char_before(&list.query, list.cursor);
                    list.query = text;
                    list.cursor = cursor;
                    self.refresh_list(&mut list);
                }
            }
            Key::Left => list.cursor = list.cursor.saturating_sub(1),
            Key::Right => list.cursor = (list.cursor + 1).min(list.query.chars().count()),
            Key::Char(ch) if !ch.is_control() => {
                list.query
                    .insert(byte_at_char(&list.query, list.cursor), ch);
                list.cursor += 1;
                self.refresh_list(&mut list);
            }
            Key::Space => {
                list.query
                    .insert(byte_at_char(&list.query, list.cursor), ' ');
                list.cursor += 1;
                self.refresh_list(&mut list);
            }
            _ => {}
        }
        if list.selected < list.scroll {
            list.scroll = list.selected;
        }
        self.ui = UiMode::List(list);
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        let UiMode::Help { lines, mut offset } = std::mem::replace(&mut self.ui, UiMode::Normal)
        else {
            return;
        };
        match key.key {
            Key::Escape | Key::Char('q') | Key::Enter => {
                self.cancel_ui();
                return;
            }
            Key::Down | Key::Char('j') => offset = (offset + 1).min(lines.len().saturating_sub(1)),
            Key::Up | Key::Char('k') => offset = offset.saturating_sub(1),
            Key::PageDown => offset = (offset + 10).min(lines.len().saturating_sub(1)),
            Key::PageUp => offset = offset.saturating_sub(10),
            _ => {}
        }
        self.ui = UiMode::Help { lines, offset };
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> EditorAction {
        let UiMode::Confirm { action, .. } = std::mem::replace(&mut self.ui, UiMode::Normal) else {
            return EditorAction::None;
        };
        match key.key {
            Key::Char('y') | Key::Char('Y') | Key::Enter => self.confirm(action),
            Key::Char('n') | Key::Char('N') | Key::Escape => {
                self.set_status("canceled");
                EditorAction::None
            }
            _ => {
                self.ui = UiMode::Confirm {
                    question: "Confirm? [y/N]".into(),
                    action,
                };
                EditorAction::None
            }
        }
    }

    fn confirm(&mut self, action: ConfirmAction) -> EditorAction {
        match action {
            ConfirmAction::Quit => {
                self.should_quit = true;
                EditorAction::Quit
            }
            ConfirmAction::CloseBuffer(id) => {
                self.close_buffer_now(id);
                EditorAction::None
            }
            ConfirmAction::Reload(id) => {
                self.reload_buffer_now(id, true);
                EditorAction::None
            }
            ConfirmAction::SaveAsOverwrite(path) => {
                let result = self.save_as_path(&path);
                self.report_save_result(result);
                EditorAction::None
            }
            ConfirmAction::ApplyWorkspaceEdit => {
                if let Some(review) = self.pending_workspace_review.take() {
                    self.apply_workspace_edit(review);
                }
                EditorAction::None
            }
            ConfirmAction::RecoverRecord(index) => {
                self.activate_recovery(index);
                EditorAction::None
            }
            ConfirmAction::InterruptedSave(index) => {
                self.activate_interrupted_save(index);
                EditorAction::None
            }
        }
    }

    fn ask_confirm(&mut self, question: impl Into<String>, action: ConfirmAction) {
        self.ui = UiMode::Confirm {
            question: question.into(),
            action,
        };
    }

    fn cancel_ui(&mut self) {
        self.ui = UiMode::Normal;
        self.pending_chord.clear();
        self.execute(Command::new(CommandId::CollapseSelection));
    }

    fn open_prompt(&mut self, kind: PromptKind, label: &str, input: &str, hint: &str) {
        self.ui = UiMode::Prompt(PromptState::new(kind, label, input, hint));
    }

    fn open_list(&mut self, kind: ListKind, title: &str, query: &str, items: Vec<ListItem>) {
        let mut list = ListState::new(kind, title, query, items);
        self.refresh_list(&mut list);
        self.ui = UiMode::List(list);
    }

    fn refresh_list(&mut self, list: &mut ListState) {
        if list.kind == ListKind::Files || list.kind == ListKind::FileBrowser {
            let files = self
                .project
                .fuzzy(&list.query, self.config.search.max_results.min(200));
            list.items = files
                .into_iter()
                .map(|path| ListItem {
                    label: path.clone(),
                    payload: path,
                })
                .collect();
            list.filtered = (0..list.items.len()).collect();
        } else {
            let query = list.query.to_ascii_lowercase();
            let mut scored = Vec::new();
            for (index, item) in list.items.iter().enumerate() {
                let score = if list.kind == ListKind::CommandPalette {
                    search::fuzzy_score(&query, &item.label)
                } else if query.is_empty() || item.label.to_ascii_lowercase().contains(&query) {
                    Some(0)
                } else {
                    None
                };
                if let Some(score) = score {
                    scored.push((index, score));
                }
            }
            if list.kind == ListKind::CommandPalette {
                scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
            }
            list.filtered = scored.into_iter().map(|(index, _)| index).collect();
        }
        list.selected = 0;
        list.scroll = 0;
    }
}

fn byte_at_char(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

fn remove_char_before(text: &str, index: usize) -> (String, usize) {
    if index == 0 {
        return (text.to_string(), 0);
    }
    let start = text
        .char_indices()
        .nth(index - 1)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let end = text
        .char_indices()
        .nth(index)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let mut out = String::with_capacity(text.len() - (end - start));
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    (out, index - 1)
}

impl Editor {
    pub fn execute(&mut self, command: Command) -> EditorAction {
        use CommandId::*;
        let mut id = command.id;
        if self.config.editor.profile == ProfileName::SelectionFirst {
            id = match id {
                MoveLeft => SelectLeft,
                MoveRight => SelectRight,
                MoveUp => SelectUp,
                MoveDown => SelectDown,
                MoveWordLeft => SelectWordLeft,
                MoveWordRight => SelectWordRight,
                MoveLineStart => SelectLineStart,
                MoveLineEnd => SelectLineEnd,
                MoveDocStart => SelectDocStart,
                MoveDocEnd => SelectDocEnd,
                other => other,
            };
        }
        let args = command.args.clone();
        let arg = args.first().cloned();
        match id {
            Noop | Confirm => EditorAction::None,
            NewBuffer => {
                let buffer = Buffer::scratch(self.next_buffer_id, "");
                let id = self.add_buffer(buffer);
                self.switch_buffer(id);
                self.set_status("new scratch buffer");
                EditorAction::None
            }
            OpenFile => {
                if let Some(path) = arg {
                    self.open_path(&PathBuf::from(path), None, None);
                } else {
                    self.open_prompt(PromptKind::Open, "Open:", "", "relative or absolute path");
                }
                EditorAction::None
            }
            Save => {
                let result = self.save_buffer(self.active_buffer_id(), false);
                self.report_save_result(result);
                EditorAction::None
            }
            SaveAs => {
                if let Some(path) = arg {
                    let result = self.save_as_path(&PathBuf::from(path));
                    self.report_save_result(result);
                } else {
                    self.open_prompt(
                        PromptKind::SaveAs,
                        "Save as:",
                        "",
                        "new path; existing files require confirmation",
                    );
                }
                EditorAction::None
            }
            SaveAll => {
                self.save_all();
                EditorAction::None
            }
            CloseBuffer => {
                let id = self.active_buffer_id();
                if self.buffers.get(&id).map(Buffer::dirty).unwrap_or(false) {
                    self.ask_confirm(
                        "Buffer has unsaved changes. Close and discard? [y/N]",
                        ConfirmAction::CloseBuffer(id),
                    );
                } else {
                    self.close_buffer_now(id);
                }
                EditorAction::None
            }
            Reload => {
                let id = self.active_buffer_id();
                if self.buffers.get(&id).map(Buffer::dirty).unwrap_or(false) {
                    self.ask_confirm(
                        "Buffer has unsaved changes. Reload from disk? [y/N]",
                        ConfirmAction::Reload(id),
                    );
                } else {
                    self.reload_buffer_now(id, false);
                }
                EditorAction::None
            }
            Overwrite => {
                let result = self.save_buffer(self.active_buffer_id(), true);
                self.report_save_result(result);
                EditorAction::None
            }
            Recover => {
                self.open_recovery_list();
                EditorAction::None
            }
            RecoverSave => {
                self.open_interrupted_saves_list();
                EditorAction::None
            }
            CleanRecovery => {
                self.clean_recovery();
                EditorAction::None
            }
            Quit => {
                let dirty = self.buffers.values().filter(|b| b.dirty()).count();
                if dirty == 0 {
                    self.should_quit = true;
                    EditorAction::Quit
                } else {
                    self.ask_confirm(
                        format!("{dirty} buffer(s) have unsaved changes. Quit and discard? [y/N]"),
                        ConfirmAction::Quit,
                    );
                    EditorAction::None
                }
            }
            ForceQuit => {
                self.should_quit = true;
                EditorAction::Quit
            }
            Suspend => EditorAction::Suspend,
            Undo => {
                self.apply_edit(|buffer| match buffer.undo_outcome()? {
                    Some(o) => Ok(o),
                    None => Err(BufferError::Edit(crate::core::transaction::TxError(
                        "nothing to undo".into(),
                    ))),
                });
                EditorAction::None
            }
            Redo => {
                self.apply_edit(|buffer| match buffer.redo_outcome()? {
                    Some(o) => Ok(o),
                    None => Err(BufferError::Edit(crate::core::transaction::TxError(
                        "nothing to redo".into(),
                    ))),
                });
                EditorAction::None
            }
            Cut => {
                self.cut(false);
                EditorAction::None
            }
            Copy => {
                self.copy(false);
                EditorAction::None
            }
            Paste => {
                self.paste_internal();
                EditorAction::None
            }
            SystemCopy => {
                self.copy(true);
                EditorAction::None
            }
            SystemCut => {
                self.cut(true);
                EditorAction::None
            }
            SystemPaste => {
                self.system_paste();
                EditorAction::None
            }
            DeleteSelection => {
                self.apply_edit(|b| b.insert_text(""));
                EditorAction::None
            }
            Backspace => {
                self.apply_edit(|b| b.backspace());
                EditorAction::None
            }
            Delete => {
                self.apply_edit(|b| b.delete_forward());
                EditorAction::None
            }
            DeleteWordLeft => {
                self.apply_edit(|b| b.delete_word_left());
                EditorAction::None
            }
            DeleteWordRight => {
                self.apply_edit(|b| b.delete_word_right());
                EditorAction::None
            }
            DeleteLine => {
                self.apply_edit(|b| b.delete_line());
                EditorAction::None
            }
            DuplicateLine => {
                self.apply_edit(|b| b.duplicate_line());
                EditorAction::None
            }
            MoveLineUp => {
                self.apply_edit(|b| b.move_lines(false));
                EditorAction::None
            }
            MoveLineDown => {
                self.apply_edit(|b| b.move_lines(true));
                EditorAction::None
            }
            Indent => {
                let width = self.config.editor.tab_width;
                self.apply_edit(move |b| b.indent(false, width, true));
                EditorAction::None
            }
            Outdent => {
                let width = self.config.editor.tab_width;
                self.apply_edit(move |b| b.indent(true, width, true));
                EditorAction::None
            }
            ToggleComment => {
                self.apply_edit(|b| b.toggle_comment());
                EditorAction::None
            }
            InsertTab => {
                let text = " ".repeat(self.config.editor.tab_width);
                self.apply_edit(move |b| b.insert_text(&text));
                EditorAction::None
            }
            InsertNewline => {
                let auto = self.config.editor.auto_indent;
                self.apply_edit(move |b| b.insert_newline(auto));
                EditorAction::None
            }
            MoveLeft | MoveRight | MoveUp | MoveDown | VisualUp | VisualDown | MoveWordLeft
            | MoveWordRight | MoveLineStart | MoveLineEnd | MoveDocStart | MoveDocEnd | PageUp
            | PageDown => {
                self.navigate(id, false);
                EditorAction::None
            }
            SelectLeft | SelectRight | SelectUp | SelectDown | SelectWordLeft | SelectWordRight
            | SelectLineStart | SelectLineEnd | SelectDocStart | SelectDocEnd => {
                let plain = match id {
                    SelectLeft => MoveLeft,
                    SelectRight => MoveRight,
                    SelectUp => MoveUp,
                    SelectDown => MoveDown,
                    SelectWordLeft => MoveWordLeft,
                    SelectWordRight => MoveWordRight,
                    SelectLineStart => MoveLineStart,
                    SelectLineEnd => MoveLineEnd,
                    SelectDocStart => MoveDocStart,
                    SelectDocEnd => MoveDocEnd,
                    _ => unreachable!(),
                };
                self.navigate(plain, true);
                EditorAction::None
            }
            SelectAll => {
                let id = self.active_buffer_id();
                if let Some(buffer) = self.buffers.get_mut(&id) {
                    let end = buffer.document.len_bytes();
                    buffer.set_selections(vec![Selection::new(BytePos(0), end)]);
                }
                EditorAction::None
            }
            CollapseSelection => {
                let id = self.active_buffer_id();
                if let Some(buffer) = self.buffers.get_mut(&id) {
                    let head = buffer.primary().head;
                    buffer.set_carets(vec![head]);
                }
                EditorAction::None
            }
            SwapSelectionEnds => {
                let id = self.active_buffer_id();
                if let Some(buffer) = self.buffers.get_mut(&id) {
                    let swapped = buffer.selections.iter().map(Selection::swap_ends).collect();
                    buffer.set_selections(swapped);
                }
                EditorAction::None
            }
            GoToLine => {
                if let Some(value) = arg {
                    if let Ok(line) = value.parse::<usize>() {
                        self.goto_line_col(line, 1);
                    }
                } else {
                    self.open_prompt(PromptKind::GoToLine, "Line:", "", "1-based line number");
                }
                EditorAction::None
            }
            Find | FindRegex => {
                let mode = if id == FindRegex {
                    SearchMode::Regex
                } else {
                    SearchMode::Literal
                };
                if let Some(pattern) = arg {
                    self.start_buffer_search(pattern, mode, true);
                } else {
                    let existing = self.search.pattern.clone();
                    self.open_prompt(
                        if id == FindRegex {
                            PromptKind::FindRegex
                        } else {
                            PromptKind::Find
                        },
                        "Find:",
                        &existing,
                        "literal/regex toggle in commands",
                    );
                }
                EditorAction::None
            }
            FindNext => {
                self.find_next(true);
                EditorAction::None
            }
            FindPrevious => {
                self.find_next(false);
                EditorAction::None
            }
            Replace | ReplaceAll => {
                let existing = self.search.pattern.clone();
                self.open_prompt(
                    PromptKind::ReplacePattern,
                    "Replace pattern:",
                    &existing,
                    "then enter replacement text",
                );
                EditorAction::None
            }
            ProjectSearch => {
                if let Some(pattern) = arg {
                    self.start_project_search(pattern);
                } else {
                    let existing = self.search.pattern.clone();
                    self.open_prompt(
                        PromptKind::ProjectSearch,
                        "Project search:",
                        &existing,
                        "cancellable; ignore rules apply",
                    );
                }
                EditorAction::None
            }
            ToggleSearchMode => {
                self.search.options.mode = self.search.options.mode.toggle();
                self.set_status(format!("search mode: {}", self.search.options.mode.label()));
                EditorAction::None
            }
            ToggleCaseSensitive => {
                self.search.options.case_sensitive = !self.search.options.case_sensitive;
                self.set_status(format!(
                    "case sensitive: {}",
                    self.search.options.case_sensitive
                ));
                EditorAction::None
            }
            ToggleHidden | ToggleIgnored => {
                if id == ToggleHidden {
                    self.config.search.hidden = !self.config.search.hidden;
                } else {
                    self.config.search.ignored = !self.config.search.ignored;
                }
                self.project
                    .start_scan(self.config.search.hidden, self.config.search.ignored);
                self.set_status("refreshing project files");
                EditorAction::None
            }
            FuzzyFiles => {
                self.open_file_list(ListKind::Files, &arg.unwrap_or_default());
                EditorAction::None
            }
            RefreshFiles => {
                self.project
                    .start_scan(self.config.search.hidden, self.config.search.ignored);
                self.set_status("refreshing files");
                EditorAction::None
            }
            ToggleFileBrowser => {
                self.open_file_list(ListKind::FileBrowser, "");
                EditorAction::None
            }
            NextBuffer | PreviousBuffer => {
                self.cycle_buffer(id == NextBuffer);
                EditorAction::None
            }
            BufferList => {
                self.open_buffer_list();
                EditorAction::None
            }
            SplitVertical => {
                self.split_view(Axis::Vertical);
                EditorAction::None
            }
            SplitHorizontal => {
                self.split_view(Axis::Horizontal);
                EditorAction::None
            }
            FocusLeft | FocusRight | FocusUp | FocusDown => {
                let right = id == FocusRight;
                let down = id == FocusDown;
                self.focus_direction(right, down);
                EditorAction::None
            }
            CloseSplit => {
                self.close_split();
                EditorAction::None
            }
            EqualizeSplits => {
                self.split.equalize();
                self.set_status("splits equalized");
                EditorAction::None
            }
            Help => {
                let lines = self.help_lines();
                self.ui = UiMode::Help { lines, offset: 0 };
                EditorAction::None
            }
            CommandPalette => {
                self.open_command_palette(arg.unwrap_or_default());
                EditorAction::None
            }
            Cancel => {
                self.cancel_ui();
                EditorAction::None
            }
            ConfigReload => {
                self.reload_config();
                EditorAction::None
            }
            ToggleReadOnly => {
                let id = self.active_buffer_id();
                if let Some(buffer) = self.buffers.get_mut(&id) {
                    let value = !buffer.read_only();
                    buffer.set_read_only(value);
                    self.set_status(format!("read-only: {value}"));
                }
                EditorAction::None
            }
            ToggleSoftWrap => {
                self.config.editor.soft_wrap = !self.config.editor.soft_wrap;
                self.set_status(format!("soft wrap: {}", self.config.editor.soft_wrap));
                EditorAction::None
            }
            ToggleLineNumbers => {
                self.config.ui.line_numbers = match self.config.ui.line_numbers {
                    LineNumberMode::Off => LineNumberMode::Absolute,
                    LineNumberMode::Absolute => LineNumberMode::Relative,
                    LineNumberMode::Relative => LineNumberMode::Off,
                };
                self.set_status("line numbers toggled");
                EditorAction::None
            }
            ToggleMouse => {
                self.config.ui.mouse = !self.config.ui.mouse;
                self.set_status(format!("mouse: {}", self.config.ui.mouse));
                EditorAction::None
            }
            AddCursorAbove => {
                self.add_cursor_vertical(false);
                EditorAction::None
            }
            AddCursorBelow => {
                self.add_cursor_vertical(true);
                EditorAction::None
            }
            AddCursorNextMatch => {
                self.add_cursor_next_match();
                EditorAction::None
            }
            RemoveSecondaryCursors => {
                let id = self.active_buffer_id();
                if let Some(buffer) = self.buffers.get_mut(&id) {
                    let head = buffer.primary().head;
                    buffer.set_carets(vec![head]);
                }
                EditorAction::None
            }
            ProfileNormal => {
                self.config.editor.profile = ProfileName::Normal;
                self.vim_mode = VimMode::Insert;
                self.set_status("profile: normal");
                EditorAction::None
            }
            ProfileVim => {
                self.config.editor.profile = ProfileName::Vim;
                self.vim_mode = VimMode::Normal;
                self.set_status("profile: partial Vim subset (see help)");
                EditorAction::None
            }
            ProfileSelectionFirst => {
                self.config.editor.profile = ProfileName::SelectionFirst;
                self.vim_mode = VimMode::Insert;
                self.set_status("profile: selection-first subset");
                EditorAction::None
            }
            VimInsert => {
                self.vim_mode = VimMode::Insert;
                self.set_status("-- INSERT --");
                EditorAction::None
            }
            VimAppend => {
                self.navigate(MoveRight, false);
                self.vim_mode = VimMode::Insert;
                EditorAction::None
            }
            VimNormal => {
                self.vim_mode = VimMode::Normal;
                self.set_status("-- NORMAL --");
                EditorAction::None
            }
            VimVisual => {
                self.vim_mode = VimMode::Visual;
                self.set_status("-- VISUAL --");
                EditorAction::None
            }
            VimDeleteLine | VimYankLine => {
                self.copy_line_to_register();
                if id == VimDeleteLine {
                    self.apply_edit(|b| b.delete_line());
                }
                EditorAction::None
            }
            Completion => {
                self.lsp_request(LspRequestKind::Completion, None);
                EditorAction::None
            }
            Hover => {
                self.lsp_request(LspRequestKind::Hover, None);
                EditorAction::None
            }
            GoToDefinition => {
                self.lsp_request(LspRequestKind::Definition, None);
                EditorAction::None
            }
            References => {
                self.lsp_request(LspRequestKind::References, None);
                EditorAction::None
            }
            RenameSymbol => {
                if let Some(name) = arg {
                    self.lsp_request(LspRequestKind::Rename, Some(&name));
                } else {
                    self.open_prompt(
                        PromptKind::RenameSymbol,
                        "New symbol name:",
                        "",
                        "validated workspace edits only",
                    );
                }
                EditorAction::None
            }
            FormatBuffer => {
                self.lsp_request(LspRequestKind::Formatting, None);
                EditorAction::None
            }
            CodeAction => {
                self.lsp_request(LspRequestKind::CodeAction, None);
                EditorAction::None
            }
            LspRestart => {
                self.restart_lsp();
                EditorAction::None
            }
            LspStatus => {
                self.set_status(self.lsp.status());
                EditorAction::None
            }
            LspStop => {
                self.lsp.shutdown();
                self.set_status("LSP stopped; editing continues");
                EditorAction::None
            }
            GitStatus => {
                self.open_git_list();
                EditorAction::None
            }
            GitRefresh => {
                if let Some(git) = self.git.as_mut() {
                    git.start_refresh();
                    self.set_status("refreshing Git status");
                }
                EditorAction::None
            }
        }
    }

    fn navigate(&mut self, id: CommandId, extend: bool) {
        let buffer_id = self.active_buffer_id();
        let view_id = self.focus;
        let (width, height, soft_wrap, tab_width, cjk_wide, desired_col) = {
            let view = self.views.get(&view_id);
            (
                self.size.cols,
                view.map(|v| v.rect.height).unwrap_or(20),
                self.config.editor.soft_wrap,
                self.config.editor.tab_width,
                self.config.editor.cjk_wide,
                view.map(|v| v.desired_col).unwrap_or(0),
            )
        };
        let Some(buffer) = self.buffers.get(&buffer_id) else {
            return;
        };
        let doc = &buffer.document;
        let current = buffer.primary().head;
        let current_line = doc.byte_to_line(current);
        let line_start = doc.line_to_byte(current_line);
        let (_display_col, _) = doc.display_info(line_start..current, tab_width, cjk_wide);
        let mut new_desired = 0usize;
        let mut target_for = |selection: Selection| -> BytePos {
            let pos = selection.head;
            let line = doc.byte_to_line(pos);
            let start = doc.line_to_byte(line);
            let (col, _) = doc.display_info(start..pos, tab_width, cjk_wide);
            match id {
                CommandId::MoveLeft | CommandId::SelectLeft => doc.prev_grapheme(pos),
                CommandId::MoveRight | CommandId::SelectRight => doc.next_grapheme(pos),
                CommandId::MoveWordLeft | CommandId::SelectWordLeft => doc.prev_word(pos),
                CommandId::MoveWordRight | CommandId::SelectWordRight => doc.next_word(pos),
                CommandId::MoveLineStart | CommandId::SelectLineStart => {
                    doc.line_content_range(line).start
                }
                CommandId::MoveLineEnd | CommandId::SelectLineEnd => {
                    doc.line_content_range(line).end
                }
                CommandId::MoveDocStart | CommandId::SelectDocStart => BytePos(0),
                CommandId::MoveDocEnd | CommandId::SelectDocEnd => doc.len_bytes(),
                CommandId::MoveUp
                | CommandId::MoveDown
                | CommandId::VisualUp
                | CommandId::VisualDown
                | CommandId::SelectUp
                | CommandId::SelectDown
                | CommandId::PageUp
                | CommandId::PageDown => {
                    let delta = match id {
                        CommandId::MoveUp | CommandId::VisualUp | CommandId::SelectUp => -1,
                        CommandId::MoveDown | CommandId::VisualDown | CommandId::SelectDown => 1,
                        CommandId::PageUp => -(height.saturating_sub(2) as isize),
                        CommandId::PageDown => height.saturating_sub(2) as isize,
                        _ => 0,
                    };
                    let preserve = desired_col.max(col.get());
                    new_desired = desired_col;
                    if soft_wrap && delta.abs() == 1 {
                        let options =
                            layout::LayoutOptions::new(width, 256, true, tab_width, cjk_wide);
                        if let Some(next) = layout::visual_move(
                            doc,
                            pos,
                            delta,
                            desired_col.max(col.get()),
                            options,
                        ) {
                            return next;
                        }
                    }
                    let target_line = (line.get() as isize + delta).max(0) as usize;
                    let target_line = LineIndex(target_line.min(doc.len_lines().saturating_sub(1)));
                    let offset = layout::byte_offset_for_display_col(
                        doc,
                        target_line,
                        preserve,
                        tab_width,
                        cjk_wide,
                    );
                    let line_start = doc.line_to_byte(target_line);
                    BytePos(
                        (line_start.get() + offset.get())
                            .min(doc.line_content_range(target_line).end.get()),
                    )
                }
                _ => pos,
            }
        };
        let selections: Vec<Selection> = buffer
            .selections
            .iter()
            .map(|selection| {
                let head = target_for(*selection);
                if extend {
                    selection.with_head(head)
                } else {
                    Selection::caret(head)
                }
            })
            .collect();
        let head = selections.last().map(|s| s.head).unwrap_or(current);
        let new_line_start = doc.line_to_byte(doc.byte_to_line(head));
        let (new_col, _) = doc.display_info(new_line_start..head, tab_width, cjk_wide);
        let final_desired = if matches!(
            id,
            CommandId::MoveLeft
                | CommandId::MoveRight
                | CommandId::MoveWordLeft
                | CommandId::MoveWordRight
                | CommandId::MoveLineStart
                | CommandId::MoveLineEnd
                | CommandId::MoveDocStart
                | CommandId::MoveDocEnd
                | CommandId::SelectLeft
                | CommandId::SelectRight
                | CommandId::SelectWordLeft
                | CommandId::SelectWordRight
                | CommandId::SelectLineStart
                | CommandId::SelectLineEnd
                | CommandId::SelectDocStart
                | CommandId::SelectDocEnd
        ) {
            0
        } else if new_desired > 0 {
            new_desired
        } else {
            new_col.get()
        };
        if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
            buffer.set_selections(selections);
        }
        if let Some(view) = self.views.get_mut(&view_id) {
            view.desired_col = final_desired;
            view.viewport.anchor_byte = head;
        }
        self.pending_chord.clear();
    }

    fn apply_edit<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Buffer) -> Result<EditOutcome, BufferError>,
    {
        let id = self.active_buffer_id();
        self.apply_edit_on(id, f);
    }

    fn apply_edit_on<F>(&mut self, id: BufferId, f: F)
    where
        F: FnOnce(&mut Buffer) -> Result<EditOutcome, BufferError>,
    {
        if self
            .buffers
            .get(&id)
            .map(Buffer::read_only)
            .unwrap_or(false)
        {
            self.set_status_styled(
                "buffer is read-only; toggle read-only explicitly to edit",
                Style::Warning,
                Duration::from_secs(6),
            );
            return;
        }
        let outcome = {
            let Some(buffer) = self.buffers.get_mut(&id) else {
                return;
            };
            f(buffer)
        };
        match outcome {
            Ok(outcome) => self.after_edit(id, outcome),
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(6)),
        }
    }

    fn after_edit(&mut self, id: BufferId, outcome: EditOutcome) {
        let edits = outcome.edits.clone();
        let after_version = outcome.after_version;
        // Diagnostics carry the pre-edit document version; transform their byte
        // ranges deterministically rather than silently trusting stale ranges.
        if let Some(diagnostics) = self.diagnostics.get_mut(&id) {
            for diagnostic in diagnostics.iter_mut() {
                diagnostic.start = transform_pos(diagnostic.start, &edits);
                diagnostic.end = transform_pos(diagnostic.end, &edits);
                diagnostic.version = after_version;
            }
        }
        // Viewport anchors are transformed in byte space; horizontal offsets
        // are recomputed from the transformed line. Each view stays independent.
        if let Some(doc) = self.buffers.get(&id).map(|b| b.document.clone()) {
            for view in self.views.values_mut() {
                if view.buffer_id != id {
                    continue;
                }
                let anchor = transform_pos(view.viewport.anchor_byte, &edits).min(doc.len_bytes());
                let line = doc.byte_to_line(anchor);
                let line_start = doc.line_to_byte(line);
                view.viewport.anchor_byte = anchor;
                view.viewport.top_line = line.get();
                view.viewport.h_offset = BytePos(anchor.get().saturating_sub(line_start.get()));
                view.viewport.top_wrap = 0;
            }
        }
        if let Some(handle) = self.buffer_search.take() {
            handle.cancel();
        }
        self.search.generation = self.search.generation.wrapping_add(1);
        self.search.version = after_version;
        self.search.buffer_id = id;
        self.search.matches.clear();
        self.search.index = 0;
        let (path, doc, language, large_mode) = match self.buffers.get(&id) {
            Some(buffer) => (
                buffer.path().map(Path::to_path_buf),
                buffer.document.clone(),
                buffer.language,
                buffer.large_mode,
            ),
            None => return,
        };
        if !self.config.lsp.enabled || large_mode {
            self.submit_recovery_for(id);
            return;
        }
        if let Some(path) = path {
            if self.lsp.has_active_client() {
                let encoding = self.lsp.encoding();
                let changes = lsp::changes_from_transaction(&outcome.before, &edits, encoding);
                self.lsp
                    .on_document_changed(id, &path, after_version, changes);
            } else {
                self.lsp.sync_document(id, &path, &doc, language);
            }
        }
        self.submit_recovery_for(id);
    }

    fn submit_recovery_for(&mut self, id: BufferId) {
        if !self.config.session.recovery || self.headless {
            return;
        }
        let Some(job) = self
            .buffers
            .get(&id)
            .filter(|b| b.dirty())
            .map(Buffer::recovery_job)
        else {
            return;
        };
        if let Some(manager) = self.recovery.as_ref() {
            manager.submit(job);
        }
    }

    fn save_buffer(&mut self, id: BufferId, allow_overwrite: bool) -> Result<(), String> {
        if self
            .buffers
            .get(&id)
            .map(Buffer::is_scratch)
            .unwrap_or(true)
        {
            self.open_prompt(
                PromptKind::SaveAs,
                "Save as:",
                "",
                "scratch buffer needs a path",
            );
            return Ok(());
        }
        if self
            .buffers
            .get(&id)
            .map(Buffer::read_only)
            .unwrap_or(false)
        {
            return Err("buffer is read-only; use Save As or toggle read-only".into());
        }
        let options = SaveOptions {
            newline: self.config.editor.newline,
            fail_after_bytes: None,
            allow_overwrite,
        };
        let result = {
            let Some(buffer) = self.buffers.get_mut(&id) else {
                return Err("no active buffer".into());
            };
            buffer.save(options).map_err(|e| e.to_string())
        };
        if result.is_ok() {
            if let Some(manager) = self.recovery.as_ref() {
                manager.remove(id);
            }
            if let Some(file) = self.recovery_files.remove(&id) {
                let _ = fs::remove_file(file);
            }
            if let Some(journal) = self.interrupted_journals.remove(&id) {
                let _ = fs::remove_file(journal);
            }
            if let Some(path) = self
                .buffers
                .get(&id)
                .and_then(Buffer::path)
                .map(Path::to_path_buf)
            {
                self.lsp.did_save(&path);
            }
        }
        result
    }

    fn save_as_path(&mut self, path: &Path) -> Result<(), String> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let current = self.active_buffer_id();
        if absolute.exists() {
            let same =
                self.buffers.get(&current).and_then(Buffer::path) == Some(absolute.as_path());
            if same {
                return self.save_buffer(current, false);
            }
            self.ask_confirm(
                format!(
                    "{} exists. Overwrite? [y/N]",
                    short_path(&absolute, &self.root)
                ),
                ConfirmAction::SaveAsOverwrite(absolute),
            );
            return Ok(());
        }
        let options = SaveOptions {
            newline: self.config.editor.newline,
            fail_after_bytes: None,
            allow_overwrite: false,
        };
        let result = {
            let Some(buffer) = self.buffers.get_mut(&current) else {
                return Err("no active buffer".into());
            };
            buffer
                .save_as(&absolute, options)
                .map_err(|e| e.to_string())
        };
        if result.is_ok() {
            if let Some(manager) = self.recovery.as_ref() {
                manager.remove(current);
            }
            self.lsp.did_save(&absolute);
        }
        result
    }

    fn report_save_result(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if matches!(self.ui, UiMode::Normal) {
                    self.set_status("saved");
                }
            }
            Err(e) => self.set_status_styled(e, Style::Warning, Duration::from_secs(10)),
        }
    }

    fn save_all(&mut self) {
        let dirty: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.dirty())
            .map(|(id, _)| *id)
            .collect();
        let mut saved = 0usize;
        for id in dirty {
            if self
                .buffers
                .get(&id)
                .map(Buffer::is_scratch)
                .unwrap_or(true)
            {
                self.switch_buffer(id);
                self.open_prompt(
                    PromptKind::SaveAs,
                    "Save as:",
                    "",
                    "scratch buffer in Save All",
                );
                self.set_status("Save All paused: scratch buffer needs a path");
                return;
            }
            match self.save_buffer(id, false) {
                Ok(()) => saved += 1,
                Err(e) => {
                    self.set_status_styled(e, Style::Warning, Duration::from_secs(10));
                    return;
                }
            }
        }
        self.set_status(format!("saved {saved} buffer(s)"));
    }

    fn close_buffer_now(&mut self, id: BufferId) {
        if self.buffers.len() == 1 {
            let welcome = Buffer::scratch(self.next_buffer_id, "");
            let new_id = self.add_buffer(welcome);
            for view in self.views.values_mut() {
                view.buffer_id = new_id;
            }
        }
        let path = self
            .buffers
            .get(&id)
            .and_then(Buffer::path)
            .map(Path::to_path_buf);
        self.buffers.remove(&id);
        if let Some(manager) = self.recovery.as_ref() {
            manager.remove(id);
        }
        if let Some(path) = path {
            if let Some(client) = self.lsp.client.as_mut() {
                let _ = client.did_close(&lsp::uri_from_path(&path));
            }
        }
        if let Some(new_id) = self.buffers.keys().next().copied() {
            for view in self.views.values_mut() {
                if !self.buffers.contains_key(&view.buffer_id) {
                    view.buffer_id = new_id;
                }
            }
        }
        self.set_status("buffer closed");
    }

    fn reload_buffer_now(&mut self, id: BufferId, force: bool) {
        let read_only = self.cli.read_only;
        let result = self
            .buffers
            .get_mut(&id)
            .ok_or("no active buffer".to_string())
            .and_then(|buffer| buffer.reload(force, read_only).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                if let Some(buffer) = self.buffers.get(&id) {
                    if let Some(path) = buffer.path().map(Path::to_path_buf) {
                        let doc = buffer.document.clone();
                        self.lsp.force_sync(id, &path, &doc, buffer.language);
                    }
                }
                self.set_status("reloaded from disk");
            }
            Err(e) => self.set_status_styled(e, Style::Warning, Duration::from_secs(8)),
        }
    }

    fn cycle_buffer(&mut self, forward: bool) {
        let ids: Vec<BufferId> = self.buffers.keys().copied().collect();
        if ids.is_empty() {
            return;
        }
        let current = self.active_buffer_id();
        let index = ids.iter().position(|id| *id == current).unwrap_or(0);
        let next = if forward {
            (index + 1) % ids.len()
        } else {
            (index + ids.len() - 1) % ids.len()
        };
        self.switch_buffer(ids[next]);
    }

    fn split_view(&mut self, axis: Axis) {
        let rects = self.view_rects();
        let focus_rect = rects
            .iter()
            .find(|(id, _)| *id == self.focus)
            .map(|(_, r)| *r)
            .unwrap_or_default();
        let min = match axis {
            Axis::Vertical => self.config.ui.min_columns * 2,
            Axis::Horizontal => self.config.ui.min_rows * 2,
        };
        let available = match axis {
            Axis::Vertical => focus_rect.width,
            Axis::Horizontal => focus_rect.height,
        };
        if available < min {
            self.set_status_styled(
                "terminal too small for another split",
                Style::Warning,
                Duration::from_secs(5),
            );
            return;
        }
        let buffer_id = self.active_buffer_id();
        let id = self.next_view_id;
        self.next_view_id += 1;
        let viewport = self
            .views
            .get(&self.focus)
            .map(|v| v.viewport)
            .unwrap_or_default();
        let mut view = View::new(id, buffer_id);
        view.viewport = viewport;
        self.views.insert(id, view);
        self.split.split(self.focus, axis, id);
        self.focus = id;
        self.set_status("split created; focus moved to new view");
    }

    fn close_split(&mut self) {
        if self.views.len() <= 1 {
            self.set_status("cannot close the last view");
            return;
        }
        if let Some((_, focus)) = self.split.close(self.focus) {
            self.views.remove(&self.focus);
            self.focus = focus;
            if !self.views.contains_key(&self.focus) {
                self.focus = *self.views.keys().next().unwrap();
            }
            self.set_status("split closed");
        }
    }

    fn focus_direction(&mut self, right: bool, down: bool) {
        let rects = self.view_rects();
        let Some((_, focus)) = rects
            .iter()
            .find(|(id, _)| *id == self.focus)
            .map(|(id, r)| (*id, *r))
        else {
            return;
        };
        let mut best: Option<(ViewId, usize)> = None;
        for (id, rect) in &rects {
            if *id == self.focus || !direction_matches(&focus, rect, right, down) {
                continue;
            }
            let distance = focus.x.abs_diff(rect.x) + focus.y.abs_diff(rect.y);
            if best.map(|(_, d)| distance < d).unwrap_or(true) {
                best = Some((*id, distance));
            }
        }
        if let Some((id, _)) = best {
            self.focus = id;
            self.set_status(format!("focus: view {id}"));
        }
    }

    fn cut(&mut self, system: bool) {
        let id = self.active_buffer_id();
        let (text, linewise) = self.selected_text(id);
        if system {
            self.system_set(&text);
        } else {
            self.clipboard.set_internal(text.clone(), linewise);
        }
        self.apply_edit_on(id, |buffer| buffer.insert_text(""));
        self.set_status(if system {
            "cut to system clipboard"
        } else {
            "cut to internal register"
        });
    }

    fn copy(&mut self, system: bool) {
        let id = self.active_buffer_id();
        let (text, linewise) = self.selected_text(id);
        if system {
            self.system_set(&text);
        } else {
            self.clipboard.set_internal(text, linewise);
        }
        self.set_status(if system {
            "copied to system clipboard"
        } else {
            "copied to internal register"
        });
    }

    fn selected_text(&self, id: BufferId) -> (String, bool) {
        let Some(buffer) = self.buffers.get(&id) else {
            return (String::new(), false);
        };
        let selection = buffer.primary();
        if selection.is_caret() {
            let line = buffer.document.byte_to_line(selection.head);
            let text = buffer.document.line_with_break(line);
            (text, true)
        } else {
            let text = buffer.document.text_in_range(selection.range());
            (text.clone(), text.ends_with(['\n', '\r']))
        }
    }

    fn copy_line_to_register(&mut self) {
        let (text, _) = self.selected_text(self.active_buffer_id());
        self.clipboard.set_internal(text, true);
    }

    fn system_set(&mut self, text: &str) {
        match self.clipboard.system_set(text) {
            Ok(tool) => self.set_status(format!("system clipboard: {tool}")),
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(6)),
        }
        if let Some(sequence) = self.clipboard.osc52_sequence(text) {
            self.output.push(sequence);
        }
    }

    fn system_paste(&mut self) {
        match self.clipboard.system_get() {
            Ok(Some(text)) => {
                self.apply_edit(|buffer| buffer.insert_text(&text));
                self.set_status("pasted from system clipboard");
            }
            Ok(None) => self.set_status("system clipboard returned no text"),
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(6)),
        }
    }

    fn paste_internal(&mut self) {
        let text = self.clipboard.internal().to_string();
        let linewise = self.clipboard.is_linewise();
        if text.is_empty() {
            self.set_status("internal register is empty");
            return;
        }
        if !linewise {
            self.apply_edit(|buffer| buffer.insert_text(&text));
            self.set_status("pasted from internal register");
            return;
        }
        let id = self.active_buffer_id();
        self.apply_edit_on(id, move |buffer| {
            let mut edits = Vec::new();
            let mut ends = Vec::new();
            for selection in &buffer.selections {
                let start = buffer
                    .document
                    .line_to_byte(buffer.document.byte_to_line(selection.head));
                ends.push(BytePos(start.get() + text.len()));
                edits.push(Replacement::insert(start, text.clone()));
            }
            let after = crate::app::buffer::shifted_carets(&edits, &ends);
            buffer.apply_edits("paste-lines", edits, after)
        });
        self.set_status("pasted linewise register");
    }

    fn add_cursor_vertical(&mut self, down: bool) {
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get_mut(&id) else {
            return;
        };
        let width = self.config.editor.tab_width;
        let cjk = self.config.editor.cjk_wide;
        let doc = &buffer.document;
        let mut carets: Vec<BytePos> = buffer.selections.iter().map(|s| s.head).collect();
        for selection in buffer.selections.clone() {
            let line = doc.byte_to_line(selection.head).get();
            let target = if down {
                line + 1
            } else {
                line.saturating_sub(1)
            };
            if (!down && line == 0) || target >= doc.len_lines() {
                continue;
            }
            let line_idx = LineIndex(target);
            let current_line = doc.byte_to_line(selection.head);
            let line_start = doc.line_to_byte(current_line);
            let (col, _) = doc.display_info(line_start..selection.head, width, cjk);
            let offset = layout::byte_offset_for_display_col(doc, line_idx, col.get(), width, cjk);
            carets.push(BytePos(
                (doc.line_to_byte(line_idx).get() + offset.get())
                    .min(doc.line_content_range(line_idx).end.get()),
            ));
        }
        buffer.set_carets(carets);
    }

    fn add_cursor_next_match(&mut self) {
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get_mut(&id) else {
            return;
        };
        let selection = buffer.primary();
        let pattern = if selection.is_caret() {
            let word_start = buffer.document.prev_word(selection.head);
            let word_end = buffer.document.next_word(selection.head);
            let word = buffer.document.text_in_range(word_start..word_end);
            if word.trim().is_empty() {
                self.set_status("select a word or find a pattern first");
                return;
            }
            word
        } else {
            buffer.document.text_in_range(selection.range())
        };
        self.search.pattern = pattern.clone();
        let matches = search::find_matches(&buffer.document, &pattern, self.search.options, None)
            .unwrap_or_default();
        let Some(found) = matches
            .iter()
            .find(|m| m.start >= selection.end())
            .or_else(|| matches.first())
            .cloned()
        else {
            self.set_status("no next match");
            return;
        };
        let mut carets: Vec<BytePos> = buffer.selections.iter().map(|s| s.head).collect();
        carets.push(found.start);
        buffer.set_carets(carets);
        let count = buffer.selections.len();
        self.set_status(format!("{count} cursors"));
    }

    fn start_buffer_search(&mut self, pattern: String, mode: SearchMode, auto_jump: bool) {
        if pattern.is_empty() {
            self.set_status("empty search pattern");
            return;
        }
        if let Some(handle) = self.buffer_search.take() {
            handle.cancel();
        }
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get(&id) else {
            return;
        };
        let document = buffer.document.clone();
        let version = document.version();
        let mut options = self.search.options;
        options.mode = mode;
        options.max_results = self.config.search.max_results;
        self.search.generation = self.search.generation.wrapping_add(1);
        let generation = self.search.generation;
        self.search.pattern = pattern.clone();
        self.search.options = options;
        self.search.version = version;
        self.search.buffer_id = id;
        self.search.matches.clear();
        self.search.index = 0;
        self.search.auto_jump = auto_jump;
        self.buffer_search = Some(search_worker::start(
            document, pattern, options, generation, version,
        ));
        self.set_status("searching buffer…");
    }

    fn find_next(&mut self, forward: bool) {
        let id = self.active_buffer_id();
        let version = self
            .buffers
            .get(&id)
            .map(|b| b.document.version())
            .unwrap_or(0);
        if self.search.pattern.is_empty() {
            self.open_prompt(PromptKind::Find, "Find:", "", "then F3 / Shift+F3");
            return;
        }
        if self.search.matches.is_empty()
            || self.search.version != version
            || self.search.buffer_id != id
        {
            let mode = self.search.options.mode;
            let pattern = self.search.pattern.clone();
            self.start_buffer_search(pattern, mode, true);
            return;
        }
        let n = self.search.matches.len();
        self.search.index = if forward {
            (self.search.index + 1) % n
        } else {
            (self.search.index + n - 1) % n
        };
        let index = self.search.index;
        self.jump_to_match(index);
    }

    fn jump_to_match(&mut self, index: usize) {
        let Some(found) = self.search.matches.get(index).cloned() else {
            return;
        };
        let id = self.active_buffer_id();
        if let Some(buffer) = self.buffers.get_mut(&id) {
            buffer.set_selections(vec![Selection::new(found.start, found.end)]);
        }
        if let Some(view) = self.views.get_mut(&self.focus) {
            view.viewport.top_line = found.line.get().saturating_sub(6);
            view.viewport.top_wrap = 0;
        }
        self.set_status(format!("match {}/{}", index + 1, self.search.matches.len()));
    }

    fn apply_replace_all(&mut self, pattern: String, template: String) {
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get(&id) else {
            return;
        };
        let doc = buffer.document.clone();
        let scope = if buffer.primary().is_caret() {
            SearchScope::Buffer
        } else {
            SearchScope::Selection(buffer.primary())
        };
        let selections = buffer.selections.clone();
        match search::replace_all(
            &doc,
            &pattern,
            &template,
            self.search.options,
            scope,
            &selections,
        ) {
            Ok((transaction, count)) => {
                self.apply_edit_on(id, |buffer| buffer.apply(transaction));
                self.set_status(format!(
                    "replaced {count} match(es) in {} scope",
                    scope.label()
                ));
            }
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(8)),
        }
    }

    fn start_project_search(&mut self, pattern: String) {
        if pattern.is_empty() {
            self.set_status("empty project search pattern");
            return;
        }
        if let Some(handle) = self.project_search.take() {
            handle.cancel();
        }
        self.project_generation = self.project_generation.wrapping_add(1);
        let generation = self.project_generation;
        let mut options = self.search.options;
        options.max_results = self.config.search.max_results;
        self.search.pattern = pattern.clone();
        let files = self.project.files.clone();
        self.project_results.clear();
        self.project_search = Some(project::start_search(
            self.root.clone(),
            files,
            pattern.clone(),
            options,
            generation,
            2 * 1024 * 1024,
        ));
        self.open_list(
            ListKind::ProjectResults,
            &format!("Project search: {pattern}"),
            &pattern,
            Vec::new(),
        );
        self.set_status("project search running; Esc cancels");
    }

    fn submit_prompt(&mut self, prompt: PromptState) {
        let input = prompt.input.trim().to_string();
        match prompt.kind {
            PromptKind::Open => {
                if !input.is_empty() {
                    self.open_path(&PathBuf::from(input), None, None);
                }
            }
            PromptKind::SaveAs => {
                if !input.is_empty() {
                    let result = self.save_as_path(&PathBuf::from(input));
                    self.report_save_result(result);
                }
            }
            PromptKind::Find => self.start_buffer_search(input, SearchMode::Literal, true),
            PromptKind::FindRegex => self.start_buffer_search(input, SearchMode::Regex, true),
            PromptKind::ReplacePattern => {
                if input.is_empty() {
                    self.set_status("empty replace pattern");
                    return;
                }
                self.search.pattern = input;
                let template = self.search.template.clone();
                self.open_prompt(
                    PromptKind::ReplaceTemplate,
                    "Replace with:",
                    &template,
                    "scope: selection if active, otherwise buffer",
                );
            }
            PromptKind::ReplaceTemplate => {
                self.search.template = input.clone();
                let pattern = self.search.pattern.clone();
                let template = input.clone();
                self.apply_replace_all(pattern, template);
            }
            PromptKind::ProjectSearch => self.start_project_search(input),
            PromptKind::GoToLine => match input.parse::<usize>() {
                Ok(line) => self.goto_line_col(line, 1),
                Err(_) => self.set_status_styled(
                    "invalid line number",
                    Style::Warning,
                    Duration::from_secs(5),
                ),
            },
            PromptKind::CommandLine => match Command::parse(&input) {
                Some(command) => {
                    self.execute(command);
                }
                None => self.set_status_styled(
                    "unknown command; try Ctrl+Shift+P",
                    Style::Warning,
                    Duration::from_secs(6),
                ),
            },
            PromptKind::RenameSymbol => self.lsp_request(LspRequestKind::Rename, Some(&input)),
        }
    }

    fn open_command_palette(&mut self, query: String) {
        let items = commands::command_defs()
            .map(|def| ListItem {
                label: format!("{} — {}", def.id.name(), def.id.description()),
                payload: def.id.name().to_string(),
            })
            .collect();
        self.open_list(ListKind::CommandPalette, "Commands", &query, items);
    }

    fn open_buffer_list(&mut self) {
        let items = self
            .buffers
            .iter()
            .map(|(id, buffer)| ListItem {
                label: format!(
                    "[{id}] {}{}",
                    buffer.label,
                    if buffer.dirty() { " *" } else { "" }
                ),
                payload: id.to_string(),
            })
            .collect();
        self.open_list(ListKind::Buffers, "Buffers", "", items);
    }

    fn open_file_list(&mut self, kind: ListKind, query: &str) {
        if self.project.files.is_empty() && !self.headless {
            self.project
                .start_scan(self.config.search.hidden, self.config.search.ignored);
        }
        self.open_list(kind, "Project files", query, Vec::new());
    }

    fn open_recovery_list(&mut self) {
        self.recovery_records = self
            .recovery
            .as_ref()
            .map(RecoveryManager::recover_all)
            .unwrap_or_default();
        let items = self
            .recovery_records
            .iter()
            .enumerate()
            .map(|(index, record)| {
                let label = record
                    .path
                    .as_ref()
                    .map(|p| short_path(p, &self.root))
                    .unwrap_or_else(|| record.label.clone());
                ListItem {
                    label: format!("{label} — snapshot #{}", record.timestamp_ms),
                    payload: index.to_string(),
                }
            })
            .collect();
        self.open_list(ListKind::Recovery, "Unsaved recovery snapshots", "", items);
        if self.recovery_records.is_empty() {
            self.set_status("no local recovery snapshots found");
        }
    }

    fn open_interrupted_saves_list(&mut self) {
        self.interrupted_saves = raster_fs::interrupted_saves(&recovery_dir());
        let items = self
            .interrupted_saves
            .iter()
            .enumerate()
            .map(|(index, save)| ListItem {
                label: format!(
                    "{} — interrupted save",
                    short_path(&save.target, &self.root)
                ),
                payload: index.to_string(),
            })
            .collect();
        self.open_list(ListKind::InterruptedSaves, "Interrupted saves", "", items);
        if self.interrupted_saves.is_empty() {
            self.set_status("no interrupted save journals found");
        }
    }

    fn clean_recovery(&mut self) {
        let retention = Duration::from_secs(
            self.config
                .session
                .recovery_retention_days
                .saturating_mul(86400),
        );
        let mut removed = self
            .recovery
            .as_ref()
            .map(|r| r.cleanup(retention))
            .unwrap_or(0);
        removed += raster_fs::cleanup_save_journals(&recovery_dir(), retention);
        self.set_status(format!("removed {removed} expired recovery item(s)"));
    }

    fn restore_recovery_records(&mut self) {
        let records = self
            .recovery
            .as_ref()
            .map(RecoveryManager::recover_all)
            .unwrap_or_default();
        if records.is_empty() {
            self.set_status("no recovery snapshots to restore");
            return;
        }
        self.recovery_records = records;
        for index in 0..self.recovery_records.len() {
            self.activate_recovery(index);
        }
        self.set_status_styled(
            format!(
                "restored {} recovery snapshot(s); review before saving",
                self.recovery_records.len()
            ),
            Style::Warning,
            Duration::from_secs(10),
        );
    }

    fn activate_recovery(&mut self, index: usize) {
        let Some(record) = self.recovery_records.get(index).cloned() else {
            return;
        };
        let id = self.next_buffer_id;
        let label = record
            .path
            .as_ref()
            .map(|p| short_path(p, &self.root))
            .unwrap_or_else(|| record.label.clone());
        let mut buffer =
            Buffer::from_recovery_document(id, label, record.document, record.path.clone());
        buffer.set_selections(record.cursors);
        let id = self.add_buffer(buffer);
        self.recovery_files.insert(id, record.file.clone());
        self.switch_buffer(id);
        self.set_status_styled(
            "recovery snapshot restored as dirty buffer; save explicitly",
            Style::Warning,
            Duration::from_secs(10),
        );
    }

    fn activate_interrupted_save(&mut self, index: usize) {
        let Some(save) = self.interrupted_saves.get(index).cloned() else {
            return;
        };
        match raster_fs::read_interrupted_save(&save) {
            Ok((document, _bytes)) => {
                let id = self.next_buffer_id;
                let label = short_path(&save.target, &self.root);
                let buffer =
                    Buffer::from_recovery_document(id, label, document, Some(save.target.clone()));
                let id = self.add_buffer(buffer);
                self.interrupted_journals.insert(id, save.journal.clone());
                self.switch_buffer(id);
                self.set_status_styled(
                    "interrupted save restored as dirty buffer; Save or Overwrite explicitly",
                    Style::Warning,
                    Duration::from_secs(10),
                );
            }
            Err(e) => {
                self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(10))
            }
        }
    }

    fn open_git_list(&mut self) {
        let Some(git) = self.git.as_ref() else {
            self.set_status("Git is disabled");
            return;
        };
        let items = git
            .snapshot
            .changed
            .iter()
            .map(|(path, status)| ListItem {
                label: format!("{} {}", status.indicator(), path),
                payload: path.clone(),
            })
            .collect();
        self.open_list(
            ListKind::GitStatus,
            &format!(
                "Git: {}",
                if git.snapshot.branch.is_empty() {
                    "detached"
                } else {
                    &git.snapshot.branch
                }
            ),
            "",
            items,
        );
    }

    fn reload_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            self.set_status("no config path to reload");
            return;
        };
        match Config::load(&path) {
            Ok(mut config) => {
                config.apply_cli(&self.cli);
                if let Err(e) = config.validate() {
                    self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(8));
                    return;
                }
                let key_map = match config.key_map() {
                    Ok(map) => map,
                    Err(e) => {
                        self.set_status_styled(
                            e.to_string(),
                            Style::Warning,
                            Duration::from_secs(8),
                        );
                        return;
                    }
                };
                self.config = config;
                self.key_map = key_map;
                self.set_status("configuration reloaded");
            }
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(8)),
        }
    }

    fn help_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Raster shortcuts — normal profile (nonmodal)".into(),
            String::new(),
            "  Type directly; arrows/Home/End navigate; Shift+arrows select.".into(),
            "  Ctrl+S save   Ctrl+O open   Ctrl+Q quit   Ctrl+N new".into(),
            "  Ctrl+Z undo   Ctrl+Y / Ctrl+Shift+Z redo".into(),
            "  Ctrl+F find   Ctrl+R replace   F3 / Shift+F3 next / previous".into(),
            "  Ctrl+P files  Ctrl+Shift+F project search".into(),
            "  Ctrl+Shift+P commands   Ctrl+B buffers   Ctrl+Tab next buffer".into(),
            "  Ctrl+W v / s split; Ctrl+W h/j/k/l focus; Ctrl+W q close".into(),
            "  Ctrl+A select all   Ctrl+C / X / V internal register".into(),
            "  Ctrl+Shift+C / X / V system clipboard (explicit remote opt-in)".into(),
            "  ':' opens named commands; aliases include :w, :q, :open, :goto".into(),
            "  F1 this help; Esc cancels a prompt or collapses selection".into(),
            String::new(),
            "Terminal ambiguities: Ctrl+S/Ctrl+Q require flow control disabled (raw mode);".into(),
            "use :w / :q if your terminal or multiplexer consumes them. Ctrl+Tab and".into(),
            "modified arrows vary by terminal; :bn / :bp and palette aliases work.".into(),
            String::new(),
            "Optional profiles are partial: Vim subset (i,a,v,h/j/k/l,x,u,dd,yy,:)".into(),
            "and selection-first movement extension. They are not full Vim/Helix clones.".into(),
            "Soft wrap, splits, mouse, syntax, LSP, and Git can be toggled in config.".into(),
            String::new(),
            "Configured key bindings:".into(),
        ];
        for (chord, command) in &self.key_map {
            lines.push(format!("  {:<28} {}", chord.as_string(), command.name()));
        }
        lines.push(String::new());
        lines.push(
            "Data safety: dirty buffers are never silently reloaded; external changes".into(),
        );
        lines.push("require :overwrite or :reload. Recovery files are private and local.".into());
        lines.push(
            "Arabic storage/editing boundaries are supported; full shaping/bidi is not.".into(),
        );
        lines.push("Up/Down move within soft-wrapped rows; PageUp/Down use logical lines.".into());
        lines
    }

    fn activate_list_item(&mut self, list: &ListState) {
        let Some(index) = list.filtered.get(list.selected).copied() else {
            return;
        };
        let Some(item) = list.items.get(index).cloned() else {
            return;
        };
        let kind = list.kind;
        let payload = item.payload;
        match kind {
            ListKind::CommandPalette => {
                if let Some(command) = Command::parse(&payload) {
                    self.execute(command);
                }
            }
            ListKind::Files | ListKind::FileBrowser | ListKind::GitStatus => {
                self.open_path(&PathBuf::from(payload), None, None);
            }
            ListKind::Buffers => {
                if let Ok(id) = payload.parse::<BufferId>() {
                    self.switch_buffer(id);
                }
            }
            ListKind::ProjectResults => {
                if let Ok(index) = payload.parse::<usize>() {
                    self.jump_to_project_result(index);
                }
            }
            ListKind::Completion => {
                let text = payload;
                self.apply_edit(|buffer| buffer.insert_text(&text));
                self.set_status("completion inserted");
            }
            ListKind::LspLocations => {
                if let Ok(index) = payload.parse::<usize>() {
                    self.jump_to_location(index);
                }
            }
            ListKind::CodeActions => {
                if let Ok(index) = payload.parse::<usize>() {
                    self.apply_code_action(index);
                }
            }
            ListKind::Recovery => {
                if let Ok(index) = payload.parse::<usize>() {
                    self.ask_confirm(
                        "Restore this recovery snapshot as a dirty buffer? [y/N]",
                        ConfirmAction::RecoverRecord(index),
                    );
                }
            }
            ListKind::InterruptedSaves => {
                if let Ok(index) = payload.parse::<usize>() {
                    self.ask_confirm(
                        "Restore this interrupted save as a dirty buffer? [y/N]",
                        ConfirmAction::InterruptedSave(index),
                    );
                }
            }
        }
    }

    fn jump_to_project_result(&mut self, index: usize) {
        let Some(result) = self.project_results.get(index).cloned() else {
            return;
        };
        self.open_path(
            &self.root.join(&result.path),
            Some(result.line),
            Some(result.column),
        );
    }

    fn jump_to_location(&mut self, index: usize) {
        let Some(location) = self.locations.get(index).cloned() else {
            return;
        };
        self.open_path(
            &location.path,
            Some(location.line + 1),
            Some(location.col + 1),
        );
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if !self.config.ui.mouse {
            return;
        }
        let rects = self.view_rects();
        let Some((view_id, rect)) = rects
            .iter()
            .find(|(_, r)| r.contains(mouse.x, mouse.y))
            .map(|(id, r)| (*id, *r))
        else {
            return;
        };
        self.focus = view_id;
        let Some(view) = self.views.get(&view_id) else {
            return;
        };
        let buffer_id = view.buffer_id;
        let Some(buffer) = self.buffers.get(&buffer_id) else {
            return;
        };
        let inner = self.view_inner_rect(rect);
        let gutter = self.gutter_width(&buffer.document, inner);
        let text_x = inner.x + gutter;
        let text_width = inner.width.saturating_sub(gutter);
        let text_height = inner.height;
        match mouse.kind {
            MouseKind::ScrollUp => {
                if let Some(view) = self.views.get_mut(&view_id) {
                    view.viewport.top_line = view.viewport.top_line.saturating_sub(3);
                }
            }
            MouseKind::ScrollDown => {
                let max = buffer.document.len_lines().saturating_sub(1);
                if let Some(view) = self.views.get_mut(&view_id) {
                    view.viewport.top_line = (view.viewport.top_line + 3).min(max);
                }
            }
            MouseKind::Press(0) | MouseKind::Drag(0) => {
                let row = mouse
                    .y
                    .saturating_sub(inner.y)
                    .min(text_height.saturating_sub(1));
                let col = mouse
                    .x
                    .saturating_sub(text_x)
                    .min(text_width.saturating_sub(1));
                let layout = layout::build_layout(
                    &buffer.document,
                    view.viewport,
                    text_width,
                    text_height,
                    self.config.editor.soft_wrap,
                    self.config.editor.tab_width,
                    self.config.editor.cjk_wide,
                );
                let pos = layout
                    .get(row)
                    .map(|r| layout::byte_at_row_col(&buffer.document, r, col))
                    .unwrap_or_else(|| buffer.document.len_bytes());
                if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
                    if let MouseKind::Drag(0) = mouse.kind {
                        let anchor = buffer.primary().anchor;
                        buffer.set_selections(vec![Selection::new(anchor, pos)]);
                    } else {
                        buffer.set_carets(vec![pos]);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Editor {
    pub fn render_frame(&mut self, size: Size) -> Frame {
        self.size = size;
        self.syntax.poll();
        let mut frame = Frame::new(size.cols, size.rows, self.config.editor.cjk_wide);
        if size.cols < self.config.ui.min_columns || size.rows < self.config.ui.min_rows {
            let message = format!(
                "Terminal too small: {}x{} (need {}x{})",
                size.cols, size.rows, self.config.ui.min_columns, self.config.ui.min_rows
            );
            if size.rows > 0 {
                frame.set(0, 0, &message, Style::Warning, size.cols);
            }
            return frame;
        }
        let content = self.content_rect();
        let rects = self.view_rects();
        for (view_id, rect) in &rects {
            if let Some(view) = self.views.get_mut(view_id) {
                view.rect = *rect;
                view.focused = *view_id == self.focus;
            }
        }
        let focused = self.focus;
        for (view_id, rect) in rects {
            self.render_view(&mut frame, view_id, rect, content, focused);
        }
        self.render_tabs(&mut frame);
        self.render_status(&mut frame);
        self.render_overlay(&mut frame, content);
        frame
    }

    pub fn dump_plain(&mut self, size: Size) -> String {
        self.render_frame(size).render_plain()
    }

    pub fn render_ansi(&mut self, size: Size) -> Vec<u8> {
        let styling = self.config.color_enabled();
        self.render_frame(size).render_ansi(styling)
    }

    fn content_rect(&self) -> Rect {
        let tab = if self.config.ui.tab_strip && self.size.rows >= self.config.ui.min_rows + 2 {
            1
        } else {
            0
        };
        let status = 1usize;
        let height = self.size.rows.saturating_sub(tab + status);
        Rect::new(0, tab, self.size.cols, height)
    }

    fn view_rects(&self) -> Vec<(ViewId, Rect)> {
        self.split.rectangles(self.content_rect())
    }

    fn view_inner_rect(&self, rect: Rect) -> Rect {
        let content = self.content_rect();
        let mut inner = rect;
        if rect.x > content.x {
            inner.x += 1;
            inner.width = inner.width.saturating_sub(1);
        }
        if rect.y > content.y {
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        inner
    }

    fn gutter_width(&self, doc: &Document, inner: Rect) -> usize {
        if self.config.ui.line_numbers == LineNumberMode::Off {
            return 0;
        }
        let digits = doc
            .len_lines()
            .max(1)
            .to_string()
            .len()
            .saturating_add(2)
            .max(3);
        if inner.width <= digits.saturating_add(2) {
            0
        } else {
            digits
        }
    }

    fn render_tabs(&mut self, frame: &mut Frame) {
        if !self.config.ui.tab_strip || self.size.rows == 0 {
            return;
        }
        frame.fill(0, Style::Plain);
        let active = self.active_buffer_id();
        let mut col = 0usize;
        for (id, buffer) in &self.buffers {
            if col >= self.size.cols {
                break;
            }
            let dirty = if buffer.dirty() { "*" } else { "" };
            let label = format!(
                "{}{}{} ",
                if *id == active { ">" } else { " " },
                ellipsize(&buffer.label, 18, self.config.editor.cjk_wide),
                dirty
            );
            let style = if *id == active {
                Style::Active
            } else {
                Style::Dim
            };
            col += frame.set(0, col, &label, style, self.size.cols.saturating_sub(col));
        }
        if col < self.size.cols {
            frame.set(0, col, "  F1 help", Style::Dim, self.size.cols - col);
        }
    }

    fn render_status(&mut self, frame: &mut Frame) {
        if self.size.rows == 0 {
            return;
        }
        let row = self.size.rows - 1;
        frame.fill(row, Style::Plain);
        let id = self.active_buffer_id();
        let (path, dirty, read_only, large, line, col, language, newline, external) = {
            let buffer = &self.buffers[&id];
            let path = buffer
                .path()
                .map(|p| short_path(p, &self.root))
                .unwrap_or_else(|| buffer.label.clone());
            (
                ellipsize(&path, 24, self.config.editor.cjk_wide),
                buffer.dirty(),
                buffer.read_only(),
                buffer.large_mode,
                buffer.main_caret_line(),
                buffer.main_caret_col(self.config.editor.tab_width, self.config.editor.cjk_wide),
                buffer.language.label(),
                buffer.newline_style().label(),
                buffer.external_change,
            )
        };
        let mode = match self.config.editor.profile {
            ProfileName::Normal => "normal",
            ProfileName::Vim => match self.vim_mode {
                VimMode::Normal => "vim:N",
                VimMode::Insert => "vim:I",
                VimMode::Visual => "vim:V",
            },
            ProfileName::SelectionFirst => "select-first",
        };
        let git = self
            .git
            .as_ref()
            .filter(|g| !g.snapshot.branch.is_empty())
            .map(|g| g.snapshot.branch.clone());
        let mut prefix = format!(
            " {}{}{} {}:{} {} {} {} view{} ",
            path,
            if dirty { "*" } else { "" },
            if read_only { " [ro]" } else { "" },
            line,
            col,
            language,
            newline,
            mode,
            self.focus
        );
        if large {
            prefix.push_str("[large] ");
        }
        if self.config.lsp.enabled && self.lsp.has_active_client() {
            prefix.push_str(&format!(
                "LSP:{} ",
                ellipsize(&self.lsp.status(), 18, false)
            ));
        }
        if let Some(branch) = git {
            prefix.push_str(&format!("git:{branch} "));
        }
        let used = frame.set(row, 0, &prefix, Style::Plain, self.size.cols);
        let message = if Instant::now() <= self.status_until && !self.status.is_empty() {
            self.status.clone()
        } else if external || self.external_warning {
            "external change: :reload or :overwrite".into()
        } else {
            "F1 help · Ctrl+Shift+P commands · Ctrl+Q quit".into()
        };
        let style = if external || self.external_warning || self.status_style == Style::Warning {
            Style::Warning
        } else {
            self.status_style
        };
        if used < self.size.cols {
            frame.set(
                row,
                used,
                &ellipsize(&message, self.size.cols - used, self.config.editor.cjk_wide),
                style,
                self.size.cols - used,
            );
        }
    }

    fn render_overlay(&self, frame: &mut Frame, content: Rect) {
        match &self.ui {
            UiMode::Normal => {}
            UiMode::Prompt(prompt) => {
                if content.height == 0 {
                    return;
                }
                let row = content.y + content.height - 1;
                frame.fill(row, Style::Plain);
                let label_used = frame.set(
                    row,
                    0,
                    &format!("{} ", prompt.label),
                    Style::Active,
                    self.size.cols,
                );
                let input_used = frame.set(
                    row,
                    label_used,
                    &prompt.input,
                    Style::Plain,
                    self.size.cols.saturating_sub(label_used),
                );
                let hint_col = (label_used + input_used + 2).min(self.size.cols.saturating_sub(1));
                frame.set(
                    row,
                    hint_col,
                    &format!("  {}", prompt.hint),
                    Style::Dim,
                    self.size.cols.saturating_sub(hint_col),
                );
                let prefix: String = prompt.input.chars().take(prompt.cursor).collect();
                let cursor_col = (label_used + display_width(&prefix, self.config.editor.cjk_wide))
                    .min(self.size.cols.saturating_sub(1));
                frame.cursor = Some((cursor_col, row));
            }
            UiMode::List(list) => {
                if content.height == 0 {
                    return;
                }
                let panel_height = content
                    .height
                    .saturating_sub(1)
                    .min((self.size.rows / 2).max(5))
                    .max(2);
                let top = content.y + content.height.saturating_sub(panel_height);
                for row in top..top + panel_height {
                    frame.fill(row, Style::Plain);
                }
                frame.set(
                    top,
                    0,
                    &"─".repeat(self.size.cols),
                    Style::Dim,
                    self.size.cols,
                );
                let title = format!(" {}  ", list.title);
                let title_used = frame.set(top, 0, &title, Style::Active, self.size.cols);
                let query = format!("/{}", list.query);
                let query_used = frame.set(
                    top,
                    title_used,
                    &query,
                    Style::Plain,
                    self.size.cols.saturating_sub(title_used),
                );
                let prefix: String = list.query.chars().take(list.cursor).collect();
                let cursor_col =
                    (title_used + 1 + display_width(&prefix, self.config.editor.cjk_wide))
                        .min(self.size.cols.saturating_sub(1));
                frame.cursor = Some((cursor_col, top));
                let _ = query_used;
                let visible = panel_height.saturating_sub(1);
                if list.selected < list.scroll { /* render clamps below */ }
                let scroll = list
                    .scroll
                    .max(list.selected.saturating_sub(visible.saturating_sub(1)));
                for (n, row) in (top + 1..top + panel_height).enumerate() {
                    let position = scroll + n;
                    let Some(index) = list.filtered.get(position).copied() else {
                        break;
                    };
                    let Some(item) = list.items.get(index) else {
                        break;
                    };
                    let selected = position == list.selected;
                    if selected {
                        frame.fill(row, Style::Reverse);
                    }
                    let label = format!("{} {}", if selected { ">" } else { " " }, item.label);
                    frame.set(
                        row,
                        0,
                        &label,
                        if selected {
                            Style::Reverse
                        } else {
                            Style::Plain
                        },
                        self.size.cols,
                    );
                }
                if list.filtered.is_empty() {
                    frame.set(top + 1, 0, "  no matches", Style::Dim, self.size.cols);
                }
            }
            UiMode::Help { lines, offset } => {
                for row in content.y..content.y + content.height {
                    frame.fill(row, Style::Plain);
                }
                if content.height == 0 {
                    return;
                }
                frame.set(
                    content.y,
                    0,
                    " Raster help — ↑/↓ scroll · Esc close ",
                    Style::Active,
                    self.size.cols,
                );
                let visible = content.height.saturating_sub(1);
                for n in 0..visible {
                    if let Some(line) = lines.get(offset.saturating_add(n)) {
                        frame.set(content.y + 1 + n, 0, line, Style::Plain, self.size.cols);
                    }
                }
                frame.cursor = None;
            }
            UiMode::Confirm { question, .. } => {
                if content.height == 0 {
                    return;
                }
                let row = content.y + content.height - 1;
                frame.fill(row, Style::Warning);
                frame.set(
                    row,
                    0,
                    &format!(" {question} "),
                    Style::Warning,
                    self.size.cols,
                );
                frame.cursor = None;
            }
        }
    }

    fn render_view(
        &mut self,
        frame: &mut Frame,
        view_id: ViewId,
        rect: Rect,
        content: Rect,
        focused: ViewId,
    ) {
        if rect.x > content.x {
            for y in rect.y..rect.y + rect.height {
                frame.set(y, rect.x, "│", Style::Dim, 1);
            }
        }
        if rect.y > content.y {
            frame.set(
                rect.y,
                rect.x,
                &"─".repeat(rect.width),
                Style::Dim,
                rect.width,
            );
        }
        let inner = self.view_inner_rect(rect);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let Some(view) = self.views.get(&view_id).cloned() else {
            return;
        };
        let buffer_id = view.buffer_id;
        let Some(buffer) = self.buffers.get(&buffer_id) else {
            return;
        };
        let doc = buffer.document.clone();
        let selections = buffer.selections.clone();
        let language = buffer.language;
        let large_mode = buffer.large_mode;
        let version = doc.version();
        let diagnostics = self
            .diagnostics
            .get(&buffer_id)
            .cloned()
            .unwrap_or_default();
        let diff_lines = self
            .git
            .as_ref()
            .and_then(|g| buffer.path().map(|p| g.diff_lines(p).to_vec()))
            .unwrap_or_default();
        let gutter = self.gutter_width(&doc, inner);
        let text_x = inner.x + gutter;
        let text_width = inner.width.saturating_sub(gutter);
        if text_width == 0 {
            return;
        }
        let primary = selections.last().map(|s| s.head).unwrap_or(BytePos(0));
        let options = layout::LayoutOptions::new(
            text_width,
            inner.height,
            self.config.editor.soft_wrap,
            self.config.editor.tab_width,
            self.config.editor.cjk_wide,
        );
        let viewport = layout::ensure_visible(&doc, view.viewport, primary, options);
        if !large_mode {
            self.syntax.request(
                buffer_id,
                &doc,
                language,
                viewport.top_line,
                viewport
                    .top_line
                    .saturating_add(inner.height)
                    .saturating_add(8),
            );
        }
        if let Some(view) = self.views.get_mut(&view_id) {
            view.viewport = viewport;
        }
        let rows = layout::build_layout(
            &doc,
            viewport,
            text_width,
            inner.height,
            self.config.editor.soft_wrap,
            self.config.editor.tab_width,
            self.config.editor.cjk_wide,
        );
        let active_line = doc.byte_to_line(primary).get();
        let primary_index = selections.len().saturating_sub(1);
        for (row_index, row) in rows.iter().enumerate() {
            let y = inner.y + row_index;
            if gutter > 0 {
                let line_number = if self.config.ui.line_numbers == LineNumberMode::Relative
                    && row.line.get() != active_line
                {
                    row.line.get().abs_diff(active_line)
                } else {
                    row.line.get() + 1
                };
                let label = if row.continuation {
                    format!("{:>width$}", "·", width = gutter - 1)
                } else {
                    format!("{:>width$}", line_number, width = gutter - 1)
                };
                frame.set(
                    y,
                    inner.x,
                    &label,
                    if view_id == focused {
                        Style::Bold
                    } else {
                        Style::Dim
                    },
                    gutter - 1,
                );
                let marker = if diff_lines.contains(&(row.line.get() + 1)) {
                    "!"
                } else {
                    " "
                };
                frame.set(y, inner.x + gutter - 1, marker, Style::Dim, 1);
            }
            let line_start = doc.line_to_byte(row.line);
            let spans = if large_mode {
                &[][..]
            } else {
                self.syntax.spans(buffer_id, version, row.line.get())
            };
            let mut col = 0usize;
            for grapheme in &row.graphemes {
                if col >= text_width {
                    break;
                }
                let mut style = Style::Plain;
                let rel = grapheme.start.get().saturating_sub(line_start.get());
                for span in spans {
                    if rel >= span.start && rel < span.end {
                        style = match span.kind {
                            SyntaxKind::Keyword | SyntaxKind::Type => Style::Bold,
                            SyntaxKind::String | SyntaxKind::Comment => Style::Dim,
                            SyntaxKind::Number => Style::Underline,
                            SyntaxKind::Plain => Style::Plain,
                        };
                        break;
                    }
                }
                if diagnostics.iter().any(|d| {
                    d.version == version && d.start <= grapheme.start && grapheme.start < d.end
                }) {
                    style = Style::Underline;
                }
                if selections
                    .iter()
                    .enumerate()
                    .any(|(i, s)| i != primary_index && s.head == grapheme.start)
                {
                    style = Style::Underline;
                }
                if selections
                    .iter()
                    .any(|s| !s.is_caret() && s.contains(grapheme.start))
                {
                    style = Style::Reverse;
                }
                let written = frame.set(
                    y,
                    text_x + col,
                    &grapheme.text,
                    style,
                    grapheme.width.min(text_width - col),
                );
                if written == 0 {
                    break;
                }
                col += written;
            }
        }
        if view_id == focused {
            if let Some((row, col)) = layout::row_col_for_pos(&doc, &rows, primary) {
                frame.cursor = Some((
                    text_x + col.min(text_width.saturating_sub(1)),
                    inner.y + row.min(inner.height.saturating_sub(1)),
                ));
            }
        }
    }

    pub fn tick(&mut self) {
        self.ensure_lsp_for_active();
        self.syntax.poll();
        if self.project.poll() { /* file index refresh */ }
        self.poll_buffer_search();
        self.poll_project_search();
        self.poll_lsp();
        self.poll_git();
        self.check_external_changes();
        self.submit_recovery_due();
        self.save_session_due();
    }

    /// Start the explicitly configured server for a real file without blocking
    /// input. No server is installed and no project script/config is executed.
    fn ensure_lsp_for_active(&mut self) {
        if !self.config.lsp.enabled {
            return;
        }
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get(&id) else {
            return;
        };
        if buffer.large_mode {
            return;
        }
        let Some(path) = buffer.path().map(Path::to_path_buf) else {
            return;
        };
        let doc = buffer.document.clone();
        let language = buffer.language;
        self.lsp.sync_document(id, &path, &doc, language);
    }

    fn poll_buffer_search(&mut self) {
        let Some(handle) = &self.buffer_search else {
            return;
        };
        let current_generation = self.search.generation;
        let current_version = self
            .buffers
            .get(&self.search.buffer_id)
            .map(|b| b.document.version())
            .unwrap_or(0);
        let mut outcome = None;
        while let Ok(result) = handle.rx.try_recv() {
            if result.generation == current_generation
                && result.version == current_version
                && !result.canceled
            {
                outcome = Some(result);
            }
        }
        let done = handle.is_done();
        if done {
            self.buffer_search = None;
        }
        let Some(outcome) = outcome else {
            return;
        };
        if let Some(error) = outcome.error {
            self.set_status_styled(error, Style::Warning, Duration::from_secs(8));
            return;
        }
        self.search.matches = outcome.matches;
        self.search.index = 0;
        let count = self.search.matches.len();
        if count == 0 {
            self.set_status("no matches");
            return;
        }
        if self.search.auto_jump {
            self.jump_to_match(0);
        }
        self.set_status(format!("{count} match(es)"));
    }

    fn poll_project_search(&mut self) {
        let mut matches = Vec::new();
        let mut done = false;
        let mut truncated = false;
        let generation = self.project_generation;
        if let Some(handle) = &self.project_search {
            if handle.generation == generation {
                while let Ok(message) = handle.rx.try_recv() {
                    match message {
                        project::ProjectSearchMessage::Match(found) => matches.push(found),
                        project::ProjectSearchMessage::Done { truncated: cut, .. } => {
                            done = true;
                            truncated = cut;
                        }
                    }
                }
                if handle.is_done() && matches.is_empty() {
                    done = true;
                }
            } else {
                done = true;
            }
        }
        if done {
            self.project_search = None;
        }
        if matches.is_empty() && !done {
            return;
        }
        self.project_results.extend(matches);
        self.update_project_results_list();
        if done {
            let count = self.project_results.len();
            self.set_status(if truncated {
                format!("project search: first {count} result(s), truncated")
            } else {
                format!("project search: {count} result(s)")
            });
        }
    }

    fn update_project_results_list(&mut self) {
        let items: Vec<ListItem> = self
            .project_results
            .iter()
            .enumerate()
            .map(|(index, result)| ListItem {
                label: format!(
                    "{}:{}:{} {}",
                    result.path, result.line, result.column, result.preview
                ),
                payload: index.to_string(),
            })
            .collect();
        let pattern = self.search.pattern.clone();
        let mut list = match std::mem::replace(&mut self.ui, UiMode::Normal) {
            UiMode::List(list) if list.kind == ListKind::ProjectResults => list,
            other => {
                self.ui = other;
                return;
            }
        };
        list.title = format!("Project search: {pattern}");
        list.items = items;
        self.refresh_list(&mut list);
        self.ui = UiMode::List(list);
    }

    fn poll_lsp(&mut self) {
        let events = self.lsp.poll();
        for event in events {
            self.handle_lsp_event(event);
        }
    }

    fn handle_lsp_event(&mut self, event: LspEvent) {
        self.remember_lsp_event(&event);
        match event {
            LspEvent::Ready { encoding } => {
                for (id, path, doc, language) in self.open_documents() {
                    self.lsp.force_sync(id, &path, &doc, language);
                }
                self.set_status(format!("LSP ready ({})", encoding.label()));
            }
            LspEvent::Status(text) => self.set_status(text),
            LspEvent::ServerExited(text) => self.set_status_styled(
                format!("{text}; editing continues; :lsp-restart"),
                Style::Warning,
                Duration::from_secs(10),
            ),
            LspEvent::Timeout(kind) => self.set_status_styled(
                format!("{kind:?} timed out; editing continues"),
                Style::Warning,
                Duration::from_secs(8),
            ),
            LspEvent::Throttled(n) => {
                self.set_status(format!("LSP output throttled ({n} messages)"))
            }
            LspEvent::Stale {
                request,
                response_version,
                current_version,
            } => {
                self.set_status(format!("discarded stale {request:?} response v{response_version}; buffer v{current_version}"));
            }
            LspEvent::Diagnostics {
                version,
                uri,
                payload,
            } => self.handle_diagnostics(version, &uri, &payload),
            LspEvent::Response {
                request,
                version,
                uri,
                payload,
            } => self.handle_lsp_response(request, version, &uri, &payload),
        }
    }

    /// A bounded log of event kinds and versions (never document contents or
    /// server strings) so transient stale/throttle notices remain inspectable.
    fn remember_lsp_event(&mut self, event: &LspEvent) {
        let label = match event {
            LspEvent::Ready { encoding } => format!("ready {}", encoding.label()),
            LspEvent::Status(_) => "status".to_string(),
            LspEvent::ServerExited(_) => "server-exited".to_string(),
            LspEvent::Timeout(kind) => format!("timeout {kind:?}"),
            LspEvent::Throttled(n) => format!("throttled {n}"),
            LspEvent::Stale { request, response_version, current_version } => format!("discarded stale {request:?} response v{response_version}; buffer v{current_version}"),
            LspEvent::Diagnostics { version, .. } => format!("diagnostics v{version}"),
            LspEvent::Response { request, version, .. } => format!("response {request:?} v{version}"),
        };
        self.lsp_event_log.push(label);
        if self.lsp_event_log.len() > 64 {
            self.lsp_event_log.remove(0);
        }
    }

    pub fn lsp_event_log(&self) -> &[String] {
        &self.lsp_event_log
    }

    fn handle_diagnostics(&mut self, version: u64, uri: &str, payload: &Json) {
        let Some(path) = lsp::path_from_uri(uri) else {
            return;
        };
        let Some((id, document)) = self
            .buffers
            .iter()
            .find(|(_, b)| b.path() == Some(path.as_path()))
            .map(|(id, b)| (*id, b.document.clone()))
        else {
            return;
        };
        if version != document.version() {
            return;
        }
        let encoding = self.lsp.encoding();
        let empty: &[Json] = &[];
        let items = payload
            .get("diagnostics")
            .and_then(Json::as_array)
            .unwrap_or(empty);
        let mut diagnostics = Vec::new();
        for item in items {
            let Some(range) = item.get("range") else {
                continue;
            };
            let Some(start) = range.get("start") else {
                continue;
            };
            let Some(end) = range.get("end") else {
                continue;
            };
            let sl = start.get("line").and_then(Json::as_u64).unwrap_or(0) as usize;
            let sc = start.get("character").and_then(Json::as_u64).unwrap_or(0) as usize;
            let el = end.get("line").and_then(Json::as_u64).unwrap_or(0) as usize;
            let ec = end.get("character").and_then(Json::as_u64).unwrap_or(0) as usize;
            diagnostics.push(Diagnostic {
                start: lsp::lsp_to_byte(&document, sl, sc, encoding),
                end: lsp::lsp_to_byte(&document, el, ec, encoding),
                message: item
                    .get("message")
                    .and_then(Json::as_str)
                    .unwrap_or("diagnostic")
                    .to_string(),
                severity: item.get("severity").and_then(Json::as_u64).unwrap_or(3) as u8,
                version,
            });
        }
        let count = diagnostics.len();
        self.diagnostics.insert(id, diagnostics);
        if count > 0 {
            self.set_status(format!("{count} diagnostic(s)"));
        }
    }

    fn handle_lsp_response(
        &mut self,
        request: LspRequestKind,
        version: u64,
        uri: &str,
        payload: &Json,
    ) {
        let Some(path) = lsp::path_from_uri(uri) else {
            return;
        };
        let Some(id) = self
            .buffers
            .iter()
            .find(|(_, b)| b.path() == Some(path.as_path()))
            .map(|(id, _)| *id)
        else {
            self.set_status("discarded LSP response for an unopened file");
            return;
        };
        let current_version = self
            .buffers
            .get(&id)
            .map(|b| b.document.version())
            .unwrap_or(0);
        if version != current_version {
            self.set_status(format!(
                "discarded stale LSP response v{version}; buffer v{current_version}"
            ));
            return;
        }
        let document = self.buffers[&id].document.clone();
        let encoding = self.lsp.encoding();
        match request {
            LspRequestKind::Completion => {
                self.completions = parse_completion_items(payload);
                let items = self
                    .completions
                    .iter()
                    .enumerate()
                    .map(|(index, text)| ListItem {
                        label: text.clone(),
                        payload: index.to_string(),
                    })
                    .collect();
                self.open_list(ListKind::Completion, "Completions", "", items);
                self.set_status(format!("{} completion(s)", self.completions.len()));
            }
            LspRequestKind::Hover => self.set_status(json_hover_text(payload)),
            LspRequestKind::Definition | LspRequestKind::References => {
                self.locations = self.parse_lsp_locations(payload, encoding, &document, &path);
                if self.locations.is_empty() {
                    self.set_status("no locations");
                    return;
                }
                if self.locations.len() == 1 {
                    self.jump_to_location(0);
                    return;
                }
                let items = self
                    .locations
                    .iter()
                    .enumerate()
                    .map(|(index, location)| ListItem {
                        label: location.label.clone(),
                        payload: index.to_string(),
                    })
                    .collect();
                self.open_list(
                    ListKind::LspLocations,
                    if request == LspRequestKind::Definition {
                        "Definitions"
                    } else {
                        "References"
                    },
                    "",
                    items,
                );
            }
            LspRequestKind::Formatting => {
                self.apply_formatting_edits(id, &document, payload, encoding)
            }
            LspRequestKind::Rename => self.handle_workspace_edit_json(payload, encoding),
            LspRequestKind::CodeAction => self.show_code_actions(payload),
            LspRequestKind::Initialize | LspRequestKind::Shutdown => {}
        }
    }

    fn apply_formatting_edits(
        &mut self,
        id: BufferId,
        doc: &Document,
        payload: &Json,
        encoding: PositionEncoding,
    ) {
        let edits = lsp::text_edits_to_replacements(doc, payload, encoding);
        if edits.is_empty() {
            self.set_status("formatter returned no edits");
            return;
        }
        let selections = self
            .buffers
            .get(&id)
            .map(|b| b.selections.clone())
            .unwrap_or_default();
        let after = selections
            .iter()
            .map(|s| {
                Selection::new(
                    transform_pos(s.anchor, &edits),
                    transform_pos(s.head, &edits),
                )
            })
            .collect();
        self.apply_edit_on(id, |buffer| buffer.apply_edits("format", edits, after));
        self.set_status("formatting applied as one transaction");
    }

    fn handle_workspace_edit_json(&mut self, payload: &Json, encoding: PositionEncoding) {
        let targets = self.workspace_targets();
        match lsp::prepare_workspace_edit(payload, encoding, &targets) {
            Ok(review) => {
                let file_count = review.plans.len();
                let edit_count: usize = review.plans.iter().map(|p| p.edits.len()).sum();
                if review.broad {
                    self.pending_workspace_review = Some(review);
                    self.ask_confirm(format!("Apply {edit_count} server edits across {file_count} file(s)? Review before saving. [y/N]"), ConfirmAction::ApplyWorkspaceEdit);
                } else {
                    self.apply_workspace_edit(review);
                }
            }
            Err(e) => {
                self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(10))
            }
        }
    }

    fn workspace_targets(&self) -> HashMap<String, WorkspaceTarget> {
        let mut out = HashMap::new();
        for buffer in self.buffers.values() {
            if let Some(path) = buffer.path() {
                out.insert(
                    lsp::uri_from_path(path),
                    WorkspaceTarget {
                        path: path.to_path_buf(),
                        document: buffer.document.clone(),
                        version: buffer.document.version(),
                    },
                );
            }
        }
        out
    }

    fn apply_workspace_edit(&mut self, review: WorkspaceEditReview) {
        let file_count = review.plans.len();
        let edit_count: usize = review.plans.iter().map(|p| p.edits.len()).sum();
        let mut applied_files = 0usize;
        for plan in review.plans {
            let Some(id) = self
                .buffers
                .iter()
                .find(|(_, b)| b.path() == Some(plan.path.as_path()))
                .map(|(id, _)| *id)
            else {
                self.set_status_styled(
                    "workspace edit skipped an unopened file",
                    Style::Warning,
                    Duration::from_secs(10),
                );
                continue;
            };
            let version = self
                .buffers
                .get(&id)
                .map(|b| b.document.version())
                .unwrap_or(0);
            if version != plan.expected_version {
                self.set_status_styled(
                    format!(
                        "stale workspace edit rejected for {}",
                        short_path(&plan.path, &self.root)
                    ),
                    Style::Warning,
                    Duration::from_secs(10),
                );
                continue;
            }
            let edits = plan.edits;
            self.apply_edit_on(id, move |buffer| {
                let selections = buffer.selections.clone();
                let version = buffer.document.version();
                buffer.apply(Transaction::new(
                    "lsp-workspace-edit",
                    edits,
                    version,
                    &selections,
                ))
            });
            applied_files += 1;
        }
        self.set_status(format!("applied {edit_count} edit(s) in {applied_files}/{file_count} file(s); undo is per file"));
    }

    fn show_code_actions(&mut self, payload: &Json) {
        let empty: &[Json] = &[];
        let items = payload.as_array().unwrap_or(empty);
        self.code_actions = items.to_vec();
        let list: Vec<ListItem> = self
            .code_actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let title = action
                    .get("title")
                    .and_then(Json::as_str)
                    .or_else(|| {
                        action
                            .get("command")
                            .and_then(|c| c.get("title"))
                            .and_then(Json::as_str)
                    })
                    .unwrap_or("code action");
                ListItem {
                    label: title.to_string(),
                    payload: index.to_string(),
                }
            })
            .collect();
        self.open_list(ListKind::CodeActions, "Code actions", "", list);
    }

    fn apply_code_action(&mut self, index: usize) {
        let Some(action) = self.code_actions.get(index).cloned() else {
            return;
        };
        if let Some(edit) = action.get("edit").cloned() {
            let encoding = self.lsp.encoding();
            self.handle_workspace_edit_json(&edit, encoding);
        } else if action.get("command").is_some() {
            self.set_status_styled(
                "server command not executed automatically; project scripts require explicit trust",
                Style::Warning,
                Duration::from_secs(10),
            );
        } else {
            self.set_status("code action has no applicable edit");
        }
    }

    fn parse_lsp_locations(
        &self,
        payload: &Json,
        encoding: PositionEncoding,
        current: &Document,
        current_path: &Path,
    ) -> Vec<EditorLocation> {
        let empty: &[Json] = &[];
        let items = payload
            .as_array()
            .unwrap_or_else(|| std::slice::from_ref(payload));
        let mut out = Vec::new();
        for item in items {
            let link = item.get("targetUri").or_else(|| item.get("uri"));
            let Some(uri) = link.and_then(Json::as_str) else {
                continue;
            };
            let Some(path) = lsp::path_from_uri(uri) else {
                continue;
            };
            let range = item
                .get("targetSelectionRange")
                .or_else(|| item.get("targetRange"))
                .or_else(|| item.get("range"));
            let Some(start) = range.and_then(|r| r.get("start")) else {
                continue;
            };
            let line = start.get("line").and_then(Json::as_u64).unwrap_or(0) as usize;
            let character = start.get("character").and_then(Json::as_u64).unwrap_or(0) as usize;
            let doc = if path == current_path {
                Some(current.clone())
            } else {
                match fs::metadata(&path) {
                    Ok(meta) if meta.len() <= 10 * 1024 * 1024 => {
                        raster_fs::load_file(&path, true).map(|(doc, _)| doc).ok()
                    }
                    _ => None,
                }
            };
            let col = doc
                .as_ref()
                .map(|doc| {
                    let byte = lsp::lsp_to_byte(doc, line, character, encoding);
                    doc.line_col(
                        byte,
                        self.config.editor.tab_width,
                        self.config.editor.cjk_wide,
                    )
                    .display_col
                    .get()
                })
                .unwrap_or(character);
            let label = format!("{}:{}:{}", short_path(&path, &self.root), line + 1, col + 1);
            out.push(EditorLocation {
                path,
                line,
                col,
                label,
            });
        }
        let _ = empty;
        out
    }

    fn open_documents(&self) -> Vec<(BufferId, PathBuf, Document, Language)> {
        self.buffers
            .values()
            .filter_map(|buffer| {
                if buffer.large_mode {
                    return None;
                }
                buffer.path().map(|path| {
                    (
                        buffer.id,
                        path.to_path_buf(),
                        buffer.document.clone(),
                        buffer.language,
                    )
                })
            })
            .collect()
    }

    fn lsp_request(&mut self, kind: LspRequestKind, extra: Option<&str>) {
        if !self.config.lsp.enabled {
            self.set_status("LSP disabled; configure [lsp] with explicit server commands");
            return;
        }
        let id = self.active_buffer_id();
        let Some(buffer) = self.buffers.get(&id) else {
            return;
        };
        if buffer.large_mode {
            self.set_status("LSP disabled in large-file mode");
            return;
        }
        let Some(path) = buffer.path().map(Path::to_path_buf) else {
            self.set_status("save the buffer before using LSP");
            return;
        };
        let doc = buffer.document.clone();
        let pos = buffer.primary().head;
        match self.lsp.request(kind, id, &path, &doc, pos, extra) {
            Ok(()) => self.set_status(format!("LSP {kind:?} requested")),
            Err(e) => self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(8)),
        }
    }

    fn restart_lsp(&mut self) {
        let id = self.active_buffer_id();
        let language = self
            .buffers
            .get(&id)
            .map(|b| b.language)
            .unwrap_or(Language::Plain);
        let root = self.root.clone();
        self.lsp.restart(language, &root);
        for (id, path, doc, language) in self.open_documents() {
            self.lsp.force_sync(id, &path, &doc, language);
        }
        self.set_status("LSP restarted; editing continued during restart");
    }

    fn poll_git(&mut self) {
        let Some(git) = self.git.as_mut() else {
            return;
        };
        let _updated = git.poll();
        let interval = Duration::from_millis(self.config.git.refresh_ms.max(100));
        if self.last_git_refresh.elapsed() >= interval {
            git.start_refresh();
            self.last_git_refresh = Instant::now();
        }
    }

    fn check_external_changes(&mut self) {
        if self.last_external_check.elapsed() < Duration::from_millis(750) {
            return;
        }
        self.last_external_check = Instant::now();
        let mut any = false;
        for buffer in self.buffers.values_mut() {
            let _ = buffer.check_external();
            any |= buffer.external_change;
        }
        if any && !self.external_warning {
            self.set_status_styled(
                "file changed on disk; no automatic dirty reload — :reload or :overwrite",
                Style::Warning,
                Duration::from_secs(10),
            );
        }
        self.external_warning = any;
    }

    fn submit_recovery_due(&mut self) {
        if !self.config.session.recovery || self.headless {
            return;
        }
        if self.last_recovery_submit.elapsed()
            < Duration::from_millis(self.config.session.recovery_interval_ms.max(100))
        {
            return;
        }
        self.last_recovery_submit = Instant::now();
        let jobs: Vec<RecoveryJob> = self
            .buffers
            .values()
            .filter(|b| b.dirty())
            .map(Buffer::recovery_job)
            .collect();
        if let Some(manager) = self.recovery.as_ref() {
            for job in jobs {
                manager.submit(job);
            }
        }
    }

    fn save_session_due(&mut self) {
        if self.headless || !self.config.session.restore || self.cli.no_session {
            return;
        }
        if self.last_session_save.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_session_save = Instant::now();
        self.save_session();
    }

    pub fn save_session(&mut self) {
        if self.cli.no_session || !self.config.session.restore {
            return;
        }
        let active = self.active_buffer_id();
        let mut files = Vec::new();
        let mut active_index = 0usize;
        for (index, buffer) in self.buffers.values().enumerate() {
            if buffer.id == active {
                active_index = index;
            }
            files.push(SessionFile {
                path: buffer.path().map(Path::to_path_buf),
                cursors: buffer.selections.clone(),
            });
        }
        let mut layout = Json::object();
        let mut views = Json::array();
        for view in self.views.values() {
            views.push(Json::Int(view.buffer_id as i64));
        }
        layout.insert("views", views);
        layout.insert("focus", Json::Int(self.focus as i64));
        let state = SessionState {
            cwd: self.root.clone(),
            files,
            active: active_index,
            layout,
        };
        if let Err(e) = state.save(&self.session_path) {
            self.set_status_styled(
                format!("session save failed: {e}"),
                Style::Warning,
                Duration::from_secs(6),
            );
        }
    }

    fn restore_session(&mut self, state: &SessionState) {
        if state.files.is_empty() {
            return;
        }
        if state.cwd.is_dir() {
            self.root = state.cwd.clone();
            self.project.root = self.root.clone();
            self.lsp.set_root(self.root.clone());
            if self.config.git.enabled && !self.headless {
                self.git = Some(GitStatus::new(self.root.clone()));
            }
        }
        self.buffers.clear();
        let mut ids = Vec::new();
        for (index, file) in state.files.iter().enumerate() {
            let Some(path) = &file.path else {
                continue;
            };
            match Buffer::open(self.next_buffer_id, path, self.cli.read_only, false) {
                Ok(buffer) => {
                    let id = self.add_buffer(buffer);
                    if let Some(buffer) = self.buffers.get_mut(&id) {
                        buffer.set_selections(file.cursors.clone());
                    }
                    ids.push((index, id));
                }
                Err(e) => {
                    self.set_status_styled(e.to_string(), Style::Warning, Duration::from_secs(6))
                }
            }
        }
        if ids.is_empty() {
            let welcome = Buffer::scratch(self.next_buffer_id, WELCOME);
            let id = self.add_buffer(welcome);
            ids.push((0, id));
        }
        let active_entry = state.active.min(state.files.len().saturating_sub(1));
        let active_id = ids
            .iter()
            .find(|(index, _)| *index == active_entry)
            .map(|(_, id)| *id)
            .unwrap_or(ids[0].1);
        let view_ids = state
            .layout
            .get("views")
            .and_then(Json::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|v| v.as_u64())
            .map(|id| id as BufferId)
            .collect::<Vec<_>>();
        let unique: Vec<BufferId> = view_ids
            .into_iter()
            .filter(|id| self.buffers.contains_key(id))
            .take(4)
            .collect();
        let view_buffers = if unique.is_empty() {
            vec![active_id]
        } else {
            unique
        };
        self.views.clear();
        self.next_view_id = 1;
        let mut view_ids = Vec::new();
        for buffer_id in view_buffers {
            let id = self.next_view_id;
            self.next_view_id += 1;
            self.views.insert(id, View::new(id, buffer_id));
            view_ids.push(id);
        }
        self.split = SplitNode::from_leaves(&view_ids);
        self.focus = *self.views.keys().next().unwrap_or(&1);
        if let Some(view) = self.views.get_mut(&self.focus) {
            view.buffer_id = active_id;
        }
        self.set_status("session restored (paths and cursors only; no document contents)");
    }

    pub fn shutdown(&mut self) {
        self.save_session();
        self.lsp.shutdown();
        if let Some(handle) = self.project_search.take() {
            handle.cancel();
        }
        if let Some(handle) = self.buffer_search.take() {
            handle.cancel();
        }
        self.project.cancel_scan();
        if let Some(git) = self.git.as_mut() {
            git.cancel();
        }
    }
}

fn parse_completion_items(payload: &Json) -> Vec<String> {
    let empty: &[Json] = &[];
    let items = payload
        .get("items")
        .and_then(Json::as_array)
        .or_else(|| payload.as_array())
        .unwrap_or(empty);
    items
        .iter()
        .map(|item| {
            item.get("insertText")
                .and_then(Json::as_str)
                .or_else(|| {
                    item.get("textEdit")
                        .and_then(|e| e.get("newText"))
                        .and_then(Json::as_str)
                })
                .or_else(|| item.get("label").and_then(Json::as_str))
                .or_else(|| item.as_str())
                .unwrap_or("")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn json_hover_text(payload: &Json) -> String {
    if let Some(text) = payload.as_str() {
        return text.to_string();
    }
    if let Some(contents) = payload.get("contents") {
        if let Some(text) = contents.as_str() {
            return text.to_string();
        }
        if let Some(value) = contents.get("value").and_then(Json::as_str) {
            return value.to_string();
        }
        if let Some(items) = contents.as_array() {
            return items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("value").and_then(Json::as_str))
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    if let Some(value) = payload.get("value").and_then(Json::as_str) {
        return value.to_string();
    }
    payload
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("hover result")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::commands::{Command, CommandId};
    use crate::term::input::{Key, KeyEvent, Modifiers};
    use crate::util::unique_suffix;
    use std::time::Duration;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raster-editor-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn key(ch: char) -> Event {
        Event::Key(KeyEvent::char(ch))
    }

    #[test]
    fn first_edit_loop_types_undoes_redoes_and_renders() {
        let mut editor = Editor::for_test(Config::default(), "hello");
        assert!(editor
            .render_frame(Size::new(80, 24))
            .render_plain()
            .contains("hello"));
        editor.handle_event(key('X'));
        assert_eq!(editor.document_text(), "Xhello");
        editor.execute(Command::new(CommandId::Undo));
        assert_eq!(editor.document_text(), "hello");
        editor.execute(Command::new(CommandId::Redo));
        assert_eq!(editor.document_text(), "Xhello");
        assert!(editor.active_buffer().dirty());
    }

    #[test]
    fn bracketed_paste_is_text_and_arabic_bytes_are_preserved() {
        let mut editor = Editor::for_test(Config::default(), "");
        editor.handle_event(Event::Paste("مرحبا 😀\nsecond".into()));
        assert_eq!(editor.document_text(), "مرحبا 😀\nsecond");
        assert!(editor
            .render_frame(Size::new(40, 10))
            .render_plain()
            .contains("مرحبا"));
    }

    #[test]
    fn save_reopen_roundtrip_and_external_change_conflict() {
        let dir = TestDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, "one\r\ntwo").unwrap();
        let mut editor = Editor::for_test(Config::default(), "");
        assert!(editor.open_path(&path, None, None));
        assert_eq!(editor.document_text(), "one\r\ntwo");
        editor.handle_event(key('!'));
        editor.execute(Command::new(CommandId::Save));
        assert_eq!(fs::read(&path).unwrap(), b"!one\r\ntwo");
        fs::write(&path, "external").unwrap();
        editor.last_external_check = Instant::now() - Duration::from_secs(2);
        editor.check_external_changes();
        editor.execute(Command::new(CommandId::Save));
        assert!(
            editor.status().contains("changed on disk"),
            "status: {}",
            editor.status()
        );
        assert_eq!(fs::read(&path).unwrap(), b"external");
        assert_eq!(editor.document_text(), "!one\r\ntwo");
    }

    #[test]
    fn dirty_quit_requires_confirmation_and_force_quit_is_explicit() {
        let mut editor = Editor::for_test(Config::default(), "work");
        editor.handle_event(key('!'));
        editor.execute(Command::new(CommandId::Quit));
        assert!(matches!(editor.ui_mode(), UiMode::Confirm { .. }));
        assert!(!editor.should_quit());
        editor.handle_event(key('n'));
        assert!(!editor.should_quit());
        editor.execute(Command::new(CommandId::ForceQuit));
        assert!(editor.should_quit());
    }

    #[test]
    fn resize_during_prompt_and_split_are_safe() {
        let mut editor = Editor::for_test(Config::default(), "one\ntwo\nthree");
        editor.handle_event(key(':'));
        assert!(matches!(editor.ui_mode(), UiMode::Prompt(_)));
        assert!(editor
            .render_frame(Size::new(10, 4))
            .render_plain()
            .contains("Terminal"));
        assert!(editor
            .render_frame(Size::new(40, 4))
            .render_plain()
            .contains("too small"));
        assert!(editor
            .render_frame(Size::new(80, 24))
            .render_plain()
            .contains(":"));
        editor.handle_event(Event::Key(KeyEvent::plain(Key::Escape)));
        editor.execute(Command::new(CommandId::SplitVertical));
        assert_eq!(editor.view_count(), 2);
        assert_eq!(
            editor
                .render_frame(Size::new(80, 24))
                .render_plain()
                .lines()
                .count(),
            24
        );
        editor.execute(Command::new(CommandId::EqualizeSplits));
    }

    #[test]
    fn buffer_search_worker_reports_matches_and_stale_results_are_rejected() {
        let mut editor = Editor::for_test(Config::default(), "needle\nnope\nneedle");
        editor.start_buffer_search("needle".into(), SearchMode::Literal, false);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && editor.buffer_search.is_some() {
            editor.poll_buffer_search();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(editor.search.matches.len(), 2);
        editor.handle_event(key('!'));
        editor.poll_buffer_search();
        assert!(editor.search.matches.is_empty());
    }

    #[test]
    fn mouse_can_be_disabled_without_affecting_editing() {
        let mut config = Config::default();
        config.ui.mouse = false;
        let mut editor = Editor::for_test(config, "abc");
        let head = editor.active_buffer().primary().head;
        editor.handle_event(Event::Mouse(MouseEvent {
            kind: MouseKind::Press(0),
            x: 3,
            y: 3,
            mods: Modifiers::default(),
        }));
        assert_eq!(editor.active_buffer().primary().head, head);
        editor.handle_event(key('z'));
        assert_eq!(editor.document_text(), "zabc");
    }

    #[test]
    fn palette_commands_and_help_are_discoverable() {
        let mut editor = Editor::for_test(Config::default(), "x");
        editor.execute(Command::new(CommandId::CommandPalette));
        assert!(matches!(
            editor.ui_mode(),
            UiMode::List(ListState {
                kind: ListKind::CommandPalette,
                ..
            })
        ));
        editor.handle_event(Event::Key(KeyEvent::plain(Key::Escape)));
        editor.execute(Command::new(CommandId::Help));
        assert!(matches!(editor.ui_mode(), UiMode::Help { .. }));
        let frame = editor.render_frame(Size::new(100, 30)).render_plain();
        assert!(frame.contains("Raster help"));
        assert!(frame.contains("Ctrl+S"));
    }
}
