use super::*;

#[test]
fn inspection_is_opt_in_and_valued() {
    let args = vec!["demo".into(), "--allow-fs".into()];
    assert_eq!(arguments(&args).unwrap(), (args.clone(), None));
    let mut enabled = args.clone();
    enabled.extend(["--inspect".into(), "/tmp/inspection".into()]);
    assert_eq!(
        arguments(&enabled).unwrap(),
        (args, Some("/tmp/inspection".into()))
    );
    for bad in [
        vec!["--inspect".into()],
        vec!["--inspect".into(), "--allow-fs".into()],
        vec![
            "--inspect".into(),
            "a".into(),
            "--inspect".into(),
            "b".into(),
        ],
    ] {
        assert!(arguments(&bad).is_err());
    }
}

#[cfg(unix)]
#[test]
fn protocol_rejects_typos_bad_types_and_unbounded_values() {
    use serde_json::json;
    for bad in [
        json!(null),
        json!({"op": "shell"}),
        json!({"op": "info", "extra": true}),
        json!({"op": "activate", "key": ""}),
        json!({"op": "key", "key": "Tab", "mods": "shift"}),
        json!({"op": "key", "key": "Tab", "mods": ["super"]}),
        json!({"op": "key", "key": "wat"}),
        json!({"op": "pick", "x": 1e100, "y": 0}),
        json!({"op": "scroll", "key": "list", "x": "3", "y": 0}),
        json!({"op": "text", "key": "input", "text": "x".repeat(16385)}),
    ] {
        assert!(commands::parse(bad.clone()).is_err(), "{bad}");
    }
    for good in [
        json!({"op": "info"}),
        json!({"op": "elements"}),
        json!({"op": "tree"}),
        json!({"op": "screenshot"}),
        json!({"op": "activate", "key": "increment"}),
        json!({"op": "key", "key": "a", "mods": ["cmd"]}),
        json!({"op": "text", "key": "input", "text": "hello 🌍"}),
    ] {
        assert!(commands::parse(good).is_ok());
    }
}
