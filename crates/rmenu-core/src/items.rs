use std::ops::Index;
use std::sync::Arc;

use crate::item::MenuItem;
use nucleo_matcher::Utf32String;

pub(crate) const CHUNK: usize = 8_192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowOffset(pub(crate) u32);

impl TryFrom<usize> for RowOffset {
    type Error = std::num::TryFromIntError;
    fn try_from(offset: usize) -> Result<Self, Self::Error> {
        u32::try_from(offset).map(Self)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    items: Vec<MenuItem>,
    pub(crate) text: Vec<Utf32String>,
}

/// Rows and prepared search text share chunks of at most 8,192 rows. Cloning is O(1).
/// Appending copies the chunk-pointer vector if a snapshot shares it.
/// Appending also copies the final partial chunk if a snapshot shares it, but retains full chunks.
/// Without shared snapshots, append work is amortized linear in the row count and prepared text size.
/// Arbitrary snapshot frequency adds pointer-vector and partial-chunk copying costs.
#[derive(Clone, Debug, Default)]
pub struct Items {
    pub(crate) chunks: Arc<Vec<Arc<Chunk>>>,
    pub(crate) len: usize,
}

impl Items {
    pub fn extend(&mut self, items: impl IntoIterator<Item = MenuItem>) {
        let chunks = Arc::make_mut(&mut self.chunks);
        for item in items {
            if chunks.last().is_none_or(|c| c.items.len() == CHUNK) {
                chunks.push(Arc::new(Chunk { items: Vec::new(), text: Vec::new() }));
            }
            let chunk = Arc::make_mut(chunks.last_mut().expect("pushed above"));
            chunk.text.push(Utf32String::from(item.search_text()));
            chunk.items.push(item);
            self.len += 1;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &MenuItem> {
        self.chunks.iter().flat_map(|c| c.items.iter())
    }
}

impl From<Vec<MenuItem>> for Items {
    fn from(items: Vec<MenuItem>) -> Self {
        let mut out = Items::default();
        out.extend(items);
        out
    }
}

impl Index<RowOffset> for Items {
    type Output = MenuItem;
    fn index(&self, offset: RowOffset) -> &MenuItem {
        &self.chunks[offset.0 as usize / CHUNK].items[offset.0 as usize % CHUNK]
    }
}

#[cfg(test)]
impl Index<usize> for Items {
    type Output = MenuItem;
    fn index(&self, i: usize) -> &MenuItem {
        &self.chunks[i / CHUNK].items[i % CHUNK]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Order;

    fn titles(items: &Items) -> Vec<String> {
        items.iter().map(|i| i.title().to_owned()).collect()
    }

    #[test]
    fn appends_past_chunks_index_in_order_and_leave_snapshots_alone() {
        let mut items = Items::default();
        items.extend((0..CHUNK + 5).map(|i| MenuItem::new(i.to_string(), format!("t{i}"))));
        let snapshot = items.clone();
        items.extend([MenuItem::new("x", "late")]);
        assert_eq!(items.iter().count(), CHUNK + 6);
        assert_eq!(items[CHUNK + 5].title(), "late");
        assert_eq!(items[CHUNK].title(), format!("t{CHUNK}"));
        assert_eq!(snapshot.iter().count(), CHUNK + 5);
        assert_eq!(titles(&snapshot).last().unwrap(), &format!("t{}", CHUNK + 4));
    }

    #[test]
    fn row_offsets_reject_truncation() {
        assert_eq!(RowOffset::try_from(u32::MAX as usize), Ok(RowOffset(u32::MAX)));
        assert!(RowOffset::try_from(u32::MAX as usize + 1).is_err());
    }

    #[test]
    fn appends_keep_search_offsets_and_item_payloads_together() {
        let mut items = Items::default();
        items.extend((0..CHUNK - 1).map(|i| MenuItem::new(i.to_string(), "old")));
        let snapshot = items.clone();
        items.extend(
            (CHUNK - 1..CHUNK + 2).map(|i| MenuItem::new(i.to_string(), "new").with_value(format!("value-{i}"))),
        );
        let offsets = items.search("new", Order::Input, &|| false).unwrap();
        assert_eq!(offsets, [CHUNK - 1, CHUNK, CHUNK + 1].map(|offset| RowOffset::try_from(offset).unwrap()));
        for offset in offsets {
            assert_eq!(items[offset].value(), format!("value-{}", offset.0));
        }
        assert!(snapshot.search("new", Order::Input, &|| false).unwrap().is_empty());
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
