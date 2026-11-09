use super::*;

fn parse(json: &str) -> MenuConfiguration {
    MenuConfiguration::parse(json.as_bytes(), None).unwrap()
}
fn error(json: &str) -> ConfigurationError {
    MenuConfiguration::parse(json.as_bytes(), None).unwrap_err()
}

#[test]
fn deserialization_rejects_invalid_configuration() {
    for json in [
        r#"{"bindings":[{"key":"ctrl+d","action":"run"}]}"#,
        r#"{"initialSource":"missing"}"#,
        r#"{"sources":[{"id":"a","label":"A","command":[]}]}"#,
        r#"{"bindings":[{"key":"ctrl+n","action":"copy"}]}"#,
    ] {
        assert!(serde_json::from_str::<MenuConfiguration>(json).is_err(), "{json}");
    }
}

#[test]
fn minimal_configuration() {
    let config = parse("{}");
    assert!(config.sources().is_empty());
    assert!(config.bindings().is_empty());
    assert!(config.root_source().is_none());
    assert_eq!(config.marks(), MarkScope::Level);
}

#[test]
fn unknown_and_irrelevant_fields_are_ignored() {
    let config = parse(
        r#"{"future":1,"sources":[{"id":"a","label":"A","command":["echo"],"x":true}],
        "bindings":[{"key":"ctrl+d","action":"copy","command":["x"],"source":"missing","reload":true}]}"#,
    );
    assert_eq!(config.bindings()[0].action, BindingAction::Copy);
}

#[test]
fn binding_ids_name_the_action_independently_of_labels() {
    let config = parse(
        r#"{"bindings":[{"key":"ctrl+v","id":"split-right","label":"Split right","action":"accept"},{"key":"cmd+d","label":"Star","action":"accept"},{"key":"Cmd+E","action":"accept"}]}"#,
    );
    assert_eq!(
        config.bindings().iter().map(MenuBinding::action_name).collect::<Vec<_>>(),
        ["split-right", "Star", "Cmd+E"]
    );
    assert!(
        matches!(error(r#"{"bindings":[{"key":"ctrl+v","id":"x","action":"accept"},{"key":"ctrl+s","id":"x","action":"accept"}]}"#),
        ConfigurationError::DuplicateBindingId(id) if id == "x")
    );
    assert!(matches!(error(r#"{"bindings":[{"key":"ctrl+v","id":"","action":"accept"}]}"#),
        ConfigurationError::EmptyBindingId { key } if key == "ctrl+v"));
}

#[test]
fn chord_parsing() {
    assert_eq!(KeyChord::parse("shift+cmd+r").unwrap(), KeyChord::parse("cmd+shift+r").unwrap());
    assert_eq!(KeyChord::parse("cmd+shift+r").unwrap().display(), "⇧⌘R");
    assert_eq!(KeyChord::parse("Ctrl+Enter").unwrap().display(), "⌃↩");
    for bad in ["cmd+cmd+r", "r", "cmd+", "cmd+foo", "hyper+r"] {
        assert!(KeyChord::parse(bad).is_err(), "{bad}");
    }
    assert!(matches!(KeyChord::parse("cmd+"), Err(ConfigurationError::InvalidChord(raw)) if raw == "cmd+"));
    assert!(matches!(KeyChord::parse("hyper+r"), Err(ConfigurationError::InvalidModifiers(raw)) if raw == "hyper+r"));
    assert!(matches!(KeyChord::parse("r"), Err(ConfigurationError::MissingModifier(raw)) if raw == "r"));
    assert!(KeyChord::parse("enter").unwrap().is_reserved());
    assert!(KeyChord::parse("ctrl+n").unwrap().is_reserved());
    assert!(KeyChord::parse("ctrl+shift+n").unwrap().is_reserved());
    assert!(!KeyChord::parse("ctrl+alt+n").unwrap().is_reserved());
    assert!(!KeyChord::parse("cmd+e").unwrap().is_reserved());
}

#[test]
fn source_and_binding_keys_share_one_normalized_namespace() {
    assert!(matches!(error(r#"{"bindings":[{"key":"cmd+a","action":"copy"}]}"#),
        ConfigurationError::ReservedChord(key) if key == "cmd+a"));
    assert!(matches!(error(r#"{"sources":[{"id":"a","label":"A","key":"shift+ctrl+r","command":["echo"]}],
        "bindings":[{"key":"CTRL+SHIFT+R","action":"copy"}]}"#),
        ConfigurationError::DuplicateChord(key) if key == "CTRL+SHIFT+R"));
    let config = parse(r#"{"bindings":[{"key":"SHIFT+CTRL+R","action":"copy"}]}"#);
    assert_eq!(config.bindings()[0].key, KeyChord::parse("ctrl+shift+r").unwrap());
}

#[test]
fn push_levels_decode() {
    let config = parse(
        r#"
    {"marks":"global","sources":[
      {"id":"repos","label":"Repositories","prompt":"Repos","command":["/bin/echo"]},
      {"id":"worktrees","label":"Worktrees","hidden":true,"preview":["/bin/echo","wt"],"command":["/bin/echo"],"format":"jsonl"}],
     "bindings":[{"key":"ctrl+w","id":"worktrees","label":"Worktrees","action":"push","source":"worktrees","sources":["repos"]},
                 {"key":"ctrl+v","id":"split","action":"accept"}]}"#,
    );
    assert_eq!(config.marks(), MarkScope::Global);
    let (repos, worktrees) = (&config.sources()[0], &config.sources()[1]);
    assert_eq!(repos.prompt.as_deref(), Some("Repos"));
    assert!(repos.preview.is_none());
    assert!(!repos.hidden);
    assert!(worktrees.hidden);
    assert!(worktrees.prompt.is_none());
    assert_eq!(worktrees.format, InputFormat::Jsonl);
    assert_eq!(worktrees.preview.as_deref(), Some(&["/bin/echo".to_owned(), "wt".to_owned()][..]));
    assert_eq!(config.bindings()[0].action, BindingAction::Push { source: worktrees.id.clone() });
    assert!(config.bindings()[0].applies(Some(&repos.id)));
    assert!(!config.bindings()[0].applies(Some(&worktrees.id)));
    assert!(!config.bindings()[0].applies(None));
    assert!(config.bindings()[1].applies(Some(&worktrees.id)));
    assert!(config.bindings()[1].applies(None));
    assert_eq!(config.root_source().map(|s| s.id.as_str()), Some("repos"));
    assert_eq!(config.source(&worktrees.id).unwrap().label, "Worktrees");
}

#[test]
fn root_source_skips_hidden_sources_unless_explicit() {
    let config = parse(
        r#"{"sources":[{"id":"w","label":"W","hidden":true,"command":["/bin/echo"]}],"bindings":[{"key":"ctrl+w","action":"push","source":"w"}]}"#,
    );
    assert!(config.root_source().is_none());
    let initial =
        parse(r#"{"initialSource":"w","sources":[{"id":"w","label":"W","hidden":true,"command":["/bin/echo"]}]}"#);
    assert_eq!(initial.root_source().map(|s| s.id.as_str()), Some("w"));
    let visible = parse(
        r#"{"sources":[{"id":"w","label":"W","hidden":true,"command":["echo"]},{"id":"v","label":"V","command":["echo"]}]}"#,
    );
    assert_eq!(visible.root_source().map(|s| s.id.as_str()), Some("v"));
}

#[test]
fn invalid_source_references_report_the_rule_and_entry() {
    let source = r#"{"id":"a","label":"A","command":["echo"]}"#;
    let hidden = r#"{"id":"h","label":"H","hidden":true,"command":["echo"]}"#;
    assert!(matches!(error(&format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"push"}}]}}"#)),
        ConfigurationError::MissingSource { key } if key == "ctrl+w"));
    assert!(
        matches!(error(&format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"source","source":"missing"}}]}}"#)),
        ConfigurationError::UnknownSource { key, source } if key == "ctrl+w" && source == "missing")
    );
    assert!(
        matches!(error(&format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"copy","sources":[]}}]}}"#)),
        ConfigurationError::EmptySources { key } if key == "ctrl+w")
    );
    assert!(
        matches!(error(&format!(r#"{{"sources":[{source}],"bindings":[{{"key":"ctrl+w","action":"copy","sources":["missing"]}}]}}"#)),
        ConfigurationError::UnknownRestriction { key, source } if key == "ctrl+w" && source == "missing")
    );
    assert!(
        matches!(error(&format!(r#"{{"sources":[{source},{hidden}],"bindings":[{{"key":"cmd+2","action":"source","source":"h"}}]}}"#)),
        ConfigurationError::HiddenSourceTarget { key, source } if key == "cmd+2" && source == "h")
    );
    assert!(matches!(error(r#"{"sources":[{"id":"h","label":"H","hidden":true,"key":"cmd+2","command":["echo"]}]}"#),
        ConfigurationError::HiddenSourceKey(id) if id == "h"));
    assert!(
        matches!(error(&format!(r#"{{"sources":[{source},{source}]}}"#)), ConfigurationError::DuplicateSourceId(id) if id == "a")
    );
    assert!(matches!(
        error(r#"{"sources":[{"id":"","label":"A","command":["echo"]}]}"#),
        ConfigurationError::EmptySourceId
    ));
    assert!(
        matches!(error(&format!(r#"{{"initialSource":"missing","sources":[{source}]}}"#)), ConfigurationError::UnknownInitialSource(id) if id == "missing")
    );
}

#[test]
fn commands_without_a_runnable_program_fail_at_load() {
    let source = |field: &str| format!(r#"{{"sources":[{{"id":"a","label":"A",{field}}}]}}"#);
    for (json, expected) in [
        (source(r#""command":[""]"#), "Source a command"),
        (source(r#""command":[]"#), "Source a command"),
        (source(r#""command":["echo"],"preview":[""]"#), "Source a preview command"),
        (r#"{"preview":["","x"]}"#.to_owned(), "Preview command"),
        (r#"{"preview":[]}"#.to_owned(), "Preview command"),
        (r#"{"bindings":[{"key":"ctrl+r","action":"run","command":[""]}]}"#.to_owned(), "Run binding ctrl+r command"),
        (r#"{"bindings":[{"key":"ctrl+r","action":"run"}]}"#.to_owned(), "Run binding ctrl+r command"),
    ] {
        assert!(matches!(error(&json), ConfigurationError::MissingProgram { entry } if entry == expected), "{json}");
    }
    for (json, expected) in [
        (source(r#""command":["echo","a\u0000b"]"#), "Source a command"),
        (source(r#""command":["echo"],"preview":["x\u0000"]"#), "Source a preview command"),
        (r#"{"preview":["x\u0000"]}"#.to_owned(), "Preview command"),
        (
            r#"{"bindings":[{"key":"ctrl+r","action":"run","command":["x\u0000"]}]}"#.to_owned(),
            "Run binding ctrl+r command",
        ),
    ] {
        assert!(matches!(error(&json), ConfigurationError::CommandNul { entry } if entry == expected), "{json}");
    }
}

#[test]
fn command_chords_are_listed_once_each_without_changing_spelling() {
    let config = parse(
        r#"{"sources":[{"id":"a","label":"A","key":"cmd+1","command":["echo"]},{"id":"b","label":"B","key":"ctrl+2","command":["echo"]}],
        "bindings":[{"key":"Shift+Cmd+R","action":"accept"},{"key":"ctrl+e","action":"accept"},{"key":"cmd+e","action":"copy"}]}"#,
    );
    assert_eq!(config.command_chords().collect::<Vec<_>>(), ["cmd+1", "Shift+Cmd+R", "cmd+e"]);
}

#[test]
fn multi_and_reload_are_off_unless_true() {
    for (json, multi) in
        [("{}", false), (r#"{"multi":null}"#, false), (r#"{"multi":false}"#, false), (r#"{"multi":true}"#, true)]
    {
        assert_eq!(parse(json).multi(), multi, "{json}");
    }
    for (extra, expected) in
        [("", false), (r#", "reload":null"#, false), (r#", "reload":false"#, false), (r#", "reload":true"#, true)]
    {
        let config = parse(&format!(r#"{{"bindings":[{{"key":"ctrl+d","action":"run","command":["x"]{extra}}}]}}"#));
        assert_eq!(
            config.bindings()[0].action,
            BindingAction::Run { command: vec!["x".into()], reload: expected },
            "{extra}"
        );
    }
}

#[test]
fn invalid_wire_types_are_json_errors() {
    for json in [
        r#"{"multi":"yes"}"#,
        r#"{"marks":"shared"}"#,
        r#"{"sources":[{"id":"a","label":"A","hidden":"yes","command":["echo"]}]}"#,
        r#"{"bindings":[{"key":"ctrl+d","action":"run","command":["x"],"reload":"yes"}]}"#,
    ] {
        assert!(matches!(error(json), ConfigurationError::Json(_)), "{json}");
    }
}

#[test]
fn former_warm_theme_name_still_parses() {
    for theme in ["rex", "native", "warm"] {
        parse(&format!(r#"{{"theme":"{theme}"}}"#));
    }
    assert_eq!(serde_json::from_str::<Theme>(r#""warm""#).unwrap(), Theme::Rex);
    assert!(matches!(error(r#"{"theme":"sepia"}"#), ConfigurationError::Json(_)));
}

#[test]
fn preview_override_is_applied_before_command_validation() {
    let config =
        MenuConfiguration::parse(br#"{"preview":[]}"#, Some(vec!["sh".into(), "-c".into(), "echo".into()])).unwrap();
    assert_eq!(config.preview(), Some(["sh".into(), "-c".into(), "echo".into()].as_slice()));
    assert!(
        matches!(MenuConfiguration::parse(b"{}", Some(vec![String::new()])), Err(ConfigurationError::MissingProgram { entry }) if entry == "Preview command")
    );
}
