//! One shared document with its own history, disk metadata, and saved-state
//! fingerprint. Views hold cursors and scroll positions, never the document.

use std::path::{Path, PathBuf};

use crate::config::NewlineMode;
use crate::core::document::{Document, NewlineStyle};
use crate::core::history::History;
use crate::core::selection::{deduplicate_carets, normalize_selections, Selection};
use crate::core::syntax::{detect_language, Language};
use crate::core::transaction::{Replacement, Transaction, TxError};
use crate::fs::{self, DiskState, ExternalState, LoadError, SaveError, SaveOptions};
use crate::position::{BytePos, LineIndex};
use crate::session::{BufferId, RecoveryJob};
use crate::util::short_path;

#[derive(Clone, Debug)]
pub struct EditOutcome {
    pub before: Document,
    pub edits: Vec<Replacement>,
    pub before_version: u64,
    pub after_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    DirtyReload,
    Load(LoadError),
    Save(SaveError),
    Edit(TxError),
}

impl std::fmt::Display for BufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirtyReload => write!(
                f,
                "buffer has unsaved changes; use force-reload or save first"
            ),
            Self::Load(e) => write!(f, "{e}"),
            Self::Save(e) => write!(f, "{e}"),
            Self::Edit(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for BufferError {}
impl From<LoadError> for BufferError {
    fn from(e: LoadError) -> Self {
        Self::Load(e)
    }
}
impl From<SaveError> for BufferError {
    fn from(e: SaveError) -> Self {
        Self::Save(e)
    }
}
impl From<TxError> for BufferError {
    fn from(e: TxError) -> Self {
        Self::Edit(e)
    }
}

pub struct Buffer {
    pub id: BufferId,
    pub label: String,
    pub document: Document,
    pub disk: DiskState,
    pub selections: Vec<Selection>,
    pub history: History,
    dirty: bool,
    forced_dirty: bool,
    saved_history_position: u64,
    pub language: Language,
    pub large_mode: bool,
    pub external_change: bool,
}

impl Buffer {
    pub fn scratch(id: BufferId, text: &str) -> Self {
        let document = Document::from_text(text);
        Self::from_parts(
            id,
            "untitled".into(),
            document,
            DiskState::scratch(0),
            false,
        )
    }

    pub fn from_document(
        id: BufferId,
        label: String,
        document: Document,
        path: Option<PathBuf>,
    ) -> Self {
        let disk = match path {
            Some(p) => DiskState::for_new_file(p, document.version()),
            None => DiskState::scratch(document.version()),
        };
        Self::from_parts(id, label, document, disk, false)
    }

    fn from_parts(
        id: BufferId,
        label: String,
        document: Document,
        disk: DiskState,
        large_mode: bool,
    ) -> Self {
        let language = disk
            .path
            .as_deref()
            .map(detect_language)
            .unwrap_or(Language::Plain);
        let end = document.len_bytes();
        Self {
            id,
            label,
            document,
            disk,
            selections: vec![Selection::caret(BytePos(0))],
            history: History::new(1000),
            dirty: false,
            forced_dirty: false,
            saved_history_position: 0,
            language,
            large_mode,
            external_change: false,
        }
        .with_end(end)
    }

    fn with_end(mut self, end: BytePos) -> Self {
        self.selections = vec![Selection::caret(end.min(self.document.len_bytes()))];
        self
    }

    pub fn open(
        id: BufferId,
        path: &Path,
        force_read_only: bool,
        large_mode: bool,
    ) -> Result<Self, BufferError> {
        let (document, mut disk) = fs::load_file(path, force_read_only)?;
        let label = short_path(
            path,
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        );
        let end = BytePos(0);
        let language = detect_language(path);
        if large_mode {
            disk.read_only = false;
        }
        Ok(Self {
            id,
            label,
            document,
            disk,
            selections: vec![Selection::caret(end)],
            history: History::new(1000),
            dirty: false,
            forced_dirty: false,
            saved_history_position: 0,
            language,
            large_mode,
            external_change: false,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.disk.path.as_deref()
    }
    pub fn is_scratch(&self) -> bool {
        self.disk.path.is_none()
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Record the current history point as clean. Called only after a
    /// successful save/reload, never on an ordinary input or render path.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.forced_dirty = false;
        self.saved_history_position = self.history.position();
    }

    /// Recovery buffers have no saved disk state; keep them dirty until the
    /// user explicitly saves, even if an edit is undone.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.forced_dirty = true;
    }

    fn sync_dirty_from_history(&mut self) {
        self.dirty = self.forced_dirty || self.history.position() != self.saved_history_position;
    }
    pub fn read_only(&self) -> bool {
        self.disk.read_only
    }
    pub fn set_read_only(&mut self, value: bool) {
        self.disk.read_only = value;
    }
    pub fn newline_style(&self) -> NewlineStyle {
        self.document.newline_style()
    }
    pub fn version(&self) -> u64 {
        self.document.version()
    }

    pub fn primary(&self) -> Selection {
        self.selections
            .last()
            .copied()
            .unwrap_or_else(|| Selection::caret(BytePos(0)))
    }

    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        let clamped: Vec<Selection> = selections
            .into_iter()
            .map(|s| Selection::new(self.document.clamp(s.anchor), self.document.clamp(s.head)))
            .collect();
        let normalized = normalize_selections(&clamped);
        self.selections = if normalized.is_empty() {
            vec![Selection::caret(BytePos(0))]
        } else {
            normalized
        };
    }

    pub fn set_carets(&mut self, carets: Vec<BytePos>) {
        self.set_selections(deduplicate_carets(
            &carets.into_iter().map(Selection::caret).collect::<Vec<_>>(),
        ));
    }

    /// The single transaction path. Returns the ascending applied edits and a
    /// cheap pre-edit document clone for incremental LSP change coordinates.
    pub fn apply(&mut self, transaction: Transaction) -> Result<EditOutcome, BufferError> {
        let before = self.document.clone();
        let before_version = self.document.version();
        let (forward, inverse) = transaction.apply(&mut self.document)?;
        self.history.record(forward.clone(), inverse);
        self.sync_dirty_from_history();
        self.set_selections(forward.after_selections.clone());
        let edits = forward.edits.clone();
        let after_version = self.document.version();
        Ok(EditOutcome {
            before,
            edits,
            before_version,
            after_version,
        })
    }

    pub fn apply_edits(
        &mut self,
        label: &'static str,
        edits: Vec<Replacement>,
        after: Vec<Selection>,
    ) -> Result<EditOutcome, BufferError> {
        let tx = Transaction::new(label, edits, self.document.version(), &self.selections)
            .with_after_selections(after);
        self.apply(tx)
    }

    pub fn undo_outcome(&mut self) -> Result<Option<EditOutcome>, BufferError> {
        let before = self.document.clone();
        let before_version = self.document.version();
        let Some((applied, selections)) = self.history.undo_detailed(&mut self.document)? else {
            return Ok(None);
        };
        self.sync_dirty_from_history();
        self.set_selections(selections);
        Ok(Some(EditOutcome {
            before,
            edits: applied.edits,
            before_version,
            after_version: self.document.version(),
        }))
    }

    pub fn undo(&mut self) -> Result<bool, BufferError> {
        Ok(self.undo_outcome()?.is_some())
    }

    pub fn redo_outcome(&mut self) -> Result<Option<EditOutcome>, BufferError> {
        let before = self.document.clone();
        let before_version = self.document.version();
        let Some((applied, selections)) = self.history.redo_detailed(&mut self.document)? else {
            return Ok(None);
        };
        self.sync_dirty_from_history();
        self.set_selections(selections);
        Ok(Some(EditOutcome {
            before,
            edits: applied.edits,
            before_version,
            after_version: self.document.version(),
        }))
    }

    pub fn redo(&mut self) -> Result<bool, BufferError> {
        Ok(self.redo_outcome()?.is_some())
    }

    pub fn insert_text(&mut self, text: &str) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        for selection in &self.selections {
            edits.push(Replacement::new(selection.range(), text.to_string()));
        }
        let after = after_carets(&edits);
        self.apply_edits("type", edits, after)
    }

    pub fn insert_newline(&mut self, auto_indent: bool) -> Result<EditOutcome, BufferError> {
        let break_text = match self.document.newline_style() {
            NewlineStyle::Crlf => "\r\n",
            NewlineStyle::Cr => "\r",
            _ => "\n",
        };
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let prefix = if auto_indent {
                self.line_indent(selection.start())
            } else {
                String::new()
            };
            let text = format!("{break_text}{prefix}");
            ends.push(BytePos(selection.start().get() + text.len()));
            edits.push(Replacement::new(selection.range(), text));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("newline", edits, after)
    }

    pub fn backspace(&mut self) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            if selection.is_caret() {
                let start = self.document.prev_grapheme(selection.head);
                ends.push(start);
                edits.push(Replacement::delete(start..selection.head));
            } else {
                ends.push(selection.start());
                edits.push(Replacement::delete(selection.range()));
            }
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("backspace", edits, after)
    }

    pub fn delete_forward(&mut self) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let end = if selection.is_caret() {
                self.document.next_grapheme(selection.head)
            } else {
                selection.end()
            };
            let start = selection.start();
            ends.push(start);
            edits.push(Replacement::delete(start..end));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("delete", edits, after)
    }

    pub fn delete_word_left(&mut self) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let start = if selection.is_caret() {
                self.document.prev_word(selection.head)
            } else {
                selection.start()
            };
            ends.push(start);
            edits.push(Replacement::delete(start..selection.end()));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("delete-word-left", edits, after)
    }

    pub fn delete_word_right(&mut self) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let end = if selection.is_caret() {
                self.document.next_word(selection.head)
            } else {
                selection.end()
            };
            let start = selection.start();
            ends.push(start);
            edits.push(Replacement::delete(start..end));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("delete-word-right", edits, after)
    }

    pub fn delete_line(&mut self) -> Result<EditOutcome, BufferError> {
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let line = self.document.byte_to_line(selection.head);
            let start = self.document.line_to_byte(line);
            let mut end = if line.get() + 1 < self.document.len_lines() {
                self.document.line_to_byte(LineIndex(line.get() + 1))
            } else {
                self.document.len_bytes()
            };
            if end == start && line.get() > 0 {
                ends.push(
                    self.document
                        .line_content_range(LineIndex(line.get() - 1))
                        .end,
                );
                edits.push(Replacement::delete(
                    self.document.line_to_byte(LineIndex(line.get() - 1))
                        ..self.document.len_bytes(),
                ));
                continue;
            }
            if end > self.document.len_bytes() {
                end = self.document.len_bytes();
            }
            ends.push(start);
            edits.push(Replacement::delete(start..end));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("delete-line", edits, after)
    }

    pub fn duplicate_line(&mut self) -> Result<EditOutcome, BufferError> {
        let newline = match self.document.newline_style() {
            NewlineStyle::Crlf => "\r\n",
            NewlineStyle::Cr => "\r",
            _ => "\n",
        };
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for selection in &self.selections {
            let line = self.document.byte_to_line(selection.head);
            let text = self.document.line_text(line);
            let insert_at = self.document.line_content_range(line).end;
            let insert = format!("{newline}{text}");
            ends.push(BytePos(
                insert_at.get()
                    + insert.len()
                    + selection
                        .head
                        .get()
                        .saturating_sub(self.document.line_to_byte(line).get()),
            ));
            edits.push(Replacement::insert(insert_at, insert));
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("duplicate-line", edits, after)
    }

    pub fn move_lines(&mut self, down: bool) -> Result<EditOutcome, BufferError> {
        let Some(selection) = self.selections.last().copied() else {
            return self.apply_edits("move-line", Vec::new(), self.selections.clone());
        };
        let line = self.document.byte_to_line(selection.head).get();
        let other = if down {
            line + 1
        } else {
            line.saturating_sub(1)
        };
        if (!down && line == 0) || other >= self.document.len_lines() {
            return self.apply_edits("move-line", Vec::new(), self.selections.clone());
        }
        let start = self.document.line_to_byte(LineIndex(line.min(other)));
        let mid = self.document.line_to_byte(LineIndex(line.max(other)));
        let end = if line.max(other) + 1 < self.document.len_lines() {
            self.document.line_to_byte(LineIndex(line.max(other) + 1))
        } else {
            self.document.len_bytes()
        };
        let a = self.document.text_in_range(start..mid);
        let b = self.document.text_in_range(mid..end);
        let replacement = format!("{b}{a}");
        let relative =
            selection
                .head
                .get()
                .saturating_sub(if down { mid.get() } else { start.get() });
        let new_offset = if down {
            start.get() + b.len() + relative
        } else {
            start.get() + relative
        };
        self.apply_edits(
            "move-line",
            vec![Replacement::new(start..end, replacement)],
            vec![Selection::caret(BytePos(new_offset))],
        )
    }

    pub fn indent(
        &mut self,
        outdent: bool,
        tab_width: usize,
        insert_spaces: bool,
    ) -> Result<EditOutcome, BufferError> {
        let unit = if insert_spaces {
            " ".repeat(tab_width)
        } else {
            "\t".into()
        };
        let mut lines = Vec::new();
        for selection in &self.selections {
            let first = self.document.byte_to_line(selection.start()).get();
            let last = self.document.byte_to_line(selection.end()).get();
            for line in first..=last {
                lines.push(line);
            }
        }
        lines.sort();
        lines.dedup();
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for line in lines {
            let line_idx = LineIndex(line);
            let start = self.document.line_to_byte(line_idx);
            let content = self.document.line_text(line_idx);
            let prefix_len = content.len() - content.trim_start_matches([' ', '\t']).len();
            if outdent {
                if prefix_len == 0 {
                    continue;
                }
                let remove = if content[..prefix_len].ends_with(&unit) {
                    unit.len()
                } else if content[..prefix_len].ends_with('\t') {
                    1
                } else {
                    prefix_len.min(tab_width)
                };
                ends.push(BytePos(
                    start.get() + selection_offset(self.primary().head, start, remove),
                ));
                edits.push(Replacement::delete(start..BytePos(start.get() + remove)));
            } else {
                ends.push(BytePos(
                    start.get()
                        + unit.len()
                        + self.primary().head.get().saturating_sub(start.get()),
                ));
                edits.push(Replacement::insert(start, unit.clone()));
            }
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits(if outdent { "outdent" } else { "indent" }, edits, after)
    }

    pub fn toggle_comment(&mut self) -> Result<EditOutcome, BufferError> {
        let marker = match self.language {
            Language::Python | Language::Toml => "# ",
            Language::Markdown => "<!-- ",
            _ => "// ",
        };
        let mut lines = Vec::new();
        for selection in &self.selections {
            let first = self.document.byte_to_line(selection.start()).get();
            let last = self.document.byte_to_line(selection.end()).get();
            for line in first..=last {
                lines.push(line);
            }
        }
        lines.sort();
        lines.dedup();
        let all_commented = lines.iter().all(|&line| {
            self.document
                .line_text(LineIndex(line))
                .trim_start()
                .starts_with(marker.trim())
        });
        let mut edits = Vec::new();
        let mut ends = Vec::new();
        for line in lines {
            let line_idx = LineIndex(line);
            let content = self.document.line_text(line_idx);
            let start = self.document.line_to_byte(line_idx);
            let prefix_len = content.len() - content.trim_start().len();
            let marker_start = BytePos(start.get() + prefix_len);
            if all_commented {
                let trimmed = &content[prefix_len..];
                let remove = if trimmed.starts_with(marker) {
                    marker.len()
                } else if trimmed.starts_with(marker.trim()) {
                    marker.trim().len()
                } else {
                    continue;
                };
                ends.push(BytePos(
                    marker_start.get()
                        + self
                            .primary()
                            .head
                            .get()
                            .saturating_sub(marker_start.get())
                            .saturating_sub(remove),
                ));
                edits.push(Replacement::delete(
                    marker_start..BytePos(marker_start.get() + remove),
                ));
            } else {
                ends.push(BytePos(
                    marker_start.get()
                        + marker.len()
                        + self.primary().head.get().saturating_sub(marker_start.get()),
                ));
                edits.push(Replacement::insert(marker_start, marker));
            }
        }
        let after = shifted_carets(&edits, &ends);
        self.apply_edits("toggle-comment", edits, after)
    }

    pub fn selection_text(&self) -> String {
        self.selections
            .iter()
            .map(|s| self.document.text_in_range(s.range()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn all_text(&self, newline: NewlineMode) -> Vec<u8> {
        self.document.to_bytes(newline)
    }

    pub fn save(&mut self, options: SaveOptions) -> Result<(), BufferError> {
        fs::save_document(&self.document, &mut self.disk, options)?;
        self.mark_clean();
        self.external_change = false;
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path, options: SaveOptions) -> Result<(), BufferError> {
        let disk = fs::save_as(&self.document, path, options)?;
        self.disk = disk;
        self.label = short_path(
            path,
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        );
        self.language = detect_language(path);
        self.mark_clean();
        self.external_change = false;
        Ok(())
    }

    pub fn check_external(&mut self) -> ExternalState {
        let state = fs::external_state(&self.disk);
        self.external_change = matches!(
            state,
            ExternalState::Modified | ExternalState::Missing | ExternalState::Error(_)
        );
        state
    }

    pub fn reload(&mut self, force: bool, read_only: bool) -> Result<(), BufferError> {
        if self.dirty() && !force {
            return Err(BufferError::DirtyReload);
        }
        let Some(path) = self.disk.path.clone() else {
            return Err(BufferError::Save(SaveError::Io(
                "scratch buffer has no file to reload".into(),
            )));
        };
        let (document, disk) = fs::load_file(&path, read_only || self.disk.read_only)?;
        self.document = document;
        self.disk = disk;
        self.language = detect_language(&path);
        self.history.clear();
        self.mark_clean();
        self.external_change = false;
        self.set_selections(vec![self.primary()]);
        Ok(())
    }

    pub fn from_recovery_document(
        id: BufferId,
        label: String,
        document: Document,
        path: Option<PathBuf>,
    ) -> Self {
        let mut buffer = Self::from_document(id, label, document, path);
        buffer.mark_dirty();
        buffer
    }

    pub fn recovery_job(&self) -> RecoveryJob {
        RecoveryJob {
            buffer_id: self.id,
            path: self.disk.path.clone(),
            label: self.label.clone(),
            rope: self.document.rope(),
            version: self.document.version(),
            cursors: self.selections.clone(),
            timestamp_ms: 0,
        }
    }

    pub fn line_indent(&self, pos: BytePos) -> String {
        let line = self.document.byte_to_line(pos);
        let text = self.document.line_text(line);
        text.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    pub fn main_caret_line(&self) -> usize {
        self.document.byte_to_line(self.primary().head).get() + 1
    }
    pub fn main_caret_col(&self, tab_width: usize, cjk_wide: bool) -> usize {
        self.document
            .line_col(self.primary().head, tab_width, cjk_wide)
            .display_col
            .get()
            + 1
    }
}

fn selection_offset(head: BytePos, line_start: BytePos, remove: usize) -> usize {
    head.get()
        .saturating_sub(line_start.get())
        .saturating_sub(remove)
}

/// Cursor positions immediately after each replacement, in ascending edit
/// order. Coordinates are computed from the pre-edit document.
pub fn after_carets(edits: &[Replacement]) -> Vec<Selection> {
    let mut sorted = edits.to_vec();
    sorted.sort_by(|a, b| (a.start, a.end, &a.text).cmp(&(b.start, b.end, &b.text)));
    let mut delta = 0i64;
    let mut out = Vec::with_capacity(sorted.len());
    for edit in &sorted {
        let pos = edit.start.get() as i64 + delta + edit.text.len() as i64;
        out.push(Selection::caret(BytePos(pos.max(0) as usize)));
        delta += edit.text.len() as i64 - (edit.end.get() - edit.start.get()) as i64;
    }
    out
}

/// Map explicit post-edit target byte positions through sorted edits. Each
/// target is supplied in pre-edit coordinates and corresponds to the edit with
/// the same index. An insertion target may include text already inserted at
/// that origin, so it is translated relative to the insertion origin rather
/// than through the generic sticky-left transform.
pub fn shifted_carets(edits: &[Replacement], ends: &[BytePos]) -> Vec<Selection> {
    let mut pairs: Vec<(Replacement, BytePos)> =
        edits.iter().cloned().zip(ends.iter().copied()).collect();
    pairs.sort_by(|a, b| (a.0.start, a.0.end, &a.0.text).cmp(&(b.0.start, b.0.end, &b.0.text)));
    let mut delta = 0i64;
    let mut out = Vec::with_capacity(pairs.len());
    for (edit, end) in &pairs {
        let start = edit.start.get() as i64;
        let old_end = edit.end.get() as i64;
        let text_len = edit.text.len() as i64;
        let new_start = start + delta;
        let target = end.get() as i64;
        let mapped = if old_end == start {
            new_start + (target - start)
        } else if target <= start {
            new_start
        } else if target >= old_end {
            new_start + text_len + (target - old_end)
        } else {
            new_start + (target - start).min(text_len)
        };
        out.push(Selection::caret(BytePos(mapped.max(0) as usize)));
        delta += text_len - (old_end - start);
    }
    out.sort_by_key(|s| s.head);
    out.dedup_by_key(|s| s.head);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_undo_restore_exact_cursors_and_text() {
        let mut buffer = Buffer::scratch(1, "hello");
        buffer.set_carets(vec![BytePos(0)]);
        let original = buffer.document.text();
        buffer.insert_text("X").unwrap();
        assert_eq!(buffer.document.text(), "Xhello");
        assert_eq!(buffer.primary().head, BytePos(1));
        assert!(buffer.dirty());
        buffer.undo().unwrap();
        assert_eq!(buffer.document.text(), original);
        assert_eq!(buffer.primary().head, BytePos(0));
        assert!(!buffer.dirty());
        buffer.redo().unwrap();
        assert_eq!(buffer.document.text(), "Xhello");
        assert_eq!(buffer.primary().head, BytePos(1));
    }

    #[test]
    fn multicursor_typing_has_no_duplicate_edits() {
        let mut buffer = Buffer::scratch(2, "abc\nabc");
        buffer.set_carets(vec![BytePos(0), BytePos(4), BytePos(0)]);
        buffer.insert_text("Z").unwrap();
        assert_eq!(buffer.document.text(), "Zabc\nZabc");
        assert_eq!(buffer.selections.len(), 2);
    }

    #[test]
    fn newline_backspace_and_line_deletion_keep_caret_sane() {
        let mut buffer = Buffer::scratch(4, "ab\ncd");
        buffer.set_carets(vec![BytePos(2)]);
        buffer.insert_newline(true).unwrap();
        assert!(buffer.document.text().starts_with("ab\n"));
        assert_eq!(buffer.primary().head, BytePos(3));
        buffer.backspace().unwrap();
        assert_eq!(buffer.document.text(), "ab\ncd");
        assert_eq!(buffer.primary().head, BytePos(2));
        buffer.set_carets(vec![BytePos(0)]);
        buffer.delete_line().unwrap();
        assert_eq!(buffer.document.text(), "cd");
        assert_eq!(buffer.primary().head, BytePos(0));
    }

    #[test]
    fn dirty_state_follows_saved_history_point_without_hashing_on_render() {
        let dir =
            std::env::temp_dir().join(format!("raster-dirty-{}", crate::util::unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dirty.txt");
        std::fs::write(&path, b"one").unwrap();
        let mut buffer = Buffer::open(1, &path, false, false).unwrap();
        buffer.set_carets(vec![BytePos(0)]);
        assert!(!buffer.dirty());
        buffer.insert_text("X").unwrap();
        assert!(buffer.dirty());
        buffer.undo().unwrap();
        assert!(!buffer.dirty(), "undo back to the original must be clean");
        buffer.redo().unwrap();
        assert!(buffer.dirty());
        buffer.save(SaveOptions::default()).unwrap();
        assert!(!buffer.dirty());
        buffer.insert_text("Y").unwrap();
        assert!(buffer.dirty());
        buffer.undo().unwrap();
        assert!(
            !buffer.dirty(),
            "undo back to the last saved state must be clean"
        );
        buffer.redo().unwrap();
        assert!(buffer.dirty());
        assert_eq!(std::fs::read(&path).unwrap(), b"Xone");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovery_buffers_stay_dirty_until_explicit_save_even_after_undo() {
        let mut buffer = Buffer::from_recovery_document(
            7,
            "recovered".into(),
            Document::from_text("unsaved"),
            None,
        );
        assert!(buffer.dirty());
        buffer.set_carets(vec![BytePos(0)]);
        buffer.insert_text("!").unwrap();
        buffer.undo().unwrap();
        assert!(buffer.dirty());
        buffer.mark_clean();
        assert!(!buffer.dirty());
    }

    #[test]
    fn reload_refuses_unsaved_changes() {
        let mut buffer = Buffer::scratch(3, "a");
        buffer.insert_text("b").unwrap();
        assert_eq!(buffer.reload(false, false), Err(BufferError::DirtyReload));
    }
}
