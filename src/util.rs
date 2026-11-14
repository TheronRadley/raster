//! Small platform-neutral helpers. Nothing in this module touches the terminal.

use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use unicode_width::UnicodeWidthChar;

/// FNV-1a is used only as a local change-detection fingerprint, never for
/// security. It is stable across processes and simpler than a cryptographic
/// digest for the external-change checks in this editor.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn fnv1a_stream<I: IntoIterator<Item = u8>>(iter: I) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in iter {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Replace characters that a terminal could interpret as control sequences or
/// that could silently change display ordering. Document bytes are never
/// modified; this is only used at the output boundary.
pub fn sanitize_char(ch: char) -> char {
    if ch == '\t' || ch == '\n' || ch == '\r' || ch.is_control() || is_bidi_control(ch) {
        '\u{fffd}'
    } else {
        ch
    }
}

pub fn is_bidi_control(ch: char) -> bool {
    matches!(ch as u32,
        0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
}

pub fn sanitize_display(text: &str) -> String {
    text.chars().map(sanitize_char).collect()
}

/// Width of one grapheme cluster. A cluster is never simply the sum of its
/// codepoint widths: ZWJ emoji and combining marks are one visual cell group.
/// Ambiguous-width East Asian characters use the configured CJK policy.
pub fn grapheme_width(grapheme: &str, cjk_wide: bool) -> usize {
    let mut width = 0usize;
    for ch in grapheme.chars() {
        if ch == '\u{200d}' || ch == '\u{fe0f}' || ch == '\u{fe0e}' {
            continue;
        }
        let w = if cjk_wide {
            UnicodeWidthChar::width_cjk(ch).unwrap_or(1)
        } else {
            UnicodeWidthChar::width(ch).unwrap_or(1)
        };
        if w > width {
            width = w;
        }
    }
    width
}

pub fn display_width(text: &str, cjk_wide: bool) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    text.graphemes(true)
        .map(|g| grapheme_width(g, cjk_wide))
        .sum()
}

/// Truncate an already sanitized string to a number of display columns.
/// A leading ellipsis is used when the beginning is hidden.
pub fn truncate_to_width(text: &str, max_cols: usize, cjk_wide: bool) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if max_cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut cols = 0usize;
    for g in text.graphemes(true) {
        let w = grapheme_width(g, cjk_wide);
        if cols + w > max_cols {
            break;
        }
        out.push_str(g);
        cols += w;
    }
    out
}

pub fn ellipsize(text: &str, max_cols: usize, cjk_wide: bool) -> String {
    let safe = sanitize_display(text);
    if display_width(&safe, cjk_wide) <= max_cols {
        return safe;
    }
    if max_cols <= 1 {
        return "…".chars().take(max_cols).collect();
    }
    let tail = truncate_to_width(&safe, max_cols - 1, cjk_wide);
    format!("…{tail}")
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn short_path(path: &Path, root: &Path) -> String {
    let candidate = path.strip_prefix(root).unwrap_or(path);
    candidate.to_string_lossy().replace('\\', "/")
}

pub fn home_dir() -> PathBuf {
    if let Ok(value) = env::var("RASTER_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Ok(value) = env::var("HOME") {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Ok(value) = env::var("USERPROFILE") {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    PathBuf::from(".")
}

pub fn is_termux() -> bool {
    env::var("PREFIX")
        .map(|p| p.contains("com.termux"))
        .unwrap_or(false)
        || env::var("TERMUX_VERSION").is_ok()
}

pub fn config_dir() -> PathBuf {
    if let Ok(value) = env::var("RASTER_CONFIG_DIR") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(value) = env::var("APPDATA") {
            return PathBuf::from(value).join("raster");
        }
    }
    let xdg = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    xdg.unwrap_or_else(|| home_dir().join(".config"))
        .join("raster")
}

pub fn state_dir() -> PathBuf {
    if let Ok(value) = env::var("RASTER_STATE_DIR") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(value) = env::var("LOCALAPPDATA") {
            return PathBuf::from(value).join("raster");
        }
    }
    let xdg = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    xdg.unwrap_or_else(|| home_dir().join(".local").join("state"))
        .join("raster")
}

pub fn recovery_dir() -> PathBuf {
    state_dir().join("recovery")
}

pub fn session_file() -> PathBuf {
    state_dir().join("session.json")
}

/// Create a directory without changing the permissions of an existing one.
/// Save code uses this: silently chmod-ing a user's project directory would be
/// a destructive side effect.
pub fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// Create a private directory. Newly created state directories are 0700;
/// existing directories are left alone by `ensure_private_dir`.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    let existed = path.exists();
    fs::create_dir_all(path)?;
    if !existed {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(path, perms)?;
        }
    }
    Ok(())
}

/// Enforce 0700 even on an existing Raster-owned state directory.
pub fn enforce_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

pub fn set_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let n = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, n)
}

pub fn stable_hash<T: Hash>(value: &T) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// Deterministic PRNG for property tests, replay tests, and benchmarks.
/// It is not cryptographic.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    pub fn pick<'a, T>(&mut self, values: &'a [T]) -> &'a T {
        &values[self.below(values.len())]
    }
}

/// Wait for a child process without blocking forever on a hung external tool.
pub fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn clamp<T: Ord>(value: T, low: T, high: T) -> T {
    value.max(low).min(high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_escape_and_bidi_controls() {
        let input = "a\x1b[31m\u{202e}b\n\t\u{200d}c";
        let out = sanitize_display(input);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\u{202e}'));
        assert!(!out.contains('\n'));
        assert!(!out.contains('\t'));
        assert!(out.contains('\u{fffd}'));
        assert!(out.ends_with('c'));
    }

    #[test]
    fn width_handles_combining_and_emoji() {
        assert_eq!(display_width("e\u{301}", false), 1);
        assert_eq!(display_width("界", false), 2);
        assert_eq!(display_width("👩\u{200d}💻", false), 2);
        assert_eq!(display_width("é", false), 1);
    }

    #[test]
    fn fnv_is_stable_for_same_bytes() {
        assert_eq!(fnv1a(b"abc"), fnv1a_stream(b"abc".iter().copied()));
        assert_ne!(fnv1a(b"abc"), fnv1a(b"abd"));
    }
}
