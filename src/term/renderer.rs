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
        self.cells
            .get(row * self.width + col)
            .map(|c| (c.text.as_str(), c.style))
    }

    /// Render as ANSI bytes. Escape bytes come only from this function.
    pub fn render_ansi(&self, styling: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.cells.len().saturating_mul(4).saturating_add(256));
        out.extend_from_slice(b"\x1b[H\x1b[J\x1b[?25l");
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
                if (styling || essential) && effective_style != last_style {
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
    fn rect_and_ansi_cursor_are_bounded() {
        let rect = Rect::new(2, 3, 4, 5);
        assert!(rect.contains(2, 3));
        assert!(!rect.contains(6, 3));
        let mut frame = Frame::new(10, 2, false);
        frame.cursor = Some((9, 1));
        assert!(String::from_utf8_lossy(&frame.render_ansi(false)).contains("\x1b[2;10H"));
    }
}
