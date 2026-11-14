//! Explicit edit transactions. Typing, deletion, multicursor replacement,
//! formatting, undo, and redo all use this one path.

use std::ops::Range;

use crate::core::document::Document;
use crate::core::selection::Selection;
use crate::position::BytePos;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub start: BytePos,
    pub end: BytePos,
    /// UTF-8 text that replaces `start..end` in the pre-transaction document.
    pub text: String,
}

impl Replacement {
    pub fn new(range: Range<BytePos>, text: impl Into<String>) -> Self {
        Self {
            start: range.start,
            end: range.end,
            text: text.into(),
        }
    }
    pub fn insert(pos: BytePos, text: impl Into<String>) -> Self {
        Self {
            start: pos,
            end: pos,
            text: text.into(),
        }
    }
    pub fn delete(range: Range<BytePos>) -> Self {
        Self::new(range, "")
    }
    pub fn range(&self) -> Range<BytePos> {
        self.start..self.end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub label: &'static str,
    /// Sorted ascending by `(start, end)`, non-overlapping.
    pub edits: Vec<Replacement>,
    pub before_version: u64,
    pub before_selections: Vec<Selection>,
    pub after_selections: Vec<Selection>,
    /// Explicit post-edit cursor positions for typing and commands. When set,
    /// this overrides the generic position transform; the inverse restores
    /// `before_selections` exactly. Used by undo/redo too.
    pub desired_after_selections: Option<Vec<Selection>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxError(pub String);

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for TxError {}

impl Transaction {
    pub fn new(
        label: &'static str,
        edits: Vec<Replacement>,
        before_version: u64,
        selections: &[Selection],
    ) -> Self {
        Self {
            label,
            edits,
            before_version,
            before_selections: selections.to_vec(),
            after_selections: Vec::new(),
            desired_after_selections: None,
        }
    }

    pub fn with_after_selections(mut self, selections: Vec<Selection>) -> Self {
        self.desired_after_selections = Some(selections);
        self
    }

    pub fn insert(
        label: &'static str,
        doc: &Document,
        pos: BytePos,
        text: impl Into<String>,
        selections: &[Selection],
    ) -> Self {
        Self::new(
            label,
            vec![Replacement::insert(pos, text)],
            doc.version(),
            selections,
        )
    }

    pub fn delete(
        label: &'static str,
        doc: &Document,
        range: Range<BytePos>,
        selections: &[Selection],
    ) -> Self {
        Self::new(
            label,
            vec![Replacement::delete(range)],
            doc.version(),
            selections,
        )
    }

    /// Reject invalid byte boundaries, reversed ranges, overlapping edits, and
    /// ambiguous duplicate origins. Identical insertions are concatenated.
    pub fn normalize(&mut self, doc: &Document) -> Result<(), TxError> {
        let len = doc.len_bytes();
        for edit in &self.edits {
            if edit.start > edit.end || edit.end > len {
                return Err(TxError(format!(
                    "invalid edit range {}..{}",
                    edit.start, edit.end
                )));
            }
            if !doc.is_char_boundary(edit.start) || !doc.is_char_boundary(edit.end) {
                return Err(TxError(
                    "edit range is not on UTF-8 character boundaries".into(),
                ));
            }
        }
        self.edits
            .sort_by(|a, b| (a.start, a.end, &a.text).cmp(&(b.start, b.end, &b.text)));
        let mut normalized: Vec<Replacement> = Vec::with_capacity(self.edits.len());
        for edit in self.edits.drain(..) {
            if let Some(last) = normalized.last_mut() {
                if edit.start < last.end {
                    return Err(TxError("overlapping edits in one transaction".into()));
                }
                if edit.start == last.start {
                    if edit.end != last.end {
                        return Err(TxError("ambiguous duplicate edit origin".into()));
                    }
                    last.text.push_str(&edit.text);
                    continue;
                }
            }
            normalized.push(edit);
        }
        self.edits = normalized;
        Ok(())
    }

    /// Apply this transaction to `doc`. Returns the applied forward transaction
    /// (with transformed selections) and its exact inverse. Neither side stores
    /// a snapshot of the whole document.
    pub fn apply(mut self, doc: &mut Document) -> Result<(Transaction, Transaction), TxError> {
        self.before_version = doc.version();
        self.normalize(doc)?;
        let mut delta: i64 = 0;
        let mut inverse_edits = Vec::with_capacity(self.edits.len());
        let mut newline_changed = false;
        let has_break = |s: &str| s.chars().any(|c| matches!(c, '\r' | '\n'));
        for edit in &self.edits {
            let removed = doc.text_in_range(edit.range());
            let new_start = (edit.start.get() as i64 + delta) as usize;
            let new_end = new_start + edit.text.len();
            inverse_edits.push(Replacement {
                start: BytePos(new_start),
                end: BytePos(new_end),
                text: removed,
            });
            newline_changed |=
                has_break(&edit.text) || has_break(&inverse_edits.last().unwrap().text);
            delta += edit.text.len() as i64 - (edit.end.get() - edit.start.get()) as i64;
        }
        // Apply from the end so earlier byte offsets remain valid.
        for edit in self.edits.iter().rev() {
            let char_start = doc.byte_to_char(edit.start);
            let char_end = doc.byte_to_char(edit.end);
            if char_end > char_start {
                doc.rope_mut().remove(char_start..char_end);
            }
            if !edit.text.is_empty() {
                doc.rope_mut().insert(char_start, &edit.text);
            }
        }
        let after_version = doc.bump_version();
        doc.note_possible_newline_change(newline_changed);
        self.after_selections = self.desired_after_selections.clone().unwrap_or_else(|| {
            self.before_selections
                .iter()
                .map(|s| {
                    Selection::new(
                        transform_pos(s.anchor, &self.edits),
                        transform_pos(s.head, &self.edits),
                    )
                })
                .collect()
        });
        let inverse = Transaction {
            label: "undo",
            edits: inverse_edits,
            before_version: after_version,
            before_selections: self.after_selections.clone(),
            after_selections: self.before_selections.clone(),
            desired_after_selections: Some(self.before_selections.clone()),
        };
        // Inverse edits are already in post-edit coordinates and ascending.
        Ok((self, inverse))
    }
}

/// Deterministic position transform:
/// - before an edit: shifted by previous deltas;
/// - at an edit start: sticky-left, maps to the new start;
/// - inside or at the old end: maps to the end of inserted text;
/// - after the edit: shifted by the length delta.
///
/// Diagnostics are deliberately *not* transformed: they carry a document
/// version and are discarded when that version changes. View cursors and
/// viewport anchors use this same transform.
pub fn transform_pos(pos: BytePos, edits_ascending: &[Replacement]) -> BytePos {
    let mut delta: i64 = 0;
    let pos = pos.get() as i64;
    for edit in edits_ascending {
        let old_start = edit.start.get() as i64;
        let old_end = edit.end.get() as i64;
        let new_start = old_start + delta;
        let new_end = new_start + edit.text.len() as i64;
        if pos <= old_start {
            return BytePos((pos + delta).max(0) as usize);
        }
        if pos <= old_end {
            return BytePos(new_end.max(0) as usize);
        }
        delta += edit.text.len() as i64 - (old_end - old_start);
    }
    BytePos((pos + delta).max(0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::selection::Selection;

    fn apply(
        label: &'static str,
        doc: &mut Document,
        edits: Vec<Replacement>,
    ) -> (Transaction, Transaction) {
        Transaction::new(label, edits, doc.version(), &[])
            .apply(doc)
            .unwrap()
    }

    #[test]
    fn insertion_deletion_and_exact_inverse() {
        let mut doc = Document::from_text("مرحبا world");
        let before = doc.text();
        let (_, inverse) = apply(
            "edit",
            &mut doc,
            vec![Replacement::new(BytePos(0)..BytePos(0), "x")],
        );
        assert_eq!(doc.text(), "xمرحبا world");
        let (_, _) = inverse.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), before);
    }

    #[test]
    fn multiple_edits_share_consistent_semantics() {
        let mut doc = Document::from_text("one two three");
        let edits = vec![
            Replacement::new(BytePos(0)..BytePos(3), "ONE"),
            Replacement::new(BytePos(8)..BytePos(13), "THREE"),
        ];
        let (_, inverse) = apply("edit", &mut doc, edits);
        assert_eq!(doc.text(), "ONE two THREE");
        inverse.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), "one two three");
    }

    #[test]
    fn transform_rules_are_documented_and_tested() {
        let edits = vec![Replacement::new(BytePos(2)..BytePos(6), "XYZ")];
        assert_eq!(transform_pos(BytePos(1), &edits), BytePos(1));
        assert_eq!(transform_pos(BytePos(2), &edits), BytePos(2));
        assert_eq!(transform_pos(BytePos(4), &edits), BytePos(5));
        assert_eq!(transform_pos(BytePos(6), &edits), BytePos(5));
        assert_eq!(transform_pos(BytePos(8), &edits), BytePos(7));
    }

    #[test]
    fn invalid_boundaries_and_overlaps_are_rejected() {
        let mut doc = Document::from_text("é");
        let tx = Transaction::new(
            "bad",
            vec![Replacement::new(BytePos(0)..BytePos(1), "x")],
            0,
            &[],
        );
        assert!(tx.apply(&mut doc).is_err());
        let tx = Transaction::new(
            "bad",
            vec![
                Replacement::new(BytePos(0)..BytePos(2), "a"),
                Replacement::new(BytePos(1)..BytePos(3), "b"),
            ],
            0,
            &[],
        );
        assert!(tx.apply(&mut doc).is_err());
        assert_eq!(doc.text(), "é");
    }

    #[test]
    fn selections_transform_with_the_transaction() {
        let mut doc = Document::from_text("abcdef");
        let selections = vec![Selection::new(BytePos(1), BytePos(4))];
        let (forward, _) = Transaction::new(
            "edit",
            vec![Replacement::new(BytePos(0)..BytePos(3), "Z")],
            0,
            &selections,
        )
        .apply(&mut doc)
        .unwrap();
        assert_eq!(
            forward.after_selections,
            vec![Selection::new(BytePos(1), BytePos(2))]
        );
        assert_eq!(doc.text(), "Zdef");
    }

    #[test]
    fn identical_insertion_origins_are_concatenated() {
        let mut doc = Document::from_text("ab");
        let (_, inverse) = apply(
            "edit",
            &mut doc,
            vec![
                Replacement::insert(BytePos(1), "x"),
                Replacement::insert(BytePos(1), "y"),
            ],
        );
        assert_eq!(doc.text(), "axyb");
        inverse.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), "ab");
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use crate::core::history::History;

    #[test]
    fn explicit_cursor_positions_survive_undo_redo() {
        let mut doc = Document::from_text("abc");
        let before = vec![Selection::caret(BytePos(0))];
        let after = vec![Selection::caret(BytePos(1))];
        let tx = Transaction::new(
            "type",
            vec![Replacement::insert(BytePos(0), "X")],
            doc.version(),
            &before,
        )
        .with_after_selections(after.clone());
        let (forward, inverse) = tx.apply(&mut doc).unwrap();
        assert_eq!(forward.after_selections, after);
        let mut history = History::new(10);
        history.record(forward, inverse);
        assert_eq!(history.undo(&mut doc).unwrap(), Some(before));
        assert_eq!(doc.text(), "abc");
        assert_eq!(history.redo(&mut doc).unwrap(), Some(after));
        assert_eq!(doc.text(), "Xabc");
    }
}
