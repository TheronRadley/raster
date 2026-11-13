//! Pure viewport/layout calculations. No terminal or I/O; these functions are
//! used by the renderer and by input-replay/layout tests.

use crate::core::document::{DisplayGrapheme, Document};
use crate::position::{BytePos, LineIndex};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct Viewport {
    pub top_line: usize,
    /// Wrap rows skipped from `top_line` in soft-wrap mode.
    pub top_wrap: usize,
    /// First displayed byte offset relative to the logical line start.
    pub h_offset: BytePos,
    /// Absolute byte anchor transformed by edit transactions; used to preserve
    /// an independent viewport when a shared document changes in another view.
    pub anchor_byte: BytePos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutRow {
    pub line: LineIndex,
    pub start_byte: BytePos,
    pub graphemes: Vec<DisplayGrapheme>,
    pub wrap_index: usize,
    pub continuation: bool,
}

impl LayoutRow {
    pub fn end_byte(&self, doc: &Document) -> BytePos {
        self.graphemes
            .last()
            .map(|g| g.end)
            .unwrap_or_else(|| doc.line_content_range(self.line).end)
    }
}

pub fn line_content_len(doc: &Document, line: LineIndex) -> usize {
    doc.line_content_range(line)
        .end
        .get()
        .saturating_sub(doc.line_to_byte(line).get())
}

/// Build visible rows from the viewport. Soft wrap uses repeated bounded line
/// windows, so a 100 MiB file with one long line does not copy the whole line.
pub fn build_layout(
    doc: &Document,
    viewport: Viewport,
    width: usize,
    max_rows: usize,
    soft_wrap: bool,
    tab_width: usize,
    cjk_wide: bool,
) -> Vec<LayoutRow> {
    let mut out = Vec::with_capacity(max_rows.min(256));
    if width == 0 || max_rows == 0 {
        return out;
    }
    let lines = doc.len_lines();
    let start = viewport.top_line.min(lines.saturating_sub(1));
    for line_idx in start..lines {
        let line = LineIndex(line_idx);
        let line_start = doc.line_to_byte(line);
        if !soft_wrap {
            let graphemes = doc.line_window(line, viewport.h_offset, width, tab_width, cjk_wide);
            let start_byte = graphemes.first().map(|g| g.start).unwrap_or_else(|| {
                BytePos(
                    (line_start.get() + viewport.h_offset.get())
                        .min(doc.line_content_range(line).end.get()),
                )
            });
            out.push(LayoutRow {
                line,
                start_byte,
                graphemes,
                wrap_index: 0,
                continuation: viewport.h_offset.get() > 0,
            });
        } else {
            let mut offset = 0usize;
            let mut wrap_index = 0usize;
            let content_len = line_content_len(doc, line);
            loop {
                let graphemes = doc.line_window(line, BytePos(offset), width, tab_width, cjk_wide);
                if graphemes.is_empty() {
                    out.push(LayoutRow {
                        line,
                        start_byte: BytePos(line_start.get() + offset.min(content_len)),
                        graphemes: Vec::new(),
                        wrap_index,
                        continuation: wrap_index > 0,
                    });
                    break;
                }
                let end_rel = graphemes
                    .last()
                    .map(|g| g.end.get().saturating_sub(line_start.get()))
                    .unwrap_or(offset);
                out.push(LayoutRow {
                    line,
                    start_byte: graphemes.first().map(|g| g.start).unwrap_or(line_start),
                    graphemes,
                    wrap_index,
                    continuation: wrap_index > 0,
                });
                if end_rel <= offset {
                    break;
                }
                offset = end_rel;
                wrap_index += 1;
                if offset >= content_len || out.len() > max_rows + viewport.top_wrap {
                    break;
                }
            }
        }
        if out.len() > max_rows + viewport.top_wrap {
            break;
        }
    }
    if out.len() > viewport.top_wrap {
        out.drain(..viewport.top_wrap);
    }
    out.truncate(max_rows);
    out
}

pub fn row_col_for_pos(
    doc: &Document,
    layout: &[LayoutRow],
    pos: BytePos,
) -> Option<(usize, usize)> {
    let line = doc.byte_to_line(pos);
    for (row_index, row) in layout.iter().enumerate() {
        if row.line != line || pos < row.start_byte {
            continue;
        }
        let end = row.end_byte(doc);
        if pos > end {
            continue;
        }
        if pos == end {
            if let Some(next) = layout.get(row_index + 1) {
                if next.line == line && next.start_byte <= pos {
                    continue;
                }
            }
        }
        let mut col = 0usize;
        for g in &row.graphemes {
            if pos <= g.start {
                break;
            }
            if pos < g.end {
                break;
            }
            col += g.width;
            if pos == g.end {
                return Some((row_index, col));
            }
        }
        return Some((row_index, col));
    }
    None
}

pub fn byte_at_row_col(doc: &Document, row: &LayoutRow, col: usize) -> BytePos {
    let mut used = 0usize;
    for g in &row.graphemes {
        if col < used + g.width.max(1) {
            return g.start;
        }
        used += g.width.max(1);
    }
    row.end_byte(doc)
}

/// Find the byte offset corresponding to a display column in one logical line.
/// Returns a byte offset relative to that line's start.
pub fn byte_offset_for_display_col(
    doc: &Document,
    line: LineIndex,
    target_col: usize,
    tab_width: usize,
    cjk_wide: bool,
) -> BytePos {
    let line_start = doc.line_to_byte(line);
    let graphemes = doc.line_window(
        line,
        BytePos(0),
        target_col.saturating_add(2),
        tab_width,
        cjk_wide,
    );
    let mut used = 0usize;
    for g in &graphemes {
        if used >= target_col {
            return BytePos(g.start.get().saturating_sub(line_start.get()));
        }
        used += g.width.max(1);
    }
    BytePos(
        doc.line_content_range(line)
            .end
            .get()
            .saturating_sub(line_start.get()),
    )
}

pub fn wrap_position(
    doc: &Document,
    pos: BytePos,
    width: usize,
    tab_width: usize,
    cjk_wide: bool,
) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let line = doc.byte_to_line(pos);
    let line_start = doc.line_to_byte(line);
    let (display_col, _) = doc.display_info(line_start..pos, tab_width, cjk_wide);
    (display_col.get() / width, display_col.get() % width)
}

/// Ensure a cursor is visible, returning an adjusted viewport. In soft-wrap
/// mode `top_wrap` is measured from `top_line`; otherwise `h_offset` scrolls
/// the logical line horizontally.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct LayoutOptions {
    pub width: usize,
    pub rows: usize,
    pub soft_wrap: bool,
    pub tab_width: usize,
    pub cjk_wide: bool,
}

impl LayoutOptions {
    pub fn new(
        width: usize,
        rows: usize,
        soft_wrap: bool,
        tab_width: usize,
        cjk_wide: bool,
    ) -> Self {
        Self {
            width,
            rows,
            soft_wrap,
            tab_width,
            cjk_wide,
        }
    }
}

pub fn ensure_visible(
    doc: &Document,
    viewport: Viewport,
    pos: BytePos,
    options: LayoutOptions,
) -> Viewport {
    let LayoutOptions {
        width,
        rows,
        soft_wrap,
        tab_width,
        cjk_wide,
    } = options;
    let mut result = viewport;
    let line = doc.byte_to_line(pos).get();
    if soft_wrap {
        let candidate = build_layout(doc, result, width, rows, true, tab_width, cjk_wide);
        if row_col_for_pos(doc, &candidate, pos).is_none() {
            let (wrap, _) = wrap_position(doc, pos, width, tab_width, cjk_wide);
            result.top_line = line;
            result.top_wrap = wrap.saturating_sub(rows / 2);
            result.h_offset = BytePos(0);
        }
    } else {
        if line < result.top_line || line >= result.top_line.saturating_add(rows) {
            result.top_line = line.saturating_sub(rows / 2);
        }
        result.top_wrap = 0;
        let line_idx = doc.byte_to_line(pos);
        let line_start = doc.line_to_byte(line_idx);
        let (display_col, _) = doc.display_info(line_start..pos, tab_width, cjk_wide);
        let offset_col = doc
            .display_info(
                line_start
                    ..BytePos(
                        (line_start.get() + result.h_offset.get()).min(doc.len_bytes().get()),
                    ),
                tab_width,
                cjk_wide,
            )
            .0
            .get();
        if display_col.get() < offset_col
            || display_col.get() >= offset_col + width.saturating_sub(1)
        {
            let target = display_col.get().saturating_sub(width / 3);
            result.h_offset =
                byte_offset_for_display_col(doc, line_idx, target, tab_width, cjk_wide);
        }
    }
    result.anchor_byte = BytePos(
        doc.line_to_byte(LineIndex(
            result.top_line.min(doc.len_lines().saturating_sub(1)),
        ))
        .get()
        .saturating_add(result.h_offset.get())
        .min(doc.len_bytes().get()),
    );
    result
}

/// Move one visual row while preserving a display column. This is deliberately
/// bounded: if the target is outside a local layout, the caller falls back to
/// logical-line movement.
pub fn visual_move(
    doc: &Document,
    pos: BytePos,
    delta_rows: isize,
    desired_col: usize,
    options: LayoutOptions,
) -> Option<BytePos> {
    let LayoutOptions {
        width,
        soft_wrap,
        tab_width,
        cjk_wide,
        ..
    } = options;
    if !soft_wrap || width == 0 {
        return None;
    }
    let line = doc.byte_to_line(pos).get();
    let start = line.saturating_sub(2 + delta_rows.unsigned_abs());
    let viewport = Viewport {
        top_line: start,
        top_wrap: 0,
        h_offset: BytePos(0),
        anchor_byte: BytePos(0),
    };
    let layout = build_layout(doc, viewport, width, 256, true, tab_width, cjk_wide);
    let (current, col) = row_col_for_pos(doc, &layout, pos)?;
    let preferred_col = if desired_col > 0 { desired_col } else { col };
    let target = current as isize + delta_rows;
    if target < 0 || target as usize >= layout.len() {
        return None;
    }
    Some(byte_at_row_col(
        doc,
        &layout[target as usize],
        preferred_col,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_wrap_rows_cover_long_line_without_copying_it() {
        let doc = Document::from_text(&"abcdefghij".repeat(20));
        let layout = build_layout(&doc, Viewport::default(), 4, 10, true, 4, false);
        assert!(layout.len() >= 10);
        assert_eq!(layout[0].wrap_index, 0);
        assert_eq!(layout[1].wrap_index, 1);
        assert!(layout[0].graphemes.len() <= 4);
        assert_eq!(row_col_for_pos(&doc, &layout, BytePos(0)), Some((0, 0)));
        assert_eq!(row_col_for_pos(&doc, &layout, BytePos(5)), Some((1, 1)));
    }

    #[test]
    fn horizontal_scroll_keeps_cursor_in_view() {
        let doc = Document::from_text(&"x".repeat(100));
        let viewport = ensure_visible(
            &doc,
            Viewport::default(),
            BytePos(80),
            LayoutOptions::new(20, 5, false, 4, false),
        );
        assert!(viewport.h_offset.get() > 0);
        let layout = build_layout(&doc, viewport, 20, 5, false, 4, false);
        assert!(row_col_for_pos(&doc, &layout, BytePos(80)).is_some());
    }

    #[test]
    fn visual_move_follows_wrapped_rows() {
        let doc = Document::from_text(&"abcdefghij".repeat(4));
        let from = BytePos(2);
        let next =
            visual_move(&doc, from, 1, 2, LayoutOptions::new(4, 256, true, 4, false)).unwrap();
        assert_eq!(next.get(), 6);
    }

    #[test]
    fn wide_and_combining_text_map_to_start_of_cluster() {
        let doc = Document::from_text("😀e\u{301}x");
        let layout = build_layout(&doc, Viewport::default(), 20, 2, false, 4, false);
        assert_eq!(byte_at_row_col(&doc, &layout[0], 1), BytePos(0));
        assert_eq!(byte_at_row_col(&doc, &layout[0], 2), BytePos(4));
    }
}
