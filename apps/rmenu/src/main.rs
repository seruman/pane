mod cli;
mod host;
mod view;

use std::io::Write;

use host::{Menu, OUTCOME};
use rmenu_core::model::Outcome;
use rune::{App, AppSpec, Tx};

fn main() {
    let launch = match cli::parse() {
        Ok(launch) => launch,
        Err(message) => exit(Outcome::Failed(message.to_string())),
    };
    // SAFETY: both run at startup, before any thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        // rmenu only ever draws in the terminal; callers shouldn't have to ask.
        if std::env::var_os("RUNE_TERMINAL").is_none() { std::env::set_var("RUNE_TERMINAL", "1"); }
    }
    let spec = AppSpec {
        setup: Some(Menu::setup),
        poll: Some(|m: &mut Menu, _, tx: &mut Tx| m.poll(tx)),
        on_key: Some(view::key),
        ..AppSpec::new("rmenu", (640.0, 400.0), || unreachable!("init is given"), view::view)
    };
    let started = App::new(spec)
        .init(move || Menu::new(launch))
        .args(Vec::new())
        .on_quit(Menu::quit)
        .try_run();
    if let Err(error) = started { exit(Outcome::Failed(error)); }
    let outcome = OUTCOME.lock().unwrap().take().unwrap_or(Outcome::Cancelled);
    exit(outcome);
}

fn exit(outcome: Outcome) -> ! {
    match &outcome {
        Outcome::Chosen(output) => {
            let mut stdout = std::io::stdout().lock();
            if let Err(error) = stdout.write_all(output).and_then(|()| stdout.flush()) {
                exit(Outcome::Failed(format!("output failed: {error}")));
            }
        }
        Outcome::Failed(message) => eprintln!("rmenu: {message}"),
        Outcome::Cancelled => {}
    }
    std::process::exit(outcome.exit_code());
}
