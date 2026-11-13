//! Internal editor registers, platform system clipboard integration, and
//! explicitly controlled OSC 52. No shell is used and no clipboard side effect
//! occurs over SSH unless the user opts in.

use std::env;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::ClipboardConfig;
use crate::util::{is_termux, wait_with_timeout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardError(pub String);
impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ClipboardError {}

#[derive(Clone, Debug)]
pub struct ClipboardManager {
    config: ClipboardConfig,
    internal: String,
    linewise: bool,
    remote: bool,
}

impl ClipboardManager {
    pub fn new(config: ClipboardConfig, remote: bool) -> Self {
        Self {
            config,
            internal: String::new(),
            linewise: false,
            remote,
        }
    }

    pub fn warning(&self) -> Option<&'static str> {
        if self.remote && (self.config.system || self.config.osc52) && !self.config.allow_remote {
            Some("remote clipboard disabled; set clipboard.allow_remote = true explicitly")
        } else {
            None
        }
    }

    pub fn set_internal(&mut self, text: String, linewise: bool) {
        self.internal = text;
        self.linewise = linewise;
    }

    pub fn internal(&self) -> &str {
        &self.internal
    }
    pub fn is_linewise(&self) -> bool {
        self.linewise
    }

    pub fn system_allowed(&self) -> bool {
        self.config.system && (!self.remote || self.config.allow_remote)
    }

    pub fn osc52_allowed(&self) -> bool {
        self.config.osc52 && (!self.remote || self.config.allow_remote)
    }

    pub fn system_set(&self, text: &str) -> Result<String, ClipboardError> {
        if !self.system_allowed() {
            return Err(ClipboardError(
                "system clipboard is disabled for this session".into(),
            ));
        }
        let (program, args) = self
            .set_command()
            .ok_or_else(|| ClipboardError("no supported clipboard tool found".into()))?;
        run_clipboard(program, &args, Some(text))?;
        Ok(program.to_string())
    }

    pub fn system_get(&self) -> Result<Option<String>, ClipboardError> {
        if !self.system_allowed() {
            return Err(ClipboardError(
                "system clipboard is disabled for this session".into(),
            ));
        }
        let (program, args) = self
            .get_command()
            .ok_or_else(|| ClipboardError("no supported clipboard tool found".into()))?;
        let text = run_clipboard(program, &args, None)?;
        Ok(Some(text.trim_end_matches(['\r', '\n']).to_string()))
    }

    /// OSC 52 payload is base64, bounded, and never contains raw document
    /// bytes. It is emitted only when explicitly enabled.
    pub fn osc52_sequence(&self, text: &str) -> Option<Vec<u8>> {
        if !self.osc52_allowed() {
            return None;
        }
        if text.len() > self.config.osc52_max_bytes {
            return None;
        }
        let encoded = base64(text.as_bytes());
        let mut out = Vec::with_capacity(encoded.len() + 8);
        out.extend_from_slice(b"\x1b]52;c;");
        out.extend_from_slice(encoded.as_bytes());
        out.extend_from_slice(b"\x07");
        Some(out)
    }

    fn set_command(&self) -> Option<(&'static str, Vec<String>)> {
        clipboard_command(true)
    }

    fn get_command(&self) -> Option<(&'static str, Vec<String>)> {
        clipboard_command(false)
    }
}

fn clipboard_command(set: bool) -> Option<(&'static str, Vec<String>)> {
    if let Ok(custom) = env::var(if set {
        "RASTER_CLIPBOARD_SET_CMD"
    } else {
        "RASTER_CLIPBOARD_GET_CMD"
    }) {
        if !custom.is_empty() {
            // Return a leaked static string so the command table has one type.
            let leaked: &'static str = Box::leak(custom.into_boxed_str());
            return Some((leaked, Vec::new()));
        }
    }
    if is_termux() {
        return Some((
            if set {
                "termux-clipboard-set"
            } else {
                "termux-clipboard-get"
            },
            Vec::new(),
        ));
    }
    if cfg!(target_os = "macos") {
        return Some((if set { "pbcopy" } else { "pbpaste" }, Vec::new()));
    }
    if cfg!(windows) {
        return Some((
            "powershell",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                if set {
                    "$input | Set-Clipboard".into()
                } else {
                    "Get-Clipboard -Raw".into()
                },
            ],
        ));
    }
    if env::var_os("WAYLAND_DISPLAY").is_some() {
        return Some((
            if set { "wl-copy" } else { "wl-paste" },
            if set {
                Vec::new()
            } else {
                vec!["--no-newline".into()]
            },
        ));
    }
    if env::var_os("DISPLAY").is_some() {
        return Some((
            "xclip",
            if set {
                vec!["-selection".into(), "clipboard".into(), "-i".into()]
            } else {
                vec!["-selection".into(), "clipboard".into(), "-o".into()]
            },
        ));
    }
    None
}

fn run_clipboard(
    program: &str,
    args: &[String],
    input: Option<&str>,
) -> Result<String, ClipboardError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("LC_ALL", "C")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| ClipboardError(format!("{program}: {e}")))?;
    if let Some(text) = input {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| ClipboardError(e.to_string()))?;
        }
    }
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut stream) = stdout.take() {
            let _ = stream.read_to_string(&mut out);
        }
        out
    });
    let status = wait_with_timeout(&mut child, Duration::from_secs(2))
        .map_err(|e| ClipboardError(e.to_string()))?
        .ok_or_else(|| ClipboardError(format!("{program} timed out")))?;
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(ClipboardError(format!("{program} exited with {status}")));
    }
    Ok(output)
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 63) as usize] as char);
        out.push(TABLE[((triple >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ClipboardConfig {
        ClipboardConfig {
            system: false,
            osc52: false,
            osc52_max_bytes: 16,
            allow_remote: false,
        }
    }

    #[test]
    fn internal_register_is_distinct_and_linewise() {
        let mut clip = ClipboardManager::new(config(), false);
        clip.set_internal("مرحبا\n".into(), true);
        assert_eq!(clip.internal(), "مرحبا\n");
        assert!(clip.is_linewise());
        assert!(clip.system_set("x").is_err());
    }

    #[test]
    fn osc52_is_opt_in_bounded_and_base64_encoded() {
        let mut cfg = config();
        cfg.osc52 = true;
        let clip = ClipboardManager::new(cfg, false);
        let seq = clip.osc52_sequence("abc").unwrap();
        assert_eq!(&seq[..7], b"\x1b]52;c;");
        assert!(seq.ends_with(&[0x07]));
        assert_eq!(String::from_utf8_lossy(&seq[7..seq.len() - 1]), "YWJj");
        assert!(clip.osc52_sequence("0123456789abcdefg").is_none());
    }

    #[test]
    fn remote_clipboard_requires_explicit_opt_in() {
        let mut cfg = config();
        cfg.system = true;
        cfg.osc52 = true;
        let clip = ClipboardManager::new(cfg, true);
        assert!(clip.warning().is_some());
        assert!(!clip.system_allowed());
        assert!(!clip.osc52_allowed());
    }

    #[test]
    fn base64_padding_is_correct() {
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }
}
