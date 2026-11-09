//! Rows and the context handed to commands. Port of `Sources/MenuCore/MenuItem.swift`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Values are data, never command fragments. Identity is scoped to a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub id: String,
    title: String,
    /// Set only when it differs from the title: a plain line's value is its title, and
    /// storing it twice would double a large input's memory.
    value: Option<String>,
    pub subtitle: Option<String>,
    pub search: Option<String>,
    pub detail: Option<String>,
}

impl MenuItem {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        MenuItem { id: id.into(), title: title.into(), value: None, subtitle: None, search: None, detail: None }
    }

    pub fn title(&self) -> &str { &self.title }

    /// What choosing this row returns; the title unless the record gave a value.
    pub fn value(&self) -> &str { self.value.as_deref().unwrap_or(&self.title) }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let value = value.into();
        self.value = (value != self.title).then_some(value);
        self
    }

    /// `search`, else the title and subtitle joined by a space.
    pub fn search_text(&self) -> String {
        match (&self.search, &self.subtitle) {
            (Some(search), _) => search.clone(),
            (None, Some(subtitle)) => format!("{} {subtitle}", self.title),
            (None, None) => self.title.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Wire<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<T>,
    id: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<T>,
    title: T,
    value: Option<T>,
}

impl Serialize for MenuItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Wire { id: self.id.as_str(), title: self.title(), value: Some(self.value()), subtitle: self.subtitle.as_deref(),
               search: self.search.as_deref(), detail: self.detail.as_deref() }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MenuItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let w = Wire::<String>::deserialize(deserializer)?;
        let item = MenuItem { id: w.id, title: w.title, value: None, subtitle: w.subtitle, search: w.search, detail: w.detail };
        Ok(match w.value { Some(value) => item.with_value(value), None => item })
    }
}

/// One level of a pushed menu: what was chosen there, root first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathEntry {
    pub items: Vec<MenuItem>,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// JSON on command stdin and `--json` stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionContext {
    pub action: String,
    pub items: Vec<MenuItem>,
    pub path: Vec<PathEntry>,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ActionContext {
    /// MMENU_ACTION, MMENU_QUERY, MMENU_SOURCE, MMENU_VALUE, MMENU_ID.
    pub fn environment(&self) -> Vec<(String, String)> {
        let first = self.items.first();
        [
            ("MMENU_ACTION", self.action.as_str()),
            ("MMENU_QUERY", self.query.as_str()),
            ("MMENU_SOURCE", self.source.as_deref().unwrap_or("")),
            ("MMENU_VALUE", first.map_or("", |i| i.value())),
            ("MMENU_ID", first.map_or("", |i| i.id.as_str())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    pub fn json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("an ActionContext is always representable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_sorts_keys_omits_absent_fields_and_keeps_values_as_data() {
        let mut item = MenuItem::new("1", "One").with_value("'; echo bad\n$(whoami) /path");
        item.subtitle = Some("sub".into());
        let context = ActionContext { action: "run".into(), query: "q".into(), source: None, items: vec![item.clone()], path: vec![] };
        let text = String::from_utf8(context.json()).unwrap();
        assert_eq!(text, r#"{"action":"run","items":[{"id":"1","subtitle":"sub","title":"One","value":"'; echo bad\n$(whoami) /path"}],"path":[],"query":"q"}"#);
        let back: ActionContext = serde_json::from_slice(&context.json()).unwrap();
        assert_eq!(back.items, vec![item]);
        assert_eq!(context.environment()[3], ("MMENU_VALUE".into(), "'; echo bad\n$(whoami) /path".into()));
    }

    #[test]
    fn wire_values_survive_storage_deduplication() {
        for value in [None, Some("T"), Some(""), Some("other")] {
            let mut input = serde_json::json!({"id": "a", "title": "T"});
            if let Some(value) = value { input["value"] = value.into(); }
            let item: MenuItem = serde_json::from_value(input).unwrap();
            let output = serde_json::to_value(&item).unwrap();
            assert_eq!(output, serde_json::json!({"id": "a", "title": "T", "value": value.unwrap_or("T")}));
            assert_eq!(serde_json::from_value::<MenuItem>(output).unwrap(), item);
        }
    }

    #[test]
    fn value_defaults_to_title_and_search_text_joins_the_subtitle() {
        let item: MenuItem = serde_json::from_str(r#"{"id":"a","title":"T","subtitle":"S"}"#).unwrap();
        assert_eq!(item.value(), "T");
        assert_eq!(item.search_text(), "T S");
        let searched: MenuItem = serde_json::from_str(r#"{"id":"a","title":"T","subtitle":"S","search":"x"}"#).unwrap();
        assert_eq!(searched.search_text(), "x");
    }
}
