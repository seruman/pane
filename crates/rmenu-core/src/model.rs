//! The menu as a reducer: `Model::handle(event) -> effects`. Port of mmenu's
//! `Sources/MMenu/MenuModel.swift`, with its async tasks turned into [`Effect::Start`] and
//! their results into [`Event`]s that carry the token they were started with.

use std::mem;
use crate::items::Items;
use std::time::Duration;

use crate::command::{CommandResult, Limits};
use crate::config::{BindingAction, InputFormat, KeyChord, MarkScope, MenuBinding, MenuConfiguration, MenuSource, SourceId};
use crate::item::{ActionContext, MenuItem, PathEntry};
use crate::search::{Order, RowOffset, SearchIndex};
use crate::selection::{Marks, MenuSelection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat { Lines, Nul, Json }

/// Rune currently keys all virtual rows on each result change. Keep this bound until
/// its list API can key only visible rows. The existing 5,000-row safety bound is not a
/// measured usability limit.
pub const DEFAULT_LIST_CAP: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListState<'a> {
    Rows(usize),
    Failed(&'a str),
    Loading,
    Searching,
    Empty,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub configuration: MenuConfiguration,
    pub input: InputFormat,
    pub multi: bool,
    pub output: OutputFormat,
    pub sorted: bool,
    pub prompt: String,
    pub query: String,
    /// Visible rows; PageUp/PageDown move by this many.
    pub rows: usize,
    /// How many result rows the host lists; the highlight stays within them.
    pub list_cap: usize,
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

// Command limits match mmenu 7c8f662, Sources/MMenu/MenuModel.swift.
pub const SOURCE_LIMITS: Limits = Limits { timeout: Duration::from_secs(30), output_bytes: 16 * 1024 * 1024 };
pub const PREVIEW_LIMITS: Limits = Limits { timeout: Duration::from_secs(5), output_bytes: 512 * 1024 };
pub const OPERATION_LIMITS: Limits = Limits { timeout: Duration::from_secs(30), output_bytes: 1024 * 1024 };

// Preserve mmenu 7c8f662 MenuModel.swift preview debounce and stderr truncation.
// These are compatibility choices, not measured optima for terminal interaction.
const PREVIEW_DELAY: Duration = Duration::from_millis(60);
const COMMAND_ERROR_BYTES: usize = 2048;

#[derive(Clone, Debug)]
pub enum Task {
    /// Run a source command under `SOURCE_LIMITS` and decode its stdout as `format`, answer
    /// `SourceDone`. A nonzero exit is an error worded by [`command_failure`].
    Source { token: Token, argv: Vec<String>, input: Vec<u8>, env: Vec<(String, String)>, format: InputFormat },
    /// Search `index` off the UI thread, answer `SearchDone`.
    Search { token: Token, index: SearchIndex, query: String, order: Order },
    /// After `delay`, run a preview command under `PREVIEW_LIMITS` with stdin on /dev/null, answer `PreviewDone`.
    Preview { token: Token, argv: Vec<String>, env: Vec<(String, String)>, delay: Duration },
    /// Run an operation under `OPERATION_LIMITS`, answer `OperationDone`.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Chosen(Vec<u8>),
    Cancelled,
    Failed(String),
}

impl Outcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Chosen(_) => 0,
            Self::Cancelled => 130,
            Self::Failed(_) => 2,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Effect {
    /// Read piped stdin as `Options::input` in `StdinItems` batches, then answer
    /// `StdinDone` or `StdinFailed`. Issued at most once.
    ReadStdin,
    Start(Task),
    Cancel(TaskKind),
    /// Put this text on the clipboard.
    Copy(String),
    /// The action was refused (mmenu's NSSound.beep). A host may flash or ignore it.
    Beep,
    /// Cancel and reap background work, then publish the process outcome.
    Finish(Outcome),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingIndex(pub usize);

#[derive(Clone, Debug)]
pub enum Event {
    SetQuery(String),
    Nav(Nav),
    /// A click: highlight this result row.
    Highlight(usize),
    /// A modified click with `--multi`: mark or unmark this row.
    ToggleRow(usize),
    /// Run the configuration's binding at this index (`configuration.bindings[i]`).
    Perform(BindingIndex),
    SwitchSource(SourceId),
    StdinItems(Vec<MenuItem>),
    StdinDone,
    StdinFailed(String),
    SourceDone { token: Token, result: Result<Vec<MenuItem>, String> },
    SearchDone { token: Token, results: Vec<RowOffset> },
    PreviewDone { token: Token, result: Result<CommandResult, String> },
    OperationDone { token: Token, result: Result<CommandResult, String> },
}

/// What a key bound in the configuration does at the current level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Perform(BindingIndex),
    SwitchSource(SourceId),
}

/// How a nonzero exit is shown: `Command exited 3: <first 2 KiB of stderr, trimmed>`.
pub fn command_failure(result: &CommandResult) -> String {
    let stderr = &result.stderr[..result.stderr.len().min(COMMAND_ERROR_BYTES)];
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        format!("Command exited {}", result.status.shell_code())
    } else {
        format!("Command exited {}: {detail}", result.status.shell_code())
    }
}

/// The rows on screen and what was matched to produce them. They may belong to an older
/// source generation than the level's `store` while its replacement loads.
#[derive(Clone, Default)]
struct Shown {
    items: Items,
    results: Vec<RowOffset>,
    selection: MenuSelection,
    /// The query `results` were matched against; None before the first search lands.
    query: Option<String>,
}

/// A level the user pushed away from, kept exactly as presented. Popping restores it
/// without re-running its command.
struct Level {
    source_id: Option<SourceId>,
    store: Items,
    shown: Shown,
    query: String,
    loading: bool,
    input_failed: bool,
    message: Option<Status>,
    /// What was chosen here when the child was pushed: the child's input and its path entry.
    chosen: Vec<MenuItem>,
    /// Piped stdin kept streaming into this (root) level while a child was shown.
    stdin_changed: bool,
}

/// The status line, and what retires it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    /// Answers one action ("Copied 2", "x completed"); the next key or move retires it.
    Ack(String),
    /// An operation is running; it changes when the operation ends.
    Running(String),
    /// Something failed or was refused and the user has to see it: kept until the next
    /// action replaces it.
    Error(String),
    StdinFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    Replace,
    Reload,
}

struct SearchRun {
    query: String,
    items: Items,
}

struct Operation {
    token: Token,
    action: String,
    reload: bool,
    source_generation: Token,
}

/// Enter or a push pressed while the typed query's results were still being computed.
enum Intent {
    Accept(String),
    Push(SourceId),
}

pub struct Model {
    options: Options,
    effects: Vec<Effect>,
    next_token: Token,

    levels: Vec<Level>,
    source_id: Option<SourceId>,
    store: Items,
    shown: Shown,
    query: String,
    loading: bool,
    input_failed: bool,
    message: Option<Status>,
    result_revision: u64,

    /// Piped stdin broke: the exit status is 2 and this is why, at whatever level is shown.
    stdin_error: Option<String>,
    search: Option<SearchRun>,
    operation: Option<Operation>,
    preview_enabled: bool,
    preview_text: String,
    preview_identity: Option<(Token, MenuItem, String)>,

    generation: Token,
    source_generation: Token,
    preview_generation: Token,
    displayed_source_generation: Token,
    pending_data_search: bool,
    pending_prune: bool,
    pending_selection_reset: bool,
    // Command sources publish only after both ingestion and matching finish.
    awaiting_source_data: bool,
    pending_intent: Option<Intent>,
    closing: bool,
}

impl Model {
    /// The model and the effects that start it: the root source, or reading stdin.
    pub fn new(options: Options) -> (Model, Vec<Effect>) {
        let query = options.query.clone();
        let mut model = Model {
            options,
            effects: Vec::new(),
            next_token: 0,
            levels: Vec::new(),
            source_id: None,
            store: Items::default(),
            shown: Shown::default(),
            query,
            loading: true,
            input_failed: false,
            message: None,
            result_revision: 0,
            stdin_error: None,
            search: None,
            operation: None,
            preview_enabled: true,
            preview_text: String::new(),
            preview_identity: None,
            generation: 0,
            source_generation: 0,
            preview_generation: 0,
            displayed_source_generation: Token::MAX,
            pending_data_search: false,
            pending_prune: false,
            pending_selection_reset: false,
            awaiting_source_data: false,
            pending_intent: None,
            closing: false,
        };
        match model.options.configuration.root_source().cloned() {
            Some(source) => model.load(&source, LoadKind::Replace),
            None => model.effects.push(Effect::ReadStdin),
        }
        let effects = mem::take(&mut model.effects);
        (model, effects)
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        if !self.closing {
            self.dispatch(event);
        }
        mem::take(&mut self.effects)
    }

    // Read-only state for drawing. Rows index the shown results.

    pub fn options(&self) -> &Options { &self.options }

    /// The prompt chip: the level's prompt, after the parent's choice once pushed (`repo › Worktrees`).
    pub fn prompt(&self) -> String {
        let source = self.source();
        let Some(parent) = self.levels.last() else {
            return source.and_then(|s| s.prompt.clone()).unwrap_or_else(|| self.options.prompt.clone());
        };
        let label = source.map(|s| s.prompt.clone().unwrap_or_else(|| s.label.clone())).unwrap_or_default();
        let Some(first) = parent.chosen.first() else { return label };
        let more = if parent.chosen.len() > 1 { format!(" +{}", parent.chosen.len() - 1) } else { String::new() };
        format!("{}{more} › {label}", first.title())
    }

    pub fn query(&self) -> &str { &self.query }

    /// How many result rows are shown.
    pub fn result_count(&self) -> usize { self.shown.results.len() }

    pub fn list_state(&self) -> ListState<'_> {
        let count = self.listed_count();
        if count > 0 { ListState::Rows(count) }
        else if self.input_failed() { ListState::Failed(self.message().unwrap_or("Input failed")) }
        else if self.loading() { ListState::Loading }
        else if self.is_searching() { ListState::Searching }
        else { ListState::Empty }
    }

    pub fn listed_row(&self, id: &str) -> Option<usize> {
        (0..self.listed_count()).find(|&row| self.result(row).id == id)
    }

    pub fn result(&self, row: usize) -> &MenuItem { &self.shown.items[self.shown.results[row]] }

    /// Changes whenever the shown results are replaced, so a host can reset scrolling.
    pub fn result_revision(&self) -> u64 { self.result_revision }

    pub fn highlighted_row(&self) -> Option<usize> { self.shown.selection.highlighted_row() }

    pub fn is_marked(&self, item: &MenuItem) -> bool { self.shown.selection.is_marked(&item.id) }

    /// How many rows are marked at this level.
    pub fn marked_count(&self) -> usize { self.shown.selection.marked_count() }

    /// Whether Enter would choose something: a mark, or a highlighted row.
    pub fn has_choice(&self) -> bool { self.marked_count() > 0 || self.highlighted().is_some() }

    pub fn loading(&self) -> bool { self.loading }

    /// A status or error line (`Copied 2`, `Command exited 1: ...`).
    pub fn message(&self) -> Option<&str> {
        match self.message.as_ref()? {
            Status::Ack(text) | Status::Running(text) | Status::Error(text) => Some(text),
            Status::StdinFailed => self.stdin_error(),
        }
    }

    /// The status line reports a failure, for drawing it as one.
    pub fn message_is_error(&self) -> bool { matches!(self.message, Some(Status::Error(_) | Status::StdinFailed)) }

    /// Why piped stdin failed, if it did: what an exit 2 reports.
    pub fn stdin_error(&self) -> Option<&str> { self.stdin_error.as_deref() }

    /// Some while the preview pane is shown; its text, possibly empty or "Loading…".
    pub fn preview(&self) -> Option<&str> { self.preview_visible().then_some(self.preview_text.as_str()) }

    /// The query the shown results were matched for, which can trail the typed query while
    /// its search runs. Match highlights use it so they agree with the rows.
    pub fn shown_query(&self) -> &str { self.shown.query.as_deref().unwrap_or("") }

    /// The current level's source ID; None for piped stdin.
    pub fn source_id(&self) -> Option<&SourceId> { self.source_id.as_ref() }

    /// How many levels are pushed above the root.
    pub fn depth(&self) -> usize { self.levels.len() }

    /// What a configured chord does here, if anything. Source keys apply at the root only.
    pub fn target(&self, chord: &KeyChord) -> Option<Target> {
        let configuration = &self.options.configuration;
        if self.levels.is_empty() {
            let source = configuration.sources().iter().filter(|s| !s.hidden).find(|s| s.key.as_ref() == Some(chord));
            if let Some(source) = source {
                return Some(Target::SwitchSource(source.id.clone()));
            }
        }
        configuration
            .bindings()
            .iter()
            .position(|b| b.applies(self.source_id.as_ref()) && &b.key == chord)
            .map(|index| Target::Perform(BindingIndex(index)))
    }

    /// The shown rows belong to the current source. Only the query may be ahead of them, so
    /// moving and marking act on what the user sees.
    pub fn can_navigate(&self) -> bool {
        let loading_command = self.loading && self.source_id.is_some();
        !self.input_failed
            && !loading_command
            && self.displayed_source_generation == self.source_generation
    }

    /// The shown rows are the current query's results: actions may run.
    fn can_act(&self) -> bool { self.can_navigate() && self.shown.query.as_deref() == Some(self.query.as_str()) }

    pub fn is_searching(&self) -> bool { self.search.is_some() }

    pub fn input_failed(&self) -> bool { self.input_failed }

    pub fn running_operation(&self) -> bool { self.operation.is_some() }

    /// Marked rows in input order, else the highlighted row: what an action would act on.
    fn chosen(&self) -> Vec<MenuItem> {
        if self.shown.selection.marked_count() == 0 {
            self.highlighted().into_iter().cloned().collect()
        } else {
            self.shown.selection.marked(&self.shown.items)
        }
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::SetQuery(value) => self.set_query(value),
            Event::Nav(nav) => self.nav(nav),
            Event::Highlight(row) => self.highlight(row),
            Event::ToggleRow(row) => self.toggle_row(row),
            Event::Perform(index) => {
                if let Some(binding) = self.options.configuration.bindings().get(index.0).cloned() {
                    self.perform(binding);
                }
            }
            Event::SwitchSource(id) => self.switch_source(&id),
            Event::StdinItems(batch) => {
                let store = match self.levels.first_mut() {
                    Some(root) => &mut root.store,
                    None => &mut self.store,
                };
                store.extend(batch);
                self.stdin_changed();
            }
            Event::StdinDone => {
                match self.levels.first_mut() {
                    Some(root) => root.loading = false,
                    None => self.loading = false,
                }
                self.stdin_changed();
            }
            Event::StdinFailed(error) => {
                self.stdin_error = Some(error);
                let (loading, failed, message) = match self.levels.first_mut() {
                    Some(root) => (&mut root.loading, &mut root.input_failed, &mut root.message),
                    None => (&mut self.loading, &mut self.input_failed, &mut self.message),
                };
                *loading = false;
                *failed = true;
                *message = Some(Status::StdinFailed);
            }
            Event::SourceDone { token, result } => self.source_done(token, result),
            Event::SearchDone { token, results } => self.search_done(token, results),
            Event::PreviewDone { token, result } => {
                if token != self.preview_generation {
                    return;
                }
                let text = match result {
                    Ok(r) if r.status.success() => String::from_utf8_lossy(&r.stdout).into_owned(),
                    Ok(r) => command_failure(&r),
                    Err(error) => error,
                };
                self.preview_text = crate::preview::plain_text(&text);
            }
            Event::OperationDone { token, result } => self.operation_done(token, result),
        }
    }

    fn nav(&mut self, nav: Nav) {
        let rows = self.options.rows as isize;
        match nav {
            Nav::Up | Nav::BackTab => self.move_by(-1),
            Nav::Down => self.move_by(1),
            Nav::PageUp => self.move_by(-rows),
            Nav::PageDown => self.move_by(rows),
            Nav::Accept => self.accept("accept".into()),
            Nav::Back => {
                if self.levels.is_empty() { self.cancel() } else { self.pop() }
            }
            Nav::PopIfEmpty => {
                if self.query.is_empty() && !self.levels.is_empty() {
                    self.pop();
                }
            }
            Nav::Tab => {
                if self.options.multi { self.toggle() } else { self.move_by(1) }
            }
            Nav::Cancel => self.cancel(),
        }
    }

    fn fresh(&mut self) -> Token {
        self.next_token += 1;
        self.next_token
    }

    fn source(&self) -> Option<&MenuSource> { self.source_id.as_ref().and_then(|id| self.options.configuration.source(id)) }

    fn preview_command(&self) -> Option<Vec<String>> {
        self.source().and_then(|s| s.preview.clone()).or_else(|| self.options.configuration.preview().map(<[String]>::to_vec))
    }

    fn preview_visible(&self) -> bool { self.preview_enabled && self.preview_command().is_some() }

    /// How many shown rows the host lists: the only rows a highlight or mark may land on.
    pub fn listed_count(&self) -> usize { self.shown.results.len().min(self.options.list_cap) }

    fn highlighted(&self) -> Option<&MenuItem> {
        let row = self.shown.selection.highlighted_row()?;
        self.shown.results.get(row).map(|&o| &self.shown.items[o])
    }

    fn set_query(&mut self, value: String) {
        if value == self.query {
            return;
        }
        self.query = value;
        self.invalidate_preview();
        self.clear_transient_message();
        self.request_search(false, false);
    }

    fn highlight(&mut self, row: usize) {
        if !self.can_navigate() || row >= self.listed_count() {
            return;
        }
        if self.shown.selection.highlighted_row() != Some(row) {
            self.clear_transient_message();
        }
        let id = self.result(row).id.clone();
        self.shown.selection.highlight(row, id);
        self.update_preview();
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.listed_count();
        if count == 0 {
            return;
        }
        let row = (self.shown.selection.highlighted_row().unwrap_or(0) as isize + delta).clamp(0, count as isize - 1);
        self.highlight(row as usize);
    }

    fn toggle(&mut self) {
        if !self.can_navigate() || !self.options.multi {
            return;
        }
        let Some(id) = self.highlighted().map(|i| i.id.clone()) else { return };
        self.shown.selection.toggle(&id);
        self.move_by(1);
    }

    fn toggle_row(&mut self, row: usize) {
        if !self.can_navigate() || !self.options.multi || row >= self.listed_count() {
            return;
        }
        let id = self.result(row).id.clone();
        self.shown.selection.toggle(&id);
        self.highlight(row);
    }

    fn perform(&mut self, binding: MenuBinding) {
        if !binding.applies(self.source_id.as_ref()) {
            return;
        }
        match &binding.action {
            BindingAction::Accept => self.accept(binding.action_name().to_owned()),
            BindingAction::Copy => {
                if !self.can_act() {
                    self.effects.push(Effect::Beep);
                    return;
                }
                let values: Vec<String> = self.chosen().iter().map(|i| i.value().to_owned()).collect();
                if values.is_empty() {
                    self.effects.push(Effect::Beep);
                    return;
                }
                self.effects.push(Effect::Copy(values.join("\n")));
                self.message = Some(Status::Ack(format!("Copied {}", values.len())));
            }
            BindingAction::Source { source } => self.switch_source(source),
            BindingAction::Push { source } => self.push(source.clone()),
            BindingAction::Reload => self.reload(),
            BindingAction::TogglePreview => {
                if self.preview_command().is_none() {
                    return;
                }
                self.preview_enabled = !self.preview_enabled;
                self.invalidate_preview();
                self.preview_text.clear();
                self.update_preview();
            }
            BindingAction::Run { command, reload } => {
                if self.operation.is_some() || !self.can_act() || self.chosen().is_empty() {
                    self.effects.push(Effect::Beep);
                    return;
                }
                self.run_operation(command.clone(), binding.action_name().to_owned(), *reload);
            }
        }
    }

    /// Sibling sources belong to the root; a pushed level's input is its parent's choice.
    fn switch_source(&mut self, id: &SourceId) {
        if !self.levels.is_empty() || self.source_id.as_ref() == Some(id) {
            return;
        }
        let Some(source) = self.options.configuration.source(id).filter(|s| !s.hidden).cloned() else { return };
        self.load(&source, LoadKind::Replace);
    }

    fn push(&mut self, source: SourceId) {
        if !self.can_act() && self.can_navigate() {
            self.pending_intent = Some(Intent::Push(source));
            return;
        }
        if !self.can_act() {
            self.effects.push(Effect::Beep);
            return;
        }
        let picked = self.chosen();
        if picked.is_empty() {
            self.effects.push(Effect::Beep);
            return;
        }
        let stdin_changed = self.source_id.is_none() && (self.search.is_some() || self.pending_data_search);
        self.levels.push(Level {
            source_id: self.source_id.take(),
            store: mem::take(&mut self.store),
            // The parent's rows stay presented, non-actionable, until the child commits.
            shown: self.shown.clone(),
            query: mem::take(&mut self.query),
            loading: self.loading,
            input_failed: self.input_failed,
            message: self.message.clone(),
            chosen: picked,
            stdin_changed,
        });
        let target = self.options.configuration.source(&source).expect("validated push source").clone();
        self.load(&target, LoadKind::Replace);
    }

    fn pop(&mut self) {
        let Some(parent) = self.levels.pop() else { return };
        self.effects.extend([TaskKind::Source, TaskKind::Search, TaskKind::Preview].map(Effect::Cancel));
        self.pending_intent = None;
        // Late child source/search/preview results carry stale tokens and cannot publish.
        self.source_generation = self.fresh();
        self.generation = self.fresh();
        self.preview_generation = self.fresh();
        self.preview_identity = None;
        self.awaiting_source_data = false;
        self.search = None;
        self.pending_data_search = false;
        self.pending_prune = false;
        self.pending_selection_reset = false;
        self.source_id = parent.source_id;
        self.store = parent.store;
        self.shown = parent.shown;
        self.query = parent.query;
        self.loading = parent.loading;
        self.input_failed = parent.input_failed;
        // A parent's "Running X…" is stale once its operation has finished.
        self.message = parent.message.filter(|m| !matches!(m, Status::Running(_)) || self.operation.is_some());
        self.preview_text.clear();
        self.displayed_source_generation = self.source_generation;
        self.result_revision += 1;
        self.update_preview();
        if parent.stdin_changed {
            self.request_search(false, true);
        }
    }

    fn reload(&mut self) {
        match self.source().cloned() {
            Some(source) => self.load(&source, LoadKind::Reload),
            None => {
                self.message = Some(Status::Error("Piped input cannot be re-run".into()));
                self.effects.push(Effect::Beep);
            }
        }
    }

    fn load(&mut self, source: &MenuSource, kind: LoadKind) {
        self.source_id = Some(source.id.clone());
        self.pending_intent = None;
        self.effects.push(Effect::Cancel(TaskKind::Search));
        self.source_generation = self.fresh();
        self.generation = self.fresh();
        self.search = None;
        self.pending_data_search = false;
        self.pending_prune = true;
        self.store = Items::default();
        self.loading = true;
        self.awaiting_source_data = true;
        self.input_failed = false;
        self.message = None;
        if kind == LoadKind::Replace {
            self.preview_text.clear();
        }
        self.pending_selection_reset |= kind == LoadKind::Replace || source.format != InputFormat::Jsonl;
        self.update_preview();
        let context = ActionContext {
            action: "source".into(),
            query: self.query.clone(),
            source: Some(source.id.to_string()),
            items: self.levels.last().map(|l| l.chosen.clone()).unwrap_or_default(),
            path: self.ancestors(),
        };
        self.effects.push(Effect::Start(Task::Source {
            token: self.source_generation,
            argv: source.command.clone(),
            input: context.json(),
            env: context.environment(),
            format: source.format,
        }));
    }

    fn source_done(&mut self, token: Token, result: Result<Vec<MenuItem>, String>) {
        if token != self.source_generation || !self.awaiting_source_data {
            return;
        }
        self.awaiting_source_data = false;
        match result {
            Ok(items) => {
                self.store = Items::from(items);
                // A reload can change preview content without changing an item's ID/value.
                self.invalidate_preview();
            }
            Err(error) => {
                // Failure is a settled empty source, published like a successful empty one.
                self.store = Items::default();
                self.input_failed = true;
                self.message = Some(Status::Error(error));
            }
        }
        self.request_search(true, false);
    }

    // Piped stdin always belongs to the root level, which may be behind pushed children.
    fn stdin_changed(&mut self) {
        match self.levels.first_mut() {
            Some(root) => root.stdin_changed = true,
            None => self.request_search(false, true),
        }
    }

    fn request_search(&mut self, prune: bool, data_changed: bool) {
        // Query edits during a command update the eventual match query, not the still-empty
        // replacement store or the last presented source snapshot.
        if self.awaiting_source_data {
            return;
        }
        // Data arrival must not perpetually cancel a slower search. Publish its snapshot,
        // then catch up; query and source changes still cancel immediately.
        if data_changed && self.search.is_some() {
            self.pending_data_search = true;
            return;
        }
        self.pending_data_search = false;
        self.pending_prune |= prune;
        self.generation = self.fresh();
        self.search = Some(SearchRun { query: self.query.clone(), items: self.store.clone() });
        self.effects.push(Effect::Start(Task::Search {
            token: self.generation,
            index: self.store.clone(),
            query: self.query.clone(),
            order: if self.options.sorted { Order::Ranked } else { Order::Input },
        }));
    }

    fn search_done(&mut self, token: Token, results: Vec<RowOffset>) {
        if token != self.generation {
            return;
        }
        let Some(run) = self.search.take() else { return };
        self.shown.items = run.items;
        self.shown.results = results;
        self.result_revision += 1;
        if self.source_id.is_some() {
            self.loading = false;
        }
        if mem::take(&mut self.pending_selection_reset) {
            self.shown.selection = MenuSelection::default();
        }
        let prune_now = self.pending_prune && !self.loading;
        let listed = self.listed_count();
        self.shown.selection.reconcile(&self.shown.items, &self.shown.results[..listed], if prune_now { Marks::Prune } else { Marks::Keep });
        if prune_now {
            self.pending_prune = false;
        }
        self.shown.query = Some(run.query);
        self.displayed_source_generation = self.source_generation;
        self.update_preview();
        if self.can_act()
            && let Some(intent) = self.pending_intent.take()
        {
            match intent {
                Intent::Accept(action) => {
                    self.accept(action);
                    if self.closing {
                        return;
                    }
                }
                Intent::Push(source) => {
                    let depth = self.levels.len();
                    self.push(source);
                    if self.levels.len() != depth {
                        return;
                    }
                }
            }
        }
        if self.pending_data_search {
            self.request_search(false, true);
        }
    }

    fn update_preview(&mut self) {
        let item = self.highlighted().cloned();
        let shown = self.preview_visible() && self.can_act();
        let identity = item.clone().filter(|_| shown).map(|i| (self.source_generation, i, self.query.clone()));
        // Settled with nothing highlighted (no matches, an empty source): there is nothing to
        // preview, so the last item's text must not stay beside it.
        if identity.is_none() && self.can_act() {
            self.preview_text.clear();
        }
        if identity == self.preview_identity {
            return;
        }
        self.preview_identity = identity;
        self.preview_generation = self.fresh();
        let (Some(argv), Some(item), true) = (self.preview_command(), item, shown) else {
            self.effects.push(Effect::Cancel(TaskKind::Preview));
            self.preview_text.clear();
            return;
        };
        let context = ActionContext {
            action: "preview".into(),
            query: self.query.clone(),
            source: self.source_id.as_ref().map(ToString::to_string),
            items: vec![item.clone()],
            path: self.path(vec![item]),
        };
        // The last preview stays until this one arrives. Only preview execution is delayed; the highlight itself changes immediately.
        self.effects.push(Effect::Start(Task::Preview {
            token: self.preview_generation,
            argv,
            env: context.environment(),
            delay: PREVIEW_DELAY,
        }));
    }

    /// Clears a status line that answered an earlier action. Source failures stay: they
    /// describe the rows on screen.
    fn clear_transient_message(&mut self) {
        if matches!(self.message, Some(Status::Ack(_))) {
            self.message = None;
        }
    }

    /// Late preview output carries a stale token and cannot publish.
    fn invalidate_preview(&mut self) {
        self.preview_generation = self.fresh();
        self.preview_identity = None;
        self.effects.push(Effect::Cancel(TaskKind::Preview));
    }

    fn run_operation(&mut self, argv: Vec<String>, action: String, reload: bool) {
        let chosen = self.chosen();
        let context = ActionContext {
            action: action.clone(),
            query: self.query.clone(),
            source: self.source_id.as_ref().map(ToString::to_string),
            items: chosen.clone(),
            path: self.path(chosen),
        };
        let token = self.fresh();
        self.message = Some(Status::Running(format!("Running {action}…")));
        self.operation = Some(Operation { token, action, reload, source_generation: self.source_generation });
        self.effects.push(Effect::Start(Task::Operation {
            token,
            argv,
            input: context.json(),
            env: context.environment(),
        }));
    }

    fn operation_done(&mut self, token: Token, result: Result<CommandResult, String>) {
        let Some(operation) = self.operation.take_if(|o| o.token == token) else { return };
        match result {
            Ok(r) if r.status.success() => {
                self.message = Some(Status::Ack(format!("{} completed", operation.action)));
                if operation.reload && operation.source_generation == self.source_generation {
                    self.reload();
                }
            }
            Ok(r) => {
                self.message = Some(Status::Error(command_failure(&r)));
            }
            Err(error) => {
                self.message = Some(Status::Error(error));
            }
        }
    }

    fn accept(&mut self, action: String) {
        // Only the query is ahead of the shown rows: accept from its results when they land.
        if !self.can_act() && self.can_navigate() && self.operation.is_none() {
            self.pending_intent = Some(Intent::Accept(action));
            return;
        }
        if !self.can_act() || self.operation.is_some() {
            self.effects.push(Effect::Beep);
            return;
        }
        let picked = self.chosen();
        if picked.is_empty() {
            self.effects.push(Effect::Beep);
            return;
        }
        let items = match self.options.configuration.marks() {
            MarkScope::Level => picked.clone(),
            MarkScope::Global => {
                let marked: Vec<MenuItem> = self
                    .levels
                    .iter()
                    .map(|l| &l.shown)
                    .chain([&self.shown])
                    .filter(|s| s.selection.marked_count() != 0)
                    .flat_map(|s| s.selection.marked(&s.items))
                    .collect();
                if marked.is_empty() { picked.clone() } else { marked }
            }
        };
        let output = if self.options.output == OutputFormat::Json {
            let context = ActionContext {
                action,
                query: self.query.clone(),
                source: self.source_id.as_ref().map(ToString::to_string),
                items,
                path: self.path(picked),
            };
            let mut output = context.json();
            output.push(b'\n');
            output
        } else {
            let delimiter = if self.options.output == OutputFormat::Nul { '\0' } else { '\n' };
            // A delimiter inside a value would split it into extra records downstream.
            if items.iter().any(|i| i.value().contains(delimiter)) {
                let hint = if self.options.output == OutputFormat::Nul { "" } else { " or --print0" };
                self.message = Some(Status::Error(format!("A selected value contains the output delimiter; use --json{hint}")));
                self.effects.push(Effect::Beep);
                return;
            }
            items.iter().map(|i| format!("{}{delimiter}", i.value())).collect::<String>().into_bytes()
        };
        self.finish(Outcome::Chosen(output));
    }

    // A failed command source is one source; the user can switch or just cancel.
    // Failed piped stdin means the caller's whole input broke, which is an error.
    fn cancel(&mut self) {
        let outcome = self.stdin_error.clone().map_or(Outcome::Cancelled, Outcome::Failed);
        self.finish(outcome);
    }

    fn finish(&mut self, outcome: Outcome) {
        self.closing = true;
        self.effects.push(Effect::Finish(outcome));
    }

    /// Root to the current level; each ancestor holds what was chosen when it was pushed.
    fn path(&self, current: Vec<MenuItem>) -> Vec<PathEntry> {
        let mut path = self.ancestors();
        path.push(PathEntry { source: self.source_id.as_ref().map(ToString::to_string), query: self.query.clone(), items: current });
        path
    }

    fn ancestors(&self) -> Vec<PathEntry> {
        self.levels
            .iter()
            .map(|l| PathEntry { source: l.source_id.as_ref().map(ToString::to_string), query: l.query.clone(), items: l.chosen.clone() })
            .collect()
    }
}

#[cfg(test)]
mod tests;
