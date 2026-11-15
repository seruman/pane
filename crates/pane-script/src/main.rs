//! Run a folder of Luau UI with upstream `rune-luau`, in a native window or Rune's terminal
//! shell. Saving a `.luau` file in the folder reloads it in place.

mod host;
mod palette;

use std::path::{Path, PathBuf};

use rune::{AppSpec, Cx, El, Tx, Waker, col};
use rune_luau::{Script, ScriptOptions, Sources};

pub(crate) struct State {
    script: Script,
    terminal_colors: Option<rune::TerminalColors>,
}

/// Loads `dir` (its `init.luau`) and watches it. A missing folder or a script that fails to
/// start still opens: the window shows the error card, and a later save recovers.
pub(crate) fn open(dir: &Path, capabilities: host::Capabilities) -> State {
    let name = dir
        .file_name()
        .map_or_else(|| "script".into(), |n| n.to_string_lossy().into_owned());
    let root = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
    let options = ScriptOptions::new()
        .setup(move |host| host::install(host, root.clone(), capabilities))
        .palette(palette::current())
        .on_log(|level, line| eprintln!("pane-script {level:?}: {line}"));
    let sources = Sources::from_dir(dir).unwrap_or_else(|error| {
        eprintln!("pane-script: reading {}: {error}", dir.display());
        Sources::new()
    });
    let script = Script::with_options(&name, sources, options);
    if let Err(error) = script.watch(dir) {
        eprintln!("pane-script: not watching {}: {error}", dir.display());
    }
    State {
        script,
        terminal_colors: rune::terminal_colors(),
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    col().w_full().h_full().child(state.script.view(cx))
}

fn setup(state: &mut State, waker: Waker) {
    state.script.set_waker(waker);
}

fn poll(state: &mut State, now: f64, tx: &mut Tx) -> bool {
    let colors = rune::terminal_colors();
    let changed = state.terminal_colors != colors;
    if changed {
        state.terminal_colors = colors;
        // Color replies can arrive after the first memoized render, even in idle scripts.
        tx.invalidate_memos_by_prefix(state.script.memo_prefix());
    }
    state.script.poll(now, tx) || changed
}

/// `pane-script examples/counter` and `pane-script examples/counter/init.luau` run the same
/// folder.
fn folder(arg: &str) -> PathBuf {
    let path = PathBuf::from(arg);
    if path.extension().is_some_and(|e| e == "luau") {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_owned()
    } else {
        path
    }
}

fn arguments(args: &[String]) -> Result<(PathBuf, host::Capabilities), String> {
    let mut capabilities = host::Capabilities::default();
    let mut dir = None;
    for arg in args {
        match arg.as_str() {
            "--allow-http" => capabilities.http = true,
            "--allow-process" => capabilities.process = true,
            "--allow-fs" => capabilities.fs = true,
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            value if dir.is_none() => dir = Some(folder(value)),
            _ => return Err("expected one script folder".into()),
        }
    }
    Ok((dir.ok_or("missing script folder")?, capabilities))
}

fn main() {
    let (dir, capabilities) = arguments(&rune::extra_args()).unwrap_or_else(|error| {
        eprintln!("{error}\nusage: pane-script <folder> [--allow-http] [--allow-process] [--allow-fs] [Rune flags]");
        std::process::exit(2);
    });
    rune::App::new(spec())
        .init(move || open(&dir, capabilities))
        .tab_visits_controls(true)
        .run();
}

/// `App::init` supplies the real state; this placeholder only satisfies `AppSpec::new`.
fn empty() -> State {
    State {
        script: Script::new(
            "empty",
            Sources::single("return { render = function() return nil end }"),
        ),
        terminal_colors: rune::terminal_colors(),
    }
}

pub(crate) fn spec() -> AppSpec<State> {
    AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new("Pane Script", (640.0, 480.0), empty, view)
    }
}

#[cfg(test)]
mod tests;
