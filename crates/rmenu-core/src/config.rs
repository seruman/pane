use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[serde(alias = "warm")]
    Rex,
    Native,
}

impl std::str::FromStr for Theme {
    type Err = ConfigurationError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "rex" => Ok(Self::Rex),
            "native" => Ok(Self::Native),
            _ => Err(ConfigurationError::UnknownTheme(raw.into())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(String);

impl SourceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(try_from = "RawConfiguration")]
pub struct MenuConfiguration {
    prompt: Option<String>,
    multi: bool,
    sources: Vec<MenuSource>,
    bindings: Vec<MenuBinding>,
    preview: Option<Vec<String>>,
    initial_source: Option<SourceId>,
    marks: MarkScope,
}

#[derive(Clone, Debug)]
pub struct MenuSource {
    pub id: SourceId,
    pub label: String,
    pub key: Option<KeyChord>,
    pub command: Vec<String>,
    pub format: InputFormat,
    pub prompt: Option<String>,
    pub preview: Option<Vec<String>>,
    pub hidden: bool,
    raw_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingAction {
    Accept,
    Copy,
    Run { command: Vec<String>, reload: bool },
    Reload,
    Source { source: SourceId },
    Push { source: SourceId },
    TogglePreview,
}

#[derive(Clone, Debug)]
pub struct MenuBinding {
    pub key: KeyChord,
    pub label: Option<String>,
    pub action: BindingAction,
    id: Option<String>,
    raw_key: String,
    sources: Option<Vec<SourceId>>,
}

#[derive(Debug)]
pub enum ConfigurationError {
    Json(serde_json::Error),
    UnknownTheme(String),
    EmptySourceId,
    DuplicateSourceId(String),
    MissingProgram { entry: String },
    CommandNul { entry: String },
    HiddenSourceKey(String),
    UnknownInitialSource(String),
    EmptyBindingId { key: String },
    DuplicateBindingId(String),
    MissingSource { key: String },
    UnknownSource { key: String, source: String },
    HiddenSourceTarget { key: String, source: String },
    EmptySources { key: String },
    UnknownRestriction { key: String, source: String },
    InvalidChord(String),
    InvalidModifiers(String),
    MissingModifier(String),
    ReservedChord(String),
    DuplicateChord(String),
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::UnknownTheme(theme) => write!(f, "Unknown theme: {theme}"),
            Self::EmptySourceId => f.write_str("Source IDs cannot be empty."),
            Self::DuplicateSourceId(id) => write!(f, "Duplicate source ID: {id}"),
            Self::MissingProgram { entry } => write!(f, "{entry} needs a program to run."),
            Self::CommandNul { entry } => write!(f, "{entry} contains a NUL character."),
            Self::HiddenSourceKey(id) => write!(f, "Hidden source {id} cannot have a key."),
            Self::UnknownInitialSource(id) => write!(f, "Unknown initialSource: {id}"),
            Self::EmptyBindingId { key } => write!(f, "Binding {key} has an empty ID."),
            Self::DuplicateBindingId(id) => write!(f, "Duplicate binding ID: {id}"),
            Self::MissingSource { key } => write!(f, "Binding {key} needs a source."),
            Self::UnknownSource { key, source } => write!(f, "Binding {key} references unknown source {source}."),
            Self::HiddenSourceTarget { key, source } => {
                write!(f, "Binding {key} cannot switch to hidden source {source}. Push it instead.")
            }
            Self::EmptySources { key } => write!(f, "Binding {key} has an empty sources list."),
            Self::UnknownRestriction { key, source } => {
                write!(f, "Binding {key} is restricted to unknown source {source}.")
            }
            Self::InvalidChord(raw) => write!(f, "Invalid key chord: {raw}"),
            Self::InvalidModifiers(raw) => write!(f, "Use the cmd, ctrl, alt, or shift modifiers in {raw}."),
            Self::MissingModifier(raw) => write!(f, "Bindings need a modifier: {raw}"),
            Self::ReservedChord(raw) => write!(f, "{raw} is reserved for editing and navigation."),
            Self::DuplicateChord(raw) => write!(f, "Duplicate binding: {raw}"),
        }
    }
}

impl std::error::Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfiguration {
    prompt: Option<String>,
    #[serde(default, deserialize_with = "null_as_false")]
    multi: bool,
    #[serde(rename = "theme")]
    _theme: Option<Theme>,
    #[serde(default)]
    sources: Vec<RawSource>,
    #[serde(default)]
    bindings: Vec<RawBinding>,
    preview: Option<Vec<String>>,
    initial_source: Option<String>,
    #[serde(default)]
    marks: MarkScope,
}

#[derive(Deserialize)]
struct RawSource {
    id: String,
    label: String,
    key: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    format: InputFormat,
    prompt: Option<String>,
    preview: Option<Vec<String>>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct RawBinding {
    key: String,
    id: Option<String>,
    label: Option<String>,
    #[serde(flatten)]
    action: RawAction,
    sources: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
enum RawAction {
    Accept,
    Copy,
    Run {
        command: Option<Vec<String>>,
        #[serde(default, deserialize_with = "null_as_false")]
        reload: bool,
    },
    Reload,
    Source {
        source: Option<String>,
    },
    Push {
        source: Option<String>,
    },
    TogglePreview,
}

fn null_as_false<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: String,
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}

impl MenuConfiguration {
    pub fn parse(bytes: &[u8], preview_override: Option<Vec<String>>) -> Result<Self, ConfigurationError> {
        let mut raw: RawConfiguration = serde_json::from_slice(bytes).map_err(ConfigurationError::Json)?;
        if let Some(preview) = preview_override {
            raw.preview = Some(preview);
        }
        raw.try_into()
    }

    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }
    pub fn multi(&self) -> bool {
        self.multi
    }
    pub fn sources(&self) -> &[MenuSource] {
        &self.sources
    }
    pub fn bindings(&self) -> &[MenuBinding] {
        &self.bindings
    }
    pub fn preview(&self) -> Option<&[String]> {
        self.preview.as_deref()
    }
    pub fn marks(&self) -> MarkScope {
        self.marks
    }

    pub fn source(&self, id: &SourceId) -> Option<&MenuSource> {
        self.sources.iter().find(|source| &source.id == id)
    }

    pub fn root_source(&self) -> Option<&MenuSource> {
        match &self.initial_source {
            Some(id) => self.source(id),
            None => self.sources.iter().find(|s| !s.hidden),
        }
    }

    pub fn command_chords(&self) -> impl Iterator<Item = &str> {
        self.sources
            .iter()
            .filter(|s| s.key.as_ref().is_some_and(|key| key.command))
            .filter_map(|s| s.raw_key.as_deref())
            .chain(self.bindings.iter().filter(|b| b.key.command).map(|b| b.raw_key.as_str()))
    }
}

impl TryFrom<RawConfiguration> for MenuConfiguration {
    type Error = ConfigurationError;

    fn try_from(raw: RawConfiguration) -> Result<Self, Self::Error> {
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        let sources =
            raw.sources.into_iter().map(|source| source.parse(&mut ids, &mut keys)).collect::<Result<Vec<_>, _>>()?;
        let initial_source = raw
            .initial_source
            .map(|id| {
                sources
                    .iter()
                    .find(|s| s.id.as_str() == id)
                    .map(|s| s.id.clone())
                    .ok_or(ConfigurationError::UnknownInitialSource(id))
            })
            .transpose()?;
        if let Some(preview) = &raw.preview {
            check_argv(preview, "Preview command".into())?;
        }
        let mut binding_ids = HashSet::new();
        let bindings = raw
            .bindings
            .into_iter()
            .map(|binding| binding.parse(&sources, &mut binding_ids, &mut keys))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            prompt: raw.prompt,
            multi: raw.multi,
            sources,
            bindings,
            preview: raw.preview,
            initial_source,
            marks: raw.marks,
        })
    }
}

impl RawSource {
    fn parse(self, ids: &mut HashSet<String>, keys: &mut HashSet<KeyChord>) -> Result<MenuSource, ConfigurationError> {
        if self.id.is_empty() {
            return Err(ConfigurationError::EmptySourceId);
        }
        if !ids.insert(self.id.clone()) {
            return Err(ConfigurationError::DuplicateSourceId(self.id));
        }
        check_argv(&self.command, format!("Source {} command", self.id))?;
        if let Some(preview) = &self.preview {
            check_argv(preview, format!("Source {} preview command", self.id))?;
        }
        let key = self
            .key
            .as_ref()
            .map(|raw| {
                if self.hidden {
                    return Err(ConfigurationError::HiddenSourceKey(self.id.clone()));
                }
                add_key(raw, keys)
            })
            .transpose()?;
        Ok(MenuSource {
            id: SourceId(self.id),
            label: self.label,
            key,
            raw_key: self.key,
            command: self.command,
            format: self.format,
            prompt: self.prompt,
            preview: self.preview,
            hidden: self.hidden,
        })
    }
}

impl RawBinding {
    fn parse(
        self,
        sources: &[MenuSource],
        ids: &mut HashSet<String>,
        keys: &mut HashSet<KeyChord>,
    ) -> Result<MenuBinding, ConfigurationError> {
        let key = add_key(&self.key, keys)?;
        if let Some(id) = &self.id {
            if id.is_empty() {
                return Err(ConfigurationError::EmptyBindingId { key: self.key });
            }
            if !ids.insert(id.clone()) {
                return Err(ConfigurationError::DuplicateBindingId(id.clone()));
            }
        }
        let target = |id: Option<String>| -> Result<&MenuSource, ConfigurationError> {
            let id = id.ok_or_else(|| ConfigurationError::MissingSource { key: self.key.clone() })?;
            sources
                .iter()
                .find(|s| s.id.as_str() == id)
                .ok_or_else(|| ConfigurationError::UnknownSource { key: self.key.clone(), source: id })
        };
        let action = match self.action {
            RawAction::Accept => BindingAction::Accept,
            RawAction::Copy => BindingAction::Copy,
            RawAction::Reload => BindingAction::Reload,
            RawAction::TogglePreview => BindingAction::TogglePreview,
            RawAction::Run { command, reload } => {
                let command = command.unwrap_or_default();
                check_argv(&command, format!("Run binding {} command", self.key))?;
                BindingAction::Run { command, reload }
            }
            RawAction::Source { source } => {
                let source = target(source)?;
                if source.hidden {
                    return Err(ConfigurationError::HiddenSourceTarget {
                        key: self.key,
                        source: source.id.to_string(),
                    });
                }
                BindingAction::Source { source: source.id.clone() }
            }
            RawAction::Push { source } => BindingAction::Push { source: target(source)?.id.clone() },
        };
        let restrictions =
            self.sources
                .map(|ids| {
                    if ids.is_empty() {
                        return Err(ConfigurationError::EmptySources { key: self.key.clone() });
                    }
                    ids.into_iter()
                        .map(|id| {
                            sources.iter().find(|s| s.id.as_str() == id).map(|s| s.id.clone()).ok_or_else(|| {
                                ConfigurationError::UnknownRestriction { key: self.key.clone(), source: id }
                            })
                        })
                        .collect()
                })
                .transpose()?;
        Ok(MenuBinding { key, id: self.id, label: self.label, action, raw_key: self.key, sources: restrictions })
    }
}

fn check_argv(argv: &[String], entry: String) -> Result<(), ConfigurationError> {
    if argv.first().is_none_or(String::is_empty) {
        return Err(ConfigurationError::MissingProgram { entry });
    }
    if argv.iter().any(|a| a.contains('\0')) {
        return Err(ConfigurationError::CommandNul { entry });
    }
    Ok(())
}

fn add_key(raw: &str, keys: &mut HashSet<KeyChord>) -> Result<KeyChord, ConfigurationError> {
    let chord = KeyChord::parse(raw)?;
    if chord.is_reserved() {
        return Err(ConfigurationError::ReservedChord(raw.into()));
    }
    if !keys.insert(chord.clone()) {
        return Err(ConfigurationError::DuplicateChord(raw.into()));
    }
    Ok(chord)
}

impl MenuBinding {
    pub fn applies(&self, source_id: Option<&SourceId>) -> bool {
        match (&self.sources, source_id) {
            (None, _) => true,
            (Some(sources), Some(id)) => sources.contains(id),
            (Some(_), None) => false,
        }
    }

    pub fn action_name(&self) -> &str {
        self.id.as_deref().or(self.label.as_deref()).unwrap_or(&self.raw_key)
    }
}

const NAMED_KEYS: [&str; 6] = ["enter", "escape", "tab", "up", "down", "space"];

impl KeyChord {
    pub fn parse(raw: &str) -> Result<KeyChord, ConfigurationError> {
        let lowered = raw.to_lowercase();
        let parts: Vec<&str> = lowered.split('+').collect();
        let (key, modifiers) = parts.split_last().expect("split yields at least one part");
        if parts.contains(&"") || (key.chars().count() != 1 && !NAMED_KEYS.contains(key)) {
            return Err(ConfigurationError::InvalidChord(raw.into()));
        }
        let unique: HashSet<_> = modifiers.iter().collect();
        if unique.len() != modifiers.len() || !modifiers.iter().all(|m| ["cmd", "ctrl", "alt", "shift"].contains(m)) {
            return Err(ConfigurationError::InvalidModifiers(raw.into()));
        }
        let has = |m: &str| modifiers.contains(&m);
        let chord = KeyChord {
            key: (*key).to_owned(),
            command: has("cmd"),
            control: has("ctrl"),
            option: has("alt"),
            shift: has("shift"),
        };
        if !chord.command && !chord.control && !chord.option && key.chars().count() == 1 {
            return Err(ConfigurationError::MissingModifier(raw.into()));
        }
        Ok(chord)
    }

    fn is_reserved(&self) -> bool {
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
        format!(
            "{}{}{}{}{key}",
            flag(self.control, "⌃"),
            flag(self.option, "⌥"),
            flag(self.shift, "⇧"),
            flag(self.command, "⌘")
        )
    }
}

#[cfg(test)]
mod tests;
