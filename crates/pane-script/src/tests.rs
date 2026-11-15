use super::*;
use std::{
    fs,
    time::{Duration, Instant},
};

const COUNTER: &str = include_str!("../tests/fixtures/counter.luau");

#[path = "board_tests.rs"]
mod board_tests;

fn runtime(source: &str) -> (tempfile::TempDir, rune::Runtime<State>) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("init.luau"), source).unwrap();
    let mut rt = rune::Runtime::new(
        open(dir.path(), host::Capabilities::default()),
        view,
        (640.0, 480.0),
        rune::Env::default(),
    );
    rt.poll = Some(poll);
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);
    (dir, rt)
}

fn texts(rt: &rune::Runtime<State>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.text)
        .collect()
}

fn shows(rt: &rune::Runtime<State>, expected: &str) -> bool {
    texts(rt).iter().any(|text| text.contains(expected))
}

fn click(rt: &mut rune::Runtime<State>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("no {key}: {:?}", texts(rt)));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.02);
}

/// Frames until `ready` holds, as the window would draw them while the watcher compiles a save.
fn frames_until(
    rt: &mut rune::Runtime<State>,
    ready: impl Fn(&rune::Runtime<State>) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut clock = 0.1;
    while Instant::now() < deadline {
        rt.frame(clock);
        clock += 0.02;
        if ready(rt) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn shipped_examples_start_and_render_without_script_errors() {
    for name in ["counter", "io"] {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name);
        // Merely opening either example needs no I/O authority or external activity.
        let state = open(&dir, host::Capabilities::default());
        let mut rt = rune::Runtime::new(state, view, (640.0, 480.0), rune::Env::default());
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(0.1);
        assert!(
            rt.state.script.error().is_none(),
            "{name}: {:?}",
            rt.state.script.error()
        );
        assert!(
            rt.state
                .script
                .log()
                .iter()
                .all(|line| line.level != rune_luau::LogLevel::Error),
            "{name}: {:?}",
            rt.state.script.log()
        );
        assert!(!texts(&rt).is_empty(), "{name} rendered no text");
    }
}

#[test]
fn a_counter_draws_and_counts_clicks() {
    let (_dir, mut rt) = runtime(COUNTER);
    assert!(shows(&rt, "Count: 0"), "{:?}", texts(&rt));
    assert!(shows(&rt, "• Save to reload"), "{:?}", texts(&rt));
    click(&mut rt, "increment");
    click(&mut rt, "increment");
    assert!(shows(&rt, "Count: 2"), "{:?}", texts(&rt));
    click(&mut rt, "reset");
    assert!(shows(&rt, "Count: 0"), "{:?}", texts(&rt));
}

#[test]
fn tab_and_space_press_the_focused_button() {
    let (_dir, mut rt) = runtime(COUNTER);
    rt.key(rune::KeyEvent {
        key: rune::Key::Tab,
        mods: rune::Mods::default(),
    });
    rt.frame(0.02);
    rt.key(rune::KeyEvent {
        key: rune::Key::Text(" ".into()),
        mods: rune::Mods::default(),
    });
    rt.frame(0.04);
    assert!(shows(&rt, "Count: 1"), "{:?}", texts(&rt));
}

#[test]
fn a_saved_edit_reloads_through_the_watcher_and_keeps_the_count() {
    let (dir, mut rt) = runtime(COUNTER);
    click(&mut rt, "increment");
    assert!(shows(&rt, "Count: 1"));
    fs::write(
        dir.path().join("init.luau"),
        COUNTER.replace("A scriptable Pane", "An edited Pane"),
    )
    .unwrap();
    assert!(
        frames_until(&mut rt, |rt| shows(rt, "An edited Pane")),
        "no reload: {:?}",
        texts(&rt)
    );
    assert!(
        shows(&rt, "Count: 1"),
        "count lost on reload: {:?}",
        texts(&rt)
    );
}

#[test]
fn a_broken_save_keeps_the_running_script() {
    let (dir, mut rt) = runtime(COUNTER);
    click(&mut rt, "increment");
    fs::write(
        dir.path().join("init.luau"),
        "return { render = function( end }",
    )
    .unwrap();
    assert!(frames_until(&mut rt, |rt| rt
        .state
        .script
        .log()
        .iter()
        .any(|l| l.level == rune_luau::LogLevel::Error)));
    assert!(
        shows(&rt, "A scriptable Pane") && shows(&rt, "Count: 1"),
        "{:?}",
        texts(&rt)
    );
    click(&mut rt, "increment");
    assert!(
        shows(&rt, "Count: 2"),
        "old script stopped: {:?}",
        texts(&rt)
    );
}

#[test]
fn a_script_that_fails_to_start_shows_the_error_card() {
    let (_dir, rt) = runtime(
        "local ui = require('@rune/ui')\nreturn { render = function() return ui.text(nil .. 'x') end }",
    );
    assert!(
        shows(&rt, "Stopped drawing") && shows(&rt, "Reload"),
        "{:?}",
        texts(&rt)
    );
}

#[test]
fn a_missing_folder_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let state = open(&dir.path().join("missing"), host::Capabilities::default());
    assert!(state.script.error().is_some());
}

#[test]
fn a_luau_file_argument_means_its_folder() {
    assert_eq!(
        folder("examples/counter/init.luau"),
        PathBuf::from("examples/counter")
    );
    assert_eq!(folder("init.luau"), PathBuf::from("."));
    assert_eq!(
        folder("examples/counter"),
        PathBuf::from("examples/counter")
    );
}
