use super::*;
use clap::Parser;
use std::sync::Arc;

fn menu_without_stdin() -> Menu {
    let launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
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
