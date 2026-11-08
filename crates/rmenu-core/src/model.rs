//! The menu as a reducer: `Model::handle(event) -> effects`. Port of mmenu's
//! `Sources/MMenu/MenuModel.swift`, with its async tasks turned into [`Effect::Start`] and
//! their results into [`Event`]s that carry the token they were started with.

use std::time::Duration;

use crate::command::CommandResult;
use crate::config::{InputFormat, MenuConfiguration};
use crate::item::MenuItem;
use crate::search::SearchIndex;

/// What the command line decided; `MenuConfiguration` is already validated.
#[derive(Clone, Debug)]
pub struct Options {
    pub configuration: MenuConfiguration,
    pub input: InputFormat,
    pub multi: bool,
    pub json: bool,
    pub print0: bool,
    pub sorted: bool,
    pub prompt: String,
    pub query: String,
    /// Visible rows; PageUp/PageDown move by this many.
    pub rows: usize,
}

/// A started task's identity. A result whose token is stale is dropped.
pub type Token = u64;

/// Background work kinds. Starting a task replaces (cancels) the running one of its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Source,
    Search,
    Preview,
    Operation,
}

#[derive(Clone, Debug)]
pub enum Task {
    /// Run a source command (16 MiB, 30 s), decode its stdout as `format`, answer `SourceDone`.
    Source { token: Token, argv: Vec<String>, input: Vec<u8>, env: Vec<(String, String)>, format: InputFormat },
    /// Search `index` off the UI thread, answer `SearchDone`.
    Search { token: Token, index: SearchIndex, query: String, sorted: bool },
    /// After `delay`, run a preview command with stdin on /dev/null (512 KiB, 5 s), answer `PreviewDone`.
    Preview { token: Token, argv: Vec<String>, env: Vec<(String, String)>, delay: Duration },
    /// Run an operation (1 MiB, 30 s), answer `OperationDone`.
    Operation { token: Token, argv: Vec<String>, input: Vec<u8>, env: Vec<(String, String)> },
}

impl Task {
    pub fn kind(&self) -> TaskKind {
        match self {
            Task::Source { .. } => TaskKind::Source,
            Task::Search { .. } => TaskKind::Search,
            Task::Preview { .. } => TaskKind::Preview,
            Task::Operation { .. } => TaskKind::Operation,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Effect {
    /// Start reading piped stdin as `Options::input`, answering `StdinItems` batches (at most
    /// one per 16 ms after the first), then `StdinDone` or `StdinFailed`. Issued at most once.
    ReadStdin,
    Start(Task),
    Cancel(TaskKind),
    /// Put this text on the clipboard.
    Copy(String),
    /// The action was refused (mmenu's NSSound.beep). A host may flash or ignore it.
    Beep,
    /// Cancel every task, reap their process groups, write `output` to stdout, exit `status`.
    Finish { status: i32, output: Vec<u8> },
}

/// Directions the host translates keys into. Keys bound in the configuration arrive as
/// `Perform` / `SwitchSource` instead (see `Model::target`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    /// Enter.
    Accept,
    /// Escape: back one level, or cancel at the root.
    Back,
    /// Backspace with an empty query: back one level when pushed, else nothing.
    PopIfEmpty,
    /// Tab: mark and advance with `--multi`, else move down.
    Tab,
    /// Shift-Tab.
    BackTab,
    /// Ctrl-C, or the terminal going away: cancel the whole menu.
    Cancel,
}

#[derive(Clone, Debug)]
pub enum Event {
    SetQuery(String),
    Nav(Nav),
    /// A click: highlight this result row.
    Highlight(usize),
    /// A modified click with `--multi`: mark or unmark this row.
    ToggleRow(usize),
    /// Run the configuration's binding at this index (`configuration.bindings[i]`).
    Perform(usize),
    SwitchSource(String),
    StdinItems(Vec<MenuItem>),
    StdinDone,
    StdinFailed(String),
    SourceDone { token: Token, result: Result<Vec<MenuItem>, String> },
    SearchDone { token: Token, results: Vec<u32> },
    PreviewDone { token: Token, text: String },
    OperationDone { token: Token, result: Result<CommandResult, String> },
}

/// What a key bound in the configuration does at the current level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Perform(usize),
    SwitchSource(String),
}

pub struct Model {
    _todo: (),
}

impl Model {
    /// The model and the effects that start it: the root source, or reading stdin.
    pub fn new(options: Options) -> (Model, Vec<Effect>) {
        let _ = options;
        todo!()
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let _ = event;
        todo!()
    }

    // Read-only state for drawing. Rows are indices into `results()`.

    pub fn options(&self) -> &Options { todo!() }
    /// The prompt chip: the level's prompt, after the parent's choice once pushed (`repo › Worktrees`).
    pub fn prompt(&self) -> String { todo!() }
    pub fn query(&self) -> &str { todo!() }
    /// How many result rows are shown.
    pub fn result_count(&self) -> usize { todo!() }
    pub fn result(&self, row: usize) -> &MenuItem { todo!() }
    /// Changes whenever the shown results are replaced, so a host can reset scrolling.
    pub fn result_revision(&self) -> u64 { todo!() }
    pub fn highlighted_row(&self) -> Option<usize> { todo!() }
    pub fn is_marked(&self, item: &MenuItem) -> bool { todo!() }
    /// Rows are shown but not actionable: a source is loading or its search is running.
    pub fn is_stale(&self) -> bool { todo!() }
    pub fn loading(&self) -> bool { todo!() }
    /// A status or error line (`Copied 2`, `Command exited 1: ...`).
    pub fn message(&self) -> Option<&str> { todo!() }
    /// Some while the preview pane is shown; its text, possibly empty or "Loading…".
    pub fn preview(&self) -> Option<&str> { todo!() }
    /// The current level's source ID; None for piped stdin.
    pub fn source_id(&self) -> Option<&str> { todo!() }
    /// How many levels are pushed above the root.
    pub fn depth(&self) -> usize { todo!() }
    /// What a configured chord does here, if anything. Source keys apply at the root only.
    pub fn target(&self, chord: &crate::config::KeyChord) -> Option<Target> {
        let _ = chord;
        todo!()
    }
}

#[cfg(test)]
mod tests;
