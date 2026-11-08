//! Rows and the context handed to commands. Port of `Sources/MenuCore/MenuItem.swift`.

use serde::{Deserialize, Serialize};

/// Values are data, never command fragments. Identity is scoped to a source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub id: String,
    pub title: String,
    /// Defaults to the title.
    pub value: String,
    pub subtitle: Option<String>,
    pub search: Option<String>,
    pub detail: Option<String>,
}

impl MenuItem {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self { todo!() }
    /// `search`, else title and subtitle joined by a space.
    pub fn search_text(&self) -> String { todo!() }
}

/// One level of a pushed menu: what was chosen there, root first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathEntry {
    pub source: Option<String>,
    pub query: String,
    pub items: Vec<MenuItem>,
}

/// JSON on command stdin and `--json` stdout. Key order and omission of nil fields must match
/// mmenu's `JSONEncoder` output with `.sortedKeys, .withoutEscapingSlashes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionContext {
    pub action: String,
    pub query: String,
    pub source: Option<String>,
    pub items: Vec<MenuItem>,
    pub path: Vec<PathEntry>,
}

impl ActionContext {
    /// MMENU_ACTION, MMENU_QUERY, MMENU_SOURCE, MMENU_VALUE, MMENU_ID.
    pub fn environment(&self) -> Vec<(String, String)> { todo!() }
    /// Byte-for-byte what mmenu writes, without a trailing newline.
    pub fn json(&self) -> Vec<u8> { todo!() }
}
