//! TOML configuration with a deliberately documented subset.
//!
//! Supported: comments, tables, dotted keys, arrays, inline tables, basic and
//! literal strings, integers, floats, booleans, and arrays of tables.
//! Unsupported: multiline strings, dates/times, binary/octal literals, and
//! arbitrary expression syntax. Unsupported syntax is a clear parse error,
//! never a silently ignored setting.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::Path;

use crate::cli::CliOptions;
use crate::core::commands::{default_key_map, CommandId, KeyMap};
use crate::term::input::{ChordError, KeyChord};

#[derive(Clone, Debug, PartialEq)]
pub enum TomlValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<TomlValue>),
    Table(BTreeMap<String, TomlValue>),
}

impl TomlValue {
    fn table() -> Self {
        TomlValue::Table(BTreeMap::new())
    }

    fn as_table_mut(&mut self) -> Option<&mut BTreeMap<String, TomlValue>> {
        match self {
            TomlValue::Table(map) => Some(map),
            _ => None,
        }
    }

    fn as_array_mut(&mut self) -> Option<&mut Vec<TomlValue>> {
        match self {
            TomlValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            TomlValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            TomlValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            TomlValue::Integer(v) => Some(*v),
            TomlValue::Float(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[TomlValue]> {
        match self {
            TomlValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, TomlValue>> {
        match self {
            TomlValue::Table(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TomlError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for TomlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TOML {}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for TomlError {}

pub fn parse_toml(text: &str) -> Result<TomlValue, TomlError> {
    let mut parser = TomlParser::new(text);
    parser.parse()
}

struct TomlParser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    root: TomlValue,
    current_path: Vec<String>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> TomlParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            root: TomlValue::table(),
            current_path: Vec::new(),
            _marker: std::marker::PhantomData,
        }
    }

    fn error(&self, message: impl Into<String>) -> TomlError {
        TomlError {
            line: self.line,
            column: self.col,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\r')) {
            self.next();
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while matches!(
                self.peek(),
                Some(' ') | Some('\t') | Some('\r') | Some('\n')
            ) {
                self.next();
            }
            if self.peek() == Some('#') {
                while !matches!(self.peek(), None | Some('\n')) {
                    self.next();
                }
            } else {
                break;
            }
        }
    }

    fn parse(&mut self) -> Result<TomlValue, TomlError> {
        loop {
            self.skip_ws_and_comments();
            if self.peek().is_none() {
                return Ok(std::mem::replace(&mut self.root, TomlValue::table()));
            }
            if self.peek() == Some('[') {
                self.parse_header()?;
            } else {
                let key_path = self.parse_key_path()?;
                self.skip_spaces();
                if self.next() != Some('=') {
                    return Err(self.error("expected '='"));
                }
                self.skip_spaces();
                let value = self.parse_value()?;
                self.end_of_statement()?;
                let mut full_path = self.current_path.clone();
                full_path.extend(key_path);
                insert_value(&mut self.root, &full_path, value)?;
            }
        }
    }

    fn parse_header(&mut self) -> Result<(), TomlError> {
        if self.next() != Some('[') {
            return Err(self.error("expected '['"));
        }
        let array = self.peek() == Some('[');
        if array {
            self.next();
        }
        self.skip_spaces();
        let path = self.parse_key_path()?;
        self.skip_spaces();
        let expected = if array { "]]" } else { "]" };
        for ch in expected.chars() {
            if self.next() != Some(ch) {
                return Err(self.error(format!("expected {expected:?}")));
            }
        }
        self.end_of_statement()?;
        if array {
            let item = TomlValue::table();
            let container = ensure_container(&mut self.root, &path, true)?;
            container.push(item);
            let index = container.len() - 1;
            let mut current = path;
            current.push(index.to_string());
            self.current_path = current;
        } else {
            ensure_table(&mut self.root, &path)?;
            self.current_path = path;
        }
        Ok(())
    }

    fn end_of_statement(&mut self) -> Result<(), TomlError> {
        self.skip_spaces();
        if self.peek() == Some('#') {
            while !matches!(self.peek(), None | Some('\n')) {
                self.next();
            }
        }
        match self.peek() {
            None | Some('\n') => {
                self.next();
                Ok(())
            }
            Some(c) => Err(self.error(format!("unexpected {c:?} after value"))),
        }
    }

    fn parse_key_path(&mut self) -> Result<Vec<String>, TomlError> {
        let mut path = Vec::new();
        loop {
            self.skip_spaces();
            let key = match self.peek() {
                Some('"') => self.parse_basic_string()?,
                Some('\'') => self.parse_literal_string()?,
                Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => {
                    let mut out = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                            out.push(c);
                            self.next();
                        } else {
                            break;
                        }
                    }
                    out
                }
                _ => return Err(self.error("expected key")),
            };
            path.push(key);
            self.skip_spaces();
            if self.peek() == Some('.') {
                self.next();
            } else {
                break;
            }
        }
        if path.is_empty() {
            return Err(self.error("empty key path"));
        }
        Ok(path)
    }

    fn parse_value(&mut self) -> Result<TomlValue, TomlError> {
        self.skip_spaces();
        match self.peek() {
            Some('"') => Ok(TomlValue::String(self.parse_basic_string()?)),
            Some('\'') => Ok(TomlValue::String(self.parse_literal_string()?)),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_inline_table(),
            Some(c) if c == '+' || c == '-' || c.is_ascii_digit() => self.parse_number_or_word(),
            Some(c) if c.is_ascii_alphabetic() => self.parse_number_or_word(),
            _ => Err(self.error("expected value")),
        }
    }

    fn parse_basic_string(&mut self) -> Result<String, TomlError> {
        if self.next() != Some('"') {
            return Err(self.error("expected string"));
        }
        if self.peek() == Some('"') && self.chars.get(self.pos + 1) == Some(&'"') {
            return Err(self.error("multiline strings are not supported"));
        }
        let mut out = String::new();
        loop {
            match self.next() {
                Some('"') => return Ok(out),
                Some('\\') => {
                    let esc = self.next().ok_or_else(|| self.error("bad escape"))?;
                    match esc {
                        'b' => out.push('\u{8}'),
                        't' => out.push('\t'),
                        'n' => out.push('\n'),
                        'f' => out.push('\u{c}'),
                        'r' => out.push('\r'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'u' => out.push(self.parse_hex(4)?),
                        'U' => out.push(self.parse_hex(8)?),
                        '\n' => return Err(self.error("multiline strings are not supported")),
                        c => return Err(self.error(format!("unsupported escape \\{c}"))),
                    }
                }
                Some('\n') => return Err(self.error("multiline strings are not supported")),
                Some(c) if c.is_control() => return Err(self.error("control character in string")),
                Some(c) => out.push(c),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn parse_hex(&mut self, digits: usize) -> Result<char, TomlError> {
        let mut value = 0u32;
        for _ in 0..digits {
            let c = self
                .next()
                .ok_or_else(|| self.error("short unicode escape"))?;
            value = value * 16 + c.to_digit(16).ok_or_else(|| self.error("bad hex digit"))?;
        }
        char::from_u32(value).ok_or_else(|| self.error("invalid Unicode scalar"))
    }

    fn parse_literal_string(&mut self) -> Result<String, TomlError> {
        if self.next() != Some('\'') {
            return Err(self.error("expected literal string"));
        }
        if self.peek() == Some('\'') && self.chars.get(self.pos + 1) == Some(&'\'') {
            return Err(self.error("multiline strings are not supported"));
        }
        let mut out = String::new();
        loop {
            match self.next() {
                Some('\'') => return Ok(out),
                Some('\n') => return Err(self.error("multiline strings are not supported")),
                Some(c) => out.push(c),
                None => return Err(self.error("unterminated literal string")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<TomlValue, TomlError> {
        if self.next() != Some('[') {
            return Err(self.error("expected array"));
        }
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == Some(']') {
                self.next();
                return Ok(TomlValue::Array(items));
            }
            items.push(self.parse_value()?);
            self.skip_ws_and_comments();
            match self.peek() {
                Some(',') => {
                    self.next();
                }
                Some(']') => {
                    self.next();
                    return Ok(TomlValue::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn parse_inline_table(&mut self) -> Result<TomlValue, TomlError> {
        if self.next() != Some('{') {
            return Err(self.error("expected inline table"));
        }
        let mut map = BTreeMap::new();
        loop {
            self.skip_spaces();
            if self.peek() == Some('}') {
                self.next();
                return Ok(TomlValue::Table(map));
            }
            let path = self.parse_key_path()?;
            self.skip_spaces();
            if self.next() != Some('=') {
                return Err(self.error("expected '='"));
            }
            self.skip_spaces();
            let value = self.parse_value()?;
            map = value_and_map(map, path, value)?;
            self.skip_spaces();
            match self.peek() {
                Some(',') => {
                    self.next();
                }
                Some('}') => {
                    self.next();
                    return Ok(TomlValue::Table(map));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn parse_number_or_word(&mut self) -> Result<TomlValue, TomlError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.') {
                self.next();
            } else {
                break;
            }
        }
        let token: String = self.chars[start..self.pos].iter().collect();
        match token.as_str() {
            "true" => return Ok(TomlValue::Bool(true)),
            "false" => return Ok(TomlValue::Bool(false)),
            _ => {}
        }
        let clean = token.replace('_', "");
        if clean.starts_with("0x") || clean.starts_with("0X") {
            let n =
                i64::from_str_radix(&clean[2..], 16).map_err(|_| self.error("bad hex integer"))?;
            return Ok(TomlValue::Integer(n));
        }
        if clean.contains('.') || clean.contains('e') || clean.contains('E') {
            return clean
                .parse::<f64>()
                .map(TomlValue::Float)
                .map_err(|_| self.error("bad float"));
        }
        clean
            .parse::<i64>()
            .map(TomlValue::Integer)
            .map_err(|_| self.error("bad integer or date (dates are unsupported)"))
    }
}

fn value_and_map(
    mut map: BTreeMap<String, TomlValue>,
    path: Vec<String>,
    value: TomlValue,
) -> Result<BTreeMap<String, TomlValue>, TomlError> {
    let mut root = TomlValue::Table(std::mem::take(&mut map));
    insert_value(&mut root, &path, value)?;
    if let TomlValue::Table(m) = root {
        map = m;
    }
    Ok(map)
}

fn ensure_table(root: &mut TomlValue, path: &[String]) -> Result<(), TomlError> {
    let mut node = root;
    for key in path {
        let map = node.as_table_mut().ok_or_else(|| TomlError {
            line: 0,
            column: 0,
            message: "table conflicts with value".into(),
        })?;
        node = map.entry(key.clone()).or_insert_with(TomlValue::table);
        if !matches!(node, TomlValue::Table(_)) {
            return Err(TomlError {
                line: 0,
                column: 0,
                message: "table conflicts with value".into(),
            });
        }
    }
    Ok(())
}

fn ensure_container<'a>(
    root: &'a mut TomlValue,
    path: &[String],
    array: bool,
) -> Result<&'a mut Vec<TomlValue>, TomlError> {
    if path.is_empty() {
        return Err(TomlError {
            line: 0,
            column: 0,
            message: "empty array-of-tables path".into(),
        });
    }
    let (parent_path, last) = path.split_at(path.len() - 1);
    ensure_table(root, parent_path)?;
    let mut node = root;
    for key in parent_path {
        node = node.as_table_mut().unwrap().get_mut(key).unwrap();
    }
    let map = node.as_table_mut().unwrap();
    let key = &last[0];
    if !map.contains_key(key) {
        map.insert(
            key.clone(),
            if array {
                TomlValue::Array(Vec::new())
            } else {
                TomlValue::table()
            },
        );
    }
    map.get_mut(key)
        .and_then(TomlValue::as_array_mut)
        .ok_or_else(|| TomlError {
            line: 0,
            column: 0,
            message: "array of tables conflicts with table".into(),
        })
}

fn insert_value(root: &mut TomlValue, path: &[String], value: TomlValue) -> Result<(), TomlError> {
    insert_at(root, path, value)
}

fn insert_at(node: &mut TomlValue, path: &[String], value: TomlValue) -> Result<(), TomlError> {
    if path.is_empty() {
        return Err(TomlError {
            line: 0,
            column: 0,
            message: "empty key".into(),
        });
    }
    if path.len() == 1 {
        let map = node.as_table_mut().ok_or_else(|| TomlError {
            line: 0,
            column: 0,
            message: "key conflicts with value".into(),
        })?;
        if map.insert(path[0].clone(), value).is_some() {
            return Err(TomlError {
                line: 0,
                column: 0,
                message: format!("duplicate key {}", path[0]),
            });
        }
        return Ok(());
    }
    // An array-of-tables header uses a numeric path segment, e.g.
    // lsp.servers.0.language. Only that exact shape descends into an array.
    if path.len() >= 3 {
        if let Ok(index) = path[1].parse::<usize>() {
            let map = node.as_table_mut().ok_or_else(|| TomlError {
                line: 0,
                column: 0,
                message: "cannot dot into array of tables".into(),
            })?;
            if let Some(items) = map.get_mut(&path[0]).and_then(TomlValue::as_array_mut) {
                let item = items.get_mut(index).ok_or_else(|| TomlError {
                    line: 0,
                    column: 0,
                    message: "array-of-tables index out of bounds".into(),
                })?;
                return insert_at(item, &path[2..], value);
            }
        }
    }
    if node.as_table().is_none() {
        return Err(TomlError {
            line: 0,
            column: 0,
            message: "cannot dot into array of tables".into(),
        });
    }
    let map = node.as_table_mut().ok_or_else(|| TomlError {
        line: 0,
        column: 0,
        message: "key conflicts with value".into(),
    })?;
    let child = map.entry(path[0].clone()).or_insert_with(TomlValue::table);
    insert_at(child, &path[1..], value)
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum ProfileName {
    Normal,
    Vim,
    SelectionFirst,
}

impl ProfileName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Vim => "vim",
            Self::SelectionFirst => "selection-first",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "vim" => Some(Self::Vim),
            "selection-first" | "selection_first" | "helix" => Some(Self::SelectionFirst),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum ColorMode {
    Auto,
    Never,
    Always,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum LineNumberMode {
    Off,
    Absolute,
    Relative,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub enum NewlineMode {
    #[default]
    Preserve,
    Lf,
    Crlf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspServerConfig {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub editor: EditorConfig,
    pub ui: UiConfig,
    pub clipboard: ClipboardConfig,
    pub search: SearchConfig,
    pub session: SessionConfig,
    pub lsp: LspConfig,
    pub git: GitConfig,
    pub key_overrides: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorConfig {
    pub tab_width: usize,
    pub soft_wrap: bool,
    pub auto_indent: bool,
    pub profile: ProfileName,
    pub large_file_threshold: u64,
    pub cjk_wide: bool,
    pub newline: NewlineMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiConfig {
    pub color: ColorMode,
    pub mouse: bool,
    pub tab_strip: bool,
    pub line_numbers: LineNumberMode,
    pub min_columns: usize,
    pub min_rows: usize,
    pub alt_screen: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardConfig {
    pub system: bool,
    pub osc52: bool,
    pub osc52_max_bytes: usize,
    pub allow_remote: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchConfig {
    pub hidden: bool,
    pub ignored: bool,
    pub max_results: usize,
    pub context_lines: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    pub restore: bool,
    pub recovery: bool,
    pub recovery_interval_ms: u64,
    pub recovery_retention_days: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LspConfig {
    pub enabled: bool,
    pub request_timeout_ms: u64,
    pub servers: Vec<LspServerConfig>,
    pub trust_workspace_edits: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GitConfig {
    pub enabled: bool,
    pub refresh_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: EditorConfig {
                tab_width: 4,
                soft_wrap: true,
                auto_indent: true,
                profile: ProfileName::Normal,
                large_file_threshold: 10 * 1024 * 1024,
                cjk_wide: false,
                newline: NewlineMode::Preserve,
            },
            ui: UiConfig {
                color: ColorMode::Auto,
                mouse: true,
                tab_strip: true,
                line_numbers: LineNumberMode::Absolute,
                min_columns: 24,
                min_rows: 6,
                alt_screen: true,
            },
            clipboard: ClipboardConfig {
                system: false,
                osc52: false,
                osc52_max_bytes: 64 * 1024,
                allow_remote: false,
            },
            search: SearchConfig {
                hidden: false,
                ignored: false,
                max_results: 500,
                context_lines: 0,
            },
            session: SessionConfig {
                restore: true,
                recovery: true,
                recovery_interval_ms: 1000,
                recovery_retention_days: 7,
            },
            lsp: LspConfig {
                enabled: false,
                request_timeout_ms: 5000,
                servers: Vec::new(),
                trust_workspace_edits: false,
            },
            git: GitConfig {
                enabled: true,
                refresh_ms: 5000,
            },
            key_overrides: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ConfigError {}

impl From<TomlError> for ConfigError {
    fn from(e: TomlError) -> Self {
        Self(e.to_string())
    }
}
impl From<ChordError> for ConfigError {
    fn from(e: ChordError) -> Self {
        Self(e.to_string())
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)
            .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
        Self::from_toml(&text)
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let root = parse_toml(text)?;
        let mut config = Self::default();
        let mut known: Vec<Vec<String>> = Vec::new();
        apply_table(&root, &mut known, &mut config)?;
        validate_all_known(&root, &known)?;
        config.validate()?;
        Ok(config)
    }

    pub fn apply_cli(&mut self, opts: &CliOptions) {
        if opts.no_mouse {
            self.ui.mouse = false;
        }
        if opts.no_color {
            self.ui.color = ColorMode::Never;
        }
        if opts.no_lsp {
            self.lsp.enabled = false;
        }
        if opts.large_file {
            self.editor.large_file_threshold = 0;
        }
        if opts.no_session {
            self.session.restore = false;
        }
        if opts.no_alt_screen {
            self.ui.alt_screen = false;
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=16).contains(&self.editor.tab_width) {
            return Err(ConfigError("editor.tab_width must be 1..=16".into()));
        }
        if self.ui.min_columns < 8 || self.ui.min_rows < 3 {
            return Err(ConfigError(
                "ui minimum dimensions must be at least 8x3".into(),
            ));
        }
        if self.clipboard.osc52_max_bytes == 0 || self.clipboard.osc52_max_bytes > 1_000_000 {
            return Err(ConfigError(
                "clipboard.osc52_max_bytes must be 1..=1000000".into(),
            ));
        }
        if self.search.max_results == 0 || self.search.max_results > 100_000 {
            return Err(ConfigError("search.max_results must be 1..=100000".into()));
        }
        if self.session.recovery_interval_ms < 100 {
            return Err(ConfigError(
                "session.recovery_interval_ms must be >= 100".into(),
            ));
        }
        if self.lsp.request_timeout_ms < 100 || self.lsp.request_timeout_ms > 120_000 {
            return Err(ConfigError(
                "lsp.request_timeout_ms must be 100..=120000".into(),
            ));
        }
        for server in &self.lsp.servers {
            if server.command.trim().is_empty() {
                return Err(ConfigError("lsp server command must not be empty".into()));
            }
            if !["rust", "c", "cpp", "go", "python"].contains(&server.language.as_str()) {
                return Err(ConfigError(format!(
                    "unsupported LSP language {:?}",
                    server.language
                )));
            }
        }
        Ok(())
    }

    pub fn key_map(&self) -> Result<KeyMap, ConfigError> {
        let mut map = default_key_map()?;
        for (chord_text, command_text) in &self.key_overrides {
            let chord = KeyChord::parse(chord_text)?;
            if command_text == "unbind" {
                map.remove(&chord);
                continue;
            }
            let id = CommandId::from_name(command_text).ok_or_else(|| {
                ConfigError(format!(
                    "unknown command {command_text:?} for key {chord_text:?}"
                ))
            })?;
            map.insert(chord, id);
        }
        Ok(map)
    }

    pub fn color_enabled(&self) -> bool {
        match self.ui.color {
            ColorMode::Never => false,
            ColorMode::Always => true,
            ColorMode::Auto => {
                if env::var_os("NO_COLOR").is_some() {
                    return false;
                }
                let term = env::var("TERM").unwrap_or_default();
                term != "dumb" && !term.is_empty()
            }
        }
    }

    pub fn is_remote(&self) -> bool {
        env::var_os("SSH_CONNECTION").is_some()
            || env::var_os("SSH_TTY").is_some()
            || crate::util::is_termux()
    }

    pub fn system_clipboard_allowed(&self) -> bool {
        self.clipboard.system && (!self.is_remote() || self.clipboard.allow_remote)
    }

    pub fn osc52_allowed(&self) -> bool {
        self.clipboard.osc52 && (!self.is_remote() || self.clipboard.allow_remote)
    }
}

fn apply_table(
    root: &TomlValue,
    known: &mut Vec<Vec<String>>,
    config: &mut Config,
) -> Result<(), ConfigError> {
    let table = root
        .as_table()
        .ok_or_else(|| ConfigError("config root must be a table".into()))?;
    for (section, value) in table {
        let section_table = value
            .as_table()
            .ok_or_else(|| ConfigError(format!("[{section}] must be a table")))?;
        match section.as_str() {
            "editor" => {
                for (key, v) in section_table {
                    known.push(vec!["editor".into(), key.clone()]);
                    match key.as_str() {
                        "tab_width" => {
                            config.editor.tab_width = usize_value(v, "editor.tab_width")?
                        }
                        "soft_wrap" => config.editor.soft_wrap = bool_value(v, "editor.soft_wrap")?,
                        "auto_indent" => {
                            config.editor.auto_indent = bool_value(v, "editor.auto_indent")?
                        }
                        "cjk_wide" => config.editor.cjk_wide = bool_value(v, "editor.cjk_wide")?,
                        "large_file_threshold" => {
                            config.editor.large_file_threshold =
                                u64_value(v, "editor.large_file_threshold")?
                        }
                        "profile" => {
                            let s = str_value(v, "editor.profile")?;
                            config.editor.profile = ProfileName::parse(s)
                                .ok_or_else(|| ConfigError(format!("unknown profile {s:?}")))?;
                        }
                        "newline" => {
                            config.editor.newline = match str_value(v, "editor.newline")? {
                                "preserve" => NewlineMode::Preserve,
                                "lf" => NewlineMode::Lf,
                                "crlf" => NewlineMode::Crlf,
                                other => {
                                    return Err(ConfigError(format!(
                                        "unknown newline mode {other:?}"
                                    )))
                                }
                            };
                        }
                        _ => return Err(ConfigError(format!("unknown key editor.{key}"))),
                    }
                }
            }
            "ui" => {
                for (key, v) in section_table {
                    known.push(vec!["ui".into(), key.clone()]);
                    match key.as_str() {
                        "mouse" => config.ui.mouse = bool_value(v, "ui.mouse")?,
                        "tab_strip" => config.ui.tab_strip = bool_value(v, "ui.tab_strip")?,
                        "alt_screen" => config.ui.alt_screen = bool_value(v, "ui.alt_screen")?,
                        "min_columns" => config.ui.min_columns = usize_value(v, "ui.min_columns")?,
                        "min_rows" => config.ui.min_rows = usize_value(v, "ui.min_rows")?,
                        "color" => {
                            config.ui.color = match str_value(v, "ui.color")? {
                                "auto" => ColorMode::Auto,
                                "never" => ColorMode::Never,
                                "always" => ColorMode::Always,
                                other => {
                                    return Err(ConfigError(format!(
                                        "unknown color mode {other:?}"
                                    )))
                                }
                            };
                        }
                        "line_numbers" => {
                            config.ui.line_numbers = match str_value(v, "ui.line_numbers")? {
                                "off" => LineNumberMode::Off,
                                "absolute" => LineNumberMode::Absolute,
                                "relative" => LineNumberMode::Relative,
                                other => {
                                    return Err(ConfigError(format!(
                                        "unknown line_numbers {other:?}"
                                    )))
                                }
                            };
                        }
                        _ => return Err(ConfigError(format!("unknown key ui.{key}"))),
                    }
                }
            }
            "clipboard" => {
                for (key, v) in section_table {
                    known.push(vec!["clipboard".into(), key.clone()]);
                    match key.as_str() {
                        "system" => config.clipboard.system = bool_value(v, "clipboard.system")?,
                        "osc52" => config.clipboard.osc52 = bool_value(v, "clipboard.osc52")?,
                        "allow_remote" => {
                            config.clipboard.allow_remote = bool_value(v, "clipboard.allow_remote")?
                        }
                        "osc52_max_bytes" => {
                            config.clipboard.osc52_max_bytes =
                                usize_value(v, "clipboard.osc52_max_bytes")?
                        }
                        _ => return Err(ConfigError(format!("unknown key clipboard.{key}"))),
                    }
                }
            }
            "search" => {
                for (key, v) in section_table {
                    known.push(vec!["search".into(), key.clone()]);
                    match key.as_str() {
                        "hidden" => config.search.hidden = bool_value(v, "search.hidden")?,
                        "ignored" => config.search.ignored = bool_value(v, "search.ignored")?,
                        "max_results" => {
                            config.search.max_results = usize_value(v, "search.max_results")?
                        }
                        "context_lines" => {
                            config.search.context_lines = usize_value(v, "search.context_lines")?
                        }
                        _ => return Err(ConfigError(format!("unknown key search.{key}"))),
                    }
                }
            }
            "session" => {
                for (key, v) in section_table {
                    known.push(vec!["session".into(), key.clone()]);
                    match key.as_str() {
                        "restore" => config.session.restore = bool_value(v, "session.restore")?,
                        "recovery" => config.session.recovery = bool_value(v, "session.recovery")?,
                        "recovery_interval_ms" => {
                            config.session.recovery_interval_ms =
                                u64_value(v, "session.recovery_interval_ms")?
                        }
                        "recovery_retention_days" => {
                            config.session.recovery_retention_days =
                                u64_value(v, "session.recovery_retention_days")?
                        }
                        _ => return Err(ConfigError(format!("unknown key session.{key}"))),
                    }
                }
            }
            "git" => {
                for (key, v) in section_table {
                    known.push(vec!["git".into(), key.clone()]);
                    match key.as_str() {
                        "enabled" => config.git.enabled = bool_value(v, "git.enabled")?,
                        "refresh_ms" => config.git.refresh_ms = u64_value(v, "git.refresh_ms")?,
                        _ => return Err(ConfigError(format!("unknown key git.{key}"))),
                    }
                }
            }
            "lsp" => {
                for (key, v) in section_table {
                    known.push(vec!["lsp".into(), key.clone()]);
                    match key.as_str() {
                        "enabled" => config.lsp.enabled = bool_value(v, "lsp.enabled")?,
                        "request_timeout_ms" => {
                            config.lsp.request_timeout_ms = u64_value(v, "lsp.request_timeout_ms")?
                        }
                        "trust_workspace_edits" => {
                            config.lsp.trust_workspace_edits =
                                bool_value(v, "lsp.trust_workspace_edits")?
                        }
                        "servers" => {
                            let items = v.as_array().ok_or_else(|| {
                                ConfigError("lsp.servers must be an array of tables".into())
                            })?;
                            config.lsp.servers.clear();
                            for item in items {
                                let t = item.as_table().ok_or_else(|| {
                                    ConfigError("each lsp server must be a table".into())
                                })?;
                                for k in t.keys() {
                                    known.push(vec!["lsp".into(), "servers".into(), k.clone()]);
                                }
                                let language = str_value(
                                    t.get("language").ok_or_else(|| {
                                        ConfigError("lsp server needs language".into())
                                    })?,
                                    "lsp.servers.language",
                                )?
                                .to_string();
                                let command = str_value(
                                    t.get("command").ok_or_else(|| {
                                        ConfigError("lsp server needs command".into())
                                    })?,
                                    "lsp.servers.command",
                                )?
                                .to_string();
                                let args = string_array(t.get("args"), "lsp.servers.args")?;
                                let root_markers = string_array(
                                    t.get("root_markers"),
                                    "lsp.servers.root_markers",
                                )?;
                                for k in t.keys() {
                                    if !["language", "command", "args", "root_markers"]
                                        .contains(&k.as_str())
                                    {
                                        return Err(ConfigError(format!(
                                            "unknown key lsp.servers.{k}"
                                        )));
                                    }
                                }
                                config.lsp.servers.push(LspServerConfig {
                                    language,
                                    command,
                                    args,
                                    root_markers,
                                });
                            }
                        }
                        _ => return Err(ConfigError(format!("unknown key lsp.{key}"))),
                    }
                }
            }
            "keys" => {
                for (key, v) in section_table {
                    known.push(vec!["keys".into(), key.clone()]);
                    config
                        .key_overrides
                        .insert(key.clone(), str_value(v, "keys")?.to_string());
                }
            }
            _ => return Err(ConfigError(format!("unknown config section [{section}]"))),
        }
    }
    Ok(())
}

fn validate_all_known(root: &TomlValue, known: &[Vec<String>]) -> Result<(), ConfigError> {
    // apply_table rejects unknown section/key pairs but dotted keys can create
    // nested tables directly; walk the tree once more to catch those.
    fn walk(
        value: &TomlValue,
        path: &mut Vec<String>,
        known: &[Vec<String>],
    ) -> Result<(), ConfigError> {
        if let Some(map) = value.as_table() {
            for (key, child) in map {
                path.push(key.clone());
                if matches!(child, TomlValue::Table(_)) {
                    walk(child, path, known)?;
                } else if matches!(child, TomlValue::Array(_))
                    && path.first().map(String::as_str) == Some("lsp")
                {
                    // array of tables validated by apply_table
                } else if !known.iter().any(|k| k == path) {
                    return Err(ConfigError(format!(
                        "unknown dotted key {}",
                        path.join(".")
                    )));
                }
                path.pop();
            }
        }
        Ok(())
    }
    walk(root, &mut Vec::new(), known)
}

fn str_value<'a>(v: &'a TomlValue, name: &str) -> Result<&'a str, ConfigError> {
    v.as_str()
        .ok_or_else(|| ConfigError(format!("{name} must be a string")))
}
fn bool_value(v: &TomlValue, name: &str) -> Result<bool, ConfigError> {
    v.as_bool()
        .ok_or_else(|| ConfigError(format!("{name} must be a boolean")))
}
fn usize_value(v: &TomlValue, name: &str) -> Result<usize, ConfigError> {
    let n = v
        .as_i64()
        .ok_or_else(|| ConfigError(format!("{name} must be an integer")))?;
    usize::try_from(n).map_err(|_| ConfigError(format!("{name} must be non-negative")))
}
fn u64_value(v: &TomlValue, name: &str) -> Result<u64, ConfigError> {
    let n = v
        .as_i64()
        .ok_or_else(|| ConfigError(format!("{name} must be an integer")))?;
    u64::try_from(n).map_err(|_| ConfigError(format!("{name} must be non-negative")))
}
fn string_array(v: Option<&TomlValue>, name: &str) -> Result<Vec<String>, ConfigError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let items = v
        .as_array()
        .ok_or_else(|| ConfigError(format!("{name} must be an array of strings")))?;
    items
        .iter()
        .map(|item| str_value(item, name).map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_toml_subset() {
        let text = r#"
# valid example
[editor]
tab_width = 2
soft_wrap = false
profile = "selection-first"
[ui]
line_numbers = "off"
[clipboard]
osc52 = true
[lsp]
enabled = true
[[lsp.servers]]
language = "rust"
command = "rust-analyzer"
args = []
root_markers = ["Cargo.toml"]
[keys]
"ctrl+s" = "save"
"#;
        let config = Config::from_toml(text).unwrap();
        assert_eq!(config.editor.tab_width, 2);
        assert!(!config.editor.soft_wrap);
        assert_eq!(config.editor.profile, ProfileName::SelectionFirst);
        assert_eq!(config.ui.line_numbers, LineNumberMode::Off);
        assert!(config.clipboard.osc52);
        assert_eq!(config.lsp.servers[0].command, "rust-analyzer");
        assert_eq!(
            config.key_map().unwrap()[&KeyChord::parse("ctrl+s").unwrap()],
            CommandId::Save
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(Config::from_toml("[editor]\ntab_wdith = 2\n").is_err());
        assert!(Config::from_toml("[nonsense]\na = 1\n").is_err());
    }

    #[test]
    fn arrays_inline_tables_and_comments() {
        let value = parse_toml("a = [1, 2, \"x\"] # tail\nb = { x = true, y = 'lit' }\n").unwrap();
        let table = value.as_table().unwrap();
        assert_eq!(table["a"].as_array().unwrap().len(), 3);
        assert_eq!(table["b"].as_table().unwrap()["y"].as_str(), Some("lit"));
    }

    #[test]
    fn unsupported_multiline_string_is_clear_error() {
        let err = parse_toml("a = \"\"\"\nhello\n\"\"\"\n").unwrap_err();
        assert!(err.to_string().contains("multiline"));
    }
}
