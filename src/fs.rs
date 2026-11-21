//! Filesystem loading and safe-save behavior.
//!
//! Regular files use a same-directory temporary file, flush/fsync, directory
//! sync where supported, and rename. Symlinks are preserved by writing through
//! their resolved target. Hard links use a backup plus in-place write so all
//! links observe the same inode; that path is explicitly not atomic. A small
//! journal containing no document content allows interrupted saves to be found.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::NewlineMode;
use crate::core::document::{Document, DocumentError};
use crate::json::Json;
use crate::util::{ensure_dir, fnv1a, now_millis, set_private_file, unique_suffix};

#[derive(Clone, Debug)]
pub struct DiskState {
    /// The path shown to the user; may be a symlink.
    pub path: Option<PathBuf>,
    /// The regular file actually read/written.
    pub resolved_path: Option<PathBuf>,
    pub original_hash: u64,
    /// Whether the path existed when the buffer was opened.
    pub existed: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub identity: (u64, u64),
    pub permissions: Option<fs::Permissions>,
    pub read_only: bool,
    pub is_symlink: bool,
    pub hard_link: bool,
    /// Document version when these bytes were read from disk.
    pub version_read: u64,
    pub external_change: bool,
}

impl DiskState {
    pub fn scratch(version: u64) -> Self {
        Self {
            path: None,
            resolved_path: None,
            original_hash: 0,
            existed: false,
            len: 0,
            modified: None,
            identity: (0, 0),
            permissions: None,
            read_only: false,
            is_symlink: false,
            hard_link: false,
            version_read: version,
            external_change: false,
        }
    }

    pub fn for_new_file(path: PathBuf, version: u64) -> Self {
        Self {
            path: Some(path.clone()),
            resolved_path: Some(path),
            original_hash: fnv1a(b""),
            existed: false,
            len: 0,
            modified: None,
            identity: (0, 0),
            permissions: None,
            read_only: false,
            is_symlink: false,
            hard_link: false,
            version_read: version,
            external_change: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalState {
    Unchanged,
    Modified,
    Missing,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    ReadOnly(String),
    Conflict(String),
    Permission(String),
    StorageFull(String),
    Io(String),
    Document(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadOnly(s)
            | Self::Conflict(s)
            | Self::Permission(s)
            | Self::StorageFull(s)
            | Self::Io(s)
            | Self::Document(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for SaveError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Document(DocumentError),
    Io(String),
}
impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Document(e) => write!(f, "{e}"),
            Self::Io(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for LoadError {}

#[derive(Copy, Clone, Debug, Default)]
pub struct SaveOptions {
    pub newline: NewlineMode,
    /// Test-only simulated ENOSPC after N bytes; `None` in normal use.
    pub fail_after_bytes: Option<usize>,
    /// Explicit `:overwrite` after an external-change conflict.
    pub allow_overwrite: bool,
}

#[derive(Clone, Debug)]
pub struct InterruptedSave {
    pub journal: PathBuf,
    pub target: PathBuf,
    pub temp: PathBuf,
    pub backup: Option<PathBuf>,
    pub timestamp_ms: u128,
    pub pid: u32,
}

pub fn load_file(path: &Path, force_read_only: bool) -> Result<(Document, DiskState), LoadError> {
    let link_meta = fs::symlink_metadata(path)
        .map_err(|e| LoadError::Io(format!("{}: {e}", path.display())))?;
    if link_meta.is_dir() {
        return Err(LoadError::Io(format!("{} is a directory", path.display())));
    }
    let is_symlink = link_meta.file_type().is_symlink();
    let resolved = if is_symlink {
        fs::canonicalize(path)
            .map_err(|e| LoadError::Io(format!("cannot resolve symlink {}: {e}", path.display())))?
    } else {
        path.to_path_buf()
    };
    let meta = fs::metadata(&resolved)
        .map_err(|e| LoadError::Io(format!("{}: {e}", resolved.display())))?;
    if !meta.is_file() {
        return Err(LoadError::Io(format!(
            "{} is not a regular file",
            resolved.display()
        )));
    }
    let bytes =
        fs::read(&resolved).map_err(|e| LoadError::Io(format!("{}: {e}", resolved.display())))?;
    let doc = Document::from_bytes(&bytes).map_err(LoadError::Document)?;
    let state = DiskState {
        path: Some(path.to_path_buf()),
        resolved_path: Some(resolved),
        original_hash: fnv1a(&bytes),
        existed: true,
        len: bytes.len() as u64,
        modified: meta.modified().ok(),
        identity: file_identity(&meta),
        permissions: Some(meta.permissions()),
        read_only: force_read_only || meta.permissions().readonly(),
        is_symlink,
        hard_link: hard_link_count(&meta) > 1,
        version_read: doc.version(),
        external_change: false,
    };
    Ok((doc, state))
}

pub fn external_state(disk: &DiskState) -> ExternalState {
    let Some(path) = disk.resolved_path.as_ref() else {
        return ExternalState::Unchanged;
    };
    if !disk.existed {
        return match fs::symlink_metadata(path) {
            Ok(_) => ExternalState::Modified,
            Err(e) if e.kind() == io::ErrorKind::NotFound => ExternalState::Unchanged,
            Err(e) => ExternalState::Error(e.to_string()),
        };
    }
    // Metadata can be deliberately or accidentally preserved across a write.
    // Always hash actual contents rather than trusting timestamps and length.
    match fs::read(path) {
        Ok(bytes) => {
            if fnv1a(&bytes) != disk.original_hash {
                ExternalState::Modified
            } else {
                ExternalState::Unchanged
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => ExternalState::Missing,
        Err(e) => ExternalState::Error(e.to_string()),
    }
}

pub fn save_document(
    doc: &Document,
    disk: &mut DiskState,
    options: SaveOptions,
) -> Result<(), SaveError> {
    let target = disk
        .resolved_path
        .clone()
        .ok_or_else(|| SaveError::Io("buffer has no path; use Save As".into()))?;
    if disk.read_only {
        return Err(SaveError::ReadOnly(
            "buffer is read-only; use Save As or toggle read-only explicitly".into(),
        ));
    }
    if !options.allow_overwrite {
        match external_state(disk) {
            ExternalState::Unchanged => {}
            ExternalState::Modified => {
                return Err(SaveError::Conflict(
                    "file changed on disk; use :reload or :overwrite".into(),
                ))
            }
            ExternalState::Missing => {
                return Err(SaveError::Conflict(
                    "file disappeared from disk; use :save-as or :overwrite".into(),
                ))
            }
            ExternalState::Error(e) => return Err(SaveError::Io(e)),
        }
    }
    let hash = if disk.hard_link || disk.is_symlink_target_hard_link() {
        write_in_place(&target, doc, disk, options)?
    } else {
        atomic_replace(&target, doc, disk, options)?
    };
    refresh_disk_state(disk, doc, hash)?;
    Ok(())
}

pub fn save_as(doc: &Document, path: &Path, options: SaveOptions) -> Result<DiskState, SaveError> {
    let mut disk = match fs::symlink_metadata(path) {
        Ok(_) if options.allow_overwrite => {
            load_file(path, false)
                .map_err(|e| SaveError::Io(format!("cannot inspect destination: {e}")))?
                .1
        }
        Ok(_) => return Err(SaveError::Conflict("destination already exists".into())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            DiskState::for_new_file(path.to_path_buf(), doc.version())
        }
        Err(e) => return Err(io_error(e)),
    };
    save_document(doc, &mut disk, options)?;
    Ok(disk)
}

impl DiskState {
    fn is_symlink_target_hard_link(&self) -> bool {
        if !self.is_symlink {
            return false;
        }
        self.resolved_path
            .as_ref()
            .and_then(|p| fs::metadata(p).ok())
            .map(|m| hard_link_count(&m) > 1)
            .unwrap_or(false)
    }
}

fn refresh_disk_state(disk: &mut DiskState, doc: &Document, hash: u64) -> Result<(), SaveError> {
    let path = disk
        .resolved_path
        .as_ref()
        .ok_or_else(|| SaveError::Io("missing resolved path".into()))?;
    let meta = fs::metadata(path).map_err(io_error)?;
    disk.original_hash = hash;
    disk.existed = true;
    disk.len = meta.len();
    disk.modified = meta.modified().ok();
    disk.identity = file_identity(&meta);
    disk.permissions = Some(meta.permissions());
    disk.version_read = doc.version();
    disk.external_change = false;
    Ok(())
}

fn atomic_replace(
    target: &Path,
    doc: &Document,
    disk: &DiskState,
    options: SaveOptions,
) -> Result<u64, SaveError> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_dir(parent).map_err(io_error)?;
    let suffix = unique_suffix();
    let temp = parent.join(format!(".raster.tmp-{suffix}"));
    let journal_path = parent.join(format!(".raster-save-{suffix}.json"));
    #[cfg(windows)]
    let backup = parent.join(format!(".raster.bak-{suffix}"));
    let journal = SaveJournal {
        target: target.to_path_buf(),
        temp: temp.clone(),
        #[cfg(windows)]
        backup: Some(backup.clone()),
        #[cfg(not(windows))]
        backup: None,
        timestamp_ms: now_millis(),
        pid: std::process::id(),
        original_hash: disk.original_hash,
        version_read: disk.version_read,
    };
    write_journal(&journal_path, &journal)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(io_error)?;
    if let Some(perms) = &disk.permissions {
        let _ = file.set_permissions(perms.clone());
    } else {
        let _ = set_private_file(&temp);
    }
    // A failed write deliberately leaves the journal and temp file in place
    // so `:recover-save` can offer the interrupted content to the user.
    let hash = write_document(&mut file, doc, options)?;
    file.flush().map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    sync_directory(parent)?;
    if !options.allow_overwrite && external_state(disk) != ExternalState::Unchanged {
        return Err(SaveError::Conflict(
            "destination changed while saving".into(),
        ));
    }
    // A hard link creates a new destination atomically without replacing a
    // file another process created after the preceding check.
    if !disk.existed && !options.allow_overwrite {
        fs::hard_link(&temp, target).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                SaveError::Conflict("destination appeared while saving".into())
            } else {
                io_error(e)
            }
        })?;
        sync_directory(parent)?;
        fs::remove_file(&temp).map_err(io_error)?;
        fs::remove_file(&journal_path).map_err(io_error)?;
        sync_directory(parent)?;
        return Ok(hash);
    }

    #[cfg(unix)]
    {
        if let Err(e) = fs::rename(&temp, target) {
            return Err(io_error(e));
        }
    }
    #[cfg(windows)]
    {
        // Windows rename does not replace an existing file. Keep a backup and
        // restore it if the second rename fails.
        if target.exists() {
            fs::rename(target, &backup).map_err(io_error)?;
            if let Err(e) = fs::rename(&temp, target) {
                if let Err(restore) = fs::rename(&backup, target) {
                    return Err(SaveError::Io(format!(
                        "replacement failed: {e}; backup restore failed: {restore}"
                    )));
                }
                return Err(io_error(e));
            }
            let _ = fs::remove_file(&backup);
        } else if let Err(e) = fs::rename(&temp, target) {
            return Err(io_error(e));
        }
    }
    sync_directory(parent)?;
    fs::remove_file(&journal_path).map_err(io_error)?;
    sync_directory(parent)?;
    Ok(hash)
}

fn write_in_place(
    target: &Path,
    doc: &Document,
    disk: &DiskState,
    options: SaveOptions,
) -> Result<u64, SaveError> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_dir(parent).map_err(io_error)?;
    let suffix = unique_suffix();
    let temp = parent.join(format!(".raster.tmp-{suffix}"));
    let backup = parent.join(format!(".raster.bak-{suffix}"));
    let journal_path = parent.join(format!(".raster-save-{suffix}.json"));
    fs::copy(target, &backup).map_err(io_error)?;
    let _ = set_private_file(&backup);
    let journal = SaveJournal {
        target: target.to_path_buf(),
        temp: temp.clone(),
        backup: Some(backup.clone()),
        timestamp_ms: now_millis(),
        pid: std::process::id(),
        original_hash: disk.original_hash,
        version_read: disk.version_read,
    };
    write_journal(&journal_path, &journal)?;
    let mut temp_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(io_error)?;
    let hash = write_document(&mut temp_file, doc, options)?;
    temp_file.flush().map_err(io_error)?;
    temp_file.sync_all().map_err(io_error)?;
    drop(temp_file);

    if !options.allow_overwrite && external_state(disk) != ExternalState::Unchanged {
        return Err(SaveError::Conflict(
            "destination changed while saving".into(),
        ));
    }
    // Write through the existing inode without copying the temporary file's
    // private permissions onto it. Restore contents the same way on failure.
    if let Err(e) = overwrite_contents(&temp, target) {
        if let Err(restore) = overwrite_contents(&backup, target) {
            return Err(SaveError::Io(format!(
                "save failed: {e}; backup restore failed: {restore}"
            )));
        }
        return Err(io_error(e));
    }
    sync_directory(parent)?;
    let _ = fs::remove_file(&temp);
    let _ = fs::remove_file(&backup);
    let _ = fs::remove_file(&journal_path);
    Ok(hash)
}

fn overwrite_contents(source: &Path, target: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).truncate(true).open(target)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.sync_all()
}

struct CountingWriter<'a, W: Write> {
    inner: &'a mut W,
    hash: u64,
    written: usize,
    limit: Option<usize>,
}

impl<'a, W: Write> Write for CountingWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(limit) = self.limit {
            if self.written >= limit {
                return Err(io::Error::other("simulated full disk"));
            }
            let allowed = (limit - self.written).min(buf.len());
            let n = self.inner.write(&buf[..allowed])?;
            for &b in &buf[..n] {
                self.hash ^= b as u64;
                self.hash = self.hash.wrapping_mul(0x100000001b3);
            }
            self.written += n;
            if self.written >= limit {
                return Err(io::Error::other("simulated full disk"));
            }
            return Ok(n);
        }
        let n = self.inner.write(buf)?;
        for &b in &buf[..n] {
            self.hash ^= b as u64;
            self.hash = self.hash.wrapping_mul(0x100000001b3);
        }
        self.written += n;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_document(file: &mut File, doc: &Document, options: SaveOptions) -> Result<u64, SaveError> {
    let mut writer = CountingWriter {
        inner: file,
        hash: 0xcbf29ce484222325,
        written: 0,
        limit: options.fail_after_bytes,
    };
    doc.write_to(&mut writer, options.newline)
        .map_err(io_error)?;
    writer.flush().map_err(io_error)?;
    Ok(writer.hash)
}

fn io_error(e: io::Error) -> SaveError {
    if e.kind() == io::ErrorKind::PermissionDenied {
        return SaveError::Permission(format!("permission denied: {e}"));
    }
    if e.raw_os_error() == Some(28) || e.to_string().contains("simulated full disk") {
        return SaveError::StorageFull(format!("no space left on device: {e}"));
    }
    SaveError::Io(e.to_string())
}

fn sync_directory(parent: &Path) -> Result<(), SaveError> {
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(io_error)
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

fn file_identity(meta: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (0, 0)
    }
}

fn hard_link_count(meta: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        1
    }
}

#[derive(Clone, Debug)]
struct SaveJournal {
    target: PathBuf,
    temp: PathBuf,
    backup: Option<PathBuf>,
    timestamp_ms: u128,
    pid: u32,
    original_hash: u64,
    version_read: u64,
}

impl SaveJournal {
    fn to_json(&self) -> Json {
        let mut value = Json::object();
        value.insert("target", Json::string(self.target.to_string_lossy()));
        value.insert("temp", Json::string(self.temp.to_string_lossy()));
        if let Some(backup) = &self.backup {
            value.insert("backup", Json::string(backup.to_string_lossy()));
        }
        value.insert("timestamp_ms", Json::Int(self.timestamp_ms as i64));
        value.insert("pid", Json::Int(self.pid as i64));
        value.insert(
            "original_hash",
            Json::string(self.original_hash.to_string()),
        );
        value.insert("version_read", Json::Int(self.version_read as i64));
        value
    }

    fn from_json(value: &Json) -> Option<Self> {
        Some(Self {
            target: PathBuf::from(value.get("target")?.as_str()?),
            temp: PathBuf::from(value.get("temp")?.as_str()?),
            backup: value
                .get("backup")
                .and_then(Json::as_str)
                .map(PathBuf::from),
            timestamp_ms: value.get("timestamp_ms")?.as_u64()? as u128,
            pid: value.get("pid")?.as_u64()? as u32,
            original_hash: value.get("original_hash")?.as_str()?.parse().ok()?,
            version_read: value.get("version_read")?.as_u64()?,
        })
    }
}

fn write_journal(path: &Path, journal: &SaveJournal) -> Result<(), SaveError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    set_private_file(path).map_err(io_error)?;
    file.write_all(journal.to_json().to_compact_string().as_bytes())
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    Ok(())
}

pub fn interrupted_saves(dir: &Path) -> Vec<InterruptedSave> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(".raster-save-") || !name.ends_with(".json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = Json::parse(&text) else {
            continue;
        };
        let Some(journal) = SaveJournal::from_json(&value) else {
            continue;
        };
        if !journal.temp.exists() {
            continue;
        }
        out.push(InterruptedSave {
            journal: entry.path(),
            target: journal.target,
            temp: journal.temp,
            backup: journal.backup,
            timestamp_ms: journal.timestamp_ms,
            pid: journal.pid,
        });
    }
    out.sort_by_key(|s| s.timestamp_ms);
    out
}

pub fn read_interrupted_save(save: &InterruptedSave) -> Result<(Document, Vec<u8>), LoadError> {
    let bytes = fs::read(&save.temp).map_err(|e| LoadError::Io(e.to_string()))?;
    let doc = Document::from_bytes(&bytes).map_err(LoadError::Document)?;
    Ok((doc, bytes))
}

pub fn cleanup_save_journals(dir: &Path, max_age: Duration) -> usize {
    let cutoff = now_millis().saturating_sub(max_age.as_millis());
    let mut removed = 0usize;
    for save in interrupted_saves(dir) {
        if save.timestamp_ms >= cutoff {
            continue;
        }
        for path in [Some(save.journal), Some(save.temp), save.backup]
            .into_iter()
            .flatten()
        {
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

pub fn system_time_millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transaction::{Replacement, Transaction};
    use crate::position::BytePos;
    use crate::util::unique_suffix;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raster-fs-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn new_file_first_save_and_existing_destination_conflict() {
        let dir = TestDir::new();
        let path = dir.0.join("new.txt");
        let doc = Document::from_text("local");
        let mut disk = DiskState::for_new_file(path.clone(), doc.version());
        save_document(&doc, &mut disk, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"local");
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            save_document(&doc, &mut disk, SaveOptions::default()),
            Err(SaveError::Conflict(_))
        ));
        let other = dir.0.join("appeared.txt");
        let mut new_disk = DiskState::for_new_file(other.clone(), doc.version());
        fs::write(&other, "external").unwrap();
        assert!(matches!(
            save_document(&doc, &mut new_disk, SaveOptions::default()),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&other).unwrap(), b"external");
    }

    #[test]
    fn changed_content_is_detected_even_if_metadata_is_unchanged() {
        let dir = TestDir::new();
        let path = dir.write("same.txt", "original");
        let (doc, mut disk) = load_file(&path, false).unwrap();
        fs::write(&path, "external").unwrap();
        disk.modified = fs::metadata(&path).unwrap().modified().ok();
        assert!(matches!(
            save_document(&doc, &mut disk, SaveOptions::default()),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"external");
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_save_and_failed_preparation_keep_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let path = dir.write("hard.txt", "original");
        let alias = dir.0.join("alias");
        fs::hard_link(&path, &alias).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        let (_, mut disk) = load_file(&path, false).unwrap();
        let doc = Document::from_text("new contents");
        let opts = SaveOptions {
            fail_after_bytes: Some(2),
            ..SaveOptions::default()
        };
        assert!(save_document(&doc, &mut disk, opts).is_err());
        assert_eq!(
            fs::metadata(&alias).unwrap().permissions().mode() & 0o777,
            0o754
        );
        assert_eq!(fs::read(&alias).unwrap(), b"original");
        save_document(&doc, &mut disk, SaveOptions::default()).unwrap();
        assert_eq!(
            fs::metadata(&alias).unwrap().permissions().mode() & 0o777,
            0o754
        );
        assert_eq!(fs::read(&alias).unwrap(), b"new contents");
    }

    #[cfg(unix)]
    #[test]
    fn save_as_preserves_existing_destination_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let path = dir.write("target", "old");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        let opts = SaveOptions {
            allow_overwrite: true,
            ..SaveOptions::default()
        };
        save_as(&Document::from_text("new"), &path, opts).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o754
        );
    }

    #[test]
    fn save_and_reopen_preserves_unicode_newlines_and_missing_final_newline() {
        let dir = TestDir::new();
        let text = "#!/usr/bin/env python\r\nمرحبا = \"😀\"\nprint(مرحبا)\rno final newline";
        let path = dir.write("mixed.py", text);
        let (mut doc, mut disk) = load_file(&path, false).unwrap();
        Transaction::new("edit", vec![Replacement::insert(BytePos(0), "# ")], 0, &[])
            .apply(&mut doc)
            .unwrap();
        save_document(&doc, &mut disk, SaveOptions::default()).unwrap();
        let (reopened, _) = load_file(&path, false).unwrap();
        assert_eq!(reopened.text(), format!("# {text}"));
        assert_eq!(fs::read(&path).unwrap(), format!("# {text}").as_bytes());
    }

    #[test]
    fn external_change_is_not_overwritten_without_explicit_overwrite() {
        let dir = TestDir::new();
        let path = dir.write("a.txt", "original\n");
        let (mut doc, mut disk) = load_file(&path, false).unwrap();
        Transaction::new(
            "edit",
            vec![Replacement::insert(BytePos(0), "local ")],
            0,
            &[],
        )
        .apply(&mut doc)
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&path, "external change\n").unwrap();
        let err = save_document(&doc, &mut disk, SaveOptions::default()).unwrap_err();
        assert!(matches!(err, SaveError::Conflict(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external change\n");
        save_document(
            &doc,
            &mut disk,
            SaveOptions {
                allow_overwrite: true,
                ..SaveOptions::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "local original\n");
    }

    #[test]
    fn simulated_full_disk_leaves_original_and_journal() {
        let dir = TestDir::new();
        let path = dir.write("a.txt", "original content\n");
        let (mut doc, mut disk) = load_file(&path, false).unwrap();
        Transaction::new(
            "edit",
            vec![Replacement::insert(BytePos(0), "NEW ")],
            0,
            &[],
        )
        .apply(&mut doc)
        .unwrap();
        let err = save_document(
            &doc,
            &mut disk,
            SaveOptions {
                fail_after_bytes: Some(3),
                ..SaveOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, SaveError::StorageFull(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original content\n");
        let saves = interrupted_saves(&dir.0);
        assert_eq!(saves.len(), 1);
        let (recovered, _) = read_interrupted_save(&saves[0]).unwrap();
        assert!(recovered.text().starts_with("NEW"));
    }

    #[cfg(unix)]
    #[test]
    fn permission_failure_does_not_change_original() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let locked = dir.0.join("locked");
        fs::create_dir(&locked).unwrap();
        let path = locked.join("a.txt");
        fs::write(&path, "original\n").unwrap();
        let (mut doc, mut disk) = load_file(&path, false).unwrap();
        Transaction::new("edit", vec![Replacement::insert(BytePos(0), "X")], 0, &[])
            .apply(&mut doc)
            .unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let err = save_document(&doc, &mut disk, SaveOptions::default()).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(err, SaveError::Permission(_) | SaveError::Io(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_hard_links_are_preserved() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = TestDir::new();
        let target = dir.write("target.txt", "one\n");
        let link = dir.0.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (mut doc, mut disk) = load_file(&link, false).unwrap();
        assert!(disk.is_symlink);
        Transaction::new(
            "edit",
            vec![Replacement::new(BytePos(0)..BytePos(3), "two")],
            0,
            &[],
        )
        .apply(&mut doc)
        .unwrap();
        save_document(&doc, &mut disk, SaveOptions::default()).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "two\n");

        let hard = dir.write("hard.txt", "one\n");
        let hard2 = dir.0.join("hard2.txt");
        fs::hard_link(&hard, &hard2).unwrap();
        let inode_before = fs::metadata(&hard).unwrap().ino();
        let (mut doc, mut disk) = load_file(&hard, false).unwrap();
        assert!(disk.hard_link);
        Transaction::new(
            "edit",
            vec![Replacement::new(BytePos(0)..BytePos(3), "two")],
            0,
            &[],
        )
        .apply(&mut doc)
        .unwrap();
        save_document(&doc, &mut disk, SaveOptions::default()).unwrap();
        assert_eq!(fs::read_to_string(&hard2).unwrap(), "two\n");
        assert_eq!(fs::metadata(&hard).unwrap().ino(), inode_before);
        let _ = fs::set_permissions(&hard, fs::Permissions::from_mode(0o644));
    }

    #[cfg(unix)]
    #[test]
    fn read_only_buffers_refuse_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let path = dir.write("ro.txt", "x\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let (doc, mut disk) = load_file(&path, false).unwrap();
        assert!(disk.read_only);
        assert!(matches!(
            save_document(&doc, &mut disk, SaveOptions::default()),
            Err(SaveError::ReadOnly(_))
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
}
