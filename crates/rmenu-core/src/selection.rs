//! The highlight and marks, by identity. Port of `Sources/MenuCore/Selection.swift`.

use std::collections::HashSet;

use crate::item::MenuItem;
use crate::items::{Items, RowOffset};

/// Filtering never silently changes marked items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSelection {
    pub highlighted_id: Option<String>,
    pub checked: HashSet<String>,
}

impl MenuSelection {
    pub fn toggle(&mut self, id: &str) {
        if !self.checked.remove(id) {
            self.checked.insert(id.to_owned());
        }
    }

    /// `visible` are offsets into `items`. With `prune`, marks whose item is gone are dropped.
    /// A highlight that isn't visible moves to the first visible row.
    pub fn reconcile(&mut self, items: &Items, visible: &[RowOffset], prune: bool) {
        if prune && !self.checked.is_empty() {
            let retained: HashSet<String> = items.iter().filter(|i| self.checked.contains(&i.id)).map(|i| i.id.clone()).collect();
            self.checked = retained;
        }
        let shown = visible.iter().any(|&o| Some(&items[o].id) == self.highlighted_id.as_ref());
        if !shown {
            self.highlighted_id = visible.first().map(|&o| items[o].id.clone());
        }
    }

    pub fn marked(&self, items: &Items) -> Vec<MenuItem> {
        items.iter().filter(|i| self.checked.contains(&i.id)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_survives_filtering() {
        let items = Items::from(vec![MenuItem::new("a", "Same"), MenuItem::new("b", "Same")]);
        let mut selection = MenuSelection { highlighted_id: Some("a".into()), ..Default::default() };
        selection.toggle("b");
        selection.reconcile(&items, &[RowOffset(0)], false);
        assert_eq!(selection.marked(&items).iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["b"]);
        selection.reconcile(&Items::from(vec![items[0].clone()]), &[RowOffset(0)], true);
        assert!(selection.checked.is_empty());
        assert!(selection.marked(&items).is_empty());
        assert_eq!(selection.highlighted_id.as_deref(), Some("a"));
    }
}
