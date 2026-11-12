//! Run one sandboxed Luau UI in a native window or Rune's terminal shell.

mod app;
mod vm;
mod watch;

fn main() {
    let args = rune::extra_args();
    if args.len() != 1 {
        eprintln!("usage: pane-script <file.luau> [Rune flags]");
        std::process::exit(2);
    }
    let state = app::State::open(std::path::PathBuf::from(&args[0]));
    rune::App::new(app::spec())
        .init(move || state)
        .tab_visits_controls(true)
        .run();
}
