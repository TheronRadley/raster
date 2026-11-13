//! Test-only JSON-RPC language server. It never reads project configuration,
//! installs anything, or executes project scripts.

use std::collections::HashMap;
use std::env;
use std::io::{self, BufReader, Write};
use std::time::Duration;

use raster::json::Json;
use raster::lsp::{read_message, write_message};

struct Options {
    delay_ms: u64,
    hang: bool,
    crash: bool,
    flood: usize,
    encoding: String,
}

fn options() -> Options {
    let mut out = Options {
        delay_ms: 0,
        hang: false,
        crash: false,
        flood: 0,
        encoding: "utf-16".into(),
    };
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--delay-ms" => {
                i += 1;
                out.delay_ms = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--hang" => out.hang = true,
            "--crash" => out.crash = true,
            "--flood" => {
                i += 1;
                out.flood = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--utf8" => out.encoding = "utf-8".into(),
            "--utf16" => out.encoding = "utf-16".into(),
            "--utf32" => out.encoding = "utf-32".into(),
            _ => {}
        }
        i += 1;
    }
    out
}

fn main() {
    let opts = options();
    let mut reader = BufReader::new(io::stdin().lock());
    let mut stdout = io::stdout().lock();
    let mut documents: HashMap<String, (u64, String)> = HashMap::new();
    while let Ok(message) = read_message(&mut reader) {
        let method = message
            .get("method")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        if opts.crash && method == "textDocument/didOpen" {
            std::process::exit(7);
        }
        if method == "exit" {
            break;
        }
        if method == "textDocument/didOpen" {
            let uri = params
                .get("textDocument")
                .and_then(|d| d.get("uri"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let version = params
                .get("textDocument")
                .and_then(|d| d.get("version"))
                .and_then(Json::as_u64)
                .unwrap_or(0);
            let text = params
                .get("textDocument")
                .and_then(|d| d.get("text"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            documents.insert(uri.clone(), (version, text));
            publish_diagnostics(&mut stdout, &uri, version);
            continue;
        }
        if method == "textDocument/didChange" {
            let doc = params.get("textDocument");
            let uri = doc
                .and_then(|d| d.get("uri"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let version = doc
                .and_then(|d| d.get("version"))
                .and_then(Json::as_u64)
                .unwrap_or(0);
            if let Some(entry) = documents.get_mut(&uri) {
                entry.0 = version;
            }
            publish_diagnostics(&mut stdout, &uri, version);
            continue;
        }
        if method == "textDocument/didClose" {
            let uri = params
                .get("textDocument")
                .and_then(|d| d.get("uri"))
                .and_then(Json::as_str)
                .unwrap_or("");
            documents.remove(uri);
            continue;
        }
        if method == "initialized" {
            for (uri, (version, _)) in documents.clone() {
                publish_diagnostics(&mut stdout, &uri, version);
            }
            for n in 0..opts.flood {
                let mut notification = Json::object();
                notification.insert("jsonrpc", Json::string("2.0"));
                notification.insert("method", Json::string("window/logMessage"));
                let mut p = Json::object();
                p.insert("type", Json::Int(3));
                p.insert("message", Json::string(format!("flood {n}")));
                notification.insert("params", p);
                let _ = write_message(&mut stdout, &notification);
            }
            continue;
        }
        let Some(id) = id else {
            continue;
        };
        if opts.hang && method.starts_with("textDocument/") && method != "shutdown" {
            std::thread::sleep(Duration::from_secs(30));
        }
        if opts.delay_ms > 0 && method != "shutdown" {
            std::thread::sleep(Duration::from_millis(opts.delay_ms));
        }
        let result = match method.as_str() {
            "initialize" => {
                let mut general = Json::object();
                let mut encodings = Json::array();
                encodings.push(Json::string(&opts.encoding));
                general.insert("positionEncodings", encodings);
                let mut capabilities = Json::object();
                capabilities.insert("positionEncoding", Json::string(&opts.encoding));
                capabilities.insert("general", general);
                capabilities.insert("completionProvider", Json::object());
                capabilities.insert("hoverProvider", Json::Bool(true));
                capabilities.insert("definitionProvider", Json::Bool(true));
                capabilities.insert("referencesProvider", Json::Bool(true));
                capabilities.insert("renameProvider", Json::Bool(true));
                capabilities.insert("documentFormattingProvider", Json::Bool(true));
                capabilities.insert("codeActionProvider", Json::Bool(true));
                let mut value = Json::object();
                value.insert("capabilities", capabilities);
                let mut server_info = Json::object();
                server_info.insert("name", Json::string("raster-mock-lsp"));
                server_info.insert("version", Json::string("0.1.0"));
                value.insert("serverInfo", server_info);
                value
            }
            "shutdown" => Json::Null,
            "textDocument/completion" => {
                let mut items = Json::array();
                for label in ["alpha", "beta", "gamma"] {
                    let mut item = Json::object();
                    item.insert("label", Json::string(label));
                    item.insert("insertText", Json::string(label));
                    items.push(item);
                }
                let mut value = Json::object();
                value.insert("isIncomplete", Json::Bool(false));
                value.insert("items", items);
                value
            }
            "textDocument/hover" => {
                let mut value = Json::object();
                let mut contents = Json::object();
                contents.insert("kind", Json::string("markdown"));
                contents.insert("value", Json::string("**mock hover**"));
                value.insert("contents", contents);
                value
            }
            "textDocument/definition" => location(&params, 0, 0),
            "textDocument/references" => {
                let mut array = Json::array();
                array.push(location(&params, 0, 0));
                array.push(location(&params, 1, 0));
                array
            }
            "textDocument/rename" => workspace_edit(&params, &documents, "RENAMED"),
            "textDocument/formatting" => {
                let mut range = Json::object();
                range.insert("start", position(0, 0));
                range.insert("end", position(0, 3));
                let mut edit = Json::object();
                edit.insert("range", range);
                edit.insert("newText", Json::string("FORMATTED"));
                let mut array = Json::array();
                array.push(edit);
                array
            }
            "textDocument/codeAction" => {
                let mut edit_action = Json::object();
                edit_action.insert("title", Json::string("Mock edit action"));
                edit_action.insert("edit", workspace_edit(&params, &documents, "ACTION"));
                let mut command = Json::object();
                command.insert("title", Json::string("Mock command (must not execute)"));
                command.insert("command", Json::string("mock.untrusted"));
                let mut command_action = Json::object();
                command_action.insert("title", Json::string("Mock command action"));
                command_action.insert("command", command);
                let mut array = Json::array();
                array.push(edit_action);
                array.push(command_action);
                array
            }
            _ => Json::Null,
        };
        let mut response = Json::object();
        response.insert("jsonrpc", Json::string("2.0"));
        response.insert("id", id);
        response.insert("result", result);
        let _ = write_message(&mut stdout, &response);
    }
}

fn publish_diagnostics<W: Write>(writer: &mut W, uri: &str, version: u64) {
    let mut range = Json::object();
    range.insert("start", position(0, 0));
    range.insert("end", position(0, 3));
    let mut diagnostic = Json::object();
    diagnostic.insert("range", range);
    diagnostic.insert("message", Json::string("mock diagnostic"));
    diagnostic.insert("severity", Json::Int(2));
    let mut diagnostics = Json::array();
    diagnostics.push(diagnostic);
    let mut params = Json::object();
    params.insert("uri", Json::string(uri));
    params.insert("version", Json::Int(version as i64));
    params.insert("diagnostics", diagnostics);
    let mut notification = Json::object();
    notification.insert("jsonrpc", Json::string("2.0"));
    notification.insert("method", Json::string("textDocument/publishDiagnostics"));
    notification.insert("params", params);
    let _ = write_message(writer, &notification);
}

fn workspace_edit(params: &Json, documents: &HashMap<String, (u64, String)>, text: &str) -> Json {
    let uri = params
        .get("textDocument")
        .and_then(|d| d.get("uri"))
        .and_then(Json::as_str)
        .unwrap_or("");
    let version = documents.get(uri).map(|(v, _)| *v).unwrap_or(0);
    let mut range = Json::object();
    range.insert("start", position(0, 0));
    range.insert("end", position(0, 3));
    let mut edit = Json::object();
    edit.insert("range", range);
    edit.insert("newText", Json::string(text));
    let mut edits = Json::array();
    edits.push(edit);
    let mut text_document = Json::object();
    text_document.insert("uri", Json::string(uri));
    text_document.insert("version", Json::Int(version as i64));
    let mut document_change = Json::object();
    document_change.insert("textDocument", text_document);
    document_change.insert("edits", edits);
    let mut changes = Json::array();
    changes.push(document_change);
    let mut result = Json::object();
    result.insert("documentChanges", changes);
    result
}

fn location(params: &Json, line: usize, character: usize) -> Json {
    let uri = params
        .get("textDocument")
        .and_then(|d| d.get("uri"))
        .and_then(Json::as_str)
        .unwrap_or("file:///unknown");
    let mut range = Json::object();
    range.insert("start", position(line, character));
    range.insert("end", position(line, character));
    let mut value = Json::object();
    value.insert("uri", Json::string(uri));
    value.insert("range", range);
    value
}

fn position(line: usize, character: usize) -> Json {
    let mut value = Json::object();
    value.insert("line", Json::Int(line as i64));
    value.insert("character", Json::Int(character as i64));
    value
}
