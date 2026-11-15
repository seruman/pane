use super::*;

const BOARD: &str = include_str!("../examples/counter/init.luau");

fn board() -> (tempfile::TempDir, rune::Runtime<State>) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("init.luau"), BOARD).unwrap();
    let mut rt = rune::Runtime::new(
        open(dir.path(), host::Capabilities::default()),
        view,
        (1100.0, 900.0),
        rune::Env::default(),
    );
    rt.poll = Some(poll);
    rt.frame(0.0);
    (dir, rt)
}

fn text_at(rt: &rune::Runtime<State>, key: &str) -> String {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
        .unwrap_or_else(|| panic!("no text at {key}: {:?}", texts(rt)))
}

// Give queued script callbacks and the next layout commit separate frame turns.
// The small counter fixture's single-frame helper is insufficient for added/moved nodes.
fn click(rt: &mut rune::Runtime<State>, key: &str) {
    rt.frame(rt.clock + 0.02);
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("no {key}: {:?}", texts(rt)));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(rt.clock + 0.02);
    rt.frame(rt.clock + 0.02);
}

fn add_and_ship(rt: &mut rune::Runtime<State>) {
    click(rt, "add-task");
    click(rt, "template-3");
    assert_eq!(text_at(rt, "draft-title"), "Ship the hot reload demo");
    click(rt, "confirm-add");
    assert_eq!(text_at(rt, "selected-lane"), "Task 4 · Next");
    click(rt, "move-4");
    assert_eq!(text_at(rt, "selected-lane"), "Task 4 · Now");
    click(rt, "move-4");
    assert_eq!(text_at(rt, "selected-lane"), "Task 4 · Done");
}

#[test]
fn shipped_board_adds_moves_and_counts_each_task_once() {
    let (_dir, mut rt) = board();
    assert!(rt.state.script.error().is_none(), "{:?}", texts(&rt));
    add_and_ship(&mut rt);
    assert_eq!(text_at(&rt, "count"), "1");
    assert_eq!(text_at(&rt, "board-progress"), "1 / 4 complete · 25%");
    click(&mut rt, "move-4"); // Reopen.
    click(&mut rt, "move-4"); // Start again.
    click(&mut rt, "move-4"); // Ship again, without double-counting.
    assert_eq!(text_at(&rt, "count"), "1");
    assert_eq!(text_at(&rt, "selected-lane"), "Task 4 · Done");
}

#[test]
fn shipped_board_preserves_tasks_selection_and_picker_across_edits() {
    let (dir, mut rt) = board();
    add_and_ship(&mut rt);
    click(&mut rt, "add-task");
    click(&mut rt, "template-2");
    fs::write(
        dir.path().join("init.luau"),
        BOARD.replace("Ship small. Stay live.", "A new coat of paint"),
    )
    .unwrap();
    assert!(frames_until(&mut rt, |rt| shows(rt, "A new coat of paint")));
    assert_eq!(text_at(&rt, "count"), "1");
    assert_eq!(text_at(&rt, "selected-title"), "Ship the hot reload demo");
    assert_eq!(text_at(&rt, "selected-lane"), "Task 4 · Done");
    assert_eq!(text_at(&rt, "draft-title"), "Check keyboard navigation");
    click(&mut rt, "confirm-add");
    assert_eq!(text_at(&rt, "selected-title"), "Check keyboard navigation");
    assert_eq!(text_at(&rt, "selected-lane"), "Task 5 · Next");
}

#[test]
fn shipped_board_migrates_the_live_counter() {
    let (dir, mut rt) = runtime(COUNTER);
    click(&mut rt, "increment");
    click(&mut rt, "increment");
    fs::write(dir.path().join("init.luau"), BOARD).unwrap();
    assert!(frames_until(&mut rt, |rt| shows(
        rt,
        "Ship small. Stay live."
    )));
    assert_eq!(text_at(&rt, "count"), "2");
    assert!(text_at(&rt, "history").starts_with("2 carried from the counter"));
}
