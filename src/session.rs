//! Session restoration and crash recovery.
//!
//! Recovery files are local, permission-restricted, and contain no log data.
//! A metadata header is followed by the exact UTF-8 document bytes. Snapshots
//! are coalesced on a background thread: only the newest dirty version of a
//! buffer is written, and the interactive input path never serializes a file.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ropey::Rope;

use crate::core::document::Document;
use crate::core::selection::Selection;
use crate::json::Json;
use crate::position::BytePos;
use crate::util::{enforce_private_dir, now_millis, set_private_file, unique_suffix};

pub type BufferId = u64;

#[derive(Clone, Debug)]
pub struct RecoveryJob {
    pub buffer_id: BufferId,
    pub path: Option<PathBuf>,
    pub label: String,
    pub rope: Rope,
    pub version: u64,
    pub cursors: Vec<Selection>,
    pub timestamp_ms: u128,
}

#[derive(Clone, Debug)]
pub struct RecoveryRecord {
    pub file: PathBuf,
    pub buffer_id: BufferId,
    pub path: Option<PathBuf>,
    pub label: String,
    pub document: Document,
    pub version: u64,
    pub cursors: Vec<Selection>,
    pub timestamp_ms: u128,
    pub session_id: String,
}

struct Shared {
    pending: HashMap<BufferId, RecoveryJob>,
    stop: bool,
}

pub struct RecoveryManager {
    dir: PathBuf,
    session_id: String,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl RecoveryManager {
    pub fn start(dir: PathBuf) -> std::io::Result<Self> {
        enforce_private_dir(&dir)?;
        let shared = Arc::new((
            Mutex::new(Shared {
                pending: HashMap::new(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let session_id = unique_suffix();
        let thread_shared = Arc::clone(&shared);
        let thread_dir = dir.clone();
        let thread_session = session_id.clone();
        let thread = std::thread::Builder::new()
            .name("raster-recovery".into())
            .spawn(move || {
                let (lock, cond) = &*thread_shared;
                loop {
                    let jobs: Vec<RecoveryJob> = {
                        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while state.pending.is_empty() && !state.stop {
                            state = cond.wait(state).unwrap_or_else(|e| e.into_inner());
                        }
                        if state.stop && state.pending.is_empty() {
                            break;
                        }
                        std::mem::take(&mut state.pending).into_values().collect()
                    };
                    for job in jobs {
                        let _ = write_recovery_file(&thread_dir, &thread_session, &job);
                    }
                }
            })?;
        Ok(Self {
            dir,
            session_id,
            shared,
            thread: Some(thread),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Coalesces obsolete snapshots: a newer job replaces the pending one.
    pub fn submit(&self, mut job: RecoveryJob) {
        job.timestamp_ms = now_millis();
        if let Ok(mut state) = self.shared.0.lock() {
            state.pending.insert(job.buffer_id, job);
        }
        self.shared.1.notify_all();
    }

    pub fn remove(&self, buffer_id: BufferId) {
        if let Ok(mut state) = self.shared.0.lock() {
            state.pending.remove(&buffer_id);
        }
        let suffix = format!("-{buffer_id}.rasterrec");
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("recovery-") && name.ends_with(&suffix) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    pub fn recover_all(&self) -> Vec<RecoveryRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("recovery-") || !name.ends_with(".rasterrec") {
                continue;
            }
            if let Ok(record) = read_recovery_file(&entry.path()) {
                out.push(record);
            }
        }
        out.sort_by_key(|r| r.timestamp_ms);
        out
    }

    pub fn cleanup(&self, retention: Duration) -> usize {
        let cutoff = now_millis().saturating_sub(retention.as_millis());
        let mut removed = 0;
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("recovery-") && !name.starts_with(".raster-recovery.tmp-") {
                    continue;
                }
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() < cutoff)
                    .unwrap_or(false);
                if old && fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
}

impl Drop for RecoveryManager {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.0.lock() {
            state.stop = true;
        }
        self.shared.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn recovery_path(dir: &Path, session_id: &str, buffer_id: BufferId) -> PathBuf {
    dir.join(format!("recovery-{session_id}-{buffer_id}.rasterrec"))
}

fn write_recovery_file(dir: &Path, session_id: &str, job: &RecoveryJob) -> std::io::Result<()> {
    enforce_private_dir(dir)?;
    let final_path = recovery_path(dir, session_id, job.buffer_id);
    let temp = dir.join(format!(".raster-recovery.tmp-{}", unique_suffix()));
    let mut meta = Json::object();
    meta.insert("schema", Json::Int(1));
    meta.insert("session_id", Json::string(session_id));
    meta.insert("buffer_id", Json::Int(job.buffer_id as i64));
    meta.insert("version", Json::Int(job.version as i64));
    meta.insert("timestamp_ms", Json::Int(job.timestamp_ms as i64));
    meta.insert("label", Json::string(&job.label));
    if let Some(path) = &job.path {
        meta.insert("path", Json::string(path.to_string_lossy()));
    }
    let mut cursors = Json::array();
    for selection in &job.cursors {
        let mut pair = Json::array();
        pair.push(Json::Int(selection.anchor.get() as i64));
        pair.push(Json::Int(selection.head.get() as i64));
        cursors.push(pair);
    }
    meta.insert("cursors", cursors);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    set_private_file(&temp)?;
    writeln!(file, "{}", meta.to_compact_string())?;
    for chunk in job.rope.chunks() {
        file.write_all(chunk.as_bytes())?;
    }
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, &final_path)?;
    Ok(())
}

fn read_recovery_file(path: &Path) -> Result<RecoveryRecord, RecoveryError> {
    let file = File::open(path).map_err(|e| RecoveryError(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .map_err(|e| RecoveryError(e.to_string()))?;
    let meta = Json::parse(header.trim_end_matches(['\n', '\r']))
        .map_err(|e| RecoveryError(e.to_string()))?;
    let mut content = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut content)
        .map_err(|e| RecoveryError(e.to_string()))?;
    let document = Document::from_bytes(&content).map_err(|e| RecoveryError(e.to_string()))?;
    let cursors = meta
        .get("cursors")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|pair| {
            let values = pair.as_array()?;
            Some(Selection::new(
                BytePos(values.first()?.as_u64()? as usize),
                BytePos(values.get(1)?.as_u64()? as usize),
            ))
        })
        .collect();
    let session_id = meta
        .get("session_id")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let buffer_id = meta.get("buffer_id").and_then(Json::as_u64).unwrap_or(0);
    let version = meta.get("version").and_then(Json::as_u64).unwrap_or(0);
    let timestamp_ms = meta.get("timestamp_ms").and_then(Json::as_u64).unwrap_or(0) as u128;
    let label = meta
        .get("label")
        .and_then(Json::as_str)
        .unwrap_or("recovered")
        .to_string();
    let file_path = meta.get("path").and_then(Json::as_str).map(PathBuf::from);
    Ok(RecoveryRecord {
        file: path.to_path_buf(),
        buffer_id,
        path: file_path,
        label,
        document,
        version,
        cursors,
        timestamp_ms,
        session_id,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryError(pub String);
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for RecoveryError {}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionFile {
    pub path: Option<PathBuf>,
    pub cursors: Vec<Selection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionState {
    pub cwd: PathBuf,
    pub files: Vec<SessionFile>,
    pub active: usize,
    /// Opaque layout JSON owned by the application layer.
    pub layout: Json,
}

impl SessionState {
    pub fn empty(cwd: PathBuf) -> Self {
        Self {
            cwd,
            files: Vec::new(),
            active: 0,
            layout: Json::object(),
        }
    }

    fn to_json(&self) -> Json {
        let mut root = Json::object();
        root.insert("schema", Json::Int(1));
        root.insert("cwd", Json::string(self.cwd.to_string_lossy()));
        root.insert("active", Json::Int(self.active as i64));
        root.insert("layout", self.layout.clone());
        let mut files = Json::array();
        for file in &self.files {
            let mut value = Json::object();
            if let Some(path) = &file.path {
                value.insert("path", Json::string(path.to_string_lossy()));
            }
            let mut cursors = Json::array();
            for selection in &file.cursors {
                let mut pair = Json::array();
                pair.push(Json::Int(selection.anchor.get() as i64));
                pair.push(Json::Int(selection.head.get() as i64));
                cursors.push(pair);
            }
            value.insert("cursors", cursors);
            files.push(value);
        }
        root.insert("files", files);
        root
    }

    fn from_json(value: &Json) -> Option<Self> {
        let cwd = PathBuf::from(value.get("cwd")?.as_str()?);
        let active = value.get("active")?.as_u64()? as usize;
        let layout = value.get("layout").cloned().unwrap_or_else(Json::object);
        let mut files = Vec::new();
        for entry in value.get("files")?.as_array()? {
            let path = entry.get("path").and_then(Json::as_str).map(PathBuf::from);
            let cursors = entry
                .get("cursors")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|pair| {
                    let values = pair.as_array()?;
                    Some(Selection::new(
                        BytePos(values.first()?.as_u64()? as usize),
                        BytePos(values.get(1)?.as_u64()? as usize),
                    ))
                })
                .collect();
            files.push(SessionFile { path, cursors });
        }
        Some(Self {
            cwd,
            files,
            active,
            layout,
        })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            enforce_private_dir(parent)?;
        }
        let temp = path.with_file_name(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            unique_suffix()
        ));
        fs::write(&temp, self.to_json().to_compact_string())?;
        set_private_file(&temp)?;
        fs::rename(temp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let value = Json::parse(&text).ok()?;
        Self::from_json(&value)
    }
}

pub fn now_system_time() -> SystemTime {
    SystemTime::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raster-session-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_file(dir: &Path) -> bool {
        for _ in 0..100 {
            if fs::read_dir(dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().ends_with(".rasterrec"))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn recovery_round_trips_exact_bytes_and_is_private() {
        let dir = TestDir::new();
        let manager = RecoveryManager::start(dir.0.join("state")).unwrap();
        let content = "a\r\nمرحبا 😀\nno final newline";
        let doc = Document::from_text(content);
        let lost_path = std::env::temp_dir().join("lost.rs");
        manager.submit(RecoveryJob {
            buffer_id: 7,
            path: Some(lost_path.clone()),
            label: "lost.rs".into(),
            rope: doc.rope(),
            version: 3,
            cursors: vec![Selection::caret(BytePos(5))],
            timestamp_ms: 0,
        });
        assert!(wait_for_file(manager.dir()));
        let records = manager.recover_all();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].document.text(), content);
        assert_eq!(records[0].cursors, vec![Selection::caret(BytePos(5))]);
        assert_eq!(records[0].path, Some(lost_path));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = fs::metadata(manager.dir()).unwrap().permissions().mode() & 0o777;
            let file_mode = fs::metadata(&records[0].file).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
        #[cfg(windows)]
        {
            // std::fs::Permissions exposes only readonly() on Windows. ACL
            // equivalence to Unix 0700/0600 is not asserted by this test.
            assert!(fs::metadata(manager.dir()).unwrap().is_dir());
            let metadata = fs::metadata(&records[0].file).unwrap();
            assert!(metadata.is_file());
            assert!(!metadata.permissions().readonly());
        }
        manager.remove(7);
        assert!(manager.recover_all().is_empty());
    }

    #[test]
    fn newest_snapshot_replaces_stale_pending_job() {
        let dir = TestDir::new();
        let manager = RecoveryManager::start(dir.0.join("state")).unwrap();
        for text in ["old", "new"] {
            manager.submit(RecoveryJob {
                buffer_id: 1,
                path: None,
                label: "scratch".into(),
                rope: Document::from_text(text).rope(),
                version: 1,
                cursors: vec![],
                timestamp_ms: 0,
            });
        }
        // Joining the worker on drop drains pending snapshots, regardless of
        // whether it started writing the old snapshot before the new submit.
        let path = recovery_path(manager.dir(), manager.session_id(), 1);
        drop(manager);
        let record = read_recovery_file(&path).unwrap();
        assert_eq!(record.document.text(), "new");
    }

    #[test]
    fn session_state_round_trips_without_document_contents() {
        let dir = TestDir::new();
        let mut session = SessionState::empty(dir.0.clone());
        session.files.push(SessionFile {
            path: Some(dir.0.join("a.rs")),
            cursors: vec![Selection::caret(BytePos(4))],
        });
        session.active = 0;
        session.layout = Json::string("split");
        let path = dir.0.join("session.json");
        session.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("document content"));
        assert_eq!(SessionState::load(&path).unwrap(), session);
    }
}
