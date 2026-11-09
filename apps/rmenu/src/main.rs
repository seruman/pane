mod cli;
mod host;
mod view;

use std::io::{IsTerminal, Write};

use clap::Parser;

use host::{Menu, OUTCOME};
use rmenu_core::model::Outcome;
use rune::{App, AppSpec, Tx};

fn main() {
    let launch = match cli::Cli::parse().launch(std::io::stdin().is_terminal()) {
        Ok(launch) => launch,
        Err(message) => exit(Outcome::Failed(message.to_string())),
    };
    for warning in cli::warnings(&launch.options.configuration) { eprintln!("rmenu: {warning}"); }
    // SAFETY: installs the ignored disposition before any thread starts.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN); }
    let spec = AppSpec {
        setup: Some(Menu::setup),
        poll: Some(|m: &mut Menu, _, tx: &mut Tx| m.poll(tx)),
        on_key: Some(view::key),
        ..AppSpec::new("rmenu", (640.0, 400.0), || unreachable!("init is given"), view::view)
    };
    let started = App::new(spec)
        .terminal()
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
