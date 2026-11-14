//! Reproducible, dependency-free engine benchmark. It reports editor
//! processing and frame construction separately; no terminal output, LSP, Git,
//! recovery, or network work is included in the engine timings.

use std::env;
use std::fs;
use std::process::Command;
use std::time::Instant;

use raster::app::editor::Editor;
use raster::cli::VERSION;
use raster::config::Config;
use raster::core::commands::{Command as EditorCommand, CommandId};
use raster::core::search::{find_matches, SearchOptions};
use raster::core::syntax::{compute_spans, Language};
use raster::json::Json;
use raster::position::{BytePos, Size};
use raster::term::input::{Event, KeyEvent};
use raster::util::{fnv1a, XorShift64};

struct Options {
    size_mib: usize,
    iterations: usize,
    seed: u64,
    output: Option<String>,
    large_file_mode: bool,
}

fn parse_args() -> Options {
    let mut out = Options {
        size_mib: 1,
        iterations: 120,
        seed: 0x526173746572,
        output: None,
        large_file_mode: false,
    };
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!(
                    "raster-bench [--size-mib N] [--iterations N] [--seed N] [--output FILE] [--large-file]"
                );
                println!("version {VERSION}");
                std::process::exit(0);
            }
            "--size-mib" => {
                i += 1;
                out.size_mib = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(out.size_mib);
            }
            "--iterations" => {
                i += 1;
                out.iterations = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(out.iterations);
            }
            "--seed" => {
                i += 1;
                out.seed = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(out.seed);
            }
            "--output" => {
                i += 1;
                out.output = args.get(i).cloned();
            }
            "--large-file" => out.large_file_mode = true,
            _ => {}
        }
        i += 1;
    }
    out.size_mib = out.size_mib.clamp(1, 256);
    out.iterations = out.iterations.clamp(1, 10_000);
    out
}

fn main() {
    let options = parse_args();
    match run(&options) {
        Ok(json) => {
            let text = json.to_compact_string();
            if let Some(path) = &options.output {
                if let Err(e) = fs::write(path, format!("{text}\n")) {
                    eprintln!("raster-bench: cannot write {path}: {e}");
                    std::process::exit(1);
                }
            } else {
                println!("{text}");
            }
        }
        Err(e) => {
            eprintln!("raster-bench: {e}");
            std::process::exit(1);
        }
    }
}

fn run(options: &Options) -> Result<Json, String> {
    let target_bytes = options.size_mib.saturating_mul(1024 * 1024);
    let start = Instant::now();
    let fixture = fixture(target_bytes, options.seed);
    let fixture_ms = start.elapsed().as_secs_f64() * 1000.0;
    if fixture.len() != target_bytes {
        return Err("fixture generation produced an unexpected size".into());
    }
    let load_start = Instant::now();
    let mut config = Config::default();
    config.session.restore = false;
    config.session.recovery = false;
    config.lsp.enabled = false;
    config.git.enabled = false;
    config.ui.mouse = false;
    config.editor.large_file_threshold = if options.large_file_mode { 0 } else { u64::MAX };
    let mut editor = Editor::for_test(config.clone(), &fixture);
    editor.active_buffer_mut().large_mode = options.large_file_mode;
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
    let mut rng = XorShift64::new(options.seed ^ 0xa5a5_5a5a);
    let mut edit_us = Vec::new();
    let mut navigation_us = Vec::new();
    for _ in 0..options.iterations {
        let begin = Instant::now();
        editor.handle_event(Event::Key(KeyEvent::char('x')));
        editor.execute(EditorCommand::new(CommandId::Undo));
        editor.execute(EditorCommand::new(CommandId::Redo));
        edit_us.push(begin.elapsed().as_secs_f64() * 1_000_000.0);
        let doc = &editor.active_buffer().document;
        let len = doc.len_bytes().get().max(1);
        let pos = doc.clamp(BytePos(rng.next_u64() as usize % len));
        let nav_begin = Instant::now();
        let _ = doc.next_grapheme(pos);
        let _ = doc.prev_grapheme(pos);
        let line = doc.byte_to_line(pos);
        let line_start = doc.line_to_byte(line);
        let _ = doc.display_info(
            line_start..pos,
            config.editor.tab_width,
            config.editor.cjk_wide,
        );
        let _ = doc.line_col(pos, config.editor.tab_width, config.editor.cjk_wide);
        navigation_us.push(nav_begin.elapsed().as_secs_f64() * 1_000_000.0);
    }
    let doc = DocumentSnapshot::from(&editor.active_buffer().document);
    let search_samples = measure_search(&doc, &fixture, options.iterations.min(20));
    let syntax_samples = measure_syntax(&doc, options.iterations.min(20));
    let render_samples = measure_render(&mut editor, options.iterations.min(20));
    let mut root = Json::object();
    root.insert("status", Json::string("ok"));
    root.insert("tool", Json::string("raster-bench"));
    root.insert("version", Json::string(VERSION));
    root.insert("method", Json::string("deterministic in-memory UTF-8 fixture; editor insert/undo/redo cycles, navigation, bounded literal search, visible-line syntax, and 80x24 frame construction measured separately; no terminal writes, LSP, Git, recovery, or network"));
    root.insert("limitations", Json::string("benchmark-process RSS includes the fixture and allocator retention, so it is not editor idle RSS; LSP memory is excluded and measured separately as null"));
    root.insert("large_file_mode", Json::Bool(options.large_file_mode));
    root.insert(
        "build_profile",
        Json::string(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }),
    );
    root.insert("os", Json::string(env::consts::OS));
    root.insert("arch", Json::string(env::consts::ARCH));
    root.insert(
        "rustc",
        Json::string(rustc_version().unwrap_or_else(|| "unavailable".into())),
    );
    root.insert(
        "cpu",
        Json::string(cpu_model().unwrap_or_else(|| "unavailable".into())),
    );
    root.insert(
        "terminal",
        Json::string(env::var("TERM").unwrap_or_else(|_| "unset".into())),
    );
    root.insert("tmux", Json::Bool(env::var_os("TMUX").is_some()));
    root.insert(
        "ssh",
        Json::Bool(env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some()),
    );
    root.insert("seed", Json::Int(options.seed as i64));
    root.insert("fixture_bytes", Json::Int(fixture.len() as i64));
    root.insert(
        "fixture_sha_like_fnv1a",
        Json::Int(fnv1a(fixture.as_bytes()) as i64),
    );
    root.insert("fixture_generation_ms", Json::Float(fixture_ms));
    root.insert("document_load_ms", Json::Float(load_ms));
    root.insert("iterations", Json::Int(options.iterations as i64));
    let mut engine = Json::object();
    engine.insert(
        "edit_cycle_p50_us",
        Json::Float(percentile(&mut edit_us.clone(), 50.0)),
    );
    engine.insert(
        "edit_cycle_p95_us",
        Json::Float(percentile(&mut edit_us.clone(), 95.0)),
    );
    engine.insert(
        "edit_cycle_max_us",
        Json::Float(edit_us.iter().cloned().fold(0.0, f64::max)),
    );
    engine.insert(
        "navigation_p50_us",
        Json::Float(percentile(&mut navigation_us.clone(), 50.0)),
    );
    engine.insert(
        "navigation_p95_us",
        Json::Float(percentile(&mut navigation_us.clone(), 95.0)),
    );
    engine.insert(
        "search_p50_ms",
        Json::Float(percentile(&mut search_samples.clone(), 50.0)),
    );
    engine.insert(
        "search_p95_ms",
        Json::Float(percentile(&mut search_samples.clone(), 95.0)),
    );
    root.insert("engine", engine);
    let mut rendering = Json::object();
    rendering.insert(
        "frame_p50_ms",
        Json::Float(percentile(&mut render_samples.clone(), 50.0)),
    );
    rendering.insert(
        "frame_p95_ms",
        Json::Float(percentile(&mut render_samples.clone(), 95.0)),
    );
    root.insert("rendering", rendering);
    let mut syntax = Json::object();
    syntax.insert(
        "visible_spans_p50_ms",
        Json::Float(percentile(&mut syntax_samples.clone(), 50.0)),
    );
    syntax.insert(
        "visible_spans_p95_ms",
        Json::Float(percentile(&mut syntax_samples.clone(), 95.0)),
    );
    root.insert("syntax", syntax);
    let mut memory = Json::object();
    memory.insert(
        "benchmark_process_rss_kib",
        proc_rss_kib()
            .map(|v| Json::Int(v as i64))
            .unwrap_or(Json::Null),
    );
    memory.insert("editor_idle_rss_without_lsp_kib", Json::Null);
    memory.insert("lsp_rss_kib", Json::Null);
    root.insert("memory", memory);
    Ok(root)
}

struct DocumentSnapshot {
    document: raster::core::document::Document,
}
impl DocumentSnapshot {
    fn from(document: &raster::core::document::Document) -> Self {
        Self {
            document: document.clone(),
        }
    }
}

fn measure_search(snapshot: &DocumentSnapshot, _fixture: &str, iterations: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let options = SearchOptions {
        case_sensitive: true,
        max_results: 1000,
        ..SearchOptions::default()
    };
    for _ in 0..iterations.max(1) {
        let start = Instant::now();
        let _ = find_matches(&snapshot.document, "needle", options, None);
        out.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    out
}

fn measure_syntax(snapshot: &DocumentSnapshot, iterations: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let last = 40.min(snapshot.document.len_lines().saturating_sub(1));
    for _ in 0..iterations.max(1) {
        let start = Instant::now();
        let _ = compute_spans(&snapshot.document, Language::Rust, 0, last);
        out.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    out
}

fn measure_render(editor: &mut Editor, iterations: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for _ in 0..iterations.max(1) {
        let start = Instant::now();
        let _frame = editor.render_frame(Size::new(80, 24));
        out.push(start.elapsed().as_secs_f64() * 1000.0);
        editor.tick();
    }
    out
}

fn fixture(target: usize, seed: u64) -> String {
    let mut rng = XorShift64::new(seed);
    let mut out = String::with_capacity(target.min(16 * 1024 * 1024));
    let mut line = 0usize;
    while out.len() < target {
        let ending = if line.is_multiple_of(17) {
            "\r\n"
        } else {
            "\n"
        };
        let text = format!(
            "{:08} let value_{} = \"needle مرحبا e\u{301} 👩‍💻 漢字\"; // raster fixture{}\n",
            line,
            rng.next_u64() % 100_000,
            ending.replace('\n', "")
        );
        // Append a controlled CRLF/LF terminator. The trailing byte budget is
        // filled with ASCII so truncation never splits a UTF-8 scalar.
        let mut chunk = text.trim_end_matches('\n').to_string();
        chunk.push_str(ending);
        if out.len() + chunk.len() > target {
            let remaining = target - out.len();
            out.push_str(&"x".repeat(remaining));
            break;
        }
        out.push_str(&chunk);
        line += 1;
    }
    out
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * (samples.len() - 1) as f64).round() as usize;
    samples[rank.min(samples.len() - 1)]
}

fn proc_rss_kib() -> Option<usize> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }
    None
}

fn rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cpu_model() -> Option<String> {
    let text = fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            if let Some(value) = rest.split_once(':').map(|(_, v)| v.trim().to_string()) {
                return Some(value);
            }
        }
    }
    None
}
