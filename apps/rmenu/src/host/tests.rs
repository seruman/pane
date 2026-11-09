use super::*;
use clap::Parser;
use std::sync::Arc;

fn menu_without_stdin() -> Menu {
    let launch = crate::cli::Cli::try_parse_from(["rmenu"]).unwrap().launch(false).unwrap();
    let mut menu = Menu::new(launch);
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
