use super::*;
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

const SAMPLE: &str = include_str!("../../examples/counter.luau");

fn runtime(source: &str) -> (tempfile::TempDir, rune::Runtime<State>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.luau");
    fs::write(&path, source).unwrap();
    let mut rt = rune::Runtime::new(
        State::open(path),
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

fn shows(rt: &rune::Runtime<State>, expected: &str) {
    assert!(
        texts(rt).iter().any(|text| text.contains(expected)),
        "expected {expected:?}, got {:?}",
        texts(rt)
    );
}

fn click(rt: &mut rune::Runtime<State>, key: &str) {
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.02);
}

fn save(rt: &mut rune::Runtime<State>, source: &str) {
    fs::write(&rt.state.path, source).unwrap();
    rt.update(|state, _| state.reload());
    rt.frame(0.02);
}

#[test]
fn sample_draws_clicks_and_keyboard() {
    let (_dir, mut rt) = runtime(SAMPLE);
    shows(&rt, "Count: 0");
    shows(&rt, "• Save to redraw");
    click(&mut rt, "script-2-0");
    shows(&rt, "Count: 1");
    // Tab traverses Pane buttons, Space activates the focused native button.
    let (_dir, mut keyboard) = runtime(SAMPLE);
    for key in [rune::Key::Tab, rune::Key::Text(" ".into())] {
        keyboard.key(rune::KeyEvent {
            key,
            mods: rune::Mods::default(),
        });
        keyboard.frame(0.02);
    }
    shows(&keyboard, "Count: 1");
}

fn themed_runtime(dark: bool, error: Option<String>) -> rune::Runtime<State> {
    let state = State {
        tree: Some(Node::Column(vec![
            Node::Text("Default script text".into()),
            Node::Button {
                label: "Action".into(),
                message: "action".into(),
            },
        ])),
        error,
        ..State::default()
    };
    let mut rt = rune::Runtime::new(state, view, (640.0, 480.0), rune::Env::new(dark, 1.0));
    rt.frame(0.0);
    rt
}

fn text_color(rt: &rune::Runtime<State>, content: &str) -> Option<rune::Color> {
    rt.elements()
        .into_iter()
        .find(|e| e.text.as_deref() == Some(content))
        .unwrap_or_else(|| panic!("missing text {content:?}"))
        .text_color
}

#[test]
fn default_text_uses_resolved_theme_in_light_and_dark() {
    for dark in [false, true] {
        let rt = themed_runtime(dark, None);
        let theme = Theme::current(dark);
        assert_eq!(rt.elements()[0].fill, theme.background);
        assert_eq!(
            text_color(&rt, "Default script text"),
            Some(theme.text),
            "dark={dark}"
        );
        assert_eq!(
            text_color(&rt, "Action"),
            Some(theme.on_accent),
            "dark={dark}"
        );
    }
}

#[test]
fn error_text_uses_resolved_theme_and_semantic_colors_in_light_and_dark() {
    for dark in [false, true] {
        let rt = themed_runtime(dark, Some("Script failed".into()));
        let theme = Theme::current(dark);
        assert_eq!(
            text_color(&rt, "Script paused — fix and save to recover"),
            Some(theme.text),
            "dark={dark}"
        );
        assert_eq!(
            text_color(&rt, "Script failed"),
            Some(theme.danger),
            "dark={dark}"
        );
        assert_eq!(
            text_color(&rt, "Default script text"),
            Some(theme.text),
            "dark={dark}"
        );
        assert_eq!(
            text_color(&rt, "Action"),
            Some(theme.on_accent),
            "dark={dark}"
        );
    }
}

#[test]
fn reload_preserves_counter_and_replaces_view() {
    let (_dir, mut rt) = runtime(SAMPLE);
    click(&mut rt, "script-2-0");
    save(&mut rt, &SAMPLE.replace("A scriptable Pane", "Edited live"));
    assert!(rt.state.error.is_none());
    shows(&rt, "Edited live");
    shows(&rt, "Count: 1");
    click(&mut rt, "script-2-0");
    shows(&rt, "Count: 2");
}

#[test]
fn syntax_and_runtime_failures_are_visible_and_recover_state() {
    let (_dir, mut rt) = runtime(SAMPLE);
    click(&mut rt, "script-2-0");
    for broken in [
        "return { init = function(".to_owned(),
        SAMPLE.replace(
            "return previous or",
            "error('init exploded'); return previous or",
        ),
        SAMPLE.replace(
            "local items = {}",
            "error('view exploded'); local items = {}",
        ),
    ] {
        save(&mut rt, &broken);
        assert!(rt.state.error.is_some());
        shows(&rt, "Script paused");
        shows(&rt, "Count: 1");
        save(&mut rt, SAMPLE);
        assert!(rt.state.error.is_none());
        shows(&rt, "Count: 1");
        assert!(!texts(&rt).iter().any(|t| t.contains("Script paused")));
    }
    save(
        &mut rt,
        &SAMPLE.replace(
            "state.count += 1",
            "state.count = 999; error('update exploded')",
        ),
    );
    click(&mut rt, "script-2-0");
    shows(&rt, "update exploded");
    shows(&rt, "Count: 1");
    save(&mut rt, SAMPLE);
    shows(&rt, "Count: 1");
    click(&mut rt, "script-2-0");
    shows(&rt, "Count: 2");
}

#[test]
fn endless_loops_are_visible_and_fixable() {
    let (_dir, mut rt) = runtime(SAMPLE);
    click(&mut rt, "script-2-0");
    for broken in [
        format!("while true do end\n{SAMPLE}"),
        SAMPLE.replace(
            "return previous or",
            "while true do end; return previous or",
        ),
        SAMPLE.replace("local items = {}", "while true do end; local items = {}"),
    ] {
        let start = Instant::now();
        save(&mut rt, &broken);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(rt.state.error.is_some());
        shows(&rt, "Script paused");
        save(&mut rt, SAMPLE);
        shows(&rt, "Count: 1");
    }
    save(
        &mut rt,
        &SAMPLE.replace("state.count += 1", "while true do end"),
    );
    click(&mut rt, "script-2-0");
    shows(&rt, "Script paused");
    save(&mut rt, SAMPLE);
    shows(&rt, "Count: 1");
}

#[test]
fn invalid_initial_file_recovers_without_a_restart() {
    let (_dir, mut rt) = runtime("not a valid script");
    shows(&rt, "Script paused");
    save(&mut rt, SAMPLE);
    shows(&rt, "Count: 0");
    assert!(rt.state.error.is_none());
}

#[test]
fn identical_notifications_do_not_repeat_init() {
    let script = SAMPLE.replace(
        "return previous or { count = 0 }",
        "return { count = if previous then previous.count + 1 else 0 }",
    );
    let (_dir, mut rt) = runtime(&script);
    rt.state.setup(Arc::new(|| {}));
    rt.update(|state, _| state.reload());
    rt.frame(0.02);
    shows(&rt, "Count: 0");
}

fn wait_for(rt: &mut rune::Runtime<State>, clock: &mut f64, predicate: impl Fn(&State) -> bool) {
    let start = Instant::now();
    while !predicate(&rt.state) {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "watcher timed out: {:?}",
            rt.state.error
        );
        std::thread::sleep(Duration::from_millis(20));
        *clock += 0.02;
        rt.frame(*clock);
    }
}

#[test]
fn watcher_wakes_reloads_atomic_saves_and_recovers_after_removal() {
    let (_dir, mut rt) = runtime(SAMPLE);
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wake_count = wakes.clone();
    rt.state.setup(Arc::new(move || {
        wake_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    click(&mut rt, "script-2-0");
    let mut clock = 0.02;
    let source = SAMPLE.replace("A scriptable Pane", "Atomic save");
    let replacement = rt.state.path.with_extension("tmp");
    fs::write(&replacement, &source).unwrap();
    fs::rename(&replacement, &rt.state.path).unwrap();
    wait_for(&mut rt, &mut clock, |s| {
        s.source.as_deref() == Some(&source)
    });
    assert!(wakes.load(std::sync::atomic::Ordering::Relaxed) > 0);
    shows(&rt, "Atomic save");
    shows(&rt, "Count: 1");
    fs::remove_file(&rt.state.path).unwrap();
    wait_for(&mut rt, &mut clock, |s| s.error.is_some());
    shows(&rt, "Script paused");
    fs::write(&rt.state.path, SAMPLE).unwrap();
    wait_for(&mut rt, &mut clock, |s| s.error.is_none());
    shows(&rt, "Count: 1");
}
