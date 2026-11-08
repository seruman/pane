//! The highlight and marks, by identity. Port of `Sources/MenuCore/Selection.swift`.

use std::collections::HashSet;

use crate::item::MenuItem;

/// Filtering never silently changes marked items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSelection {
    pub highlighted_id: Option<String>,
    pub checked: HashSet<String>,
}
