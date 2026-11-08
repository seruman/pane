//! fzf-ranked matching. Port of `Sources/MenuCore/SearchIndex.swift`, `FuzzyMatch.swift` and
//! `FzfNormalize.swift`.

use std::sync::Arc;

use crate::item::MenuItem;

/// Prepared search text for a list of rows. Cloning is cheap (shared, append makes a new
/// generation), so a host can search a snapshot on another thread while input keeps arriving.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    _todo: Arc<()>,
}

/// Search was cancelled before finishing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl SearchIndex {
    pub fn new(items: &[MenuItem]) -> Self { todo!() }
    pub fn len(&self) -> usize { todo!() }
    pub fn append(&mut self, items: &[MenuItem]) { todo!() }
    /// Offsets of matching rows. Whitespace-separated tokens are ANDed; sorted follows
    /// `fzf --filter`'s default scheme (summed score, then shorter text, then input order).
    /// `cancelled` is polled every few hundred rows.
    pub fn search(&self, query: &str, sorted: bool, cancelled: &dyn Fn() -> bool) -> Result<Vec<u32>, Cancelled> { todo!() }
}
