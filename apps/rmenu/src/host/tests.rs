use super::*;
use clap::Parser;
use std::sync::Arc;

fn menu_without_stdin() -> Menu {
    let launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
    menu_with_launch(launch)
}

fn menu_with_launch(launch: crate::cli::Launch) -> Menu {
    let mut menu = Menu::new(launch, Rc::default());
    assert!(matches!(menu.events.try_recv().unwrap(), Message::Start(effects)
        if effects.iter().any(|e| matches!(e, Effect::ReadStdin))));
    assert!(menu.events.try_recv().is_err());
    menu.setup(Arc::new(|| {}));
    menu
}

fn poll_until(menu: &mut Menu, ready: impl Fn(&Menu) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready(menu) {
        assert!(Instant::now() < deadline, "background result did not arrive");
        menu.poll(&mut Tx::default());
        std::thread::yield_now();
    }
    menu.stop();
}

#[test]
fn each_menu_owns_its_result_and_finishes_only_once() {
    let mut first = menu_without_stdin();
    let mut second = menu_without_stdin();
    let first_result = first.outcome.clone();
    let second_result = second.outcome.clone();
    first.apply(vec![Effect::Finish(Outcome::Chosen(b"chosen\\n".to_vec()))], &mut Tx::default());
    first.quit(&mut Tx::default());
    first.apply(vec![Effect::Finish(Outcome::Failed("late".into()))], &mut Tx::default());
    assert!(second_result.borrow().is_none());
    second.quit(&mut Tx::default());
    drop(first);
    drop(second);
    assert_eq!(first_result.borrow_mut().take(), Some(Outcome::Chosen(b"chosen\\n".to_vec())));
    assert_eq!(second_result.borrow_mut().take(), Some(Outcome::Cancelled));
}

#[test]
fn settle_lands_highlight_color_that_reduced_motion_fades() {
    fn runtime() -> rune::Runtime<Menu> {
        let mut menu = menu_without_stdin();
        menu.handle(Event::StdinItems(vec![MenuItem::new("a", "alpha"), MenuItem::new("b", "beta")]), &mut Tx::default());
        menu.handle(Event::StdinDone, &mut Tx::default());
        poll_until(&mut menu, |m| m.model.result_count() == 2 && !m.model.is_searching());
        let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }
    fn row_fills(rt: &rune::Runtime<Menu>) -> Vec<(String, rune::Color)> {
        rt.elements().into_iter().filter(|e| e.path.contains("/rows/")).map(|e| (e.path, e.fill)).collect()
    }
    let mut settled = runtime();
    let mut reduced_only = runtime();
    settled.update(|m, tx| m.handle(Event::Highlight(1), tx));
    reduced_only.update(|m, tx| {
        let effects = m.model.handle(Event::Highlight(1));
        m.apply(effects, tx);
        m.reveal(tx);
    });
    settled.frame(1.0 / 120.0);
    reduced_only.frame(1.0 / 120.0);
    let landed = row_fills(&settled);
    let fading = row_fills(&reduced_only);
    settled.frame(0.3);
    reduced_only.frame(0.3);
    assert_eq!(landed, row_fills(&settled), "settle paints the final highlight immediately");
    assert_ne!(fading, row_fills(&reduced_only), "reduced motion alone still fades highlight colors");
    assert_eq!(row_fills(&settled), row_fills(&reduced_only));
}

#[test]
fn reduced_motion_and_settle_still_allow_hover_fades() {
    let mut menu = menu_without_stdin();
    menu.handle(Event::StdinItems((0..30).map(|i| MenuItem::new(format!("id{i}"), format!("row-{i}"))).collect()), &mut Tx::default());
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| m.model.result_count() == 30 && !m.model.is_searching());
    let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.reduced_motion);
    let row = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("row-1")).unwrap();
    rt.pointer_move(row.rect[0] + 4.0, row.rect[1] + 4.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let hovered = rt.elements().into_iter().find(|e| e.hover == 1.0).expect("row hover enters immediately");
    rt.pointer_leave();
    rt.frame(0.02);
    let fading = rt.elements().into_iter().find(|e| e.path == hovered.path).unwrap();
    assert!(fading.hover > 0.0 && fading.hover < 1.0, "reduced motion retains opacity fades, got {}", fading.hover);
    rt.update(|menu, tx| menu.handle(Event::Highlight(0), tx));
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let after_event = rt.elements().into_iter().find(|e| e.path == hovered.path).unwrap();
    assert!(after_event.hover > 0.0 && after_event.hover < fading.hover);
    rt.frame(0.2);
    let finished = rt.elements().into_iter().find(|e| e.path == hovered.path).unwrap();
    assert_eq!(finished.hover, 0.0);
}

#[test]
fn footer_fits_labels_then_keycaps_then_drops_bindings() {
    let mut launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
    launch.options.configuration = serde_json::from_value(serde_json::json!({"bindings": [
        {"key": "ctrl+w", "label": "Open the selected worktree", "action": "accept"},
        {"key": "ctrl+o", "label": "Open the selected repository", "action": "accept"},
        {"key": "alt+n", "label": "Read all repository memos", "action": "accept"}
    ]})).unwrap();
    let mut rt = rune::Runtime::new(menu_with_launch(launch), crate::view::view, (1200.0, 400.0), rune::Env::new(true, 1.0));
    for (width, labelled, bindings) in [(1200.0, true, 3), (400.0, false, 3), (270.0, false, 1), (230.0, false, 0)] {
        rt.set_viewport((width, 400.0), 1.0);
        rt.frame(0.0);
        rt.frame(0.0);
        let elements = rt.elements();
        let has = |text: &str| elements.iter().any(|e| e.text.as_deref() == Some(text));
        assert_eq!(has("Open the selected worktree"), labelled, "width={width}");
        assert_eq!(["⌃W", "⌃O", "⌥N"].iter().filter(|key| has(key)).count(), bindings, "width={width}");
        for text in ["Esc", "Choose", "↩"] {
            let e = elements.iter().find(|e| e.text.as_deref() == Some(text)).unwrap();
            assert!(e.rect[0] >= 0.0 && e.rect[0] + e.rect[2] <= width, "{text} at {:?}, width={width}", e.rect);
        }
    }
}

#[test]
fn query_keys_are_not_taken_by_the_menu() {
    let mut menu = menu_without_stdin();
    for (key, mods) in [
        (rune::Key::Text("w".into()), rune::Mods::default()),
        (rune::Key::Text("W".into()), rune::Mods { shift: true, ..rune::Mods::default() }),
        (rune::Key::Text(" ".into()), rune::Mods::default()),
    ] {
        assert!(!crate::view::key(&mut menu, &rune::KeyEvent { key, mods }, &mut Tx::default()));
        assert_eq!(menu.model.query(), "");
    }
    for key in ["w", "shift+w", "space"] {
        let config = serde_json::json!({"bindings": [{"key": key, "action": "accept"}]});
        assert!(serde_json::from_value::<rmenu_core::config::MenuConfiguration>(config).is_err());
    }
}

#[test]
fn startup_effects_are_queued_once() {
    let menu = menu_without_stdin();
    assert!(menu.tasks.is_empty());
    assert!(menu.stdin.is_none());
}

#[test]
fn consecutive_stdin_batches_publish_together_before_the_queue_empties() {
    let mut menu = menu_without_stdin();
    let revision = menu.model.result_revision();
    for title in ["alpha", "beta"] {
        menu.sender.send(Message::Event(Event::StdinItems(vec![MenuItem::new(title, title)]))).unwrap();
    }
    poll_until(&mut menu, |m| m.model.result_count() == 2);
    assert_eq!(menu.model.result_revision(), revision + 1);
    assert_eq!(menu.model.result(0).value(), "alpha");
    assert_eq!(menu.model.result(1).value(), "beta");
}

#[test]
fn a_query_between_batches_keeps_its_position_in_the_event_stream() {
    let mut menu = menu_without_stdin();
    for event in [
        Event::StdinItems(vec![MenuItem::new("alpha", "alpha")]),
        Event::SetQuery("beta".into()),
        Event::StdinItems(vec![MenuItem::new("beta", "beta")]),
        Event::StdinDone,
    ] {
        menu.sender.send(Message::Event(event)).unwrap();
    }
    poll_until(&mut menu, |m| m.model.result_count() == 1 && m.model.result(0).value() == "beta");
    assert_eq!(menu.model.query(), "beta");
    assert_eq!(menu.field.text, "beta");
    assert!(!menu.model.loading());
}
