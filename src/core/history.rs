//! Linear undo/redo. Each entry holds only inverse edits and selection state,
//! never a copy of the document.

use crate::core::document::Document;
use crate::core::selection::Selection;
use crate::core::transaction::{Transaction, TxError};

#[derive(Clone, Debug)]
struct UndoEntry {
    forward: Transaction,
    inverse: Transaction,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    limit: usize,
    /// Monotonic count of applied edits. Unlike `undo.len()`, this does not
    /// decrease when an old entry is evicted by the history limit.
    position: u64,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            position: 0,
        }
    }

    pub fn record(&mut self, forward: Transaction, inverse: Transaction) {
        self.position = self.position.wrapping_add(1);
        self.undo.push(UndoEntry { forward, inverse });
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.position = 0;
    }

    /// Used by buffers to compare the current history point with the last
    /// saved point without hashing the whole document on every frame.
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<Vec<Selection>>, TxError> {
        Ok(self.undo_detailed(doc)?.map(|(_, selections)| selections))
    }

    pub fn undo_detailed(
        &mut self,
        doc: &mut Document,
    ) -> Result<Option<(Transaction, Vec<Selection>)>, TxError> {
        let Some(entry) = self.undo.pop() else {
            return Ok(None);
        };
        let (applied_inverse, new_inverse) = entry.inverse.clone().apply(doc)?;
        self.position = self.position.saturating_sub(1);
        let selections = applied_inverse.after_selections.clone();
        self.redo.push(UndoEntry {
            forward: entry.forward,
            inverse: new_inverse,
        });
        Ok(Some((applied_inverse, selections)))
    }

    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<Vec<Selection>>, TxError> {
        Ok(self.redo_detailed(doc)?.map(|(_, selections)| selections))
    }

    pub fn redo_detailed(
        &mut self,
        doc: &mut Document,
    ) -> Result<Option<(Transaction, Vec<Selection>)>, TxError> {
        let Some(entry) = self.redo.pop() else {
            return Ok(None);
        };
        let (applied_forward, new_inverse) = entry.forward.clone().apply(doc)?;
        self.position = self.position.wrapping_add(1);
        let selections = applied_forward.after_selections.clone();
        self.undo.push(UndoEntry {
            forward: applied_forward.clone(),
            inverse: new_inverse,
        });
        Ok(Some((applied_forward, selections)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transaction::Replacement;
    use crate::position::BytePos;

    #[test]
    fn undo_returns_exact_content_and_redo_restores_it() {
        let mut doc = Document::from_text("a\r\nb\nمرحبا");
        let original = doc.text();
        let mut history = History::new(100);
        let (f, i) = Transaction::new(
            "type",
            vec![Replacement::insert(BytePos(3), "XX")],
            doc.version(),
            &[],
        )
        .apply(&mut doc)
        .unwrap();
        history.record(f, i);
        let edited = doc.text();
        assert_eq!(
            history.undo(&mut doc).unwrap().unwrap(),
            Vec::<Selection>::new()
        );
        assert_eq!(doc.text(), original);
        history.redo(&mut doc).unwrap();
        assert_eq!(doc.text(), edited);
        assert_eq!(history.len(), (1, 0));
    }

    #[test]
    fn position_survives_old_entry_eviction() {
        let mut history = History::new(1);
        let mut working = Document::from_text("a");
        for text in ["b", "c"] {
            let (f, i) = Transaction::new(
                "x",
                vec![Replacement::insert(BytePos(1), text)],
                working.version(),
                &[],
            )
            .apply(&mut working)
            .unwrap();
            history.record(f, i);
        }
        assert_eq!(history.len(), (1, 0));
        assert_eq!(history.position(), 2);
        history.undo(&mut working).unwrap();
        assert_eq!(history.position(), 1);
        assert!(!history.can_undo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::from_text("a");
        let mut history = History::new(10);
        let (f, i) = Transaction::new("x", vec![Replacement::insert(BytePos(1), "b")], 0, &[])
            .apply(&mut doc)
            .unwrap();
        history.record(f, i);
        history.undo(&mut doc).unwrap();
        assert!(history.can_redo());
        let (f, i) = Transaction::new("x", vec![Replacement::insert(BytePos(1), "c")], 0, &[])
            .apply(&mut doc)
            .unwrap();
        history.record(f, i);
        assert!(!history.can_redo());
    }
}
