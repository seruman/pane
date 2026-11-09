//! The menu as a reducer: `Model::handle(event) -> effects`. Port of mmenu's
//! `Sources/MMenu/MenuModel.swift`, with its async tasks turned into [`Effect::Start`] and
//! their results into [`Event`]s that carry the token they were started with.

use std::mem;
use crate::items::Items;
use std::time::Duration;

use crate::command::CommandResult;
use crate::config::{BindingAction, InputFormat, KeyChord, MarkScope, MenuBinding, MenuConfiguration, MenuSource};
use crate::item::{ActionContext, MenuItem, PathEntry};
use crate::search::SearchIndex;
use crate::selection::MenuSelection;

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

#[derive(Clone, Debug)]
pub enum Task {
    /// Run a source command (16 MiB, 30 s) and decode its stdout as `format`, answer
    /// `SourceDone`. A nonzero exit is an error worded by [`command_failure`].
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
    /// Read piped stdin as `Options::input` in `StdinItems` batches, then answer
    /// `StdinDone` or `StdinFailed`. Issued at most once.
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
    PreviewDone { token: Token, result: Result<CommandResult, String> },
    OperationDone { token: Token, result: Result<CommandResult, String> },
}

/// What a key bound in the configuration does at the current level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Perform(usize),
    SwitchSource(String),
}

/// How a nonzero exit is shown: `Command exited 3: <first 2 KiB of stderr, trimmed>`.
pub fn command_failure(result: &CommandResult) -> String {
    let stderr = &result.stderr[..result.stderr.len().min(2048)];
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        format!("Command exited {}", result.status)
    } else {
        format!("Command exited {}: {detail}", result.status)
    }
}

/// Rows of one source generation. Append-only; a reload gets a new store.
#[derive(Clone, Default)]
struct Store {
    items: Items,
    index: SearchIndex,
}

/// The rows on screen and what was matched to produce them. They may belong to an older
/// source generation than the level's `store` while its replacement loads.
#[derive(Clone, Default)]
struct Shown {
    items: Items,
    results: Vec<u32>,
    highlighted_row: Option<usize>,
    selection: MenuSelection,
    /// The query `results` were matched against; None before the first search lands.
    query: Option<String>,
}

/// A level the user pushed away from, kept exactly as presented. Popping restores it
/// without re-running its command.
struct Level {
    source_id: Option<String>,
    store: Store,
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
}

impl Status {
    fn text(&self) -> &str {
        match self { Status::Ack(s) | Status::Running(s) | Status::Error(s) => s }
    }
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
    Push(MenuBinding),
}

pub struct Model {
    options: Options,
    effects: Vec<Effect>,
    next_token: Token,

    levels: Vec<Level>,
    source_id: Option<String>,
    store: Store,
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
            store: Store::default(),
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
        match model.options.configuration.source(model.options.configuration.root_source()).cloned() {
            Some(source) => {
                model.source_id = Some(source.id.clone());
                model.pending_selection_reset = true;
                model.load(&source);
            }
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
        format!("{}{more} › {label}", first.title)
    }

    pub fn query(&self) -> &str { &self.query }

    /// How many result rows are shown.
    pub fn result_count(&self) -> usize { self.shown.results.len() }

    pub fn result(&self, row: usize) -> &MenuItem { &self.shown.items[self.shown.results[row] as usize] }

    /// Changes whenever the shown results are replaced, so a host can reset scrolling.
    pub fn result_revision(&self) -> u64 { self.result_revision }

    pub fn highlighted_row(&self) -> Option<usize> { self.shown.highlighted_row }

    pub fn is_marked(&self, item: &MenuItem) -> bool { self.shown.selection.checked.contains(&item.id) }

    /// How many rows are marked at this level.
    pub fn marked_count(&self) -> usize { self.shown.selection.checked.len() }

    /// Whether Enter would choose something: a mark, or a highlighted row.
    pub fn has_choice(&self) -> bool { self.marked_count() > 0 || self.highlighted().is_some() }

    /// Rows are shown but not actionable: a source is loading or its search is running.
    pub fn is_stale(&self) -> bool { !self.can_act() }

    pub fn loading(&self) -> bool { self.loading }

    /// A status or error line (`Copied 2`, `Command exited 1: ...`).
    pub fn message(&self) -> Option<&str> { self.message.as_ref().map(Status::text) }

    /// The status line reports a failure, for drawing it as one.
    pub fn message_is_error(&self) -> bool { matches!(self.message, Some(Status::Error(_))) }

    /// Why piped stdin failed, if it did: what an exit 2 reports.
    pub fn stdin_error(&self) -> Option<&str> { self.stdin_error.as_deref() }

    /// Some while the preview pane is shown; its text, possibly empty or "Loading…".
    pub fn preview(&self) -> Option<&str> { self.preview_visible().then_some(self.preview_text.as_str()) }

    /// The query the shown results were matched for, which can trail the typed query while
    /// its search runs. Match highlights use it so they agree with the rows.
    pub fn shown_query(&self) -> &str { self.shown.query.as_deref().unwrap_or("") }

    /// The current level's source ID; None for piped stdin.
    pub fn source_id(&self) -> Option<&str> { self.source_id.as_deref() }

    /// How many levels are pushed above the root.
    pub fn depth(&self) -> usize { self.levels.len() }

    /// What a configured chord does here, if anything. Source keys apply at the root only.
    pub fn target(&self, chord: &KeyChord) -> Option<Target> {
        let configuration = &self.options.configuration;
        if self.levels.is_empty() {
            let source = configuration.sources.iter().filter(|s| !s.hidden).find(|s| {
                s.key.as_deref().and_then(|k| KeyChord::parse(k).ok()).as_ref() == Some(chord)
            });
            if let Some(source) = source {
                return Some(Target::SwitchSource(source.id.clone()));
            }
        }
        configuration
            .bindings
            .iter()
            .position(|b| b.applies(self.source_id.as_deref()) && KeyChord::parse(&b.key).ok().as_ref() == Some(chord))
            .map(Target::Perform)
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
    pub fn can_act(&self) -> bool { self.can_navigate() && self.shown.query.as_deref() == Some(self.query.as_str()) }

    pub fn is_searching(&self) -> bool { self.search.is_some() }

    pub fn input_failed(&self) -> bool { self.input_failed }

    pub fn running_operation(&self) -> bool { self.operation.is_some() }

    /// Marked rows in input order, else the highlighted row: what an action would act on.
    pub fn chosen(&self) -> Vec<MenuItem> {
        if self.shown.selection.checked.is_empty() {
            self.highlighted().into_iter().cloned().collect()
        } else {
            self.shown.selection.chosen(&self.shown.items)
        }
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::SetQuery(value) => self.set_query(value),
            Event::Nav(nav) => self.nav(nav),
            Event::Highlight(row) => self.highlight(row),
            Event::ToggleRow(row) => self.toggle_row(row),
            Event::Perform(index) => {
                if let Some(binding) = self.options.configuration.bindings.get(index).cloned() {
                    self.perform(binding);
                }
            }
            Event::SwitchSource(id) => self.switch_source(&id),
            Event::StdinItems(batch) => {
                let store = match self.levels.first_mut() {
                    Some(root) => &mut root.store,
                    None => &mut self.store,
                };
                store.index.append(&batch);
                store.items.extend(batch);
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
                self.stdin_error = Some(error.clone());
                let (loading, failed, message) = match self.levels.first_mut() {
                    Some(root) => (&mut root.loading, &mut root.input_failed, &mut root.message),
                    None => (&mut self.loading, &mut self.input_failed, &mut self.message),
                };
                *loading = false;
                *failed = true;
                *message = Some(Status::Error(error));
            }
            Event::SourceDone { token, result } => self.source_done(token, result),
            Event::SearchDone { token, results } => self.search_done(token, results),
            Event::PreviewDone { token, result } => {
                if token != self.preview_generation {
                    return;
                }
                self.preview_text = match result {
                    Ok(r) if r.status == 0 => String::from_utf8_lossy(&r.stdout).into_owned(),
                    Ok(r) => command_failure(&r),
                    Err(error) => error,
                };
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

    fn source(&self) -> Option<&MenuSource> { self.options.configuration.source(self.source_id.as_deref()) }

    fn preview_command(&self) -> Option<Vec<String>> {
        self.source().and_then(|s| s.preview.clone()).or_else(|| self.options.configuration.preview.clone())
    }

    fn preview_visible(&self) -> bool { self.preview_enabled && self.preview_command().is_some() }

    /// How many shown rows the host lists: the only rows a highlight or mark may land on.
    fn listed(&self) -> usize { self.shown.results.len().min(self.options.list_cap) }

    fn highlighted(&self) -> Option<&MenuItem> {
        let row = self.shown.highlighted_row?;
        self.shown.results.get(row).map(|&o| &self.shown.items[o as usize])
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
        if !self.can_navigate() || row >= self.listed() {
            return;
        }
        if self.shown.highlighted_row != Some(row) {
            self.clear_transient_message();
        }
        self.shown.highlighted_row = Some(row);
        let id = self.result(row).id.clone();
        self.shown.selection.highlighted_id = Some(id);
        self.update_preview();
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.listed();
        if count == 0 {
            return;
        }
        let row = (self.shown.highlighted_row.unwrap_or(0) as isize + delta).clamp(0, count as isize - 1);
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
        if !self.can_navigate() || !self.options.multi || row >= self.listed() {
            return;
        }
        let id = self.result(row).id.clone();
        self.shown.selection.toggle(&id);
        self.highlight(row);
    }

    fn perform(&mut self, binding: MenuBinding) {
        if !binding.applies(self.source_id.as_deref()) {
            return;
        }
        match binding.action {
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
            BindingAction::Source => {
                if let Some(id) = &binding.source {
                    self.switch_source(id);
                }
            }
            BindingAction::Push => self.push(binding),
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
            BindingAction::Run => {
                let Some(command) = binding.command.clone() else { return };
                if self.operation.is_some() || !self.can_act() || self.chosen().is_empty() {
                    self.effects.push(Effect::Beep);
                    return;
                }
                self.run_operation(command, binding.action_name().to_owned(), binding.reload);
            }
        }
    }

    /// Sibling sources belong to the root; a pushed level's input is its parent's choice.
    fn switch_source(&mut self, id: &str) {
        if !self.levels.is_empty() || self.source_id.as_deref() == Some(id) {
            return;
        }
        let Some(source) = self.options.configuration.source(Some(id)).filter(|s| !s.hidden).cloned() else {
            return;
        };
        self.source_id = Some(source.id.clone());
        self.pending_selection_reset = true;
        self.load(&source);
    }

    fn push(&mut self, binding: MenuBinding) {
        let Some(target) = self.options.configuration.source(binding.source.as_deref()).cloned() else { return };
        if !self.can_act() && self.can_navigate() {
            self.pending_intent = Some(Intent::Push(binding));
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
        self.source_id = Some(target.id.clone());
        self.pending_selection_reset = true;
        self.load(&target);
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
            Some(source) => self.load(&source),
            None => {
                self.message = Some(Status::Error("Piped input cannot be re-run".into()));
                self.effects.push(Effect::Beep);
            }
        }
    }

    fn load(&mut self, source: &MenuSource) {
        self.pending_intent = None;
        self.effects.push(Effect::Cancel(TaskKind::Search));
        self.source_generation = self.fresh();
        self.generation = self.fresh();
        self.search = None;
        self.pending_data_search = false;
        self.pending_prune = true;
        self.store = Store::default();
        self.loading = true;
        self.awaiting_source_data = true;
        self.input_failed = false;
        self.message = None;
        // Switching or pushing replaces the rows with another source's; a reload keeps them.
        if self.pending_selection_reset {
            self.preview_text.clear();
        }
        // Keep the presented selection until the replacement commits. Plain ordinal IDs
        // cannot carry checks across reloads; JSONL IDs can.
        self.pending_selection_reset |= source.format != InputFormat::Jsonl;
        self.update_preview();
        let context = ActionContext {
            action: "source".into(),
            query: self.query.clone(),
            source: Some(source.id.clone()),
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
                self.store = Store { index: SearchIndex::new(&items), items: Items::from(items) };
                // A reload can change preview content without changing an item's ID/value.
                self.invalidate_preview();
            }
            Err(error) => {
                // Failure is a settled empty source, published like a successful empty one.
                self.store = Store::default();
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
        self.search = Some(SearchRun { query: self.query.clone(), items: self.store.items.clone() });
        self.effects.push(Effect::Start(Task::Search {
            token: self.generation,
            index: self.store.index.clone(),
            query: self.query.clone(),
            sorted: self.options.sorted,
        }));
    }

    fn search_done(&mut self, token: Token, results: Vec<u32>) {
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
        let listed = self.listed();
        self.shown.selection.reconcile(&self.shown.items, &self.shown.results[..listed], prune_now);
        if prune_now {
            self.pending_prune = false;
        }
        self.shown.query = Some(run.query);
        self.displayed_source_generation = self.source_generation;
        let highlighted = self.shown.selection.highlighted_id.as_ref();
        self.shown.highlighted_row =
            self.shown.results[..listed].iter().position(|&o| Some(&self.shown.items[o as usize].id) == highlighted);
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
                Intent::Push(binding) => {
                    let depth = self.levels.len();
                    self.push(binding);
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
            source: self.source_id.clone(),
            items: vec![item.clone()],
            path: self.path(vec![item]),
        };
        // The last preview stays until this one arrives. Only preview execution is delayed; the highlight itself changes immediately.
        self.effects.push(Effect::Start(Task::Preview {
            token: self.preview_generation,
            argv,
            env: context.environment(),
            delay: Duration::from_millis(60),
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
            source: self.source_id.clone(),
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
            Ok(r) if r.status == 0 => {
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
        let items = match self.options.configuration.marks {
            MarkScope::Level => picked.clone(),
            MarkScope::Global => {
                let marked: Vec<MenuItem> = self
                    .levels
                    .iter()
                    .map(|l| &l.shown)
                    .chain([&self.shown])
                    .filter(|s| !s.selection.checked.is_empty())
                    .flat_map(|s| s.selection.chosen(&s.items))
                    .collect();
                if marked.is_empty() { picked.clone() } else { marked }
            }
        };
        let output = if self.options.json {
            let context = ActionContext {
                action,
                query: self.query.clone(),
                source: self.source_id.clone(),
                items,
                path: self.path(picked),
            };
            let mut output = context.json();
            output.push(b'\n');
            output
        } else {
            let delimiter = if self.options.print0 { '\0' } else { '\n' };
            // A delimiter inside a value would split it into extra records downstream.
            if items.iter().any(|i| i.value().contains(delimiter)) {
                let hint = if self.options.print0 { "" } else { " or --print0" };
                self.message = Some(Status::Error(format!("A selected value contains the output delimiter; use --json{hint}")));
                self.effects.push(Effect::Beep);
                return;
            }
            items.iter().map(|i| format!("{}{delimiter}", i.value())).collect::<String>().into_bytes()
        };
        self.finish(0, output);
    }

    // A failed command source is one source; the user can switch or just cancel.
    // Failed piped stdin means the caller's whole input broke, which is an error.
    fn cancel(&mut self) {
        let status = if self.stdin_error.is_some() { 2 } else { 130 };
        self.finish(status, Vec::new());
    }

    fn finish(&mut self, status: i32, output: Vec<u8>) {
        self.closing = true;
        self.effects.push(Effect::Finish { status, output });
    }

    /// Root to the current level; each ancestor holds what was chosen when it was pushed.
    fn path(&self, current: Vec<MenuItem>) -> Vec<PathEntry> {
        let mut path = self.ancestors();
        path.push(PathEntry { source: self.source_id.clone(), query: self.query.clone(), items: current });
        path
    }

    fn ancestors(&self) -> Vec<PathEntry> {
        self.levels
            .iter()
            .map(|l| PathEntry { source: l.source_id.clone(), query: l.query.clone(), items: l.chosen.clone() })
            .collect()
    }
}

#[cfg(test)]
mod tests;
