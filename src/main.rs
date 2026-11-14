//! Raster executable entry point: CLI, terminal loop, and headless test modes.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use raster::app::editor::{Editor, EditorAction};
use raster::cli::{CliOptions, HELP, VERSION};
use raster::config::Config;
use raster::fs as raster_fs;
use raster::json::Json;
use raster::position::Size;
use raster::session::RecoveryManager;
use raster::term::input::{Event, InputParser, KeyChord};
use raster::term::terminal::{self, Terminal};
use raster::util::{config_dir, recovery_dir, session_file};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            let _ = writeln!(io::stderr(), "raster: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> io::Result<u8> {
    let cli = CliOptions::from_env().map_err(to_io)?;
    if cli.show_help {
        print!("{HELP}");
        return Ok(0);
    }
    if cli.show_version {
        println!("raster {VERSION}");
        println!(
            "edition 2021 · license MIT · {} / {}",
            env::consts::OS,
            env::consts::ARCH
        );
        println!(
            "build profile: {}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        return Ok(0);
    }
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(|| config_dir().join("config.toml"));
    let mut config = load_config(&cli, &config_path)?;
    config.apply_cli(&cli);
    config.validate().map_err(to_io)?;
    if cli.bench_startup {
        bench_startup(&config)?;
        return Ok(0);
    }
    if cli.health {
        print_health(&config, &config_path)?;
        return Ok(0);
    }
    if cli.clean_recovery {
        clean_recovery(&config)?;
        return Ok(0);
    }
    if cli.dump_screen.is_some() || cli.replay.is_some() {
        run_headless(config, cli)?;
        return Ok(0);
    }
    run_interactive(config, cli).map(|()| 0)
}

fn load_config(cli: &CliOptions, path: &Path) -> io::Result<Config> {
    let mut config = if cli.no_config || !path.exists() {
        Config::default()
    } else {
        Config::load(path).map_err(to_io)?
    };
    config.apply_cli(cli);
    Ok(config)
}

fn to_io<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
}

fn print_health(config: &Config, config_path: &Path) -> io::Result<()> {
    use std::io::IsTerminal;
    let mut root = Json::object();
    root.insert("status", Json::string("ok"));
    root.insert("config_path", Json::string(config_path.to_string_lossy()));
    root.insert("config_exists", Json::Bool(config_path.exists()));
    root.insert("config_valid", Json::Bool(config.validate().is_ok()));
    root.insert(
        "recovery_dir",
        Json::string(recovery_dir().to_string_lossy()),
    );
    root.insert(
        "session_file",
        Json::string(session_file().to_string_lossy()),
    );
    root.insert("stdin_tty", Json::Bool(io::stdin().is_terminal()));
    root.insert("stdout_tty", Json::Bool(io::stdout().is_terminal()));
    root.insert("os", Json::string(env::consts::OS));
    root.insert("arch", Json::string(env::consts::ARCH));
    println!("{}", root.to_compact_string());
    Ok(())
}

fn clean_recovery(config: &Config) -> io::Result<()> {
    let dir = recovery_dir();
    let retention =
        Duration::from_secs(config.session.recovery_retention_days.saturating_mul(86400));
    let manager = RecoveryManager::start(dir.clone())?;
    let snapshots = manager.cleanup(retention);
    let journals = raster_fs::cleanup_save_journals(&dir, retention);
    let mut result = Json::object();
    result.insert("status", Json::string("ok"));
    result.insert("removed_snapshots", Json::Int(snapshots as i64));
    result.insert("removed_journals", Json::Int(journals as i64));
    println!("{}", result.to_compact_string());
    Ok(())
}

fn bench_startup(config: &Config) -> io::Result<()> {
    let mut config = config.clone();
    config.session.restore = false;
    config.session.recovery = false;
    config.lsp.enabled = false;
    config.git.enabled = false;
    let cli = CliOptions {
        no_config: true,
        no_session: true,
        ..Default::default()
    };
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let mut editor =
            Editor::headless(config.clone(), cli.clone(), Size::new(80, 24)).map_err(to_io)?;
        let _ = editor.render_frame(Size::new(80, 24));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut result = Json::object();
    result.insert("status", Json::string("ok"));
    result.insert("method", Json::string("headless Editor construction + one 80x24 frame; no terminal, LSP, Git, recovery, or file I/O"));
    result.insert(
        "build_profile",
        Json::string(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }),
    );
    result.insert("os", Json::string(env::consts::OS));
    result.insert("arch", Json::string(env::consts::ARCH));
    result.insert("iterations", Json::Int(samples.len() as i64));
    result.insert("min_ms", Json::Float(samples[0]));
    result.insert("median_ms", Json::Float(samples[samples.len() / 2]));
    result.insert("max_ms", Json::Float(samples[samples.len() - 1]));
    println!("{}", result.to_compact_string());
    Ok(())
}

fn run_headless(mut config: Config, mut cli: CliOptions) -> io::Result<()> {
    // Headless replay is deterministic and local: never read session/recovery
    // state and never start optional services from a user's real environment.
    config.session.restore = false;
    config.session.recovery = false;
    config.lsp.enabled = false;
    config.git.enabled = false;
    config.ui.mouse = false;
    cli.no_session = true;
    cli.recover = false;
    let size = terminal::current_size();
    let mut editor = Editor::headless(config, cli.clone(), size).map_err(to_io)?;
    if let Some(replay) = &cli.replay {
        let text = fs::read_to_string(replay)?;
        replay_events(&mut editor, &text);
    }
    editor.shutdown();
    let screen = editor.dump_plain(size);
    if let Some(path) = &cli.dump_screen {
        fs::write(path, screen)?;
    } else {
        print!("{screen}");
    }
    Ok(())
}

fn replay_events(editor: &mut Editor, script: &str) {
    let mut parser = InputParser::new();
    for raw in script.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix("key:") {
            if let Ok(chord) = KeyChord::parse(name.trim()) {
                if let Some(event) = chord.keys.first().copied() {
                    editor.handle_event(Event::Key(event));
                }
            }
            continue;
        }
        if let Some(text) = line.strip_prefix("paste:") {
            editor.handle_event(Event::Paste(text.to_string()));
            continue;
        }
        if let Some(value) = line.strip_prefix("resize:") {
            let mut parts = value.split('x');
            let cols = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(80);
            let rows = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(24);
            editor.handle_event(Event::Resize(Size::new(cols, rows)));
            continue;
        }
        if line == "tick" {
            editor.handle_event(Event::Tick);
            continue;
        }
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\r');
        for event in parser.feed(&bytes) {
            editor.handle_event(event);
        }
        for event in parser.flush() {
            editor.handle_event(event);
        }
    }
    editor.handle_event(Event::Tick);
}

fn run_interactive(config: Config, cli: CliOptions) -> io::Result<()> {
    use std::io::IsTerminal;
    terminal::install_panic_hook();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no interactive terminal; use --dump-screen or --replay for headless output",
        ));
    }
    let mut terminal = Terminal::new(&config)?;
    terminal.enter(config.ui.mouse, config.ui.alt_screen)?;
    let mut editor = Editor::new(config, cli, terminal.size).map_err(to_io)?;
    let mut parser = InputParser::new();
    let mut last_frame = editor.render_ansi(terminal.size);
    terminal.write(&last_frame)?;
    loop {
        let events = terminal.poll_events(&mut parser, Duration::from_millis(50));
        let mut requested = EditorAction::None;
        for event in events {
            let action = editor.handle_event(event);
            if requested == EditorAction::None && action != EditorAction::None {
                requested = action;
            }
            if editor.should_quit() {
                requested = EditorAction::Quit;
                break;
            }
        }
        terminal.set_mouse(editor.mouse_enabled());
        for bytes in editor.take_output() {
            terminal.write(&bytes)?;
        }
        let frame = editor.render_ansi(terminal.size);
        if frame != last_frame {
            terminal.write(&frame)?;
            last_frame = frame;
        }
        match requested {
            EditorAction::Quit => break,
            EditorAction::Suspend => {
                let result = terminal.suspend();
                // The alternate screen may have been re-entered even if the
                // suspend command failed, so redraw in either case.
                last_frame.clear();
                match result {
                    Ok(()) => {
                        editor
                            .handle_event(Event::Signal(raster::term::input::SignalKind::Continue));
                    }
                    Err(e) => editor.set_status(e.to_string()),
                }
            }
            EditorAction::None => {}
        }
    }
    editor.shutdown();
    terminal.restore();
    Ok(())
}
