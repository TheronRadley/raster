//! Distinct coordinate types. These are intentionally not interchangeable:
//! byte offsets address UTF-8 storage, grapheme indices address editing
//! boundaries, and display columns address terminal cells.

use std::fmt;
use std::ops::{Add, AddAssign, Sub};

macro_rules! pos_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Default)]
        pub struct $name(pub usize);

        impl $name {
            pub const fn get(self) -> usize {
                self.0
            }
            pub const fn saturating_sub(self, rhs: usize) -> Self {
                Self(self.0.saturating_sub(rhs))
            }
            pub fn checked_add(self, rhs: usize) -> Option<Self> {
                self.0.checked_add(rhs).map(Self)
            }
        }

        impl Add<usize> for $name {
            type Output = Self;
            fn add(self, rhs: usize) -> Self {
                Self(self.0 + rhs)
            }
        }
        impl AddAssign<usize> for $name {
            fn add_assign(&mut self, rhs: usize) {
                self.0 += rhs;
            }
        }
        impl Sub<usize> for $name {
            type Output = Self;
            fn sub(self, rhs: usize) -> Self {
                Self(self.0 - rhs)
            }
        }
        impl From<usize> for $name {
            fn from(value: usize) -> Self {
                Self(value)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

pos_type!(BytePos, "Byte offset into the UTF-8 document.");
pos_type!(CharIndex, "Unicode scalar-value index used by ropey APIs.");
pos_type!(
    GraphemeIndex,
    "Extended grapheme cluster index within a line."
);
pos_type!(DisplayColumn, "Zero-based terminal cell column.");
pos_type!(LineIndex, "Zero-based logical line index.");
pos_type!(VisualRow, "Zero-based row within a soft-wrapped view.");

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Size {
    pub const fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows }
    }
    pub const fn area(self) -> usize {
        self.cols.saturating_mul(self.rows)
    }
    pub const fn is_degraded(self) -> bool {
        self.cols < 24 || self.rows < 6
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct LineCol {
    pub line: LineIndex,
    /// Byte offset from the start of the logical line, not a display column.
    pub byte_col: BytePos,
    /// Grapheme index from the start of the logical line.
    pub grapheme_col: GraphemeIndex,
    /// Terminal columns from the start of the unscrolled logical line.
    pub display_col: DisplayColumn,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct VisualPos {
    pub row: VisualRow,
    pub col: DisplayColumn,
}

impl LineCol {
    pub fn human(&self) -> String {
        format!("{}:{}", self.line.get() + 1, self.grapheme_col.get() + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_types_are_distinct() {
        let b = BytePos(3);
        let c = CharIndex(3);
        // A compile-time type distinction; the values are deliberately equal.
        assert_eq!(b.get(), c.get());
        let pos = LineCol {
            line: LineIndex(4),
            byte_col: BytePos(9),
            grapheme_col: GraphemeIndex(7),
            display_col: DisplayColumn(8),
        };
        assert_eq!(pos.human(), "5:8");
    }
}
