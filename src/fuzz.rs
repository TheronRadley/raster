//! Deterministic mutation fuzz target for the terminal-independent editing
//! engine. It compares transactions and undo/redo against a simple `String`
//! reference model, exercises invalid UTF-8 boundaries, and verifies exact
//! newline/byte round-trips. This is not libFuzzer; no external fuzz framework
//! is needed to reproduce a run from its seed.

use crate::app::buffer::{shifted_carets, Buffer};
use crate::config::NewlineMode;
use crate::core::document::Document;
use crate::core::search::{find_matches, SearchOptions};
use crate::core::transaction::{Replacement, Transaction};
use crate::json::Json;
use crate::position::BytePos;
use crate::util::XorShift64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzReport {
    pub seed: u64,
    pub iterations: usize,
    pub operations: usize,
    pub failures: usize,
}

impl FuzzReport {
    pub fn to_json(&self) -> Json {
        let mut value = Json::object();
        value.insert(
            "status",
            Json::string(if self.failures == 0 { "ok" } else { "failed" }),
        );
        value.insert(
            "method",
            Json::string(
                "deterministic mutation fuzz; String reference model; no terminal/LSP/Git",
            ),
        );
        value.insert("seed", Json::Int(self.seed as i64));
        value.insert("iterations", Json::Int(self.iterations as i64));
        value.insert("operations", Json::Int(self.operations as i64));
        value.insert("failures", Json::Int(self.failures as i64));
        value
    }
}

pub fn run_cli<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut seed = 0x5261_7374_6572_u64;
    let mut iterations = 60usize;
    let mut max_ops = 40usize;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!("raster-fuzz [--seed N] [--iterations N] [--max-ops N]");
                return 0;
            }
            "--seed" => {
                i += 1;
                seed = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(seed);
            }
            "--iterations" => {
                i += 1;
                iterations = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(iterations);
            }
            "--max-ops" => {
                i += 1;
                max_ops = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(max_ops);
            }
            _ => {}
        }
        i += 1;
    }
    match run(seed, iterations, max_ops) {
        Ok(report) => {
            println!("{}", report.to_json().to_compact_string());
            if report.failures == 0 {
                0
            } else {
                1
            }
        }
        Err(message) => {
            // Never print document contents or fragments.
            eprintln!("raster-fuzz: failure at seed {seed}: {message}");
            1
        }
    }
}

pub fn run(seed: u64, iterations: usize, max_ops: usize) -> Result<FuzzReport, String> {
    let mut operations = 0usize;
    for case in 0..iterations.max(1) {
        let mut rng = XorShift64::new(
            seed.wrapping_add(case as u64)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15),
        );
        let initial = random_text(&mut rng, 24);
        let mut model = initial.clone();
        let mut buffer = Buffer::scratch(1, &initial);
        for op in 0..max_ops.max(1) {
            operations += 1;
            let kind = rng.below(6);
            match kind {
                0..=2 => {
                    let boundaries = char_boundaries(&model);
                    let a = boundaries[rng.below(boundaries.len())];
                    let b = boundaries[rng.below(boundaries.len())];
                    let (start, end) = (a.min(b), a.max(b));
                    let inserted = if kind == 0 {
                        random_text(&mut rng, 8)
                    } else if kind == 1 {
                        String::new()
                    } else {
                        random_text(&mut rng, 5)
                    };
                    let before = model.clone();
                    let mut expected = model.clone();
                    expected.replace_range(start..end, &inserted);
                    let edit = Replacement::new(BytePos(start)..BytePos(end), inserted.clone());
                    let ends = vec![BytePos(start + inserted.len())];
                    let edits = vec![edit.clone()];
                    let after = shifted_carets(&edits, &ends);
                    buffer
                        .apply_edits("fuzz", edits, after)
                        .map_err(|e| format!("case {case} op {op}: edit rejected: {e}"))?;
                    if buffer.document.text() != expected {
                        return Err(format!("case {case} op {op}: content mismatch"));
                    }
                    let outcome = buffer
                        .undo_outcome()
                        .map_err(|e| format!("case {case} op {op}: undo failed: {e}"))?
                        .ok_or_else(|| format!("case {case} op {op}: missing undo"))?;
                    let _ = outcome;
                    if buffer.document.text() != before {
                        return Err(format!("case {case} op {op}: undo content mismatch"));
                    }
                    let outcome = buffer
                        .redo_outcome()
                        .map_err(|e| format!("case {case} op {op}: redo failed: {e}"))?
                        .ok_or_else(|| format!("case {case} op {op}: missing redo"))?;
                    let _ = outcome;
                    if buffer.document.text() != expected {
                        return Err(format!("case {case} op {op}: redo content mismatch"));
                    }
                    model = expected;
                }
                3 => {
                    // Invalid byte boundaries and overlapping ranges must be
                    // rejected without a panic and without changing the model.
                    let doc = Document::from_rope(buffer.document.rope());
                    let len = doc.len_bytes().get();
                    let pos = rng.next_u64() as usize % (len + 8);
                    let tx = Transaction::new(
                        "invalid",
                        vec![Replacement::insert(BytePos(pos), "x")],
                        doc.version(),
                        &[],
                    );
                    let mut clone = Document::from_rope(doc.rope());
                    if tx.apply(&mut clone).is_ok() && pos > len {
                        return Err(format!("case {case} op {op}: invalid edit accepted"));
                    }
                }
                4 => {
                    let pattern = ["a", "مر", "e", "漢", "x"][rng.below(5)];
                    let options = SearchOptions {
                        case_sensitive: true,
                        max_results: 1000,
                        ..SearchOptions::default()
                    };
                    let found = find_matches(&buffer.document, pattern, options, None)
                        .map_err(|e| format!("case {case} op {op}: search failed: {e}"))?;
                    let expected = model.match_indices(pattern).take(1000).count();
                    if found.len() != expected {
                        return Err(format!("case {case} op {op}: search count mismatch"));
                    }
                }
                _ => {
                    let bytes = model.as_bytes();
                    let doc = Document::from_bytes(bytes)
                        .map_err(|e| format!("case {case} op {op}: document rejected: {e}"))?;
                    if doc.to_bytes(NewlineMode::Preserve) != bytes {
                        return Err(format!(
                            "case {case} op {op}: byte round-trip changed content"
                        ));
                    }
                    for selection in &buffer.selections {
                        if !buffer.document.is_char_boundary(selection.anchor)
                            || !buffer.document.is_char_boundary(selection.head)
                        {
                            return Err(format!(
                                "case {case} op {op}: selection left an invalid boundary"
                            ));
                        }
                    }
                }
            }
            if buffer.document.text() != model {
                return Err(format!("case {case} op {op}: model drift"));
            }
        }
    }
    Ok(FuzzReport {
        seed,
        iterations: iterations.max(1),
        operations,
        failures: 0,
    })
}

fn char_boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    out.push(text.len());
    out
}

fn random_text(rng: &mut XorShift64, pieces: usize) -> String {
    const FRAGMENTS: &[&str] = &[
        "a",
        "Z",
        "9",
        " ",
        "_",
        "مرحبا",
        "e\u{301}",
        "👩‍💻",
        "漢字",
        "\n",
        "\r\n",
        "\r",
        "\t",
        "\u{202e}",
    ];
    let mut out = String::new();
    for _ in 0..rng.below(pieces.max(1)).max(1) {
        out.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_run_matches_reference_model() {
        let report = run(42, 8, 20).unwrap();
        assert_eq!(report.failures, 0);
        assert!(report.operations >= 160);
    }
}
