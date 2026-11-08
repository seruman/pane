//! The `--config` JSON. Port of `Sources/MenuCore/Configuration.swift`.

use std::collections::HashSet;
use std::fmt;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationError(pub String);

impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

impl std::error::Error for ConfigurationError {}

fn error<T>(message: impl Into<String>) -> Result<T, ConfigurationError> { Err(ConfigurationError(message.into())) }

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

impl MenuConfiguration {
    pub const THEMES: [&str; 2] = ["native", "rex"];

    /// The configured theme with former names mapped to their replacement.
    pub fn theme_name(&self) -> Option<&str> {
        self.theme.as_deref().map(|theme| if theme == "warm" { "rex" } else { theme })
    }

    pub fn source(&self, id: Option<&str>) -> Option<&MenuSource> {
        self.sources.iter().find(|source| Some(source.id.as_str()) == id)
    }

    /// The root level's source; `None` means the root reads piped stdin. Hidden sources are
    /// push targets, so a config with only hidden sources still reads stdin.
    pub fn root_source(&self) -> Option<&str> {
        self.initial_source.as_deref().or_else(|| self.sources.iter().find(|s| !s.hidden).map(|s| s.id.as_str()))
    }

    pub fn validate(&self) -> Result<(), ConfigurationError> {
        if let Some(theme) = self.theme_name().filter(|theme| !Self::THEMES.contains(theme)) {
            return error(format!("Unknown theme: {theme}"));
        }
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for source in &self.sources {
            if source.id.is_empty() { return error("Source IDs cannot be empty."); }
            if !ids.insert(source.id.as_str()) { return error(format!("Duplicate source ID: {}", source.id)); }
            if source.command.is_empty() { return error(format!("Source {} needs an argv command.", source.id)); }
            if source.preview.as_ref().is_some_and(Vec::is_empty) {
                return error(format!("Source {} preview command cannot be empty.", source.id));
            }
            if let Some(key) = &source.key {
                if source.hidden { return error(format!("Hidden source {} cannot have a key.", source.id)); }
                add_key(key, &mut keys)?;
            }
        }
        if let Some(initial) = self.initial_source.as_deref().filter(|id| !ids.contains(id)) {
            return error(format!("Unknown initialSource: {initial}"));
        }
        if self.preview.as_ref().is_some_and(Vec::is_empty) { return error("Preview command cannot be empty."); }
        let mut binding_ids = HashSet::new();
        for binding in &self.bindings {
            let key = &binding.key;
            add_key(key, &mut keys)?;
            if let Some(id) = &binding.id {
                if id.is_empty() { return error(format!("Binding {key} has an empty ID.")); }
                if !binding_ids.insert(id.as_str()) { return error(format!("Duplicate binding ID: {id}")); }
            }
            if binding.action == BindingAction::Run && binding.command.as_ref().is_none_or(Vec::is_empty) {
                return error(format!("Run binding {key} needs a command."));
            }
            if matches!(binding.action, BindingAction::Source | BindingAction::Push) {
                match binding.source.as_deref() {
                    None => return error(format!("Binding {key} needs a source.")),
                    Some(source) if !ids.contains(source) =>
                        return error(format!("Binding {key} references unknown source {source}.")),
                    Some(_) => {}
                }
            }
            if binding.action == BindingAction::Source
                && self.source(binding.source.as_deref()).is_some_and(|s| s.hidden) {
                let source = binding.source.as_deref().unwrap_or("");
                return error(format!("Binding {key} cannot switch to hidden source {source}; push it instead."));
            }
            if let Some(levels) = &binding.sources {
                if levels.is_empty() { return error(format!("Binding {key} has an empty sources list.")); }
                if let Some(id) = levels.iter().find(|id| !ids.contains(id.as_str())) {
                    return error(format!("Binding {key} is restricted to unknown source {id}."));
                }
            }
        }
        Ok(())
    }
}

fn add_key(raw: &str, keys: &mut HashSet<KeyChord>) -> Result<(), ConfigurationError> {
    let chord = KeyChord::parse(raw)?;
    if chord.is_reserved() { return error(format!("{raw} is reserved for editing and navigation.")); }
    if !keys.insert(chord) { return error(format!("Duplicate binding: {raw}")); }
    Ok(())
}

impl MenuBinding {
    /// A binding restricted to sources never applies to piped stdin (`None`).
    pub fn applies(&self, source_id: Option<&str>) -> bool {
        match (&self.sources, source_id) {
            (None, _) => true,
            (Some(sources), Some(id)) => sources.iter().any(|s| s == id),
            (Some(_), None) => false,
        }
    }

    /// Reported as the context's `action` and MMENU_ACTION.
    pub fn action_name(&self) -> &str {
        self.id.as_deref().or(self.label.as_deref()).unwrap_or(&self.key)
    }
}

const NAMED_KEYS: [&str; 6] = ["enter", "escape", "tab", "up", "down", "space"];

impl KeyChord {
    /// `[cmd+][ctrl+][alt+][shift+]key` in any modifier order, case-insensitive. A single
    /// character key needs cmd, ctrl or alt.
    pub fn parse(raw: &str) -> Result<KeyChord, ConfigurationError> {
        let lowered = raw.to_lowercase();
        let parts: Vec<&str> = lowered.split('+').collect();
        let (key, modifiers) = parts.split_last().expect("split yields at least one part");
        if parts.contains(&"") || (key.chars().count() != 1 && !NAMED_KEYS.contains(key)) {
            return error(format!("Invalid key chord: {raw}"));
        }
        let unique: HashSet<_> = modifiers.iter().collect();
        if unique.len() != modifiers.len() || !modifiers.iter().all(|m| ["cmd", "ctrl", "alt", "shift"].contains(m)) {
            return error(format!("Use the cmd, ctrl, alt, or shift modifiers in {raw}."));
        }
        let has = |m: &str| modifiers.contains(&m);
        let chord = KeyChord { key: (*key).to_owned(), command: has("cmd"), control: has("ctrl"),
                               option: has("alt"), shift: has("shift") };
        if !chord.command && !chord.control && !chord.option && key.chars().count() == 1 {
            return error(format!("Bindings need a modifier: {raw}"));
        }
        Ok(chord)
    }

    /// Keys the input field and list keep for editing and navigation.
    pub fn is_reserved(&self) -> bool {
        let key = self.key.as_str();
        match (self.command, self.control, self.option) {
            (false, false, false) => true,
            (true, false, false) => ["a", "v", "x", "z"].contains(&key),
            (false, true, false) => ["n", "p", "c"].contains(&key),
            _ => false,
        }
    }

    pub fn display(&self) -> String {
        let key = match self.key.as_str() {
            "enter" => "↩".to_owned(),
            "space" => "Space".to_owned(),
            "tab" => "⇥".to_owned(),
            "up" => "↑".to_owned(),
            "down" => "↓".to_owned(),
            "escape" => "⎋".to_owned(),
            other => other.to_uppercase(),
        };
        let flag = |on: bool, symbol: &'static str| if on { symbol } else { "" };
        format!("{}{}{}{}{key}", flag(self.control, "⌃"), flag(self.option, "⌥"), flag(self.shift, "⇧"),
                flag(self.command, "⌘"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> MenuConfiguration { serde_json::from_str(json).unwrap() }

    #[test]
    fn minimal_configuration() {
        let config = parse("{}");
        config.validate().unwrap();
        assert!(config.sources.is_empty());
        assert_eq!(config.marks, MarkScope::Level);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        parse(r#"{"future":1,"sources":[{"id":"a","label":"A","command":["echo"],"x":true}]}"#).validate().unwrap();
    }

    #[test]
    fn invalid_configuration() {
        for json in [
            r#"{"bindings":[{"key":"cmd+a","action":"copy"}]}"#,
            r#"{"bindings":[{"key":"cmd+d","action":"run"}]}"#,
            r#"{"bindings":[{"key":"cmd+1","action":"source","source":"missing"}]}"#,
            r#"{"sources":[{"id":"a","label":"A","key":"cmd+1","command":["echo"]}],"bindings":[{"key":"cmd+1","action":"copy"}]}"#,
            r#"{"preview":[]}"#,
        ] {
            assert!(parse(json).validate().is_err(), "{json}");
        }
    }

    #[test]
    fn binding_ids_name_the_action_independently_of_labels() {
        let config = parse(r#"{"bindings":[{"key":"ctrl+v","id":"split-right","label":"Split right","action":"accept"},{"key":"cmd+d","label":"Star","action":"accept"},{"key":"cmd+e","action":"accept"}]}"#);
        config.validate().unwrap();
        assert_eq!(config.bindings.iter().map(MenuBinding::action_name).collect::<Vec<_>>(), ["split-right", "Star", "cmd+e"]);
        let duplicate = parse(r#"{"bindings":[{"key":"ctrl+v","id":"x","action":"accept"},{"key":"ctrl+s","id":"x","action":"accept"}]}"#);
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn chord_parsing() {
        assert_eq!(KeyChord::parse("shift+cmd+r"), KeyChord::parse("cmd+shift+r"));
        assert_eq!(KeyChord::parse("cmd+shift+r").unwrap().display(), "⇧⌘R");
        assert_eq!(KeyChord::parse("Ctrl+Enter").unwrap().display(), "⌃↩");
        for bad in ["cmd+cmd+r", "r", "cmd+", "cmd+foo", "hyper+r"] {
            assert!(KeyChord::parse(bad).is_err(), "{bad}");
        }
        assert!(KeyChord::parse("enter").unwrap().is_reserved());
        assert!(KeyChord::parse("ctrl+n").unwrap().is_reserved());
        assert!(KeyChord::parse("ctrl+shift+n").unwrap().is_reserved());
        assert!(!KeyChord::parse("ctrl+alt+n").unwrap().is_reserved());
        assert!(!KeyChord::parse("cmd+e").unwrap().is_reserved());
    }

    #[test]
    fn push_levels_decode() {
        let config = parse(r#"
        {"marks":"global","sources":[
          {"id":"repos","label":"Repositories","prompt":"Repos","command":["/bin/echo"]},
          {"id":"worktrees","label":"Worktrees","hidden":true,"preview":["/bin/echo","wt"],"command":["/bin/echo"],"format":"jsonl"}],
         "bindings":[{"key":"ctrl+w","id":"worktrees","label":"Worktrees","action":"push","source":"worktrees","sources":["repos"]},
                     {"key":"ctrl+v","id":"split","action":"accept"}]}"#);
        config.validate().unwrap();
        assert_eq!(config.marks, MarkScope::Global);
        let (repos, worktrees) = (&config.sources[0], &config.sources[1]);
        assert!(repos.prompt.as_deref() == Some("Repos") && repos.preview.is_none() && !repos.hidden);
        assert!(worktrees.hidden && worktrees.prompt.is_none() && worktrees.format == InputFormat::Jsonl);
        assert_eq!(worktrees.preview.as_deref(), Some(&["/bin/echo".to_owned(), "wt".to_owned()][..]));
        assert_eq!(config.bindings[0].action, BindingAction::Push);
        assert!(config.bindings[0].applies(Some("repos")) && !config.bindings[0].applies(Some("worktrees")));
        assert!(!config.bindings[0].applies(None), "restricted bindings do not apply to piped stdin");
        assert!(config.bindings[1].applies(Some("worktrees")) && config.bindings[1].applies(None));
        assert_eq!(config.root_source(), Some("repos"));
        assert_eq!(config.source(Some("worktrees")).map(|s| s.label.as_str()), Some("Worktrees"));
    }

    #[test]
    fn only_hidden_sources_means_the_root_reads_stdin() {
        let config = parse(r#"{"sources":[{"id":"w","label":"W","hidden":true,"command":["/bin/echo"]}],"bindings":[{"key":"ctrl+w","action":"push","source":"w"}]}"#);
        config.validate().unwrap();
        assert_eq!(config.root_source(), None);
        let initial = parse(r#"{"initialSource":"w","sources":[{"id":"w","label":"W","hidden":true,"command":["/bin/echo"]}]}"#);
        initial.validate().unwrap();
        assert_eq!(initial.root_source(), Some("w"));
    }

    #[test]
    fn invalid_push_configuration() {
        for json in [r#"{"marks":"shared"}"#, r#"{"sources":[{"id":"a","label":"A","hidden":"yes","command":["echo"]}]}"#] {
            assert!(serde_json::from_str::<MenuConfiguration>(json).is_err(), "{json}");
        }
        let source = r#"{"id":"a","label":"A","command":["echo"]}"#;
        let hidden = r#"{"id":"h","label":"H","hidden":true,"command":["echo"]}"#;
        for json in [
            format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"push"}}]}}"#),
            format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"push","source":"missing"}}]}}"#),
            format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"copy","sources":[]}}]}}"#),
            format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"copy","sources":["missing"]}}]}}"#),
            format!(r#"{{"sources":[{source},{hidden}],"bindings":[{{"key":"cmd+2","action":"source","source":"h"}}]}}"#),
            r#"{"sources":[{"id":"h","label":"H","hidden":true,"key":"cmd+2","command":["echo"]}]}"#.to_owned(),
            r#"{"sources":[{"id":"a","label":"A","preview":[],"command":["echo"]}]}"#.to_owned(),
            format!(r#"{{"sources":[{source},{source}]}}"#),
            format!(r#"{{"initialSource":"missing","sources":[{source}]}}"#),
        ] {
            assert!(parse(&json).validate().is_err(), "{json}");
        }
    }

    #[test]
    fn errors_name_the_rejected_item() {
        let source = r#"{"id":"a","label":"A","command":["echo"]}"#;
        for (json, message) in [
            (format!(r#"{{"sources":[{source},{source}]}}"#), "Duplicate source ID: a"),
            (r#"{"sources":[{"id":"","label":"A","command":["echo"]}]}"#.to_owned(), "Source IDs cannot be empty."),
            (format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"push","source":"b"}}]}}"#),
             "Binding ctrl+w references unknown source b."),
            (format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"source"}}]}}"#),
             "Binding ctrl+w needs a source."),
            (r#"{"bindings":[{"key":"ctrl+v","id":"","action":"accept"}]}"#.to_owned(), "Binding ctrl+v has an empty ID."),
            (r#"{"bindings":[{"key":"ctrl+n","action":"accept"}]}"#.to_owned(), "ctrl+n is reserved for editing and navigation."),
            (r#"{"bindings":[{"key":"hyper+r","action":"accept"}]}"#.to_owned(),
             "Use the cmd, ctrl, alt, or shift modifiers in hyper+r."),
            (r#"{"bindings":[{"key":"ctrl+v","id":"x","action":"accept"},{"key":"ctrl+s","id":"x","action":"accept"}]}"#.to_owned(),
             "Duplicate binding ID: x"),
        ] {
            assert_eq!(parse(&json).validate(), Err(ConfigurationError(message.into())), "{json}");
        }
    }

    #[test]
    fn former_warm_theme_name_still_validates() {
        for theme in ["rex", "native", "warm"] {
            parse(&format!(r#"{{"theme":"{theme}"}}"#)).validate().unwrap();
        }
        assert_eq!(parse(r#"{"theme":"warm"}"#).theme_name(), Some("rex"));
        assert!(parse(r#"{"theme":"sepia"}"#).validate().is_err());
    }
}
