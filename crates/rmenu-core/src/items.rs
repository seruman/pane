//! The rows of one source generation, in input order.

use std::ops::Index;
use std::sync::Arc;

use crate::item::MenuItem;

const CHUNK: usize = 8_192;

/// Rows in shared fixed-size chunks. Cloning copies one pointer, and appending while a clone
/// is alive (a search running over a snapshot) copies at most the last, partial chunk, so
/// streaming N rows costs O(N) however many snapshots are taken along the way.
#[derive(Clone, Debug, Default)]
pub struct Items {
    chunks: Arc<Vec<Arc<Vec<MenuItem>>>>,
    len: usize,
}

impl Items {
    pub fn len(&self) -> usize { self.len }

    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn extend(&mut self, items: impl IntoIterator<Item = MenuItem>) {
        let chunks = Arc::make_mut(&mut self.chunks);
        for item in items {
            if chunks.last().is_none_or(|c| c.len() == CHUNK) {
                chunks.push(Arc::new(Vec::with_capacity(CHUNK)));
            }
            Arc::make_mut(chunks.last_mut().expect("pushed above")).push(item);
            self.len += 1;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &MenuItem> { self.chunks.iter().flat_map(|c| c.iter()) }
}

impl From<Vec<MenuItem>> for Items {
    fn from(items: Vec<MenuItem>) -> Self {
        let mut out = Items::default();
        out.extend(items);
        out
    }
}

impl Index<usize> for Items {
    type Output = MenuItem;
    fn index(&self, i: usize) -> &MenuItem { &self.chunks[i / CHUNK][i % CHUNK] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(items: &Items) -> Vec<String> { items.iter().map(|i| i.title.clone()).collect() }

    #[test]
    fn appends_past_chunks_index_in_order_and_leave_snapshots_alone() {
        let mut items = Items::default();
        items.extend((0..CHUNK + 5).map(|i| MenuItem::new(i.to_string(), format!("t{i}"))));
        let snapshot = items.clone();
        items.extend([MenuItem::new("x", "late")]);
        assert_eq!(items.len(), CHUNK + 6);
        assert_eq!(snapshot.len(), CHUNK + 5);
        assert_eq!(items[CHUNK + 5].title, "late");
        assert_eq!(items[CHUNK].title, format!("t{CHUNK}"));
        assert_eq!(snapshot.iter().count(), CHUNK + 5);
        assert_eq!(titles(&snapshot).last().unwrap(), &format!("t{}", CHUNK + 4));
    }

    #[test]
    fn appending_with_a_snapshot_alive_shares_full_chunks() {
        let mut items = Items::from((0..CHUNK * 3).map(|i| MenuItem::new(i.to_string(), "t")).collect::<Vec<_>>());
        let snapshot = items.clone();
        items.extend([MenuItem::new("x", "late")]);
        for c in 0..3 {
            assert!(Arc::ptr_eq(&items.chunks[c], &snapshot.chunks[c]), "chunk {c} was copied");
        }
    }
}
