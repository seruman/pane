#![allow(clippy::float_cmp, reason = "Exact equality checks for no clipping or animation must not use a tolerance.")]

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
    first.handle(Event::StdinItems(vec![MenuItem::new("chosen", "chosen")]), &mut Tx::default());
    first.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut first, |m| m.model.result_count() == 1 && !m.model.is_searching());
    first.handle(Event::Nav(rmenu_core::model::Nav::Accept), &mut Tx::default());
    first.quit(&mut Tx::default());
    first.handle(Event::StdinFailed("late".into()), &mut Tx::default());
    first.handle(Event::Nav(rmenu_core::model::Nav::Cancel), &mut Tx::default());
    assert!(second_result.borrow().is_none());
    second.quit(&mut Tx::default());
    drop(first);
    drop(second);
    assert_eq!(first_result.borrow_mut().take(), Some(Outcome::Chosen(b"chosen\n".to_vec())));
    assert_eq!(second_result.borrow_mut().take(), Some(Outcome::Cancelled));
}

#[test]
fn enter_during_search_publishes_the_new_choice_to_the_owner() {
    let mut menu = menu_without_stdin();
    menu.handle(Event::StdinItems(vec![MenuItem::new("a", "alpha"), MenuItem::new("b", "beta")]), &mut Tx::default());
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| m.model.result_count() == 2 && !m.model.is_searching());
    menu.handle(Event::SetQuery("beta".into()), &mut Tx::default());
    assert!(crate::view::key(
        &mut menu,
        &rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() },
        &mut Tx::default()
    ));
    assert!(menu.outcome.borrow().is_none());
    poll_until(&mut menu, |m| m.outcome.borrow().is_some());
    assert_eq!(*menu.outcome.borrow(), Some(Outcome::Chosen(b"beta\n".to_vec())));
    assert!(menu.tasks.is_empty());
    menu.quit(&mut Tx::default());
    assert_eq!(*menu.outcome.borrow(), Some(Outcome::Chosen(b"beta\n".to_vec())));
}

#[test]
fn quit_cancels_and_joins_work_and_keeps_input_failure() {
    for failure in [None, Some("broken input")] {
        let mut menu = menu_without_stdin();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = stopped.clone();
        let running = menu.spawn(move |cancel, _| {
            while !cancel.is_cancelled() {
                std::thread::yield_now();
            }
            signal.store(true, std::sync::atomic::Ordering::Release);
        });
        menu.tasks.insert(TaskKind::Preview, running);
        if let Some(error) = failure {
            menu.handle(Event::StdinFailed(error.into()), &mut Tx::default());
        }
        menu.quit(&mut Tx::default());
        assert!(stopped.load(std::sync::atomic::Ordering::Acquire));
        assert!(menu.tasks.is_empty());
        assert!(menu.stdin.is_none());
        assert!(menu.retired.is_empty());
        assert_eq!(
            *menu.outcome.borrow(),
            Some(failure.map_or(Outcome::Cancelled, |error| Outcome::Failed(error.into())))
        );
    }
}

#[test]
fn escape_and_control_c_finish_through_the_key_adapter() {
    for (key, mods) in [
        (rune::Key::Escape, rune::Mods::default()),
        (rune::Key::Command('c'), rune::Mods { ctrl: true, cmd: true, ..rune::Mods::default() }),
    ] {
        let mut menu = menu_without_stdin();
        assert!(crate::view::key(&mut menu, &rune::KeyEvent { key, mods }, &mut Tx::default()));
        assert_eq!(*menu.outcome.borrow(), Some(Outcome::Cancelled));
    }
}

#[test]
fn navigation_keys_drive_the_model_and_modified_arrows_pass_through() {
    use rune::{Key, KeyEvent, Mods};
    let mut launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
    launch.options.rows = 3;
    let mut menu = menu_with_launch(launch);
    menu.handle(
        Event::StdinItems((0..10).map(|i| MenuItem::new(i.to_string(), i.to_string())).collect()),
        &mut Tx::default(),
    );
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| m.model.result_count() == 10 && !m.model.is_searching());
    let plain = Mods::default();
    let shift = Mods { shift: true, ..plain };
    let ctrl = Mods { ctrl: true, cmd: true, ..plain };
    for (key, mods, expected) in [
        (Key::Up, plain, Some(3)),
        (Key::Down, plain, Some(5)),
        (Key::PageUp, plain, Some(1)),
        (Key::PageDown, plain, Some(7)),
        (Key::Tab, plain, Some(5)),
        (Key::Tab, shift, Some(3)),
        (Key::Command('n'), ctrl, Some(5)),
        (Key::Command('p'), ctrl, Some(3)),
        (Key::Down, shift, None),
        (Key::Up, ctrl, None),
        (Key::Backspace, plain, Some(4)),
    ] {
        menu.handle(Event::Highlight(4), &mut Tx::default());
        let event = KeyEvent { key, mods };
        assert_eq!(crate::view::key(&mut menu, &event, &mut Tx::default()), expected.is_some(), "{event:?}");
        assert_eq!(menu.model.highlighted_row(), Some(expected.unwrap_or(4)), "{event:?}");
    }
    menu.stop();
}

#[test]
fn highlight_reveal_scrolls_the_virtual_list_without_a_glide() {
    let mut menu = menu_without_stdin();
    menu.handle(
        Event::StdinItems((0..80).map(|i| MenuItem::new(i.to_string(), format!("row-{i}"))).collect()),
        &mut Tx::default(),
    );
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| m.model.result_count() == 80 && !m.model.is_searching());
    let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|menu, tx| menu.handle(Event::Highlight(20), tx));
    rt.frame(1.0 / 60.0);
    let row = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("row-20")).unwrap();
    assert!(row.scrolled[1] > 0.0);
    assert_eq!(row.visible, row.rect);
    rt.frame(0.3);
    let later = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("row-20")).unwrap();
    assert_eq!(row.rect, later.rect);
}

#[test]
fn row_click_and_text_input_use_the_real_view_handlers() {
    for (multi, ctrl) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
        launch.options.multi = multi;
        let mut menu = menu_with_launch(launch);
        menu.handle(
            Event::StdinItems(vec![MenuItem::new("a", "alpha"), MenuItem::new("b", "beta")]),
            &mut Tx::default(),
        );
        menu.handle(Event::StdinDone, &mut Tx::default());
        poll_until(&mut menu, |m| m.model.result_count() == 2 && !m.model.is_searching());
        let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        let beta = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("beta")).unwrap();
        let (x, y) = (beta.rect[0] + 4.0, beta.rect[1] + 4.0);
        rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods { ctrl, ..rune::Mods::default() }));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.model.highlighted_row(), Some(1), "multi={multi}, ctrl={ctrl}");
        assert_eq!(rt.state.model.marked_count(), usize::from(multi && ctrl), "multi={multi}, ctrl={ctrl}");
        assert!(rt.state.field.focused, "multi={multi}, ctrl={ctrl}");
        rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods::default()));
        rt.key(rune::KeyEvent { key: rune::Key::Text("beta".into()), mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.field.text, "beta", "multi={multi}, ctrl={ctrl}");
        assert_eq!(rt.state.model.query(), "beta", "multi={multi}, ctrl={ctrl}");
        rt.state.stop();
    }
}

#[test]
#[ignore = "timing probe, run with --ignored --nocapture"]
fn poll_budget_measurement() {
    let mut samples = Vec::new();
    for _ in 0..21 {
        let mut menu = menu_without_stdin();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = wakes.clone();
        menu.setup(Arc::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
        for _ in 0..500_000 {
            menu.sender.send(Message::Event(Event::StdinItems(Vec::new()))).unwrap();
        }
        let start = Instant::now();
        assert!(menu.poll(&mut Tx::default()));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(menu.events.try_recv().is_ok(), "the budget yields before draining a flood");
        assert!(wakes.load(std::sync::atomic::Ordering::Relaxed) > 1, "pending messages schedule another frame");
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "poll drain only, 21 runs of 500000 queued empty stdin batches, milliseconds min={:.3} median={:.3} p90={:.3} max={:.3}",
        samples[0], samples[10], samples[18], samples[20]
    );
}

#[test]
fn query_sync_preserves_edit_state_until_the_model_changes_the_value() {
    let mut menu = menu_without_stdin();
    menu.field.undo_group(|field| field.insert("alpha"));
    menu.handle(Event::SetQuery(menu.field.text.clone()), &mut Tx::default());
    menu.field.anchor = 1;
    menu.field.focus = 3;
    menu.field.ime(&rune::Ime::Preedit("候補".into(), Some((0, 3)), Vec::new()));
    menu.handle(Event::StdinDone, &mut Tx::default());
    assert_eq!(menu.model.query(), "alpha");
    assert_eq!((menu.field.anchor, menu.field.focus), (1, 3));
    assert_eq!(menu.field.preedit().map(|p| p.0), Some("候補"));
    menu.field.undo();
    assert!(menu.field.preedit().is_none());
    menu.field.undo();
    assert_eq!(menu.field.text, "");
    menu.handle(Event::SetQuery("replacement".into()), &mut Tx::default());
    assert_eq!(menu.field.text, menu.model.query());
    assert_eq!((menu.field.anchor, menu.field.focus), (11, 11));
    assert!(menu.field.preedit().is_none());
    menu.stop();
}

fn query_border(rt: &rune::Runtime<Menu>) -> ([f32; 4], f32, [f32; 4]) {
    let elements = rt.elements();
    let query = elements.iter().find(|e| e.key.as_deref() == Some("query")).unwrap();
    let line = &elements[query.parent.unwrap()];
    let instance = rt.instances.iter().find(|i| i.rect == line.rect).unwrap();
    (instance.rect, instance.params[0], instance.border)
}

#[test]
fn refusal_paints_query_border_without_changing_menu() {
    for appearance in [crate::cli::Appearance::Light, crate::cli::Appearance::Dark] {
        let mut menu = menu_without_stdin();
        menu.appearance = appearance;
        menu.handle(Event::StdinDone, &mut Tx::default());
        menu.handle(Event::SetQuery("absent".into()), &mut Tx::default());
        poll_until(&mut menu, |m| !m.model.is_searching());
        let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        let (rect, width, _) = query_border(&rt);
        assert_eq!(width, 0.0);
        rt.update(|menu, tx| {
            assert!(crate::view::key(menu, &rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() }, tx));
        });
        rt.frame(1.0 / 120.0);
        let (flashed_rect, width, color) = query_border(&rt);
        assert!(width > 0.0 && color[3] > 0.0, "refusal must paint a visible border");
        assert_eq!(flashed_rect, rect);
        assert!(rt.reduced_motion && rt.settled);
        assert!(rt.wake_deadline().is_some(), "the flash must schedule its own expiry");
        assert_eq!(rt.state.model.query(), "absent");
        assert_eq!(rt.state.field.text, "absent");
        assert_eq!(rt.state.model.highlighted_row(), None);
        assert!(rt.state.outcome.borrow().is_none());
        assert!(rt.shell_request.is_none());
    }
}

#[test]
fn refusal_expiry_redraws_without_queued_messages() {
    let mut menu = menu_without_stdin();
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| !m.model.is_searching());
    let mut rt = rune::Runtime::new(menu, crate::view::view, (640.0, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    let before = query_border(&rt);
    rt.update(|menu, tx| menu.handle(Event::Nav(rmenu_core::model::Nav::Accept), tx));
    rt.frame(1.0 / 120.0);
    assert!(query_border(&rt).1 > 0.0);
    rt.state.refusal_until = Some(Instant::now());
    assert!(rt.state.events.try_recv().is_err());
    assert!(rt.update_if(Menu::poll), "expiry must invalidate the view even with no messages");
    rt.frame(1.0 / 120.0);
    assert_eq!(query_border(&rt), before, "expiry removes the border without a fade or layout shift");
    assert!(rt.reduced_motion && rt.settled);
    assert!(!rt.update_if(Menu::poll));
}

#[test]
fn repeated_refusal_reschedules_an_early_wake_until_the_latest_deadline() {
    let mut menu = menu_without_stdin();
    menu.handle(Event::StdinDone, &mut Tx::default());
    poll_until(&mut menu, |m| !m.model.is_searching());
    let mut rt = rune::Runtime::new(menu, |_, _| rune::el(), (100.0, 50.0), rune::Env::default());
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);
    rt.update(|menu, tx| menu.handle(Event::Nav(rmenu_core::model::Nav::Accept), tx));
    rt.frame(0.0);
    let first = rt.state.refusal_until.unwrap();
    assert_eq!(rt.wake_deadline(), Some(rt.clock + REFUSAL_FLASH.as_secs_f64()));
    rt.update(|menu, tx| menu.handle(Event::Nav(rmenu_core::model::Nav::Accept), tx));
    rt.frame(0.0);
    let latest = rt.state.refusal_until.unwrap();
    assert!(latest > first, "a repeated refusal extends the flash");
    rt.frame(REFUSAL_FLASH.as_secs_f64());
    assert_eq!(rt.wake_deadline(), None);
    assert!(!rt.update_if(|menu, tx| menu.poll_refusal(first, tx)));
    rt.frame(0.0);
    let remaining = (latest - first).as_secs_f64();
    assert_eq!(rt.wake_deadline(), Some(rt.clock + remaining));
    assert_eq!(rt.state.refusal_until, Some(latest));
    assert!(rt.update_if(|menu, tx| menu.poll_refusal(latest, tx)));
    rt.frame(remaining);
    assert_eq!(rt.state.refusal_until, None);
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn settle_lands_highlight_color_that_reduced_motion_fades() {
    fn runtime() -> rune::Runtime<Menu> {
        let mut menu = menu_without_stdin();
        menu.handle(
            Event::StdinItems(vec![MenuItem::new("a", "alpha"), MenuItem::new("b", "beta")]),
            &mut Tx::default(),
        );
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
    menu.handle(
        Event::StdinItems((0..30).map(|i| MenuItem::new(format!("id{i}"), format!("row-{i}"))).collect()),
        &mut Tx::default(),
    );
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
    ]}))
    .unwrap();
    let mut rt =
        rune::Runtime::new(menu_with_launch(launch), crate::view::view, (1200.0, 400.0), rune::Env::new(true, 1.0));
    for (width, labelled, bindings) in [(1200.0, true, 3), (600.0, false, 3), (340.0, false, 1), (260.0, false, 0)] {
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
