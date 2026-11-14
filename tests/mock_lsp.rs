use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use raster::app::editor::{Editor, ListKind, ListState, UiMode};
use raster::cli::CliOptions;
use raster::config::{Config, LspServerConfig};
use raster::core::commands::{Command as EditorCommand, CommandId};
use raster::json::Json;
use raster::lsp::{uri_from_path, LspClient, LspEvent, LspRequestKind};
use raster::position::Size;
use raster::term::input::{Event, Key, KeyEvent};
use raster::util::unique_suffix;

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("raster-lsp-{}", unique_suffix()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mock_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn wait_event<F>(
    client: &mut LspClient,
    versions: &HashMap<String, u64>,
    mut predicate: F,
) -> Option<LspEvent>
where
    F: FnMut(&LspEvent) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        for event in client.poll(versions) {
            if predicate(&event) {
                return Some(event);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn initialized_client(dir: &TestDir, args: &[&str], timeout_ms: u64) -> LspClient {
    let mut client = LspClient::new(
        env!("CARGO_BIN_EXE_mock-lsp"),
        &mock_args(args),
        &dir.0,
        Duration::from_millis(timeout_ms),
    )
    .unwrap();
    client.send_initialize(&dir.0).unwrap();
    let versions = HashMap::new();
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Ready { .. }
        ))
        .is_some(),
        "initialize timed out"
    );
    client
}

#[test]
fn mock_server_initializes_reports_diagnostics_and_completion() {
    let dir = TestDir::new();
    let file = dir.0.join("a.rs");
    fs::write(&file, "abc\n").unwrap();
    let uri = uri_from_path(&file);
    let mut client = initialized_client(&dir, &["--utf8"], 1500);
    client.did_open(&uri, "rust", 7, "abc\n").unwrap();
    let mut versions = HashMap::new();
    versions.insert(uri.clone(), 7);
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Diagnostics { version: 7, .. }
        ))
        .is_some(),
        "no diagnostics"
    );
    let mut params = Json::object();
    let mut document = Json::object();
    document.insert("uri", Json::string(&uri));
    params.insert("textDocument", document);
    client
        .request(
            LspRequestKind::Completion,
            "textDocument/completion",
            params,
            uri.clone(),
            7,
        )
        .unwrap();
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Response {
                request: LspRequestKind::Completion,
                ..
            }
        ))
        .is_some(),
        "no completion"
    );
    client.shutdown();
}

#[test]
fn stale_completion_response_is_rejected_when_document_version_changes() {
    let dir = TestDir::new();
    let uri = "file:///stale.rs".to_string();
    let mut client = initialized_client(&dir, &["--delay-ms", "200"], 2000);
    client.did_open(&uri, "rust", 1, "abc").unwrap();
    let mut versions = HashMap::new();
    versions.insert(uri.clone(), 1);
    client
        .request(
            LspRequestKind::Completion,
            "textDocument/completion",
            Json::object(),
            uri.clone(),
            1,
        )
        .unwrap();
    versions.insert(uri, 2);
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Stale { .. }
        ))
        .is_some(),
        "stale response was not rejected"
    );
    client.shutdown();
}

#[test]
fn hung_server_times_out_and_shutdown_does_not_hang() {
    let dir = TestDir::new();
    let uri = "file:///hang.rs".to_string();
    let mut client = initialized_client(&dir, &["--hang"], 150);
    client.did_open(&uri, "rust", 1, "abc").unwrap();
    let versions = HashMap::from([(uri.clone(), 1)]);
    client
        .request(
            LspRequestKind::Hover,
            "textDocument/hover",
            Json::object(),
            uri,
            1,
        )
        .unwrap();
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Timeout(LspRequestKind::Hover)
        ))
        .is_some(),
        "no timeout"
    );
    let start = Instant::now();
    client.shutdown();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "shutdown took too long"
    );
}

#[test]
fn crashed_server_is_reported_without_stopping_the_client_poll_loop() {
    let dir = TestDir::new();
    let uri = "file:///crash.rs".to_string();
    let mut client = initialized_client(&dir, &["--crash"], 1500);
    client.did_open(&uri, "rust", 1, "abc").unwrap();
    let versions = HashMap::from([(uri, 1)]);
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::ServerExited(_)
        ))
        .is_some(),
        "crash not reported"
    );
    client.shutdown();
}

#[test]
fn excessive_server_output_is_throttled() {
    let dir = TestDir::new();
    let mut client = initialized_client(&dir, &["--flood", "500"], 2000);
    let versions = HashMap::new();
    assert!(
        wait_event(&mut client, &versions, |e| matches!(
            e,
            LspEvent::Throttled(_)
        ))
        .is_some(),
        "flood was not throttled"
    );
    client.shutdown();
}

fn editor_with_lsp(dir: &TestDir, args: &[&str], timeout_ms: u64) -> Editor {
    let mut config = Config::default();
    config.session.restore = false;
    config.session.recovery = false;
    config.git.enabled = false;
    config.ui.mouse = false;
    config.lsp.enabled = true;
    config.lsp.request_timeout_ms = timeout_ms;
    config.lsp.trust_workspace_edits = false;
    config.lsp.servers = vec![LspServerConfig {
        language: "rust".into(),
        command: env!("CARGO_BIN_EXE_mock-lsp").into(),
        args: mock_args(args),
        root_markers: vec!["Cargo.toml".into()],
    }];
    let cli = CliOptions {
        no_config: true,
        no_session: true,
        project: Some(dir.0.clone()),
        ..Default::default()
    };
    Editor::headless(config, cli, Size::new(80, 24)).unwrap()
}

fn wait_editor<F>(editor: &mut Editor, mut predicate: F) -> bool
where
    F: FnMut(&Editor) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        editor.tick();
        if predicate(editor) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn editor_lsp_completion_formatting_rename_and_code_action_trust() {
    let dir = TestDir::new();
    let file = dir.0.join("main.rs");
    fs::write(&file, "abcd\n").unwrap();
    let mut editor = editor_with_lsp(&dir, &["--utf8"], 1500);
    assert!(editor.open_path(&file, None, None));
    assert!(
        wait_editor(&mut editor, |e| e.lsp_ready()),
        "LSP never became ready: {}",
        editor.lsp_status()
    );
    editor.execute(EditorCommand::new(CommandId::Completion));
    assert!(
        wait_editor(&mut editor, |e| !e.completion_items().is_empty()),
        "completion never arrived"
    );
    assert!(matches!(
        editor.ui_mode(),
        UiMode::List(ListState {
            kind: ListKind::Completion,
            ..
        })
    ));
    editor.handle_event(Event::Key(KeyEvent::plain(Key::Escape)));
    editor.execute(EditorCommand::new(CommandId::FormatBuffer));
    assert!(
        wait_editor(&mut editor, |e| e.document_text().contains("FORMATTED")),
        "formatting edit not applied: status {}",
        editor.status()
    );
    editor.execute(EditorCommand::with_args(
        CommandId::RenameSymbol,
        vec!["NEW".into()],
    ));
    assert!(
        wait_editor(&mut editor, |e| e.document_text().contains("RENAMED")),
        "validated workspace edit not applied: status {}",
        editor.status()
    );
    editor.execute(EditorCommand::new(CommandId::CodeAction));
    assert!(
        wait_editor(&mut editor, |e| matches!(
            e.ui_mode(),
            UiMode::List(ListState {
                kind: ListKind::CodeActions,
                ..
            })
        )),
        "code actions not shown"
    );
    editor.handle_event(Event::Key(KeyEvent::plain(Key::Down)));
    editor.handle_event(Event::Key(KeyEvent::plain(Key::Enter)));
    assert!(
        editor.status().contains("not executed automatically"),
        "status: {}",
        editor.status()
    );
    editor.shutdown();
}

#[test]
fn stale_editor_response_is_discarded_and_editing_continues() {
    let dir = TestDir::new();
    let file = dir.0.join("stale.rs");
    fs::write(&file, "abc\n").unwrap();
    let mut editor = editor_with_lsp(&dir, &["--delay-ms", "250"], 2000);
    assert!(editor.open_path(&file, None, None));
    assert!(
        wait_editor(&mut editor, |e| e.lsp_ready()),
        "LSP never became ready"
    );
    editor.execute(EditorCommand::new(CommandId::Completion));
    editor.handle_event(Event::Key(KeyEvent::char('!')));
    assert!(
        wait_editor(&mut editor, |e| e
            .lsp_event_log()
            .iter()
            .any(|event| event.contains("discarded stale"))),
        "stale event missing: {:?}",
        editor.lsp_event_log()
    );
    assert!(editor.document_text().starts_with('!'));
    editor.shutdown();
}

#[test]
fn editing_continues_after_server_crash() {
    let dir = TestDir::new();
    let file = dir.0.join("crash.rs");
    fs::write(&file, "abc\n").unwrap();
    let mut editor = editor_with_lsp(&dir, &["--crash"], 1500);
    assert!(editor.open_path(&file, None, None));
    assert!(
        wait_editor(&mut editor, |e| e.lsp_ready()),
        "LSP never became ready"
    );
    assert!(
        wait_editor(&mut editor, |e| e.lsp_status().contains("exited")),
        "crash not reported: {}",
        editor.lsp_status()
    );
    editor.handle_event(Event::Key(KeyEvent::char('z')));
    assert!(editor.document_text().starts_with('z'));
    editor.shutdown();
}
