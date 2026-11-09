//! Selection tracks the highlight and marks by item identity.

use std::collections::HashSet;

use crate::item::MenuItem;
use crate::items::{Items, RowOffset};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSelection {
    highlight: Option<Highlight>,
    checked: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Highlight {
    id: String,
    row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marks {
    Keep,
    Prune,
}

impl MenuSelection {
    pub fn highlighted_row(&self) -> Option<usize> {
        self.highlight.as_ref().map(|highlight| highlight.row)
    }

    pub fn highlight(&mut self, row: usize, id: String) {
        self.highlight = Some(Highlight { id, row });
    }

    pub fn is_marked(&self, id: &str) -> bool {
        self.checked.contains(id)
    }

    pub fn marked_count(&self) -> usize {
        self.checked.len()
    }

    pub fn toggle(&mut self, id: &str) {
        if !self.checked.remove(id) {
            self.checked.insert(id.to_owned());
        }
    }

    /// Keeps the highlighted identity if visible, or highlights the first visible row.
    /// Clears the highlight when no rows are visible.
    /// `Marks::Keep` preserves all marks. `Marks::Prune` removes marks for items absent from `items`.
    pub fn reconcile(&mut self, items: &Items, visible: &[RowOffset], marks: Marks) {
        if marks == Marks::Prune && !self.checked.is_empty() {
            let retained: HashSet<String> =
                items.iter().filter(|i| self.checked.contains(&i.id)).map(|i| i.id.clone()).collect();
            self.checked = retained;
        }
        let row = self
            .highlight
            .as_ref()
            .and_then(|highlight| visible.iter().position(|&offset| items[offset].id == highlight.id))
            .or_else(|| (!visible.is_empty()).then_some(0));
        self.highlight = row.map(|row| Highlight { id: items[visible[row]].id.clone(), row });
    }

    pub fn marked(&self, items: &Items) -> Vec<MenuItem> {
        items.iter().filter(|i| self.checked.contains(&i.id)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_rows_keep_the_highlighted_identity() {
        let items = Items::from(vec![MenuItem::new("a", "A"), MenuItem::new("b", "B")]);
        let mut selection = MenuSelection::default();
        selection.highlight(1, "b".into());
        selection.reconcile(&items, &[RowOffset(1), RowOffset(0)], Marks::Keep);
        assert_eq!(selection.highlighted_row(), Some(0));
    }

    #[test]
    fn hidden_highlight_moves_to_the_first_visible_identity() {
        let items = Items::from(vec![MenuItem::new("a", "A"), MenuItem::new("b", "B")]);
        let mut selection = MenuSelection::default();
        selection.highlight(0, "b".into());
        selection.reconcile(&items, &[RowOffset(0)], Marks::Keep);
        assert_eq!(selection.highlighted_row(), Some(0));
        selection.reconcile(&items, &[RowOffset(1), RowOffset(0)], Marks::Keep);
        assert_eq!(selection.highlighted_row(), Some(1));
    }

    #[test]
    fn empty_visible_rows_clear_the_highlight() {
        let items = Items::from(vec![MenuItem::new("a", "A"), MenuItem::new("b", "B")]);
        let mut selection = MenuSelection::default();
        selection.highlight(1, "a".into());
        selection.reconcile(&items, &[], Marks::Keep);
        assert_eq!(selection.highlighted_row(), None);
    }

    #[test]
    fn selection_survives_filtering() {
        let items = Items::from(vec![MenuItem::new("a", "Same"), MenuItem::new("b", "Same")]);
        let mut selection = MenuSelection::default();
        selection.highlight(0, "a".into());
        selection.toggle("b");
        selection.reconcile(&items, &[RowOffset(0)], Marks::Keep);
        assert_eq!(selection.marked(&items).iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["b"]);
        selection.reconcile(&Items::from(vec![items[0].clone()]), &[RowOffset(0)], Marks::Prune);
        assert_eq!(selection.marked_count(), 0);
        assert!(selection.marked(&items).is_empty());
        assert_eq!(selection.highlighted_row(), Some(0));
    }
}
