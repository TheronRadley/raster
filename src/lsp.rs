//! Optional language-server client. Servers are configured explicitly; Raster
//! never installs one, executes a project script, or reads executable
//! project-local configuration.
//!
//! The client is non-blocking: a reader thread parses JSON-RPC, pending
//! requests carry a document version, stale responses are rejected, and
//! requests time out. Output is bounded to protect the interactive loop.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::{LspConfig, LspServerConfig};
use crate::core::document::Document;
use crate::core::syntax::Language;
use crate::core::transaction::Replacement;
use crate::json::Json;
use crate::position::{BytePos, LineIndex};
use crate::util::wait_with_timeout;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}

impl PositionEncoding {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "utf-8" => Some(Self::Utf8),
            "utf-16" => Some(Self::Utf16),
            "utf-32" => Some(Self::Utf32),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspError(pub String);
impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for LspError {}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum LspRequestKind {
    Initialize,
    Shutdown,
    Completion,
    Hover,
    Definition,
    References,
    Rename,
    Formatting,
    CodeAction,
}

#[derive(Clone, Debug)]
pub enum LspEvent {
    Ready {
        encoding: PositionEncoding,
    },
    Response {
        request: LspRequestKind,
        version: u64,
        uri: String,
        payload: Json,
    },
    Diagnostics {
        version: u64,
        uri: String,
        payload: Json,
    },
    Status(String),
    ServerExited(String),
    Throttled(usize),
    Stale {
        request: LspRequestKind,
        response_version: u64,
        current_version: u64,
    },
    Timeout(LspRequestKind),
}

#[derive(Clone, Debug)]
pub struct LspChange {
    pub start_line: usize,
    pub start_character: usize,
    pub end_line: usize,
    pub end_character: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
struct Pending {
    kind: LspRequestKind,
    version: u64,
    uri: String,
    started: Instant,
}

enum ReaderEvent {
    Message(Json),
    Error(String),
    Dropped(usize),
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<ReaderEvent>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    pub encoding: PositionEncoding,
    pub initialized: bool,
    timeout: Duration,
}

impl LspClient {
    pub fn new(
        command: &str,
        args: &[String],
        root: &Path,
        timeout: Duration,
    ) -> Result<Self, LspError> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| LspError(format!("cannot start {command}: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError("server stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError("server stdout unavailable".into()))?;
        let (tx, rx) = mpsc::sync_channel::<ReaderEvent>(128);
        let reader = std::thread::Builder::new()
            .name("raster-lsp-reader".into())
            .spawn(move || {
                reader_loop(stdout, tx);
            })
            .map_err(|e| LspError(e.to_string()))?;
        Ok(Self {
            child,
            stdin,
            rx,
            reader: Some(reader),
            next_id: 1,
            pending: HashMap::new(),
            encoding: PositionEncoding::Utf16,
            initialized: false,
            timeout,
        })
    }

    pub fn send_initialize(&mut self, root: &Path) -> Result<(), LspError> {
        let mut capabilities = Json::object();
        let mut general = Json::object();
        let mut encodings = Json::array();
        for encoding in ["utf-8", "utf-16", "utf-32"] {
            encodings.push(Json::string(encoding));
        }
        general.insert("positionEncodings", encodings);
        capabilities.insert("general", general);
        let mut text_document = Json::object();
        text_document.insert("synchronization", Json::object());
        capabilities.insert("textDocument", text_document);
        let mut params = Json::object();
        params.insert("processId", Json::Int(std::process::id() as i64));
        params.insert("rootUri", Json::string(uri_from_path(root)));
        params.insert("capabilities", capabilities);
        self.request(
            LspRequestKind::Initialize,
            "initialize",
            params,
            String::new(),
            0,
        )
    }

    pub fn request(
        &mut self,
        kind: LspRequestKind,
        method: &str,
        params: Json,
        uri: String,
        version: u64,
    ) -> Result<(), LspError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = Json::object();
        message.insert("jsonrpc", Json::string("2.0"));
        message.insert("id", Json::Int(id as i64));
        message.insert("method", Json::string(method));
        message.insert("params", params);
        self.write(&message)?;
        self.pending.insert(
            id,
            Pending {
                kind,
                version,
                uri,
                started: Instant::now(),
            },
        );
        Ok(())
    }

    pub fn notify(&mut self, method: &str, params: Json) -> Result<(), LspError> {
        let mut message = Json::object();
        message.insert("jsonrpc", Json::string("2.0"));
        message.insert("method", Json::string(method));
        message.insert("params", params);
        self.write(&message)
    }

    pub fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        version: u64,
        text: &str,
    ) -> Result<(), LspError> {
        let mut doc = Json::object();
        doc.insert("uri", Json::string(uri));
        doc.insert("languageId", Json::string(language_id));
        doc.insert("version", Json::Int(version as i64));
        doc.insert("text", Json::string(text));
        let mut params = Json::object();
        params.insert("textDocument", doc);
        self.notify("textDocument/didOpen", params)
    }

    pub fn did_change(
        &mut self,
        uri: &str,
        version: u64,
        changes: &[LspChange],
    ) -> Result<(), LspError> {
        let mut doc = Json::object();
        doc.insert("uri", Json::string(uri));
        doc.insert("version", Json::Int(version as i64));
        let mut content = Json::array();
        for change in changes {
            let mut item = Json::object();
            let mut range = Json::object();
            range.insert(
                "start",
                lsp_position(change.start_line, change.start_character),
            );
            range.insert("end", lsp_position(change.end_line, change.end_character));
            item.insert("range", range);
            item.insert("text", Json::string(&change.text));
            content.push(item);
        }
        let mut params = Json::object();
        params.insert("textDocument", doc);
        params.insert("contentChanges", content);
        self.notify("textDocument/didChange", params)
    }

    pub fn did_close(&mut self, uri: &str) -> Result<(), LspError> {
        let mut doc = Json::object();
        doc.insert("uri", Json::string(uri));
        let mut params = Json::object();
        params.insert("textDocument", doc);
        self.notify("textDocument/didClose", params)
    }

    pub fn did_save(&mut self, uri: &str) -> Result<(), LspError> {
        let mut doc = Json::object();
        doc.insert("uri", Json::string(uri));
        let mut params = Json::object();
        params.insert("textDocument", doc);
        self.notify("textDocument/didSave", params)
    }

    fn write(&mut self, message: &Json) -> Result<(), LspError> {
        write_message(&mut self.stdin, message)
            .map_err(|e| LspError(format!("LSP write failed: {e}")))
    }

    pub fn poll(&mut self, doc_versions: &HashMap<String, u64>) -> Vec<LspEvent> {
        let mut events = Vec::new();
        while let Ok(raw) = self.rx.try_recv() {
            match raw {
                ReaderEvent::Error(e) => {
                    events.push(LspEvent::Status(format!("LSP read error: {e}")))
                }
                ReaderEvent::Dropped(n) => events.push(LspEvent::Throttled(n)),
                ReaderEvent::Message(message) => {
                    self.handle_message(message, doc_versions, &mut events)
                }
            }
        }
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.started.elapsed() > self.timeout)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                events.push(LspEvent::Timeout(pending.kind));
            }
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            events.push(LspEvent::ServerExited(format!(
                "server exited with {status}"
            )));
        }
        events
    }

    fn handle_message(
        &mut self,
        message: Json,
        doc_versions: &HashMap<String, u64>,
        events: &mut Vec<LspEvent>,
    ) {
        let method = message
            .get("method")
            .and_then(Json::as_str)
            .map(str::to_string);
        let id = message.get("id").and_then(Json::as_u64);
        if let Some(method) = method.clone() {
            if let Some(id) = id {
                self.handle_server_request(id, &method, &message);
                return;
            }
            if method == "textDocument/publishDiagnostics" {
                let params = message.get("params").cloned().unwrap_or_else(Json::object);
                let uri = params
                    .get("uri")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string();
                let version = params.get("version").and_then(Json::as_u64).unwrap_or(0);
                let current = doc_versions.get(&uri).copied().unwrap_or(version);
                if version == current {
                    events.push(LspEvent::Diagnostics {
                        version,
                        uri,
                        payload: params,
                    });
                }
                return;
            }
            if method == "window/showMessage" || method == "window/logMessage" {
                let params = message.get("params");
                let text = params
                    .and_then(|p| p.get("message"))
                    .and_then(Json::as_str)
                    .unwrap_or("LSP message");
                events.push(LspEvent::Status(text.to_string()));
            }
            return;
        }
        let Some(id) = id else {
            return;
        };
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        if let Some(error) = message.get("error") {
            let text = error
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or("LSP error");
            events.push(LspEvent::Status(text.to_string()));
            return;
        }
        let result = message.get("result").cloned().unwrap_or(Json::Null);
        if pending.kind == LspRequestKind::Initialize {
            let encoding = result
                .get("capabilities")
                .and_then(|c| c.get("positionEncoding"))
                .and_then(Json::as_str)
                .and_then(PositionEncoding::parse)
                .unwrap_or(PositionEncoding::Utf16);
            self.encoding = encoding;
            self.initialized = true;
            let _ = self.notify("initialized", Json::object());
            events.push(LspEvent::Ready { encoding });
            return;
        }
        if pending.kind == LspRequestKind::Shutdown {
            return;
        }
        let current = doc_versions
            .get(&pending.uri)
            .copied()
            .unwrap_or(pending.version);
        if pending.version != current {
            events.push(LspEvent::Stale {
                request: pending.kind,
                response_version: pending.version,
                current_version: current,
            });
            return;
        }
        events.push(LspEvent::Response {
            request: pending.kind,
            version: pending.version,
            uri: pending.uri,
            payload: result,
        });
    }

    fn handle_server_request(&mut self, id: u64, method: &str, _message: &Json) {
        let result = match method {
            "workspace/configuration" => Json::array(),
            "client/registerCapability" | "window/workDoneProgress/create" => Json::Null,
            "workspace/applyEdit" => {
                let mut response = Json::object();
                response.insert("applied", Json::Bool(false));
                response
            }
            _ => Json::Null,
        };
        let mut message = Json::object();
        message.insert("jsonrpc", Json::string("2.0"));
        message.insert("id", Json::Int(id as i64));
        message.insert("result", result);
        let _ = self.write(&message);
    }

    pub fn shutdown(&mut self) {
        if !self.initialized {
            let _ = self.child.kill();
            let _ = self.child.wait();
            return;
        }
        let _ = self.request(
            LspRequestKind::Shutdown,
            "shutdown",
            Json::Null,
            String::new(),
            0,
        );
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            // Drain the whole bounded queue: a blocked throttle notice must
            // not keep the reader thread alive when shutdown joins it.
            let mut received = false;
            while self.rx.try_recv().is_ok() {
                received = true;
            }
            if received && self.pending.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        while self.rx.try_recv().is_ok() {}
        let _ = self.notify("exit", Json::Null);
        if wait_with_timeout(&mut self.child, Duration::from_millis(500))
            .ok()
            .flatten()
            .is_none()
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.initialized = false;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Retry a throttle notice briefly without allowing a full queue to block the
/// reader indefinitely (shutdown and crashes must always remain bounded).
fn send_drop_notice(tx: &SyncSender<ReaderEvent>, dropped: usize) -> bool {
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        match tx.try_send(ReaderEvent::Dropped(dropped)) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(_)) => {
                if Instant::now() >= deadline {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn reader_loop(stdout: std::process::ChildStdout, tx: SyncSender<ReaderEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut dropped = 0usize;
    loop {
        match read_message(&mut reader) {
            Ok(message) => {
                let low_priority = message
                    .get("method")
                    .and_then(Json::as_str)
                    .map(|m| m == "window/logMessage" || m.starts_with("$/"))
                    .unwrap_or(false);
                let mut sent = false;
                for _ in 0..8 {
                    match tx.try_send(ReaderEvent::Message(message.clone())) {
                        Ok(()) => {
                            sent = true;
                            break;
                        }
                        Err(TrySendError::Full(_)) => {
                            if low_priority {
                                dropped += 1;
                                sent = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
                if !sent {
                    dropped += 1;
                }
                if dropped > 0 && !send_drop_notice(&tx, dropped) {
                    return;
                }
                if dropped > 0 {
                    dropped = 0;
                }
            }
            Err(e) => {
                // Make one bounded final attempt to deliver a pending count.
                if dropped > 0 {
                    let _ = send_drop_notice(&tx, dropped);
                }
                let _ = tx.try_send(ReaderEvent::Error(e));
                return;
            }
        }
    }
}

pub fn write_message<W: Write>(writer: &mut W, message: &Json) -> std::io::Result<()> {
    let body = message.to_compact_string();
    write!(
        writer,
        "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}",
        body.len(),
        body
    )?;
    writer.flush()
}

pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Json, String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("LSP server closed stdout".into());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| "missing Content-Length".to_string())?;
    if length > 16 * 1024 * 1024 {
        return Err("LSP message too large".into());
    }
    let mut body = vec![0u8; length];
    std::io::Read::read_exact(reader, &mut body).map_err(|e| e.to_string())?;
    let text = String::from_utf8(body).map_err(|e| e.to_string())?;
    Json::parse(&text).map_err(|e| e.to_string())
}

pub fn uri_from_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let text = absolute.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::new();
    let raw = rest.as_bytes();
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(PathBuf::from(text))
}

pub fn position_to_lsp(doc: &Document, pos: BytePos, encoding: PositionEncoding) -> (usize, usize) {
    let pos = doc.clamp(pos);
    let line = doc.byte_to_line(pos);
    let line_start = doc.line_to_byte(line);
    let text = doc.text_in_range(line_start..pos);
    let character = match encoding {
        PositionEncoding::Utf8 => text.len(),
        PositionEncoding::Utf16 => text.encode_utf16().count(),
        PositionEncoding::Utf32 => text.chars().count(),
    };
    (line.get(), character)
}

pub fn lsp_to_byte(
    doc: &Document,
    line: usize,
    character: usize,
    encoding: PositionEncoding,
) -> BytePos {
    let line_idx = LineIndex(line.min(doc.len_lines().saturating_sub(1)));
    let start = doc.line_to_byte(line_idx);
    let content_end = doc.line_content_range(line_idx).end;
    let text = doc.text_in_range(start..content_end);
    let mut byte = 0usize;
    let mut count = 0usize;
    for ch in text.chars() {
        let width = match encoding {
            PositionEncoding::Utf8 => ch.len_utf8(),
            PositionEncoding::Utf16 => ch.len_utf16(),
            PositionEncoding::Utf32 => 1,
        };
        if count + width > character {
            break;
        }
        count += width;
        byte += ch.len_utf8();
    }
    BytePos(start.get() + byte).min(doc.len_bytes())
}

pub fn changes_from_transaction(
    doc_before: &Document,
    edits: &[Replacement],
    encoding: PositionEncoding,
) -> Vec<LspChange> {
    edits
        .iter()
        .map(|edit| {
            let (start_line, start_character) = position_to_lsp(doc_before, edit.start, encoding);
            let (end_line, end_character) = position_to_lsp(doc_before, edit.end, encoding);
            LspChange {
                start_line,
                start_character,
                end_line,
                end_character,
                text: edit.text.clone(),
            }
        })
        .collect()
}

pub fn text_edits_to_replacements(
    doc: &Document,
    edits: &Json,
    encoding: PositionEncoding,
) -> Vec<Replacement> {
    let mut out = Vec::new();
    for edit in edits.as_array().unwrap_or(&[]) {
        let Some(range) = edit.get("range") else {
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
        let start_byte = lsp_to_byte(doc, sl, sc, encoding);
        let end_byte = lsp_to_byte(doc, el, ec, encoding);
        if start_byte <= end_byte {
            out.push(Replacement::new(
                start_byte..end_byte,
                edit.get("newText").and_then(Json::as_str).unwrap_or(""),
            ));
        }
    }
    out.sort_by(|a, b| (b.start, b.end, &b.text).cmp(&(a.start, a.end, &a.text)));
    out
}

#[derive(Clone, Debug)]
pub struct WorkspaceTarget {
    pub path: PathBuf,
    pub document: Document,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct WorkspaceEditPlan {
    pub uri: String,
    pub path: PathBuf,
    pub expected_version: u64,
    pub edits: Vec<Replacement>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceEditReview {
    pub plans: Vec<WorkspaceEditPlan>,
    pub broad: bool,
}

pub fn prepare_workspace_edit(
    payload: &Json,
    encoding: PositionEncoding,
    targets: &HashMap<String, WorkspaceTarget>,
) -> Result<WorkspaceEditReview, LspError> {
    let mut plans = Vec::new();
    if let Some(changes) = payload.get("documentChanges").and_then(Json::as_array) {
        for change in changes {
            if change.get("kind").is_some() {
                return Err(LspError(
                    "create/rename/delete workspace edits are not applied automatically".into(),
                ));
            }
            let text_doc = change
                .get("textDocument")
                .ok_or_else(|| LspError("workspace edit is missing textDocument".into()))?;
            let uri = text_doc
                .get("uri")
                .and_then(Json::as_str)
                .ok_or_else(|| LspError("workspace edit is missing URI".into()))?
                .to_string();
            let version = text_doc
                .get("version")
                .and_then(Json::as_u64)
                .ok_or_else(|| {
                    LspError("workspace edit is missing a document version; rejected".into())
                })?;
            let target = targets
                .get(&uri)
                .ok_or_else(|| LspError(format!("workspace edit targets unopened file {uri}")))?;
            if target.version != version {
                return Err(LspError(format!(
                    "stale workspace edit for {}: server version {}, current {}",
                    uri, version, target.version
                )));
            }
            let edits_json = change.get("edits").cloned().unwrap_or_else(Json::array);
            let edits = text_edits_to_replacements(&target.document, &edits_json, encoding);
            plans.push(WorkspaceEditPlan {
                uri,
                path: target.path.clone(),
                expected_version: version,
                edits,
            });
        }
    } else if let Some(changes) = payload.get("changes").and_then(Json::as_object) {
        return Err(LspError(format!(
            "workspace edit uses {} file(s) without document versions; rejected",
            changes.len()
        )));
    } else {
        return Err(LspError("unsupported workspace edit shape".into()));
    }
    let total_edits: usize = plans.iter().map(|p| p.edits.len()).sum();
    Ok(WorkspaceEditReview {
        broad: plans.len() > 1 || total_edits > 5,
        plans,
    })
}

pub fn detect_project_root(path: &Path, extra_markers: &[String]) -> PathBuf {
    let start = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };
    let mut markers: Vec<&str> = vec![
        ".git",
        "Cargo.toml",
        "go.mod",
        "pyproject.toml",
        "CMakeLists.txt",
        "compile_commands.json",
    ];
    for marker in extra_markers {
        markers.push(marker.as_str());
    }
    for ancestor in start.ancestors() {
        if markers.iter().any(|m| ancestor.join(m).exists()) {
            return ancestor.to_path_buf();
        }
    }
    start
}

fn lsp_position(line: usize, character: usize) -> Json {
    let mut value = Json::object();
    value.insert("line", Json::Int(line as i64));
    value.insert("character", Json::Int(character as i64));
    value
}

pub struct LspManager {
    pub config: LspConfig,
    pub root: PathBuf,
    pub client: Option<LspClient>,
    pub active_language: Option<String>,
    pub doc_versions: HashMap<String, u64>,
    pub opened: HashSet<String>,
    pub last_synced: HashMap<u64, u64>,
    pub status: String,
}

impl LspManager {
    pub fn new(config: LspConfig, root: PathBuf) -> Self {
        Self {
            config,
            root,
            client: None,
            active_language: None,
            doc_versions: HashMap::new(),
            opened: HashSet::new(),
            last_synced: HashMap::new(),
            status: "LSP disabled".into(),
        }
    }

    pub fn server_for(&self, language: Language) -> Option<LspServerConfig> {
        let key = match language {
            Language::Rust => "rust",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Go => "go",
            Language::Python => "python",
            _ => return None,
        };
        self.config
            .servers
            .iter()
            .find(|s| s.language == key)
            .cloned()
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }

    pub fn has_active_client(&self) -> bool {
        self.client.as_ref().map(|c| c.initialized).unwrap_or(false)
    }

    pub fn encoding(&self) -> PositionEncoding {
        self.client
            .as_ref()
            .map(|c| c.encoding)
            .unwrap_or(PositionEncoding::Utf16)
    }

    pub fn start_server(&mut self, language: Language, root: &Path) {
        if !self.config.enabled {
            self.status = "LSP disabled".into();
            return;
        }
        let Some(server) = self.server_for(language) else {
            self.status = "no server configured".into();
            return;
        };
        if self.active_language.as_deref() == Some(server.language.as_str())
            && self.client.is_some()
        {
            return;
        }
        self.stop();
        self.root = root.to_path_buf();
        match LspClient::new(
            &server.command,
            &server.args,
            root,
            Duration::from_millis(self.config.request_timeout_ms),
        ) {
            Ok(mut client) => {
                if let Err(e) = client.send_initialize(root) {
                    self.status = format!("initialize failed: {e}");
                    return;
                }
                self.active_language = Some(server.language.clone());
                self.status = format!("starting {}", server.command);
                self.client = Some(client);
            }
            Err(e) => self.status = format!("server failed: {e}"),
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.shutdown();
        }
        self.opened.clear();
        self.last_synced.clear();
        self.active_language = None;
        self.status = "LSP stopped".into();
    }

    pub fn restart(&mut self, language: Language, root: &Path) {
        self.stop();
        self.start_server(language, root);
    }

    pub fn shutdown(&mut self) {
        self.stop();
    }

    pub fn poll(&mut self) -> Vec<LspEvent> {
        let Some(client) = self.client.as_mut() else {
            return Vec::new();
        };
        let events = client.poll(&self.doc_versions);
        for event in &events {
            match event {
                LspEvent::Ready { encoding } => {
                    self.status = format!("ready ({})", encoding.label())
                }
                LspEvent::ServerExited(e) => {
                    self.status = format!("server exited: {e}; restart with :lsp-restart")
                }
                LspEvent::Timeout(kind) => {
                    self.status = format!("{kind:?} timed out; editing continues")
                }
                LspEvent::Throttled(n) => {
                    self.status = format!("server output throttled ({n} messages)")
                }
                _ => {}
            }
        }
        events
    }

    pub fn sync_document(
        &mut self,
        buffer_id: u64,
        path: &Path,
        doc: &Document,
        language: Language,
    ) {
        if !self.config.enabled
            || self.server_for(language).is_none()
            || doc.len_bytes().get() > 10 * 1024 * 1024
        {
            return;
        }
        let configured = self.server_for(language).map(|server| server.language);
        if self.client.is_none() || self.active_language.as_deref() != configured.as_deref() {
            let root = self.root.clone();
            self.start_server(language, &root);
        }
        let uri = uri_from_path(path);
        self.doc_versions.insert(uri.clone(), doc.version());
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if !client.initialized {
            self.status = "initializing".into();
            return;
        }
        let last = self.last_synced.get(&buffer_id).copied();
        let needs_full_sync =
            !self.opened.contains(&uri) || last.map(|v| v != doc.version()).unwrap_or(true);
        if needs_full_sync {
            if self.opened.contains(&uri) {
                let _ = client.did_close(&uri);
            }
            let text = doc.text();
            let language_id = match language {
                Language::Rust => "rust",
                Language::C => "c",
                Language::Cpp => "cpp",
                Language::Go => "go",
                Language::Python => "python",
                _ => "plaintext",
            };
            if client
                .did_open(&uri, language_id, doc.version(), &text)
                .is_ok()
            {
                self.opened.insert(uri.clone());
                self.last_synced.insert(buffer_id, doc.version());
            }
        }
    }

    pub fn on_document_changed(
        &mut self,
        buffer_id: u64,
        path: &Path,
        new_version: u64,
        changes: Vec<LspChange>,
    ) {
        let uri = uri_from_path(path);
        self.doc_versions.insert(uri.clone(), new_version);
        let last = self.last_synced.get(&buffer_id).copied().unwrap_or(0);
        if last + 1 != new_version {
            return;
        }
        if let Some(client) = self.client.as_mut() {
            if client.initialized
                && self.opened.contains(&uri)
                && client.did_change(&uri, new_version, &changes).is_ok()
            {
                self.last_synced.insert(buffer_id, new_version);
            }
        }
    }

    /// Force a full close/open after undo, redo, or a lost incremental sync.
    pub fn force_sync(&mut self, buffer_id: u64, path: &Path, doc: &Document, language: Language) {
        let uri = uri_from_path(path);
        self.doc_versions.insert(uri.clone(), doc.version());
        self.last_synced.remove(&buffer_id);
        self.sync_document(buffer_id, path, doc, language);
    }

    pub fn force_sync_all<F>(&mut self, documents: F)
    where
        F: Fn() -> Vec<(u64, std::path::PathBuf, Document, Language)>,
    {
        for (id, path, doc, language) in documents() {
            self.force_sync(id, &path, &doc, language);
        }
    }

    pub fn did_save(&mut self, path: &Path) {
        let uri = uri_from_path(path);
        if let Some(client) = self.client.as_mut() {
            let _ = client.did_save(&uri);
        }
    }

    pub fn request(
        &mut self,
        kind: LspRequestKind,
        buffer_id: u64,
        path: &Path,
        doc: &Document,
        pos: BytePos,
        extra: Option<&str>,
    ) -> Result<(), LspError> {
        self.sync_document(
            buffer_id,
            path,
            doc,
            crate::core::syntax::detect_language(path),
        );
        let Some(client) = self.client.as_mut() else {
            return Err(LspError(self.status.clone()));
        };
        if !client.initialized {
            return Err(LspError("language server is still initializing".into()));
        }
        let uri = uri_from_path(path);
        let (line, character) = position_to_lsp(doc, pos, client.encoding);
        let mut text_document = Json::object();
        text_document.insert("uri", Json::string(&uri));
        let position = lsp_position(line, character);
        let mut params = Json::object();
        params.insert("textDocument", text_document.clone());
        params.insert("position", position.clone());
        let method = match kind {
            LspRequestKind::Completion => "textDocument/completion",
            LspRequestKind::Hover => "textDocument/hover",
            LspRequestKind::Definition => "textDocument/definition",
            LspRequestKind::References => {
                let mut context = Json::object();
                context.insert("includeDeclaration", Json::Bool(true));
                params.insert("context", context);
                "textDocument/references"
            }
            LspRequestKind::Rename => {
                params.insert("newName", Json::string(extra.unwrap_or("renamed")));
                "textDocument/rename"
            }
            LspRequestKind::Formatting => {
                let mut options = Json::object();
                options.insert("tabSize", Json::Int(4));
                options.insert("insertSpaces", Json::Bool(true));
                params.insert("options", options);
                "textDocument/formatting"
            }
            LspRequestKind::CodeAction => {
                let mut range = Json::object();
                range.insert("start", position.clone());
                range.insert("end", position);
                params.insert("range", range);
                "textDocument/codeAction"
            }
            LspRequestKind::Initialize | LspRequestKind::Shutdown => {
                return Err(LspError("internal request".into()))
            }
        };
        client.request(kind, method, params, uri, doc.version())
    }

    pub fn status(&self) -> String {
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_encoding_round_trips_non_ascii() {
        let doc = Document::from_text("مرحبا 😀\ne\u{301}x");
        let target = BytePos("مرحبا 😀\ne\u{301}".len());
        for encoding in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            let (line, character) = position_to_lsp(&doc, target, encoding);
            assert_eq!(
                lsp_to_byte(&doc, line, character, encoding),
                doc.clamp(target)
            );
        }
    }

    #[test]
    fn uri_round_trips_unicode_paths() {
        let path = PathBuf::from("/tmp/مرحبا file.rs");
        let uri = uri_from_path(&path);
        assert!(uri.starts_with("file://"));
        assert!(!uri.contains(' '));
        assert_eq!(path_from_uri(&uri), Some(path));
    }

    #[test]
    fn workspace_edit_requires_matching_versions() {
        let doc = Document::from_text("one\ntwo\n");
        let mut targets = HashMap::new();
        let uri = "file:///tmp/a.rs".to_string();
        targets.insert(
            uri.clone(),
            WorkspaceTarget {
                path: PathBuf::from("/tmp/a.rs"),
                document: doc.clone(),
                version: 3,
            },
        );
        let mut edit = Json::object();
        let mut range = Json::object();
        range.insert("start", lsp_position(0, 0));
        range.insert("end", lsp_position(0, 3));
        edit.insert("range", range);
        edit.insert("newText", Json::string("ONE"));
        let mut edits = Json::array();
        edits.push(edit);
        let mut text_doc = Json::object();
        text_doc.insert("uri", Json::string(&uri));
        text_doc.insert("version", Json::Int(2));
        let mut doc_change = Json::object();
        doc_change.insert("textDocument", text_doc.clone());
        doc_change.insert("edits", edits.clone());
        let mut changes = Json::array();
        changes.push(doc_change);
        let mut payload = Json::object();
        payload.insert("documentChanges", changes.clone());
        assert!(prepare_workspace_edit(&payload, PositionEncoding::Utf8, &targets).is_err());
        text_doc.insert("version", Json::Int(3));
        let mut doc_change = Json::object();
        doc_change.insert("textDocument", text_doc);
        doc_change.insert("edits", edits);
        let mut changes = Json::array();
        changes.push(doc_change);
        let mut payload = Json::object();
        payload.insert("documentChanges", changes);
        let review = prepare_workspace_edit(&payload, PositionEncoding::Utf8, &targets).unwrap();
        assert_eq!(review.plans.len(), 1);
        assert_eq!(review.plans[0].edits.len(), 1);
    }
}
