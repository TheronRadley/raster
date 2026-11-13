//! Project file indexing, ignore rules, fuzzy file lookup, and cancellable
//! project-wide search. Expensive traversal runs on worker threads.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TrySendError};
use std::sync::Arc;
use std::thread;

use regex_lite::Regex;

use crate::core::search::{find_matches_in_text, fuzzy_filter, SearchOptions};
use crate::util::short_path;

#[derive(Clone, Debug)]
struct IgnoreRule {
    base: PathBuf,
    regex: Regex,
    negated: bool,
    dir_only: bool,
    has_slash: bool,
}

impl IgnoreRule {
    fn new(base: PathBuf, pattern: &str) -> Option<Self> {
        let mut raw = pattern.trim().to_string();
        if raw.is_empty() || raw.starts_with('#') {
            return None;
        }
        let negated = raw.starts_with('!');
        if negated {
            raw.remove(0);
        }
        let dir_only = raw.ends_with('/');
        if dir_only {
            raw.pop();
        }
        let anchored = raw.starts_with('/');
        if anchored {
            raw.remove(0);
        }
        if raw.is_empty() {
            return None;
        }
        let has_slash = raw.contains('/');
        let regex_source = glob_to_regex(&raw, anchored, has_slash);
        let regex = Regex::new(&regex_source).ok()?;
        Some(Self {
            base,
            regex,
            negated,
            dir_only,
            has_slash,
        })
    }

    fn matches(&self, rel: &Path, is_dir: bool) -> bool {
        let Ok(from_base) = rel.strip_prefix(&self.base) else {
            return false;
        };
        let text = from_base.to_string_lossy().replace('\\', "/");
        if text.is_empty() {
            return false;
        }
        if self.has_slash {
            if self.regex.is_match(&text) && (!self.dir_only || is_dir || text.contains('/')) {
                return true;
            }
            return false;
        }
        let basename = from_base
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        if self.regex.is_match(&basename) && !self.dir_only {
            return true;
        }
        // Directory-only patterns apply to every ancestor of a file.
        for ancestor in from_base.ancestors().skip(1) {
            if let Some(name) = ancestor.file_name() {
                if self.regex.is_match(&name.to_string_lossy()) {
                    return true;
                }
            }
        }
        is_dir && self.regex.is_match(&basename)
    }
}

fn glob_to_regex(pattern: &str, anchored: bool, has_slash: bool) -> String {
    let mut out = String::from("^");
    // Gitignore-style patterns without a slash may match at any depth;
    // patterns containing a slash are rooted at the ignore file's directory.
    if !anchored && !has_slash {
        out.push_str("(.*/)?");
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' {
            if i + 1 < chars.len() && chars[i + 1] == '*' {
                if i + 2 < chars.len() && chars[i + 2] == '/' {
                    out.push_str("(.*/)?");
                    i += 3;
                } else {
                    out.push_str(".*");
                    i += 2;
                }
            } else {
                out.push_str("[^/]*");
                i += 1;
            }
        } else if c == '?' {
            out.push_str("[^/]");
            i += 1;
        } else {
            if "\\.+^${}()|[]".contains(c) {
                out.push('\\');
            }
            out.push(c);
            i += 1;
        }
    }
    out.push('$');
    out
}

#[derive(Clone, Debug, Default)]
pub struct IgnoreRules {
    rules: Vec<IgnoreRule>,
}

impl IgnoreRules {
    pub fn load(root: &Path) -> Self {
        let mut rules = Vec::new();
        for pattern in [
            ".git/",
            "target/",
            "node_modules/",
            ".venv/",
            "__pycache__/",
            "dist/",
            "build/",
            ".cache/",
            ".DS_Store",
            "*.o",
            "*.so",
            "*.pyc",
            ".raster.tmp-*",
            ".raster-save*.json",
            ".raster.bak-*",
        ] {
            if let Some(rule) = IgnoreRule::new(PathBuf::new(), pattern) {
                rules.push(rule);
            }
        }
        add_ignore_file(&mut rules, PathBuf::new(), &root.join(".gitignore"));
        add_ignore_file(&mut rules, PathBuf::new(), &root.join(".ignore"));
        Self { rules }
    }

    fn with_directory(&self, root: &Path, dir: &Path) -> Self {
        let mut rules = self.rules.clone();
        let base = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
        add_ignore_file(&mut rules, base.clone(), &dir.join(".gitignore"));
        add_ignore_file(&mut rules, base, &dir.join(".ignore"));
        Self { rules }
    }

    pub fn should_include(
        &self,
        rel: &Path,
        is_dir: bool,
        include_hidden: bool,
        include_ignored: bool,
    ) -> bool {
        if !include_hidden {
            for component in rel.components() {
                let name = component.as_os_str().to_string_lossy();
                if name.starts_with('.') && name != "." && name != ".." {
                    return false;
                }
            }
        }
        if include_ignored {
            return true;
        }
        let mut ignored = false;
        for rule in &self.rules {
            if rule.matches(rel, is_dir) {
                ignored = !rule.negated;
            }
        }
        !ignored
    }
}

fn add_ignore_file(rules: &mut Vec<IgnoreRule>, base: PathBuf, path: &Path) {
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            if let Some(rule) = IgnoreRule::new(base.clone(), line) {
                rules.push(rule);
            }
        }
    }
}

/// Deterministic file scan. Symlinks are not followed, preventing loops.
pub fn scan_project(
    root: &Path,
    include_hidden: bool,
    include_ignored: bool,
    cancel: Option<&AtomicBool>,
) -> Vec<String> {
    let mut out = Vec::new();
    let root_rules = IgnoreRules::load(root);
    let options = ScanOptions {
        include_hidden,
        include_ignored,
        cancel,
        depth: 0,
    };
    scan_dir(root, root, &root_rules, options, &mut out);
    out.sort();
    out
}

#[derive(Copy, Clone)]
struct ScanOptions<'a> {
    include_hidden: bool,
    include_ignored: bool,
    cancel: Option<&'a AtomicBool>,
    depth: usize,
}

impl ScanOptions<'_> {
    fn canceled(self) -> bool {
        self.cancel
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    fn next_level(self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self
        }
    }
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    rules: &IgnoreRules,
    options: ScanOptions<'_>,
    out: &mut Vec<String>,
) {
    if options.depth > 64 || options.canceled() {
        return;
    }
    let local_rules = if options.depth == 0 {
        rules.clone()
    } else {
        rules.with_directory(root, dir)
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if options.canceled() {
            return;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let is_dir = meta.is_dir();
        if !local_rules.should_include(
            &rel,
            is_dir,
            options.include_hidden,
            options.include_ignored,
        ) {
            continue;
        }
        if is_dir {
            scan_dir(root, &path, &local_rules, options.next_level(), out);
        } else if meta.is_file() {
            out.push(short_path(&path, root));
            if out.len() >= 100_000 {
                return;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub generation: u64,
    pub files: Vec<String>,
    pub canceled: bool,
}

pub struct ProjectIndex {
    pub root: PathBuf,
    pub files: Vec<String>,
    pub generation: u64,
    rx: Option<Receiver<ScanResult>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl ProjectIndex {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: Vec::new(),
            generation: 0,
            rx: None,
            cancel: None,
        }
    }

    pub fn start_scan(&mut self, include_hidden: bool, include_ignored: bool) {
        self.cancel_scan();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        let root = self.root.clone();
        let cancel_thread = Arc::clone(&cancel);
        thread::Builder::new()
            .name("raster-file-index".into())
            .spawn(move || {
                let files =
                    scan_project(&root, include_hidden, include_ignored, Some(&cancel_thread));
                let _ = tx.send(ScanResult {
                    generation,
                    files,
                    canceled: cancel_thread.load(Ordering::Relaxed),
                });
            })
            .ok();
        self.cancel = Some(cancel);
        self.rx = Some(rx);
    }

    pub fn cancel_scan(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut updated = false;
        if let Some(rx) = &self.rx {
            while let Ok(result) = rx.try_recv() {
                if result.generation == self.generation && !result.canceled {
                    self.files = result.files;
                    updated = true;
                }
            }
        }
        updated
    }

    pub fn fuzzy(&self, query: &str, limit: usize) -> Vec<String> {
        fuzzy_filter(query, &self.files)
            .into_iter()
            .take(limit)
            .map(|(p, _)| p)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMatch {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectSearchMessage {
    Match(ProjectMatch),
    Done { total: usize, truncated: bool },
}

pub struct ProjectSearchHandle {
    pub generation: u64,
    pub rx: Receiver<ProjectSearchMessage>,
    pub cancel: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
}

impl ProjectSearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

pub fn start_search(
    root: PathBuf,
    files: Vec<String>,
    pattern: String,
    options: SearchOptions,
    generation: u64,
    max_file_bytes: u64,
) -> ProjectSearchHandle {
    let (tx, rx) = mpsc::sync_channel::<ProjectSearchMessage>(256);
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);
    let done_thread = Arc::clone(&done);
    thread::Builder::new()
        .name("raster-project-search".into())
        .spawn(move || {
            let mut total = 0usize;
            let mut truncated = false;
            for (index, relative) in files.iter().enumerate() {
                if cancel_thread.load(Ordering::Relaxed) {
                    break;
                }
                if index % 16 == 0 && cancel_thread.load(Ordering::Relaxed) {
                    break;
                }
                let path = root.join(relative);
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if !meta.is_file() || meta.len() > max_file_bytes {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes);
                let Ok(matches) =
                    find_matches_in_text(&text, &pattern, options, Some(&cancel_thread))
                else {
                    continue;
                };
                for found in matches {
                    if cancel_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    total += 1;
                    let message = ProjectSearchMessage::Match(ProjectMatch {
                        path: relative.clone(),
                        line: found.line.get() + 1,
                        column: found.column + 1,
                        preview: found.preview,
                    });
                    match tx.try_send(message) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            truncated = true;
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            cancel_thread.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                    if total >= options.max_results {
                        truncated = true;
                        break;
                    }
                }
                if total >= options.max_results {
                    break;
                }
            }
            for _ in 0..20 {
                match tx.try_send(ProjectSearchMessage::Done { total, truncated }) {
                    Ok(()) => break,
                    Err(TrySendError::Full(_)) => {
                        thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
            done_thread.store(true, Ordering::Relaxed);
        })
        .ok();
    ProjectSearchHandle {
        generation,
        rx,
        cancel,
        done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::unique_suffix;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raster-project-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn ignore_rules_respect_negation_hidden_and_ignored_controls() {
        let dir = TestDir::new();
        dir.write(".gitignore", "target/\n*.tmp\n!keep.tmp\n");
        dir.write("target/out.rs", "x");
        dir.write("skip.tmp", "x");
        dir.write("keep.tmp", "x");
        dir.write(".hidden.rs", "x");
        dir.write("src/main.rs", "x");
        let rules = IgnoreRules::load(&dir.0);
        assert!(!rules.should_include(Path::new("target"), true, false, false));
        assert!(!rules.should_include(Path::new("target/out.rs"), false, false, false));
        assert!(!rules.should_include(Path::new("skip.tmp"), false, false, false));
        assert!(rules.should_include(Path::new("keep.tmp"), false, false, false));
        assert!(!rules.should_include(Path::new(".hidden.rs"), false, false, false));
        assert!(rules.should_include(Path::new(".hidden.rs"), false, true, false));
        assert!(rules.should_include(Path::new("skip.tmp"), false, false, true));
        let files = scan_project(&dir.0, false, false, None);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"keep.tmp".to_string()));
        assert!(!files.contains(&"skip.tmp".to_string()));
        assert!(!files.iter().any(|f| f.starts_with("target/")));
    }

    #[test]
    fn project_search_finds_matches_and_reports_done() {
        let dir = TestDir::new();
        dir.write("a.rs", "needle\nnope\n");
        dir.write("b.rs", "nothing\nneedle here\n");
        let files = scan_project(&dir.0, false, false, None);
        let handle = start_search(
            dir.0.clone(),
            files,
            "needle".into(),
            SearchOptions {
                max_results: 50,
                ..SearchOptions::default()
            },
            1,
            1024 * 1024,
        );
        let mut matches = Vec::new();
        let mut done = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline && !done {
            match handle
                .rx
                .recv_timeout(std::time::Duration::from_millis(100))
            {
                Ok(ProjectSearchMessage::Match(m)) => matches.push(m),
                Ok(ProjectSearchMessage::Done { .. }) => done = true,
                Err(_) => {}
            }
        }
        assert!(done);
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|m| m.path == "a.rs"));
        assert!(matches.iter().any(|m| m.path == "b.rs"));
    }

    #[test]
    fn cancellation_stops_scan() {
        let dir = TestDir::new();
        for i in 0..100 {
            dir.write(&format!("f{i}.rs"), "x\n");
        }
        let cancel = AtomicBool::new(true);
        assert!(scan_project(&dir.0, false, false, Some(&cancel)).is_empty());
    }
}
