//! Linux-only interactive PTY smoke test. Other platforms validate the same
//! controller through headless replay; CI platform status is documented in
//! docs/PLATFORMS.md.
#![cfg(target_os = "linux")]

use std::process::Command;

use raster::json::Json;

#[test]
fn interactive_pty_edits_saves_resizes_and_restores_terminal() {
    let python = std::env::var("RASTER_PYTHON").unwrap_or_else(|_| "python3".into());
    let output = match Command::new(&python)
        .args([
            "scripts/pty_smoke.py",
            "--binary",
            env!("CARGO_BIN_EXE_raster"),
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            eprintln!("PTY smoke test unavailable: {python} not found");
            return;
        }
    };
    assert!(
        output.status.success(),
        "PTY smoke test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = Json::parse(String::from_utf8_lossy(&output.stdout).trim())
        .expect("PTY smoke report must be valid JSON");
    assert_eq!(report.get("passed").and_then(Json::as_bool), Some(true));
    let checks = report.get("checks").expect("missing checks");
    for name in [
        "first_frame",
        "paste_frame",
        "help_overlay",
        "resize_frame",
        "saved_exact",
        "exit_clean",
        "terminal_flags_restored",
        "cleanup_sequences",
        "no_untrusted_raw_escape",
    ] {
        assert_eq!(
            checks.get(name).and_then(Json::as_bool),
            Some(true),
            "PTY check {name} failed"
        );
    }
}
