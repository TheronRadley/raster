//! Text storage. All stored positions are byte offsets; ropey's char indices
//! are confined to this module. No ordinary edit copies the whole document.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::io::{self, Write};
use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};

use crate::config::NewlineMode;
use crate::position::{BytePos, DisplayColumn, GraphemeIndex, LineCol, LineIndex};
use crate::util::{fnv1a_stream, grapheme_width, sanitize_char};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum NewlineStyle {
    None,
    Lf,
    Crlf,
    Cr,
    Mixed,
}

impl NewlineStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
            Self::Cr => "CR",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    rope: Rope,
    version: u64,
    newline: NewlineStyle,
    had_final_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentError(pub String);

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for DocumentError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayGrapheme {
    pub start: BytePos,
    pub end: BytePos,
    /// Sanitized text suitable for terminal output; tabs are already expanded.
    pub text: String,
    pub width: usize,
}

impl Document {
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            version: 0,
            newline: NewlineStyle::None,
            had_final_newline: false,
        }
    }

    pub fn from_text(text: &str) -> Self {
        Self::from_rope(Rope::from_str(text))
    }

    pub fn from_rope(rope: Rope) -> Self {
        let newline = detect_newline_style(rope.bytes());
        let had_final_newline = ends_with_line_break(&rope);
        Self {
            rope,
            version: 0,
            newline,
            had_final_newline,
        }
    }

    /// Refuse binary or undecodable content rather than replacing bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DocumentError> {
        if bytes.contains(&0) {
            return Err(DocumentError(
                "file contains NUL bytes; open it as binary is not supported".into(),
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| {
            DocumentError(format!(
                "file is not valid UTF-8 at byte {}; no bytes were converted",
                e.valid_up_to()
            ))
        })?;
        Ok(Self::from_text(text))
    }

    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn bump_version(&mut self) -> u64 {
        self.version = self.version.wrapping_add(1);
        self.version
    }
    pub fn newline_style(&self) -> NewlineStyle {
        self.newline
    }
    pub fn had_final_newline(&self) -> bool {
        self.had_final_newline
    }
    pub fn len_bytes(&self) -> BytePos {
        BytePos(self.rope.len_bytes())
    }
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }
    pub fn rope(&self) -> Rope {
        self.rope.clone()
    }
    pub fn rope_mut(&mut self) -> &mut Rope {
        &mut self.rope
    }

    pub fn is_char_boundary(&self, byte: BytePos) -> bool {
        let byte = byte.get().min(self.rope.len_bytes());
        self.rope.char_to_byte(self.rope.byte_to_char(byte)) == byte
    }

    pub fn floor_boundary(&self, byte: BytePos) -> BytePos {
        let byte = byte.get().min(self.rope.len_bytes());
        BytePos(self.rope.char_to_byte(self.rope.byte_to_char(byte)))
    }

    pub fn ceil_boundary(&self, byte: BytePos) -> BytePos {
        let byte = byte.get().min(self.rope.len_bytes());
        let char_idx = self.rope.byte_to_char(byte);
        let start = self.rope.char_to_byte(char_idx);
        if start < byte {
            BytePos(
                self.rope
                    .char_to_byte((char_idx + 1).min(self.rope.len_chars())),
            )
        } else {
            BytePos(byte)
        }
    }

    pub fn byte_to_char(&self, byte: BytePos) -> usize {
        self.rope
            .byte_to_char(byte.get().min(self.rope.len_bytes()))
    }

    pub fn char_to_byte(&self, char_idx: usize) -> BytePos {
        BytePos(self.rope.char_to_byte(char_idx.min(self.rope.len_chars())))
    }

    pub fn byte_to_line(&self, byte: BytePos) -> LineIndex {
        LineIndex(
            self.rope
                .byte_to_line(byte.get().min(self.rope.len_bytes())),
        )
    }

    pub fn line_to_byte(&self, line: LineIndex) -> BytePos {
        let line = line.get().min(self.len_lines().saturating_sub(1));
        BytePos(self.rope.line_to_byte(line))
    }

    pub fn line_content_range(&self, line: LineIndex) -> Range<BytePos> {
        let line = line.get().min(self.len_lines().saturating_sub(1));
        let start = self.line_to_byte(LineIndex(line));
        let end = if line + 1 < self.len_lines() {
            self.line_to_byte(LineIndex(line + 1))
        } else {
            self.len_bytes()
        };
        // ropey counts CR and LF as separate line boundaries, so a CRLF pair
        // can describe an empty intermediate line. Clamp to the line start to
        // avoid an inverted range for that line.
        start
            ..BytePos(
                end.get()
                    .saturating_sub(line_break_len_before(&self.rope, end))
                    .max(start.get()),
            )
    }

    pub fn line_text(&self, line: LineIndex) -> String {
        let range = self.line_content_range(line);
        self.text_in_range(range)
    }

    pub fn line_with_break(&self, line: LineIndex) -> String {
        let line = line.get().min(self.len_lines().saturating_sub(1));
        let start = self.line_to_byte(LineIndex(line));
        let end = if line + 1 < self.len_lines() {
            self.line_to_byte(LineIndex(line + 1))
        } else {
            self.len_bytes()
        };
        self.text_in_range(start..end)
    }

    pub fn text_in_range(&self, range: Range<BytePos>) -> String {
        let start = self.floor_boundary(BytePos(range.start.get().min(range.end.get())));
        let end = self.ceil_boundary(BytePos(range.end.get().max(range.start.get())));
        if start >= end {
            return String::new();
        }
        let slice = self.rope.byte_slice(start.get()..end.get());
        let cow: Cow<'_, str> = slice.into();
        cow.into_owned()
    }

    pub fn text(&self) -> String {
        let cow: Cow<'_, str> = self.rope.slice(..).into();
        cow.into_owned()
    }

    pub fn fingerprint(&self) -> u64 {
        fnv1a_stream(self.rope.bytes())
    }

    pub fn write_to<W: Write>(&self, writer: &mut W, newline: NewlineMode) -> io::Result<()> {
        if newline == NewlineMode::Preserve {
            for chunk in self.rope.chunks() {
                writer.write_all(chunk.as_bytes())?;
            }
            return Ok(());
        }
        let target: &[u8] = match newline {
            NewlineMode::Lf => b"\n",
            NewlineMode::Crlf => b"\r\n",
            NewlineMode::Preserve => unreachable!(),
        };
        let mut iter = self.rope.bytes().peekable();
        let mut pending_cr = false;
        for byte in iter.by_ref() {
            if pending_cr {
                pending_cr = false;
                if byte != b'\n' {
                    writer.write_all(target)?;
                }
            }
            match byte {
                b'\r' => pending_cr = true,
                b'\n' => writer.write_all(target)?,
                b => writer.write_all(&[b])?,
            }
        }
        if pending_cr {
            writer.write_all(target)?;
        }
        Ok(())
    }

    /// Called by save/recovery tests; normal rendering uses bounded windows.
    pub fn to_bytes(&self, newline: NewlineMode) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rope.len_bytes());
        let _ = self.write_to(&mut out, newline);
        out
    }

    pub fn prev_grapheme(&self, pos: BytePos) -> BytePos {
        if pos.get() == 0 {
            return BytePos(0);
        }
        let previous = BytePos(pos.get().saturating_sub(1));
        self.grapheme_boundary_at_or_before(previous)
    }

    pub fn next_grapheme(&self, pos: BytePos) -> BytePos {
        let len = self.len_bytes();
        if pos >= len {
            return len;
        }
        // Start from a known extended-grapheme boundary. A window starting in
        // the middle of a cluster would produce incorrect boundaries.
        let start = self.grapheme_boundary_at_or_before(pos);
        let rel = pos.get().saturating_sub(start.get());
        let mut end = self.ceil_boundary(BytePos((pos.get() + 512).min(len.get())));
        for _ in 0..12 {
            let text = self.text_in_range(start..end);
            let mut items: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
            if end < len {
                if let Some(last) = items.last() {
                    if last.0 + last.1.len() == text.len() {
                        items.pop();
                    }
                }
            }
            for (idx, grapheme) in items {
                let after = idx + grapheme.len();
                if after > rel {
                    return BytePos(start.get() + after);
                }
            }
            if end >= len {
                return len;
            }
            end = self.ceil_boundary(BytePos((end.get() + 512).min(len.get())));
        }
        len
    }

    /// Find an extended grapheme cluster boundary at or before `pos`, using
    /// `GraphemeCursor` with bounded chunks so it does not copy a huge line.
    pub fn grapheme_boundary_at_or_before(&self, pos: BytePos) -> BytePos {
        let mut candidate = self.floor_boundary(pos.min(self.len_bytes()));
        for _ in 0..8192 {
            if candidate.get() == 0 || self.is_grapheme_boundary(candidate) {
                return candidate;
            }
            candidate = self.floor_boundary(BytePos(candidate.get().saturating_sub(1)));
        }
        // A cluster longer than 8192 scalar values is pathological; fall back
        // to a valid UTF-8 boundary rather than scanning the whole document.
        BytePos(0)
    }

    fn is_grapheme_boundary(&self, pos: BytePos) -> bool {
        let len = self.len_bytes().get();
        let pos = self.floor_boundary(pos.min(self.len_bytes())).get();
        if pos == 0 || pos == len {
            return true;
        }
        let mut cursor = GraphemeCursor::new(pos, len, true);
        let mut chunk_start = self.floor_boundary(BytePos(pos.saturating_sub(512))).get();
        let mut chunk_end = self.ceil_boundary(BytePos((pos + 512).min(len))).get();
        for _ in 0..24 {
            let chunk = self.text_in_range(BytePos(chunk_start)..BytePos(chunk_end));
            match cursor.is_boundary(&chunk, chunk_start) {
                Ok(value) => return value,
                Err(GraphemeIncomplete::PreContext(ctx)) => {
                    if ctx == 0 {
                        return true;
                    }
                    let pre_end = BytePos(ctx);
                    let pre_start = self.floor_boundary(BytePos(ctx.saturating_sub(512)));
                    let pre = self.text_in_range(pre_start..pre_end);
                    if pre_start.get() + pre.len() != ctx {
                        return false;
                    }
                    cursor.provide_context(&pre, pre_start.get());
                }
                Err(GraphemeIncomplete::NextChunk) => {
                    chunk_end = self
                        .ceil_boundary(BytePos((chunk_end + 512).min(len)))
                        .get();
                }
                Err(GraphemeIncomplete::PrevChunk) => {
                    let old_start = chunk_start;
                    chunk_start = self
                        .floor_boundary(BytePos(old_start.saturating_sub(512)))
                        .get();
                    chunk_end = old_start;
                }
                Err(GraphemeIncomplete::InvalidOffset) => {
                    let cur = cursor.cur_cursor();
                    chunk_start = self.floor_boundary(BytePos(cur.saturating_sub(512))).get();
                    chunk_end = self.ceil_boundary(BytePos((cur + 512).min(len))).get();
                }
            }
        }
        false
    }

    pub fn prev_word(&self, pos: BytePos) -> BytePos {
        self.word_boundary(pos, false)
    }

    pub fn next_word(&self, pos: BytePos) -> BytePos {
        self.word_boundary(pos, true)
    }

    fn word_boundary(&self, pos: BytePos, forward: bool) -> BytePos {
        let start = self.floor_boundary(BytePos(pos.get().saturating_sub(512)));
        let end = self.ceil_boundary(BytePos((pos.get() + 512).min(self.len_bytes().get())));
        let text = self.text_in_range(start..end);
        let rel = pos.get().saturating_sub(start.get()).min(text.len());
        let boundaries = text.split_word_bound_indices().collect::<Vec<_>>();
        let is_word_start = |idx: usize| -> bool {
            if idx >= text.len() {
                return false;
            }
            let ch = text[idx..].chars().next().unwrap_or(' ');
            let prev = if idx == 0 {
                None
            } else {
                text[..idx].chars().next_back()
            };
            (ch.is_alphanumeric() || ch == '_')
                && !prev
                    .map(|c| c.is_alphanumeric() || c == '_')
                    .unwrap_or(false)
        };
        if forward {
            for (idx, _) in boundaries.iter().copied() {
                if idx > rel && is_word_start(idx) {
                    return BytePos(start.get() + idx);
                }
            }
            self.len_bytes()
        } else {
            let mut candidate = None;
            for (idx, _) in boundaries.iter().copied() {
                if idx < rel && is_word_start(idx) {
                    candidate = Some(idx);
                }
            }
            BytePos(start.get() + candidate.unwrap_or(0))
        }
    }

    /// Display columns and grapheme count between two byte offsets. Chunks are
    /// streamed with a small carry so grapheme clusters spanning rope chunk
    /// boundaries are still one unit.
    pub fn display_info(
        &self,
        range: Range<BytePos>,
        tab_width: usize,
        cjk_wide: bool,
    ) -> (DisplayColumn, GraphemeIndex) {
        let start = self.floor_boundary(BytePos(range.start.get().min(range.end.get())));
        let end = self.ceil_boundary(BytePos(range.end.get().max(range.start.get())));
        if start >= end {
            return (DisplayColumn(0), GraphemeIndex(0));
        }
        let slice = self.rope.byte_slice(start.get()..end.get());
        let mut col = 0usize;
        let mut graphemes = 0usize;
        let mut carry = String::new();
        for chunk in slice.chunks() {
            carry.push_str(chunk);
            // Keep a pathological ZWJ chain bounded rather than accumulating
            // an entire huge line in memory.
            if carry.len() > 4096 {
                process_graphemes(&carry, &mut col, &mut graphemes, tab_width, cjk_wide);
                carry.clear();
                continue;
            }
            let mut items: Vec<(usize, &str)> = carry.grapheme_indices(true).collect();
            if items.len() <= 1 {
                continue;
            }
            let last = items.pop().unwrap();
            for (_, g) in items {
                process_graphemes(g, &mut col, &mut graphemes, tab_width, cjk_wide);
            }
            carry = last.1.to_string();
        }
        if !carry.is_empty() {
            process_graphemes(&carry, &mut col, &mut graphemes, tab_width, cjk_wide);
        }
        (DisplayColumn(col), GraphemeIndex(graphemes))
    }

    pub fn line_col(&self, pos: BytePos, tab_width: usize, cjk_wide: bool) -> LineCol {
        let pos = pos.min(self.len_bytes());
        let line = self.byte_to_line(pos);
        let line_start = self.line_to_byte(line);
        let (display_col, grapheme_col) = self.display_info(line_start..pos, tab_width, cjk_wide);
        LineCol {
            line,
            byte_col: BytePos(pos.get().saturating_sub(line_start.get())),
            grapheme_col,
            display_col,
        }
    }

    /// Bounded window of one logical line for rendering. `first_byte` is an
    /// offset from the start of the line content, not an absolute offset.
    pub fn line_window(
        &self,
        line: LineIndex,
        first_byte: BytePos,
        max_cols: usize,
        tab_width: usize,
        cjk_wide: bool,
    ) -> Vec<DisplayGrapheme> {
        let content = self.line_content_range(line);
        let raw_start = BytePos((content.start.get() + first_byte.get()).min(content.end.get()));
        let start = self
            .grapheme_boundary_at_or_before(raw_start.max(content.start))
            .max(content.start);
        let byte_budget = max_cols
            .saturating_mul(4)
            .saturating_add(512)
            .min(16 * 1024);
        let raw_end = BytePos((start.get() + byte_budget).min(content.end.get()));
        let end = self.ceil_boundary(raw_end);
        let text = self.text_in_range(start..end);
        let mut out = Vec::new();
        let mut col = 0usize;
        for (idx, grapheme) in text.grapheme_indices(true) {
            if col >= max_cols {
                break;
            }
            let raw_width = if grapheme == "\t" {
                tab_width.saturating_sub(col % tab_width.max(1)).max(1)
            } else {
                let safe: String = grapheme.chars().map(sanitize_char).collect();
                grapheme_width(&safe, cjk_wide).max(1)
            };
            let width = raw_width.min(max_cols.saturating_sub(col).max(1));
            let text = if grapheme == "\t" {
                " ".repeat(width)
            } else {
                grapheme.chars().map(sanitize_char).collect()
            };
            out.push(DisplayGrapheme {
                start: BytePos(start.get() + idx),
                end: BytePos(start.get() + idx + grapheme.len()),
                text,
                width,
            });
            col += width;
        }
        out
    }

    pub fn clamp(&self, pos: BytePos) -> BytePos {
        self.grapheme_boundary_at_or_before(pos.min(self.len_bytes()))
    }

    /// Convert all ASCII-style line breaks explicitly. Unicode line separators
    /// are left untouched; Raster never changes them implicitly.
    pub fn convert_line_endings(&self, mode: NewlineMode) -> String {
        if mode == NewlineMode::Preserve {
            return self.text();
        }
        self.text()
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .pipe(|s| {
                if mode == NewlineMode::Crlf {
                    s.replace('\n', "\r\n")
                } else {
                    s
                }
            })
    }

    pub(crate) fn note_possible_newline_change(&mut self, changed: bool) {
        if changed {
            self.newline = NewlineStyle::Mixed;
        }
        self.had_final_newline = ends_with_line_break(&self.rope);
    }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

fn process_graphemes(
    text: &str,
    col: &mut usize,
    count: &mut usize,
    tab_width: usize,
    cjk_wide: bool,
) {
    for g in text.graphemes(true) {
        *count += 1;
        if is_line_break(g) {
            continue;
        }
        if g == "\t" {
            *col += tab_width.saturating_sub(*col % tab_width.max(1)).max(1);
        } else {
            let safe: String = g.chars().map(sanitize_char).collect();
            *col += grapheme_width(&safe, cjk_wide);
        }
    }
}

fn is_line_break(g: &str) -> bool {
    g.chars()
        .all(|c| matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

fn detect_newline_style(bytes: impl Iterator<Item = u8>) -> NewlineStyle {
    let mut style = NewlineStyle::None;
    let mut prev_cr = false;
    for byte in bytes {
        if prev_cr {
            prev_cr = false;
            if byte == b'\n' {
                merge_style(&mut style, NewlineStyle::Crlf);
                continue;
            }
            merge_style(&mut style, NewlineStyle::Cr);
        }
        match byte {
            b'\r' => prev_cr = true,
            b'\n' => merge_style(&mut style, NewlineStyle::Lf),
            _ => {}
        }
    }
    if prev_cr {
        merge_style(&mut style, NewlineStyle::Cr);
    }
    style
}

fn merge_style(current: &mut NewlineStyle, found: NewlineStyle) {
    match (*current, found) {
        (NewlineStyle::None, _) => *current = found,
        (a, b) if a == b => {}
        _ => *current = NewlineStyle::Mixed,
    }
}

fn ends_with_line_break(rope: &Rope) -> bool {
    let len = rope.len_bytes();
    if len == 0 {
        return false;
    }
    let start = rope.char_to_byte(rope.byte_to_char(len.saturating_sub(8)));
    let slice = rope.byte_slice(start..len);
    let cow: Cow<'_, str> = slice.into();
    cow.chars()
        .last()
        .map(|c| matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}'))
        .unwrap_or(false)
}

fn line_break_len_before(rope: &Rope, end: BytePos) -> usize {
    let len = rope.len_bytes();
    let end = end.get().min(len);
    if end == 0 {
        return 0;
    }
    let start = rope.char_to_byte(rope.byte_to_char(end.saturating_sub(8)));
    if start >= end {
        return 0;
    }
    let slice = rope.byte_slice(start..end);
    let cow: Cow<'_, str> = slice.into();
    let text: &str = cow.as_ref();
    if text.ends_with("\r\n") {
        return 2;
    }
    match text.chars().last() {
        Some('\n') | Some('\r') => 1,
        Some('\u{85}') => 2,
        Some('\u{2028}') | Some('\u{2029}') => 3,
        _ => 0,
    }
}

impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.text() == other.text()
    }
}
impl Eq for Document {}

impl PartialOrd for Document {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.text().cmp(&other.text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_exact_bytes_newlines_and_missing_final_newline() {
        let text = "a\r\nb\nc\rd\u{2028}e";
        let doc = Document::from_text(text);
        assert_eq!(doc.text(), text);
        assert_eq!(doc.newline_style(), NewlineStyle::Mixed);
        assert!(!doc.had_final_newline());
        assert_eq!(doc.to_bytes(NewlineMode::Preserve), text.as_bytes());
    }

    #[test]
    fn crlf_intermediate_line_ranges_are_not_inverted() {
        // ropey treats CR and LF as separate line boundaries. A CRLF pair
        // therefore creates an empty intermediate logical line; layout must
        // never compute a negative length from its byte range.
        let doc = Document::from_text("one\r\ntwo\r\n");
        for line in 0..doc.len_lines() {
            let range = doc.line_content_range(LineIndex(line));
            assert!(
                range.start <= range.end,
                "inverted range on line {line}: {range:?}"
            );
            let len = crate::app::layout::line_content_len(&doc, LineIndex(line));
            assert_eq!(len, range.end.get() - range.start.get());
        }
        let rows =
            crate::app::layout::build_layout(&doc, Default::default(), 20, 10, false, 4, true);
        assert!(!rows.is_empty());
    }

    #[test]
    fn refuses_invalid_utf8_and_nul() {
        assert!(Document::from_bytes(b"abc\xff").is_err());
        assert!(Document::from_bytes(b"abc\0def").is_err());
    }

    #[test]
    fn grapheme_boundaries_include_combining_and_zwj() {
        let text = "a\u{301}ب👩\u{200d}💻x";
        let doc = Document::from_text(text);
        let mut positions = vec![BytePos(0)];
        loop {
            let next = doc.next_grapheme(*positions.last().unwrap());
            if next == *positions.last().unwrap() {
                break;
            }
            positions.push(next);
            if next == doc.len_bytes() {
                break;
            }
        }
        assert_eq!(positions.last(), Some(&doc.len_bytes()));
        assert_eq!(doc.text_in_range(positions[0]..positions[1]), "a\u{301}");
        assert_eq!(
            doc.text_in_range(positions[2]..positions[3]),
            "👩\u{200d}💻"
        );
    }

    #[test]
    fn display_columns_are_not_byte_or_char_offsets() {
        let doc = Document::from_text("a界e\u{301}\t😀");
        let (col, grams) = doc.display_info(BytePos(0)..doc.len_bytes(), 4, false);
        assert_eq!(grams.get(), 5);
        assert_eq!(col.get(), 1 + 2 + 1 + 4 + 2);
    }

    #[test]
    fn line_window_handles_wide_chars_and_tabs_without_full_copy() {
        let doc = Document::from_text("ab\t界😀cd");
        let cells = doc.line_window(LineIndex(0), BytePos(0), 20, 4, false);
        assert_eq!(cells.iter().map(|c| c.width).sum::<usize>(), 10);
        assert!(cells.iter().all(|c| !c.text.contains('\t')));
    }

    #[test]
    fn explicit_line_ending_conversion_only() {
        let doc = Document::from_text("a\r\nb\nc\rd");
        assert_eq!(
            doc.convert_line_endings(NewlineMode::Preserve),
            "a\r\nb\nc\rd"
        );
        assert_eq!(doc.convert_line_endings(NewlineMode::Lf), "a\nb\nc\nd");
        assert_eq!(
            doc.convert_line_endings(NewlineMode::Crlf),
            "a\r\nb\r\nc\r\nd"
        );
    }
}
