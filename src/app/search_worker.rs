//! Cancellable buffer search worker. Results carry the document version and a
//! generation; the editor discards anything older than the current state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use crate::core::document::Document;
use crate::core::search::{find_matches, Match, SearchOptions};

#[derive(Clone, Debug)]
pub struct SearchOutcome {
    pub generation: u64,
    pub version: u64,
    pub matches: Vec<Match>,
    pub canceled: bool,
    pub error: Option<String>,
}

pub struct BufferSearchHandle {
    pub generation: u64,
    pub version: u64,
    pub rx: Receiver<SearchOutcome>,
    pub cancel: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
}

impl BufferSearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

pub fn start(
    document: Document,
    pattern: String,
    options: SearchOptions,
    generation: u64,
    version: u64,
) -> BufferSearchHandle {
    let (tx, rx) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);
    let done_thread = Arc::clone(&done);
    std::thread::Builder::new()
        .name("raster-buffer-search".into())
        .spawn(move || {
            let result = find_matches(&document, &pattern, options, Some(&cancel_thread));
            let (matches, error) = match result {
                Ok(matches) => (matches, None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            };
            let _ = tx.send(SearchOutcome {
                generation,
                version,
                matches,
                canceled: cancel_thread.load(Ordering::Relaxed),
                error,
            });
            done_thread.store(true, Ordering::Relaxed);
        })
        .ok();
    BufferSearchHandle {
        generation,
        version,
        rx,
        cancel,
        done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn search_result_carries_version_and_generation() {
        let doc = Document::from_text("needle\nnothing\nneedle\n");
        let handle = start(doc, "needle".into(), SearchOptions::default(), 7, 3);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(outcome) = handle.rx.recv_timeout(Duration::from_millis(20)) {
                assert_eq!(outcome.generation, 7);
                assert_eq!(outcome.version, 3);
                assert_eq!(outcome.matches.len(), 2);
                return;
            }
        }
        panic!("search worker did not report");
    }
}
