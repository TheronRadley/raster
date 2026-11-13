//! Literal and regex search, replacement, and fuzzy scoring.
//!
//! Search is line-scoped: a pattern cannot match across a line break. This is
//! documented because it keeps a bounded working set and makes result ranges
//! predictable. `regex-lite` provides bounded-time regex matching; its Unicode
//! limitations are documented in `docs/KNOWN_LIMITATIONS.md`.

use std::sync::atomic::{AtomicBool, Ordering};

use regex_lite::{escape, Regex, RegexBuilder};

use crate::core::document::Document;
use crate::core::selection::Selection;
use crate::core::transaction::{Replacement, Transaction};
use crate::position::{BytePos, LineIndex};
use crate::util::sanitize_display;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum SearchMode {
    Literal,
    Regex,
}

impl SearchMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Regex => "regex",
        }
    }
    pub fn toggle(self) -> Self {
        match self {
            Self::Literal => Self::Regex,
            Self::Regex => Self::Literal,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SearchOptions {
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub max_results: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            mode: SearchMode::Literal,
            case_sensitive: false,
            max_results: 500,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum SearchScope {
    Buffer,
    Selection(Selection),
}

impl SearchScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Buffer => "buffer",
            Self::Selection(_) => "selection",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchError(pub String);

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SearchError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: BytePos,
    pub end: BytePos,
    pub line: LineIndex,
    pub column: usize,
    pub preview: String,
}

pub struct CompiledPattern {
    regex: Regex,
    mode: SearchMode,
}

impl CompiledPattern {
    pub fn new(pattern: &str, options: SearchOptions) -> Result<Self, SearchError> {
        let source = if options.mode == SearchMode::Literal {
            escape(pattern)
        } else {
            pattern.to_string()
        };
        let mut builder = RegexBuilder::new(&source);
        builder.case_insensitive(!options.case_sensitive);
        let regex = builder
            .build()
            .map_err(|e| SearchError(format!("invalid {}: {e}", options.mode.label())))?;
        Ok(Self {
            regex,
            mode: options.mode,
        })
    }

    pub fn mode(&self) -> SearchMode {
        self.mode
    }

    pub fn find_iter<'r, 'h>(&'r self, text: &'h str) -> regex_lite::Matches<'r, 'h> {
        self.regex.find_iter(text)
    }

    pub fn captures_iter<'r, 'h>(&'r self, text: &'h str) -> regex_lite::CaptureMatches<'r, 'h> {
        self.regex.captures_iter(text)
    }
}

pub fn find_matches(
    doc: &Document,
    pattern: &str,
    options: SearchOptions,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<Match>, SearchError> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let compiled = CompiledPattern::new(pattern, options)?;
    let mut out = Vec::new();
    for line in 0..doc.len_lines() {
        if line % 64 == 0 && cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            break;
        }
        let line_idx = LineIndex(line);
        let line_start = doc.line_content_range(line_idx).start;
        let text = doc.line_text(line_idx);
        for found in compiled.find_iter(&text) {
            if found.start() == found.end() {
                continue;
            }
            let start = BytePos(line_start.get() + found.start());
            let end = BytePos(line_start.get() + found.end());
            out.push(Match {
                start,
                end,
                line: line_idx,
                column: found.start(),
                preview: sanitize_display(&text),
            });
            if out.len() >= options.max_results {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

pub fn find_matches_in_text(
    text: &str,
    pattern: &str,
    options: SearchOptions,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<Match>, SearchError> {
    let doc = Document::from_text(text);
    let mut matches = find_matches(&doc, pattern, options, cancel)?;
    for m in &mut matches {
        let preview = m.preview.clone();
        let start = m.column.min(preview.len());
        let end = (m.column + 160).min(preview.len());
        m.preview = sanitize_display(&preview[start..end]);
    }
    Ok(matches)
}

/// Replace every match in the explicit scope as one undoable transaction.
/// Regex replacement supports `$0`, `$1`.. and `${name}` expansion; literal
/// replacement inserts the template verbatim.
pub fn replace_all(
    doc: &Document,
    pattern: &str,
    template: &str,
    options: SearchOptions,
    scope: SearchScope,
    selections: &[Selection],
) -> Result<(Transaction, usize), SearchError> {
    let compiled = CompiledPattern::new(pattern, options)?;
    let range = match scope {
        SearchScope::Buffer => BytePos(0)..doc.len_bytes(),
        SearchScope::Selection(sel) => sel.range(),
    };
    let mut edits = Vec::new();
    for line in 0..doc.len_lines() {
        let line_idx = LineIndex(line);
        let content = doc.line_content_range(line_idx);
        let text = doc.text_in_range(content.clone());
        for captures in compiled.captures_iter(&text) {
            let whole = captures
                .get(0)
                .ok_or_else(|| SearchError("missing whole match".into()))?;
            let abs_start = BytePos(content.start.get() + whole.start());
            let abs_end = BytePos(content.start.get() + whole.end());
            if abs_start < range.start || abs_end > range.end || whole.start() == whole.end() {
                continue;
            }
            let mut replacement = String::new();
            if options.mode == SearchMode::Regex {
                captures.expand(template, &mut replacement);
            } else {
                replacement.push_str(template);
            }
            edits.push(Replacement::new(abs_start..abs_end, replacement));
        }
    }
    // Descending replacement ordering is deterministic and avoids offset shifts.
    edits.sort_by(|a, b| (b.start, b.end, &b.text).cmp(&(a.start, a.end, &a.text)));
    let count = edits.len();
    Ok((
        Transaction::new("replace-all", edits, doc.version(), selections),
        count,
    ))
}

/// Fuzzy subsequence score. Higher is better; `None` means no match.
/// Bonuses reward consecutive matches, word starts, path boundaries, and early
/// matches, while gaps are penalized.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let q: Vec<char> = query.to_ascii_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let c: Vec<char> = candidate.chars().collect();
    let mut score: i64 = 0;
    let mut qi = 0usize;
    let mut last_match: Option<usize> = None;
    for (ci, ch) in c.iter().enumerate() {
        let lower = ch.to_ascii_lowercase();
        if qi < q.len() && lower == q[qi] {
            score += 1;
            if last_match == Some(ci.saturating_sub(1)) {
                score += 4;
            }
            let prev = if ci == 0 {
                None
            } else {
                c.get(ci - 1).copied()
            };
            if ci == 0
                || prev
                    .map(|p| matches!(p, '/' | '\\' | '.' | '-' | '_' | ' '))
                    .unwrap_or(false)
            {
                score += 6;
            }
            if ch.is_uppercase() {
                score += 1;
            }
            qi += 1;
            last_match = Some(ci);
            if qi == q.len() {
                score -= (c.len().saturating_sub(ci + 1) as i64) / 4;
                return Some(score);
            }
        } else if last_match.is_some() {
            score -= 1;
        }
    }
    None
}

pub fn fuzzy_filter(query: &str, candidates: &[String]) -> Vec<(String, i64)> {
    let mut scored: Vec<(String, i64)> = candidates
        .iter()
        .filter_map(|c| fuzzy_score(query, c).map(|score| (c.clone(), score)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_and_regex_modes_are_explicit() {
        let doc = Document::from_text("a.b\naxb\n12\nمرحبا 12");
        let literal = SearchOptions {
            mode: SearchMode::Literal,
            case_sensitive: true,
            max_results: 50,
        };
        let regex = SearchOptions {
            mode: SearchMode::Regex,
            case_sensitive: true,
            max_results: 50,
        };
        assert_eq!(find_matches(&doc, "a.b", literal, None).unwrap().len(), 1);
        assert_eq!(find_matches(&doc, "a.b", regex, None).unwrap().len(), 2);
        assert_eq!(find_matches(&doc, r"\d+", regex, None).unwrap().len(), 2);
        assert_eq!(find_matches(&doc, "12", literal, None).unwrap().len(), 2);
    }

    #[test]
    fn replace_all_uses_one_transaction_and_respects_selection_scope() {
        let mut doc = Document::from_text("one one\none one\n");
        let opts = SearchOptions {
            mode: SearchMode::Literal,
            case_sensitive: true,
            max_results: 100,
        };
        let (tx, count) = replace_all(&doc, "one", "X", opts, SearchScope::Buffer, &[]).unwrap();
        assert_eq!(count, 4);
        let (_, inverse) = tx.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), "X X\nX X\n");
        inverse.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), "one one\none one\n");
        let scope = SearchScope::Selection(Selection::new(BytePos(0), BytePos(7)));
        let (_tx, count) = replace_all(&doc, "one", "X", opts, scope, &[]).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn regex_capture_expansion_and_unicode() {
        let mut doc = Document::from_text("مرحبا 12 و 34");
        let opts = SearchOptions {
            mode: SearchMode::Regex,
            case_sensitive: true,
            max_results: 100,
        };
        let (tx, count) =
            replace_all(&doc, r"(\d+)", "[$1]", opts, SearchScope::Buffer, &[]).unwrap();
        assert_eq!(count, 2);
        tx.apply(&mut doc).unwrap();
        assert_eq!(doc.text(), "مرحبا [12] و [34]");
    }

    #[test]
    fn invalid_regex_is_reported() {
        let doc = Document::from_text("x");
        let opts = SearchOptions {
            mode: SearchMode::Regex,
            ..SearchOptions::default()
        };
        assert!(find_matches(&doc, "(", opts, None).is_err());
    }

    #[test]
    fn fuzzy_matching_prefers_path_boundaries() {
        assert!(fuzzy_score("main", "src/main.rs") > fuzzy_score("main", "domain_name.rs"));
        assert_eq!(fuzzy_score("zzz", "src/main.rs"), None);
        assert!(!fuzzy_filter("mn", &["src/main.rs".into(), "notes.md".into()]).is_empty());
    }
}
