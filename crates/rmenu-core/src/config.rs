//! The `--config` JSON. Port of `Sources/MenuCore/Configuration.swift`.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputFormat {
    #[default]
    Lines,
    Jsonl,
    Nul,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkScope {
    #[default]
    Level,
    Global,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuConfiguration {
    pub prompt: Option<String>,
    pub multi: Option<bool>,
    pub theme: Option<String>,
    #[serde(default)]
    pub sources: Vec<MenuSource>,
    #[serde(default)]
    pub bindings: Vec<MenuBinding>,
    pub preview: Option<Vec<String>>,
    pub initial_source: Option<String>,
    #[serde(default)]
    pub marks: MarkScope,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MenuSource {
    pub id: String,
    pub label: String,
    pub key: Option<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub format: InputFormat,
    pub prompt: Option<String>,
    pub preview: Option<Vec<String>>,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BindingAction {
    Accept,
    Copy,
    Run,
    Reload,
    Source,
    Push,
    TogglePreview,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MenuBinding {
    pub key: String,
    pub id: Option<String>,
    pub label: Option<String>,
    pub action: BindingAction,
    pub command: Option<Vec<String>>,
    pub source: Option<String>,
    pub reload: Option<bool>,
    pub sources: Option<Vec<String>>,
}

/// mmenu's messages, verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationError(pub String);

/// A key and its modifiers. `cmd` parses and validates as in mmenu, but a terminal never
/// delivers Command, so a cmd chord can't fire in rmenu (see README).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Lowercased: one character, or enter/escape/tab/up/down/space.
    pub key: String,
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}
