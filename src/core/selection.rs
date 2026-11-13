//! Cursors and selections. Byte offsets are the only stored coordinates.

use crate::position::BytePos;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct Selection {
    /// The fixed end of a selection. It may be after or before `head`.
    pub anchor: BytePos,
    /// The moving end and the cursor position.
    pub head: BytePos,
}

impl Selection {
    pub fn caret(pos: BytePos) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }
    pub fn new(anchor: BytePos, head: BytePos) -> Self {
        Self { anchor, head }
    }
    pub fn start(&self) -> BytePos {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> BytePos {
        self.anchor.max(self.head)
    }
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
    pub fn range(&self) -> std::ops::Range<BytePos> {
        self.start()..self.end()
    }
    pub fn with_head(&self, head: BytePos) -> Self {
        Self {
            anchor: self.anchor,
            head,
        }
    }
    pub fn collapsed_at_head(&self) -> Self {
        Self::caret(self.head)
    }
    pub fn collapsed_at_start(&self) -> Self {
        Self::caret(self.start())
    }
    pub fn swap_ends(&self) -> Self {
        Self {
            anchor: self.head,
            head: self.anchor,
        }
    }
    pub fn contains(&self, pos: BytePos) -> bool {
        self.start() <= pos && pos <= self.end()
    }
}

/// Deterministic multicursor normalization: sort by byte range, merge
/// overlapping or touching selections into their union, and drop duplicate
/// carets. The result is always ordered ascending with no overlaps.
pub fn normalize_selections(selections: &[Selection]) -> Vec<Selection> {
    let mut sorted = selections.to_vec();
    sorted.sort_by_key(|s| (s.start().get(), s.end().get(), s.head.get()));
    let mut out: Vec<Selection> = Vec::with_capacity(sorted.len());
    for sel in sorted {
        if let Some(last) = out.last_mut() {
            if sel.start() <= last.end() {
                let start = last.start().min(sel.start());
                let end = last.end().max(sel.end());
                // Keep orientation of the earlier selection; a leftward
                // selection that fully contains the merge keeps its head.
                if last.anchor <= last.head {
                    *last = Selection::new(start, end);
                } else {
                    *last = Selection::new(end, start);
                }
                continue;
            }
        }
        out.push(sel);
    }
    out
}

/// Merge selections that share the same caret position. This is used before
/// multicursor commands so duplicate cursors cannot create duplicate edits.
pub fn deduplicate_carets(selections: &[Selection]) -> Vec<Selection> {
    let mut out: Vec<Selection> = Vec::new();
    let mut sorted = selections.to_vec();
    sorted.sort_by_key(|s| (s.head.get(), s.start().get(), s.end().get()));
    for sel in sorted {
        if let Some(last) = out.last() {
            if last.head == sel.head && last.is_caret() && sel.is_caret() {
                continue;
            }
        }
        out.push(sel);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_selections_merge_deterministically() {
        let a = Selection::new(BytePos(10), BytePos(4));
        let b = Selection::new(BytePos(8), BytePos(20));
        let c = Selection::caret(BytePos(20));
        let merged = normalize_selections(&[a, b, c]);
        assert_eq!(merged, vec![Selection::new(BytePos(20), BytePos(4))]);
    }

    #[test]
    fn duplicate_carets_are_removed() {
        let s = deduplicate_carets(&[
            Selection::caret(BytePos(5)),
            Selection::caret(BytePos(2)),
            Selection::caret(BytePos(5)),
        ]);
        assert_eq!(
            s,
            vec![Selection::caret(BytePos(2)), Selection::caret(BytePos(5))]
        );
    }
}
