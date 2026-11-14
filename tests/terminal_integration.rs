use std::fs;
use std::path::PathBuf;
use std::process::Command;

use raster::app::editor::{Editor, UiMode};
use raster::cli::CliOptions;
use raster::config::Config;
use raster::core::commands::{Command as EditorCommand, CommandId};
use raster::position::Size;
use raster::term::input::{Event, Key, KeyEvent};
use raster::util::unique_suffix;

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("raster-terminal-{}", unique_suffix()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn raster_command(dir: &TestDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_raster"));
    command
        .env("XDG_CONFIG_HOME", dir.0.join("config"))
        .env("XDG_STATE_HOME", dir.0.join("state"))
        .env("RASTER_CONFIG_DIR", dir.0.join("config"))
        .env("RASTER_STATE_DIR", dir.0.join("state"))
        .env("HOME", &dir.0)
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .env("LINES", "24");
    command
}

#[test]
fn cli_help_version_health_and_startup_json() {
    let dir = TestDir::new();
    let help = raster_command(&dir).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));
    let version = raster_command(&dir).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("raster 0.1.0"));
    let health = raster_command(&dir)
        .args(["--no-config", "--health"])
        .output()
        .unwrap();
    assert!(
        health.status.success(),
        "{}",
        String::from_utf8_lossy(&health.stderr)
    );
    let health_json =
        raster::json::Json::parse(String::from_utf8_lossy(&health.stdout).trim()).unwrap();
    assert_eq!(
        health_json
            .get("status")
            .and_then(raster::json::Json::as_str),
        Some("ok")
    );
    let startup = raster_command(&dir)
        .args(["--no-config", "--bench-startup"])
        .output()
        .unwrap();
    assert!(
        startup.status.success(),
        "{}",
        String::from_utf8_lossy(&startup.stderr)
    );
    let startup_json =
        raster::json::Json::parse(String::from_utf8_lossy(&startup.stdout).trim()).unwrap();
    assert!(
        startup_json
            .get("median_ms")
            .and_then(raster::json::Json::as_i64)
            .is_some()
            || startup_json
                .get("median_ms")
                .unwrap()
                .to_compact_string()
                .contains('.')
    );
}

#[test]
fn replayed_input_dumps_a_plain_screen_without_a_terminal() {
    let dir = TestDir::new();
    let script = dir.0.join("replay.txt");
    let screen = dir.0.join("screen.txt");
    fs::write(&script, "# deterministic input replay\npaste:مرحبا 😀\nkey:ctrl+z\nkey:ctrl+y\nkey:ctrl+shift+p\nkey:esc\n").unwrap();
    let output = raster_command(&dir)
        .args([
            "--no-config",
            "--no-session",
            "--no-lsp",
            "--no-mouse",
            "--no-color",
            "--replay",
            script.to_str().unwrap(),
            "--dump-screen",
            screen.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(&screen).unwrap();
    assert!(text.contains("مرحبا"));
    assert!(text.contains("F1 help"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn shipped_replay_example_produces_a_plain_screen() {
    let dir = TestDir::new();
    let screen = dir.0.join("screen.txt");
    let output = raster_command(&dir)
        .args([
            "--no-config",
            "--no-session",
            "--no-lsp",
            "--no-mouse",
            "--no-color",
            "--replay",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join("replay.txt")
                .to_str()
                .unwrap(),
            "--dump-screen",
        ])
        .arg(&screen)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(&screen).unwrap();
    assert!(text.contains("مرحبا"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn untrusted_filename_and_content_never_emit_raw_sgr() {
    let dir = TestDir::new();
    // Avoid backslashes and control characters in the filename: Windows
    // treats `\` as a separator and rejects 0x1B in filenames. The status
    // line below still exercises a real escape sequence.
    let path = dir.0.join("safe_[31m_\u{202e}.rs");
    fs::write(&path, "let x = 1;\u{202e}\n").unwrap();
    let cli = CliOptions {
        no_session: true,
        project: Some(dir.0.clone()),
        ..Default::default()
    };
    let mut config = Config::default();
    config.session.restore = false;
    config.session.recovery = false;
    config.lsp.enabled = false;
    config.git.enabled = false;
    let mut editor = Editor::headless(config, cli, Size::new(80, 24)).unwrap();
    assert!(editor.open_path(&path, None, None));
    editor.set_status("diagnostic \u{1b}]52;c;danger");
    let ansi = editor.render_ansi(Size::new(80, 24));
    assert!(!String::from_utf8_lossy(&ansi).contains("\x1b[31m"));
    assert!(!String::from_utf8_lossy(&ansi).contains("\x1b]52"));
    editor.shutdown();
}

#[test]
fn resize_during_dialog_and_nested_splits_have_bounded_layout() {
    let mut editor = Editor::for_test(Config::default(), "one\ntwo\nthree\nfour\nfive");
    editor.execute(EditorCommand::new(CommandId::CommandPalette));
    for size in [
        Size::new(12, 4),
        Size::new(24, 6),
        Size::new(120, 40),
        Size::new(31, 9),
    ] {
        let frame = editor.render_frame(size);
        assert_eq!(frame.height, size.rows);
        assert_eq!(frame.width, size.cols);
        assert!(matches!(editor.ui_mode(), UiMode::List(_)));
    }
    editor.handle_event(Event::Key(KeyEvent::plain(Key::Escape)));
    // The previous resize left a small viewport; restore a practical size
    // before requesting additional splits (a small terminal must refuse them).
    editor.render_frame(Size::new(80, 24));
    editor.execute(EditorCommand::new(CommandId::SplitVertical));
    editor.execute(EditorCommand::new(CommandId::SplitHorizontal));
    assert_eq!(editor.view_count(), 3);
    let frame = editor.render_frame(Size::new(80, 24));
    assert_eq!(frame.render_plain().lines().count(), 24);
    editor.execute(EditorCommand::new(CommandId::EqualizeSplits));
}

#[test]
fn minimal_shell_lists_commands_and_persists_cd() {
    use std::io::Write;
    use std::process::Stdio;
    let dir = TestDir::new();
    let target = dir.0.join("folder with spaces");
    fs::create_dir(&target).unwrap();
    let mut child = raster_command(&dir)
        .arg("--shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    write!(
        child.stdin.take().unwrap(),
        "help\ncd \"{}\"\ncd\necho raster-shell-ok\nexit\n",
        target.display()
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Raster commands:"));
    assert!(text.contains("edit [file]"));
    assert!(text.contains(&*target.to_string_lossy()));
    assert!(text.contains("raster-shell-ok"));
    assert!(!text.contains("F1 help"));
}

#[test]
fn shell_requires_explicit_flag_and_editor_needs_terminal() {
    use std::io::Write;
    use std::process::Stdio;
    let dir = TestDir::new();
    let mut child = raster_command(&dir)
        .arg("--shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    write!(child.stdin.take().unwrap(), "edit a.txt\nexit\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires an interactive terminal"));
    let bad = raster_command(&dir)
        .args(["--shell", "notes.txt"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}
