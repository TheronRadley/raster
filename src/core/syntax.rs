//! Small, dependency-free lexical syntax highlighting for the initial target
//! languages. It is intentionally not a parser: missing or malformed grammar
//! never prevents editing, and unknown languages simply return no spans.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::core::document::Document;
use crate::position::LineIndex;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Go,
    Python,
    Toml,
    Markdown,
    Plain,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Go => "Go",
            Self::Python => "Python",
            Self::Toml => "TOML",
            Self::Markdown => "Markdown",
            Self::Plain => "Plain",
        }
    }
}

pub fn detect_language(path: &Path) -> Language {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Language::Rust,
        "c" | "h" => Language::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
        "go" => Language::Go,
        "py" | "pyi" => Language::Python,
        "toml" => Language::Toml,
        "md" | "markdown" => Language::Markdown,
        _ => Language::Plain,
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum SyntaxKind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Type,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SyntaxKind,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
enum State {
    Normal,
    BlockComment,
    TripleString(char),
}

fn keywords(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        Language::C | Language::Cpp => &[
            "auto",
            "break",
            "case",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "double",
            "else",
            "enum",
            "extern",
            "false",
            "float",
            "for",
            "goto",
            "if",
            "inline",
            "int",
            "long",
            "namespace",
            "new",
            "nullptr",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "true",
            "typedef",
            "union",
            "unsigned",
            "using",
            "void",
            "volatile",
            "while",
        ],
        Language::Go => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
        ],
        Language::Python => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        _ => &[],
    }
}

fn line_comment(language: Language) -> Option<&'static str> {
    match language {
        Language::Rust | Language::C | Language::Cpp | Language::Go => Some("//"),
        Language::Python | Language::Toml => Some("#"),
        _ => None,
    }
}

/// Compute spans for `first_line..=last_line`, carrying block-comment and
/// triple-quoted-string state from the start of the document. This runs on the
/// syntax worker thread, never on the interactive input path.
pub fn compute_spans(
    doc: &Document,
    language: Language,
    first_line: usize,
    last_line: usize,
) -> Vec<Vec<Span>> {
    let last_line = last_line.min(doc.len_lines().saturating_sub(1));
    let mut state = State::Normal;
    let mut result = Vec::new();
    for line_idx in 0..=last_line {
        let line = doc.line_text(LineIndex(line_idx));
        let (spans, next) = tokenize_line(&line, language, state);
        state = next;
        if line_idx >= first_line {
            result.push(spans);
        }
    }
    result
}

fn tokenize_line(line: &str, language: Language, mut state: State) -> (Vec<Span>, State) {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match state {
            State::BlockComment => {
                if let Some(rel) = line[i..].find("*/") {
                    let end = i + rel + 2;
                    spans.push(Span {
                        start: i,
                        end,
                        kind: SyntaxKind::Comment,
                    });
                    state = State::Normal;
                    i = end;
                } else {
                    spans.push(Span {
                        start: i,
                        end: bytes.len(),
                        kind: SyntaxKind::Comment,
                    });
                    i = bytes.len();
                }
            }
            State::TripleString(delim) => {
                let marker = if delim == '"' { "\"\"\"" } else { "'''" };
                if let Some(rel) = line[i..].find(marker) {
                    let end = i + rel + 3;
                    spans.push(Span {
                        start: i,
                        end,
                        kind: SyntaxKind::String,
                    });
                    state = State::Normal;
                    i = end;
                } else {
                    spans.push(Span {
                        start: i,
                        end: bytes.len(),
                        kind: SyntaxKind::String,
                    });
                    i = bytes.len();
                }
            }
            State::Normal => {
                let rest = &line[i..];
                if matches!(
                    language,
                    Language::Rust | Language::C | Language::Cpp | Language::Go
                ) && rest.starts_with("/*")
                {
                    if let Some(rel) = rest[2..].find("*/") {
                        let end = i + 2 + rel + 2;
                        spans.push(Span {
                            start: i,
                            end,
                            kind: SyntaxKind::Comment,
                        });
                        i = end;
                    } else {
                        spans.push(Span {
                            start: i,
                            end: bytes.len(),
                            kind: SyntaxKind::Comment,
                        });
                        state = State::BlockComment;
                        i = bytes.len();
                    }
                    continue;
                }
                if let Some(prefix) = line_comment(language) {
                    if rest.starts_with(prefix) {
                        spans.push(Span {
                            start: i,
                            end: bytes.len(),
                            kind: SyntaxKind::Comment,
                        });
                        i = bytes.len();
                        continue;
                    }
                }
                if language == Language::Python
                    && (rest.starts_with("\"\"\"") || rest.starts_with("'''"))
                {
                    let delim = rest.as_bytes()[0] as char;
                    let marker = if delim == '"' { "\"\"\"" } else { "'''" };
                    if let Some(rel) = rest[3..].find(marker) {
                        let end = i + 3 + rel + 3;
                        spans.push(Span {
                            start: i,
                            end,
                            kind: SyntaxKind::String,
                        });
                        i = end;
                    } else {
                        spans.push(Span {
                            start: i,
                            end: bytes.len(),
                            kind: SyntaxKind::String,
                        });
                        state = State::TripleString(delim);
                        i = bytes.len();
                    }
                    continue;
                }
                let ch = rest.chars().next().unwrap();
                if ch == '"' || ch == '\'' {
                    let start = i;
                    i += ch.len_utf8();
                    let mut escaped = false;
                    while i < bytes.len() {
                        let c = line[i..].chars().next().unwrap();
                        if escaped {
                            escaped = false;
                            i += c.len_utf8();
                            continue;
                        }
                        if c == '\\' {
                            escaped = true;
                            i += 1;
                            continue;
                        }
                        if c == ch {
                            i += c.len_utf8();
                            break;
                        }
                        i += c.len_utf8();
                    }
                    spans.push(Span {
                        start,
                        end: i.min(bytes.len()),
                        kind: SyntaxKind::String,
                    });
                    continue;
                }
                if ch.is_ascii_digit() {
                    let start = i;
                    while i < bytes.len() {
                        let c = line[i..].chars().next().unwrap();
                        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                            i += c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    spans.push(Span {
                        start,
                        end: i,
                        kind: SyntaxKind::Number,
                    });
                    continue;
                }
                if ch.is_alphabetic() || ch == '_' {
                    let start = i;
                    while i < bytes.len() {
                        let c = line[i..].chars().next().unwrap();
                        if c.is_alphanumeric() || c == '_' {
                            i += c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    let word = &line[start..i];
                    let kind = if keywords(language).contains(&word) {
                        SyntaxKind::Keyword
                    } else if word
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                    {
                        SyntaxKind::Type
                    } else {
                        SyntaxKind::Plain
                    };
                    if kind != SyntaxKind::Plain {
                        spans.push(Span {
                            start,
                            end: i,
                            kind,
                        });
                    }
                    continue;
                }
                i += ch.len_utf8();
            }
        }
    }
    (spans, state)
}

pub type BufferId = u64;

#[derive(Clone, Debug)]
pub struct SyntaxResult {
    pub buffer_id: BufferId,
    pub version: u64,
    pub first_line: usize,
    pub spans: Vec<Vec<Span>>,
}

struct SyntaxRequest {
    buffer_id: BufferId,
    version: u64,
    first_line: usize,
    last_line: usize,
    language: Language,
    document: Document,
}

#[derive(Clone, Debug, Default)]
struct SyntaxCache {
    version: u64,
    first_line: usize,
    spans: Vec<Vec<Span>>,
}

/// One bounded worker. Obsolete requests are dropped; results with a stale
/// document version are rejected by `poll`.
pub struct SyntaxEngine {
    tx: SyncSender<SyntaxRequest>,
    rx: Receiver<SyntaxResult>,
    pending: bool,
    caches: HashMap<BufferId, SyntaxCache>,
    stop: Arc<Mutex<bool>>,
    thread: Option<JoinHandle<()>>,
}

impl Default for SyntaxEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxEngine {
    pub fn new() -> Self {
        let (tx, request_rx) = mpsc::sync_channel::<SyntaxRequest>(1);
        let (result_tx, rx) = mpsc::channel::<SyntaxResult>();
        let stop = Arc::new(Mutex::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("raster-syntax".into())
            .spawn(move || {
                while !*stop_thread.lock().unwrap_or_else(|e| e.into_inner()) {
                    match request_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(req) => {
                            // A Rope clone is cheap and preserves newline and
                            // version metadata; rebuilding a Document from
                            // bytes would scan the whole file off the input path.
                            let spans = compute_spans(
                                &req.document,
                                req.language,
                                req.first_line,
                                req.last_line,
                            );
                            let _ = result_tx.send(SyntaxResult {
                                buffer_id: req.buffer_id,
                                version: req.version,
                                first_line: req.first_line,
                                spans,
                            });
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .ok();
        Self {
            tx,
            rx,
            pending: false,
            caches: HashMap::new(),
            stop,
            thread,
        }
    }

    pub fn request(
        &mut self,
        buffer_id: BufferId,
        doc: &Document,
        language: Language,
        first_line: usize,
        last_line: usize,
    ) {
        self.poll();
        if self.pending {
            return;
        }
        let request = SyntaxRequest {
            buffer_id,
            version: doc.version(),
            first_line,
            last_line,
            language,
            document: doc.clone(),
        };
        match self.tx.try_send(request) {
            Ok(()) => self.pending = true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn poll(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            self.pending = false;
            let cache = self.caches.entry(result.buffer_id).or_default();
            if result.version >= cache.version {
                *cache = SyntaxCache {
                    version: result.version,
                    first_line: result.first_line,
                    spans: result.spans,
                };
            }
        }
    }

    pub fn spans(&self, buffer_id: BufferId, version: u64, line: usize) -> &[Span] {
        match self.caches.get(&buffer_id) {
            Some(cache) if cache.version == version => {
                let idx = line.saturating_sub(cache.first_line);
                cache.spans.get(idx).map(Vec::as_slice).unwrap_or(&[])
            }
            _ => &[],
        }
    }
}

impl Drop for SyntaxEngine {
    fn drop(&mut self) {
        if let Ok(mut stop) = self.stop.lock() {
            *stop = true;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_initial_languages() {
        assert_eq!(detect_language(Path::new("a.rs")), Language::Rust);
        assert_eq!(detect_language(Path::new("x.py")), Language::Python);
        assert_eq!(detect_language(Path::new("x.unknown")), Language::Plain);
    }

    #[test]
    fn highlights_keywords_strings_numbers_and_comments() {
        let doc = Document::from_text("fn main() {\n let x = 42; // hi\n \"text\"\n}\n");
        let spans = compute_spans(&doc, Language::Rust, 0, 3);
        assert!(spans[0].iter().any(|s| s.kind == SyntaxKind::Keyword));
        assert!(spans[1].iter().any(|s| s.kind == SyntaxKind::Number));
        assert!(spans[1].iter().any(|s| s.kind == SyntaxKind::Comment));
        assert!(spans[2].iter().any(|s| s.kind == SyntaxKind::String));
    }

    #[test]
    fn block_comment_state_crosses_lines() {
        let doc = Document::from_text("/* start\nstill comment\n*/\nlet x = 1;\n");
        let spans = compute_spans(&doc, Language::Rust, 0, 3);
        assert!(spans[1].iter().any(|s| s.kind == SyntaxKind::Comment));
        assert!(spans[3].iter().any(|s| s.kind == SyntaxKind::Keyword));
    }

    #[test]
    fn malformed_and_unicode_input_does_not_panic() {
        let doc = Document::from_text("let مرحبا = \"unterminated\n\u{202e}x /*\n😀");
        let spans = compute_spans(&doc, Language::Rust, 0, 2);
        assert_eq!(spans.len(), 3);
    }
}
