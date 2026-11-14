//! Pure terminal frame buffer. All user-provided text is sanitized here before
//! it can reach a terminal; the renderer itself owns every escape sequence.
//! Styling is quiet and monochrome: dim, underline, bold, and reverse video.

use unicode_segmentation::UnicodeSegmentation;

use crate::util::{grapheme_width, sanitize_display};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub enum Style {
    #[default]
    Plain,
    Dim,
    Bold,
    Underline,
    Reverse,
    Active,
    Warning,
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Self::Plain => "\x1b[0m",
            Self::Dim => "\x1b[2m",
            Self::Bold => "\x1b[1m",
            Self::Underline => "\x1b[4m",
            Self::Reverse => "\x1b[7m",
            Self::Active => "\x1b[1;4m",
            Self::Warning => "\x1b[1;7m",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cell {
    text: String,
    width: usize,
    continuation: bool,
    style: Style,
}

impl Cell {
    fn blank() -> Self {
        Self {
            text: " ".into(),
            width: 1,
            continuation: false,
            style: Style::Plain,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub cursor: Option<(usize, usize)>,
    cjk_wide: bool,
    cells: Vec<Cell>,
}

impl Frame {
    pub fn new(width: usize, height: usize, cjk_wide: bool) -> Self {
        Self {
            width,
            height,
            cursor: None,
            cjk_wide,
            cells: vec![Cell::blank(); width.saturating_mul(height)],
        }
    }

    /// Sanitize and write text at a cell position, clipped to `max_width`.
    /// Returns display columns used. Text is clipped on grapheme boundaries;
    /// wide graphemes never overwrite a neighbor's second cell.
    pub fn set(
        &mut self,
        row: usize,
        col: usize,
        text: &str,
        style: Style,
        max_width: usize,
    ) -> usize {
        if row >= self.height || col >= self.width || max_width == 0 {
            return 0;
        }
        let safe = sanitize_display(text);
        let limit = max_width.min(self.width.saturating_sub(col));
        let mut used = 0usize;
        for grapheme in safe.graphemes(true) {
            let width = grapheme_width(grapheme, self.cjk_wide).max(1);
            if used + width > limit {
                break;
            }
            for offset in 0..width {
                self.clear_glyph_at(row, col + used + offset);
            }
            for offset in 0..width {
                let index = row * self.width + col + used + offset;
                self.cells[index] = Cell {
                    text: if offset == 0 {
                        grapheme.to_string()
                    } else {
                        String::new()
                    },
                    width: if offset == 0 { width } else { 0 },
                    continuation: offset > 0,
                    style,
                };
            }
            used += width;
        }
        used
    }

    // Overwriting either half of a wide glyph must erase the whole glyph;
    // otherwise its stale continuation makes later output shift or disappear.
    fn clear_glyph_at(&mut self, row: usize, col: usize) {
        let row_start = row * self.width;
        let mut start = col;
        while start > 0 && self.cells[row_start + start].continuation {
            start -= 1;
        }
        let width = self.cells[row_start + start].width.max(1);
        for offset in 0..width.min(self.width - start) {
            self.cells[row_start + start + offset] = Cell::blank();
        }
    }

    pub fn fill(&mut self, row: usize, style: Style) {
        if row >= self.height {
            return;
        }
        for col in 0..self.width {
            self.cells[row * self.width + col] = Cell {
                style,
                ..Cell::blank()
            };
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<(&str, Style)> {
        if row >= self.height || col >= self.width {
            return None;
        }
        self.cells
            .get(row * self.width + col)
            .map(|c| (c.text.as_str(), c.style))
    }

    /// Render as ANSI bytes. Escape bytes come only from this function.
    pub fn render_ansi(&self, styling: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.cells.len().saturating_mul(4).saturating_add(256));
        out.extend_from_slice(b"\x1b[H\x1b[?25l");
        for row in 0..self.height {
            out.extend_from_slice(format!("\x1b[{};1H\x1b[K", row + 1).as_bytes());
            let mut col = 0usize;
            let mut last_style = Style::Plain;
            if styling {
                out.extend_from_slice(Style::Plain.sgr().as_bytes());
            }
            while col < self.width {
                let cell = &self.cells[row * self.width + col];
                if cell.continuation || cell.width == 0 {
                    col += 1;
                    continue;
                }
                let essential =
                    matches!(cell.style, Style::Reverse | Style::Active | Style::Warning);
                let effective_style = if styling || essential {
                    cell.style
                } else {
                    Style::Plain
                };
                if effective_style != last_style {
                    out.extend_from_slice(effective_style.sgr().as_bytes());
                    last_style = effective_style;
                }
                if cell.text.is_empty() {
                    out.push(b' ');
                } else {
                    out.extend_from_slice(cell.text.as_bytes());
                }
                col += cell.width.max(1);
            }
            if last_style != Style::Plain {
                out.extend_from_slice(Style::Plain.sgr().as_bytes());
            }
        }
        if let Some((x, y)) = self.cursor {
            if x < self.width && y < self.height {
                out.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1).as_bytes());
            }
        } else {
            out.extend_from_slice(b"\x1b[?25l");
        }
        out
    }

    /// Plain text for tests and `--dump-screen`. One line per row, no escapes.
    pub fn render_plain(&self) -> String {
        let mut out = String::new();
        for row in 0..self.height {
            let mut col = 0usize;
            while col < self.width {
                let cell = &self.cells[row * self.width + col];
                if cell.continuation || cell.width == 0 {
                    out.push(' ');
                    col += 1;
                } else {
                    out.push_str(if cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    });
                    col += cell.width.max(1);
                }
            }
            out.push('\n');
        }
        out
    }
}

pub fn horizontal_rule(width: usize) -> String {
    "─".repeat(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_controls_never_reach_terminal_output() {
        let mut frame = Frame::new(20, 2, false);
        frame.set(0, 0, "safe\x1b[31m\u{202e}name", Style::Plain, 20);
        let ansi = frame.render_ansi(true);
        // The bytes of the pasted SGR sequence may appear as harmless text,
        // but the ESC that would make it a control sequence must be gone.
        assert!(!String::from_utf8_lossy(&ansi).contains("\x1b[31m"));
        let plain = frame.render_plain();
        assert!(plain.contains("\u{fffd}"));
        assert!(!plain.contains('\u{202e}'));
    }

    #[test]
    fn wide_graphemes_use_two_cells_and_plain_output_is_aligned() {
        let mut frame = Frame::new(6, 1, false);
        let used = frame.set(0, 0, "😀ab", Style::Dim, 6);
        assert_eq!(used, 4);
        assert_eq!(frame.render_plain().lines().next(), Some("😀ab  "));
    }

    #[test]
    fn overwriting_either_half_of_a_wide_glyph_clears_the_other_half() {
        for col in [0, 1] {
            let mut frame = Frame::new(5, 1, false);
            frame.set(0, 0, "😀xy", Style::Plain, 5);
            frame.set(0, col, "A", Style::Bold, 1);
            let expected = if col == 0 { "A xy " } else { " Axy " };
            assert_eq!(frame.render_plain().lines().next(), Some(expected));
            assert_eq!(frame.cell(0, 5), None);
        }
    }

    #[test]
    fn essential_style_resets_before_plain_text_without_styling() {
        let mut frame = Frame::new(3, 1, false);
        frame.set(0, 0, "A", Style::Reverse, 1);
        frame.set(0, 1, "B", Style::Plain, 1);
        let ansi = String::from_utf8(frame.render_ansi(false)).unwrap();
        assert!(ansi.contains("\x1b[7mA\x1b[0mB"), "{ansi:?}");
    }

    #[test]
    fn rect_and_ansi_cursor_are_bounded() {
        let rect = Rect::new(2, 3, 4, 5);
        assert!(rect.contains(2, 3));
        assert!(!rect.contains(6, 3));
        let mut frame = Frame::new(10, 2, false);
        frame.cursor = Some((9, 1));
        assert!(String::from_utf8_lossy(&frame.render_ansi(false)).contains("\x1b[2;10H"));
    }
}
