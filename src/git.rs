//! Lightweight, non-destructive Git information: branch, changed-file status,
//! and per-file diff line indicators. All commands are read-only and bounded.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use crate::util::{short_path, wait_with_timeout};

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
    Conflicted,
    Unknown,
}

impl GitFileStatus {
    pub fn indicator(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Untracked => "?",
            Self::Renamed => "R",
            Self::Conflicted => "!",
            Self::Unknown => "~",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub branch: String,
    pub changed: Vec<(String, GitFileStatus)>,
    pub diff_lines: HashMap<String, Vec<usize>>,
    pub error: Option<String>,
}

pub struct GitStatus {
    pub root: PathBuf,
    pub snapshot: GitSnapshot,
    pub generation: u64,
    rx: Option<Receiver<(u64, GitSnapshot)>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl GitStatus {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            snapshot: GitSnapshot::default(),
            generation: 0,
            rx: None,
            cancel: None,
        }
    }

    pub fn start_refresh(&mut self) {
        self.cancel();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        let root = self.root.clone();
        let cancel_thread = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("raster-git".into())
            .spawn(move || {
                let snapshot = if cancel_thread.load(Ordering::Relaxed) {
                    GitSnapshot::default()
                } else {
                    refresh_sync(&root)
                };
                let _ = tx.send((generation, snapshot));
            })
            .ok();
        self.cancel = Some(cancel);
        self.rx = Some(rx);
    }

    pub fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut updated = false;
        if let Some(rx) = &self.rx {
            while let Ok((generation, snapshot)) = rx.try_recv() {
                if generation == self.generation {
                    self.snapshot = snapshot;
                    updated = true;
                }
            }
        }
        updated
    }

    pub fn indicator(&self, path: &Path) -> &'static str {
        let rel = short_path(path, &self.root);
        self.snapshot
            .changed
            .iter()
            .find(|(p, _)| p == &rel)
            .map(|(_, status)| status.indicator())
            .unwrap_or("")
    }

    pub fn diff_lines(&self, path: &Path) -> &[usize] {
        let rel = short_path(path, &self.root);
        self.snapshot
            .diff_lines
            .get(&rel)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

pub fn refresh_sync(root: &Path) -> GitSnapshot {
    match run_git(
        root,
        &["status", "--porcelain=v1", "-b", "--untracked-files=normal"],
    ) {
        Ok(status) => {
            let mut snapshot = parse_status(&status);
            for (path, _) in snapshot.changed.clone() {
                if let Ok(diff) = run_git(root, &["diff", "--unified=0", "--no-color", "--", &path])
                {
                    let lines = parse_diff_lines(&diff);
                    if !lines.is_empty() {
                        snapshot.diff_lines.insert(path, lines);
                    }
                }
            }
            snapshot
        }
        Err(e) => GitSnapshot {
            error: Some(e),
            ..GitSnapshot::default()
        },
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "--no-optional-locks",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("git: {e}"))?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut stream) = stdout.take() {
            let _ = stream.read_to_string(&mut out);
        }
        out
    });
    let status = wait_with_timeout(&mut child, Duration::from_secs(2))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "git timed out".to_string())?;
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("git exited with {status}"));
    }
    Ok(output)
}

fn parse_status(text: &str) -> GitSnapshot {
    let mut snapshot = GitSnapshot::default();
    for line in text.lines() {
        if let Some(branch) = line.strip_prefix("## ") {
            snapshot.branch = branch
                .split("...")
                .next()
                .unwrap_or(branch)
                .trim()
                .to_string();
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let bytes = line.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let mut path = line[3..].to_string();
        if let Some(idx) = path.find(" -> ") {
            path = path[idx + 4..].to_string();
        }
        let status = match (x, y) {
            ('?', '?') => GitFileStatus::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => GitFileStatus::Conflicted,
            ('R', _) | (_, 'R') => GitFileStatus::Renamed,
            ('A', _) | (_, 'A') => GitFileStatus::Added,
            ('D', _) | (_, 'D') => GitFileStatus::Deleted,
            ('M', _) | (_, 'M') => GitFileStatus::Modified,
            _ => GitFileStatus::Unknown,
        };
        snapshot.changed.push((path, status));
    }
    snapshot.changed.sort();
    snapshot
}

fn parse_diff_lines(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(plus) = rest.split_whitespace().find(|p| p.starts_with('+')) {
                let range = plus
                    .trim_start_matches('+')
                    .split(',')
                    .next()
                    .unwrap_or("0");
                if let Ok(start) = range.parse::<usize>() {
                    let count = plus
                        .trim_start_matches('+')
                        .split(',')
                        .nth(1)
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1);
                    for n in 0..count.max(1) {
                        out.push(start + n);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::unique_suffix;
    use std::fs;

    struct TestRepo(PathBuf);
    impl TestRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raster-git-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            let run = |args: &[&str]| {
                let status = Command::new("git")
                    .current_dir(&path)
                    .args(args)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_AUTHOR_NAME", "Test")
                    .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                    .env("GIT_COMMITTER_NAME", "Test")
                    .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                    .status()
                    .unwrap();
                assert!(status.success(), "git {args:?} failed");
            };
            run(&["init", "-q"]);
            fs::write(path.join("a.rs"), "one\ntwo\nthree\n").unwrap();
            run(&["add", "a.rs"]);
            run(&["commit", "-q", "-m", "init"]);
            fs::write(path.join("a.rs"), "one\nTWO\nthree\n").unwrap();
            fs::write(path.join("b.rs"), "new\n").unwrap();
            Self(path)
        }
    }
    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_status_and_hunk_headers() {
        assert_eq!(
            parse_status("## main...origin/main\n M a.rs\n?? b.rs\n").branch,
            "main"
        );
        assert_eq!(
            parse_status("## main\n?? b.rs\n").changed[0].1,
            GitFileStatus::Untracked
        );
        assert_eq!(parse_diff_lines("@@ -2 +2 @@\n-TWO\n+TWO\n"), vec![2]);
    }

    #[test]
    fn reads_branch_changed_files_and_diff() {
        let repo = TestRepo::new();
        let snapshot = refresh_sync(&repo.0);
        assert!(!snapshot.branch.is_empty(), "error: {:?}", snapshot.error);
        assert!(snapshot
            .changed
            .iter()
            .any(|(p, s)| p == "a.rs" && *s == GitFileStatus::Modified));
        assert!(snapshot
            .changed
            .iter()
            .any(|(p, s)| p == "b.rs" && *s == GitFileStatus::Untracked));
        assert!(snapshot
            .diff_lines
            .get("a.rs")
            .map(|v| v.contains(&2))
            .unwrap_or(false));
    }
}
