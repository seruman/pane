mod cli;
mod host;
mod view;

use std::io::Write;

use host::{Menu, OUTCOME};
use rune::{App, AppSpec, Tx};

fn main() {
    let launch = match cli::parse() {
        Ok(launch) => launch,
        Err(message) => {
            eprintln!("rmenu: {message}");
            std::process::exit(2);
        }
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
        .on_quit(|m: &mut Menu, _| m.quit())
        .try_run();
    if let Err(error) = started {
        eprintln!("rmenu: {error}");
        std::process::exit(2);
    }
    let outcome = OUTCOME.lock().unwrap().take();
    let Some(outcome) = outcome else { std::process::exit(130) };
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = stdout.write_all(&outcome.output).and_then(|()| stdout.flush()) {
        eprintln!("rmenu: output failed: {error}");
        std::process::exit(2);
    }
    if let Some(message) = outcome.error {
        eprintln!("rmenu: {message}");
    }
    std::process::exit(outcome.status);
}
