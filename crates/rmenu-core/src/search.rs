//! fzf-ranked matching. Port of `Sources/MenuCore/SearchIndex.swift`, `FuzzyMatch.swift` and
//! `FzfNormalize.swift`.
//!
//! PLACEHOLDER: case-insensitive subsequence matching in input order, so other modules can be
//! built and tested against the real API. The search workstream replaces this file.

use std::sync::Arc;

use crate::item::MenuItem;

/// Prepared search text for a list of rows. Cloning is cheap (shared; append makes a new
/// generation), so a host can search a snapshot on another thread while input keeps arriving.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    texts: Arc<Vec<String>>,
}

/// Search was cancelled before finishing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl SearchIndex {
    pub fn new(items: &[MenuItem]) -> Self {
        let mut index = SearchIndex::default();
        index.append(items);
        index
    }

    pub fn len(&self) -> usize { self.texts.len() }

    pub fn is_empty(&self) -> bool { self.texts.is_empty() }

    pub fn append(&mut self, items: &[MenuItem]) {
        Arc::make_mut(&mut self.texts).extend(items.iter().map(|i| i.search_text().to_lowercase()));
    }

    /// Offsets of matching rows. Whitespace-separated tokens are ANDed; sorted follows
    /// `fzf --filter`'s default scheme (summed score, then shorter text, then input order).
    /// `cancelled` is polled every few hundred rows.
    pub fn search(&self, query: &str, _sorted: bool, cancelled: &dyn Fn() -> bool) -> Result<Vec<u32>, Cancelled> {
        let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut out = Vec::new();
        for (i, text) in self.texts.iter().enumerate() {
            if i % 256 == 0 && cancelled() { return Err(Cancelled); }
            let subsequence = |t: &str| { let mut c = text.chars(); t.chars().all(|q| c.any(|x| x == q)) };
            if tokens.iter().all(|t| subsequence(t)) { out.push(i as u32); }
        }
        Ok(out)
    }
}
