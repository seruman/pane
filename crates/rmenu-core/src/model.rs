//! The menu reducer emits work for the host and accepts token-tagged completions.

use std::{mem, time::Duration};

use crate::command::{CommandResult, Limits};
use crate::config::{BindingAction, InputFormat, KeyChord, MarkScope, MenuBinding, MenuConfiguration, MenuSource, SourceId};
use crate::item::{ActionContext, MenuItem, PathEntry};
use crate::items::Items;
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
    /// PageUp and PageDown move by this many rows.
    pub rows: usize,
    /// The highlight stays within the rows the host lists.
    pub list_cap: usize,
}

/// A started task's identity. A result whose token is stale is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceGeneration(Token);

/// Starting a task replaces the running task of its kind.
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
    /// Run under `SOURCE_LIMITS` and decode stdout as `format`, then answer `SourceDone`.
    Source { token: Token, argv: Vec<String>, input: Vec<u8>, env: Vec<(String, String)>, format: InputFormat },
    /// Search off the UI thread, then answer `SearchDone`.
    Search { token: Token, index: SearchIndex, query: String, order: Order },
    /// Wait for `delay`, run under `PREVIEW_LIMITS`, then answer `PreviewDone`.
    Preview { token: Token, argv: Vec<String>, env: Vec<(String, String)>, delay: Duration },
    /// Run under `OPERATION_LIMITS`, then answer `OperationDone`.
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
    /// Read piped stdin as `Options::input` in batches. Issued at most once.
    ReadStdin,
    Start(Task),
    Cancel(TaskKind),
    Copy(String),
    Beep,
    /// Cancel and reap background work, then publish the process outcome.
    Finish(Outcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    Accept,
    Back,
    PopIfEmpty,
    Tab,
    BackTab,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingIndex(pub usize);

#[derive(Clone, Debug)]
pub enum Event {
    SetQuery(String),
    Nav(Nav),
    Highlight(usize),
    ToggleRow(usize),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Perform(BindingIndex),
    SwitchSource(SourceId),
}

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

#[derive(Clone, Default)]
struct Shown {
    items: Items,
    results: Vec<RowOffset>,
    selection: MenuSelection,
    query: Option<String>,
    source_generation: Option<SourceGeneration>,
}

enum Input {
    Stdin,
    Command(SourceId),
}

impl Input {
    fn source_id(&self) -> Option<&SourceId> {
        match self {
            Self::Stdin => None,
            Self::Command(id) => Some(id),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ingestion {
    Reading,
    Complete,
    Failed,
}

struct Level {
    input: Input,
    store: Items,
    shown: Shown,
    query: String,
    ingestion: Ingestion,
    message: Option<Status>,
    source_generation: SourceGeneration,
    phase: Phase,
}

impl Level {
    fn new(query: String, shown: Shown) -> Self {
        Self {
            input: Input::Stdin,
            store: Items::default(),
            shown,
            query,
            ingestion: Ingestion::Reading,
            message: None,
            source_generation: SourceGeneration(Token(0)),
            phase: Phase::Ready,
        }
    }

    fn loading(&self) -> bool {
        match self.input {
            Input::Stdin => self.ingestion == Ingestion::Reading,
            Input::Command(_) => matches!(self.phase,
                Phase::AwaitingSource { .. } | Phase::Matching(SearchRun { publication: Publication::Source(_), .. })),
        }
    }
}

struct SuspendedLevel {
    level: Level,
    chosen: Vec<MenuItem>,
    stdin_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Ack(String),
    Operation(Token),
    Error(String),
    StdinFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    Replace,
    Reload,
}

#[derive(Clone, Copy)]
enum SelectionPolicy {
    Prune,
    Reset,
}

#[derive(Clone, Copy)]
enum Publication {
    Query,
    Source(SelectionPolicy),
}

enum Phase {
    AwaitingSource { selection: SelectionPolicy },
    Matching(SearchRun),
    CatchUp,
    Ready,
}

impl Phase {
    fn defer(&mut self, intent: Intent) {
        if let Self::Matching(run) = self {
            run.intent = Some(intent);
        }
    }
}

struct SearchRun {
    token: Token,
    query: String,
    items: Items,
    publication: Publication,
    catch_up: bool,
    intent: Option<Intent>,
}

struct Operation {
    token: Token,
    action: String,
    message: String,
    reload: bool,
    source_generation: SourceGeneration,
}

enum Intent {
    Accept(String),
    Push(SourceId),
}

#[derive(PartialEq, Eq)]
enum IntentOutcome {
    Deferred,
    Refused,
    Applied,
}

pub struct Model {
    options: Options,
    effects: Vec<Effect>,
    next_token: u64,
    levels: Vec<SuspendedLevel>,
    current: Level,
    result_revision: u64,
    stdin_error: Option<String>,
    operation: Option<Operation>,
    preview_enabled: bool,
    preview_text: String,
    preview_identity: Option<(SourceGeneration, MenuItem, String)>,
    preview_generation: Option<Token>,
    closing: bool,
}

impl Model {
    pub fn new(options: Options) -> (Model, Vec<Effect>) {
        let current = Level::new(options.query.clone(), Shown::default());
        let mut model = Model {
            options,
            effects: Vec::new(),
            next_token: 0,
            levels: Vec::new(),
            current,
            result_revision: 0,
            stdin_error: None,
            operation: None,
            preview_enabled: true,
            preview_text: String::new(),
            preview_identity: None,
            preview_generation: None,
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

    pub fn options(&self) -> &Options { &self.options }

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

    pub fn query(&self) -> &str { &self.current.query }

    pub fn result_count(&self) -> usize { self.current.shown.results.len() }

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

    pub fn result(&self, row: usize) -> &MenuItem { &self.current.shown.items[self.current.shown.results[row]] }

    /// Changes whenever the shown results are replaced, so a host can reset scrolling.
    pub fn result_revision(&self) -> u64 { self.result_revision }

    pub fn highlighted_row(&self) -> Option<usize> { self.current.shown.selection.highlighted_row() }

    pub fn is_marked(&self, item: &MenuItem) -> bool { self.current.shown.selection.is_marked(&item.id) }

    pub fn marked_count(&self) -> usize { self.current.shown.selection.marked_count() }

    pub fn has_choice(&self) -> bool { self.marked_count() > 0 || self.highlighted().is_some() }

    pub fn loading(&self) -> bool { self.current.loading() }

    pub fn message(&self) -> Option<&str> {
        match self.current.message.as_ref()? {
            Status::Ack(text) | Status::Error(text) => Some(text),
            Status::Operation(token) => self.operation.as_ref().filter(|operation| operation.token == *token).map(|operation| operation.message.as_str()),
            Status::StdinFailed => self.stdin_error(),
        }
    }

    pub fn message_is_error(&self) -> bool { matches!(self.current.message, Some(Status::Error(_) | Status::StdinFailed)) }

    pub fn stdin_error(&self) -> Option<&str> { self.stdin_error.as_deref() }

    pub fn preview(&self) -> Option<&str> { self.preview_visible().then_some(self.preview_text.as_str()) }

    /// Match highlights use the query that produced the shown rows, not a pending edit.
    pub fn shown_query(&self) -> &str { self.current.shown.query.as_deref().unwrap_or("") }

    pub fn source_id(&self) -> Option<&SourceId> { self.current.input.source_id() }

    pub fn depth(&self) -> usize { self.levels.len() }

    pub fn target(&self, chord: &KeyChord) -> Option<Target> {
        let configuration = &self.options.configuration;
        if self.levels.is_empty() {
            let source = configuration.sources().iter().filter(|s| !s.hidden).find(|s| s.key.as_ref() == Some(chord));
            if let Some(source) = source {
                return Some(Target::SwitchSource(source.id.clone()));
            }
        }
        configuration.bindings().iter()
            .position(|b| b.applies(self.source_id()) && &b.key == chord)
            .map(|index| Target::Perform(BindingIndex(index)))
    }

    pub fn can_navigate(&self) -> bool {
        let loading_command = matches!(self.current.input, Input::Command(_)) && self.loading();
        !self.input_failed() && !loading_command
            && self.current.shown.source_generation == Some(self.current.source_generation)
    }

    fn can_act(&self) -> bool { self.can_navigate() && self.current.shown.query.as_deref() == Some(self.query()) }

    pub fn is_searching(&self) -> bool { matches!(self.current.phase, Phase::Matching(_)) }

    pub fn input_failed(&self) -> bool { self.current.ingestion == Ingestion::Failed }

    pub fn running_operation(&self) -> bool { self.operation.is_some() }

    fn chosen(&self) -> Vec<MenuItem> {
        if self.marked_count() == 0 {
            self.highlighted().into_iter().cloned().collect()
        } else {
            self.current.shown.selection.marked(&self.current.shown.items)
        }
    }

    fn root_mut(&mut self) -> &mut Level {
        self.levels.first_mut().map_or(&mut self.current, |root| &mut root.level)
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
                self.root_mut().store.extend(batch);
                self.stdin_changed();
            }
            Event::StdinDone => {
                self.root_mut().ingestion = Ingestion::Complete;
                self.stdin_changed();
            }
            Event::StdinFailed(error) => {
                self.stdin_error = Some(error);
                let root = self.root_mut();
                root.ingestion = Ingestion::Failed;
                root.message = Some(Status::StdinFailed);
            }
            Event::SourceDone { token, result } => self.source_done(token, result),
            Event::SearchDone { token, results } => self.search_done(token, results),
            Event::PreviewDone { token, result } => {
                if Some(token) != self.preview_generation {
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
            Nav::Accept => { self.apply_intent(Intent::Accept("accept".into())); },
            Nav::Back => {
                if self.levels.is_empty() { self.cancel() } else { self.pop() }
            }
            Nav::PopIfEmpty => {
                if self.query().is_empty() && !self.levels.is_empty() {
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
        self.next_token = self.next_token.checked_add(1).expect("task identity exhausted");
        Token(self.next_token)
    }

    fn source(&self) -> Option<&MenuSource> { self.source_id().and_then(|id| self.options.configuration.source(id)) }

    fn preview_command(&self) -> Option<Vec<String>> {
        self.source().and_then(|s| s.preview.clone()).or_else(|| self.options.configuration.preview().map(<[String]>::to_vec))
    }

    fn preview_visible(&self) -> bool { self.preview_enabled && self.preview_command().is_some() }

    pub fn listed_count(&self) -> usize { self.result_count().min(self.options.list_cap) }

    fn highlighted(&self) -> Option<&MenuItem> {
        let row = self.highlighted_row()?;
        self.current.shown.results.get(row).map(|&o| &self.current.shown.items[o])
    }

    fn set_query(&mut self, value: String) {
        if value == self.query() {
            return;
        }
        self.current.query = value;
        self.invalidate_preview();
        self.clear_transient_message();
        self.request_search(false);
    }

    fn highlight(&mut self, row: usize) {
        if !self.can_navigate() || row >= self.listed_count() {
            return;
        }
        if self.highlighted_row() != Some(row) {
            self.clear_transient_message();
        }
        let id = self.result(row).id.clone();
        self.current.shown.selection.highlight(row, id);
        self.update_preview();
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.listed_count();
        if count == 0 {
            return;
        }
        let row = (self.highlighted_row().unwrap_or(0) as isize + delta).clamp(0, count as isize - 1);
        self.highlight(row as usize);
    }

    fn toggle(&mut self) {
        if !self.can_navigate() || !self.options.multi {
            return;
        }
        let Some(id) = self.highlighted().map(|i| i.id.clone()) else { return };
        self.current.shown.selection.toggle(&id);
        self.move_by(1);
    }

    fn toggle_row(&mut self, row: usize) {
        if !self.can_navigate() || !self.options.multi || row >= self.listed_count() {
            return;
        }
        let id = self.result(row).id.clone();
        self.current.shown.selection.toggle(&id);
        self.highlight(row);
    }

    fn perform(&mut self, binding: MenuBinding) {
        if !binding.applies(self.source_id()) {
            return;
        }
        match &binding.action {
            BindingAction::Accept => { self.apply_intent(Intent::Accept(binding.action_name().to_owned())); },
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
                self.current.message = Some(Status::Ack(format!("Copied {}", values.len())));
            }
            BindingAction::Source { source } => self.switch_source(source),
            BindingAction::Push { source } => { self.apply_intent(Intent::Push(source.clone())); },
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

    fn switch_source(&mut self, id: &SourceId) {
        if !self.levels.is_empty() || self.source_id() == Some(id) {
            return;
        }
        let Some(source) = self.options.configuration.source(id).filter(|s| !s.hidden).cloned() else { return };
        self.load(&source, LoadKind::Replace);
    }

    fn apply_intent(&mut self, intent: Intent) -> IntentOutcome {
        let blocked = matches!(intent, Intent::Accept(_)) && self.operation.is_some();
        if !self.can_act() && self.can_navigate() && !blocked {
            self.current.phase.defer(intent);
            return IntentOutcome::Deferred;
        }
        if !self.can_act() || blocked {
            self.effects.push(Effect::Beep);
            return IntentOutcome::Refused;
        }
        let picked = self.chosen();
        if picked.is_empty() {
            self.effects.push(Effect::Beep);
            return IntentOutcome::Refused;
        }
        match intent {
            Intent::Accept(action) => self.accept(action, picked),
            Intent::Push(source) => {
                self.push(source, picked);
                IntentOutcome::Applied
            }
        }
    }

    fn push(&mut self, source: SourceId, picked: Vec<MenuItem>) {
        let stdin_changed = matches!(self.current.input, Input::Stdin)
            && matches!(self.current.phase, Phase::Matching(_) | Phase::CatchUp);
        let child = Level::new(String::new(), self.current.shown.clone());
        let mut parent = mem::replace(&mut self.current, child);
        parent.phase = Phase::Ready;
        self.levels.push(SuspendedLevel { level: parent, chosen: picked, stdin_changed });
        let target = self.options.configuration.source(&source).expect("validated push source").clone();
        self.load(&target, LoadKind::Replace);
    }

    fn pop(&mut self) {
        let Some(parent) = self.levels.pop() else { return };
        self.effects.extend([TaskKind::Source, TaskKind::Search, TaskKind::Preview].map(Effect::Cancel));
        self.current = parent.level;
        self.current.source_generation = SourceGeneration(self.fresh());
        self.current.shown.source_generation = Some(self.current.source_generation);
        self.preview_generation = None;
        self.preview_identity = None;
        self.preview_text.clear();
        self.result_revision += 1;
        self.update_preview();
        if parent.stdin_changed {
            self.request_search(true);
        }
    }

    fn reload(&mut self) {
        match self.source().cloned() {
            Some(source) => self.load(&source, LoadKind::Reload),
            None => {
                self.current.message = Some(Status::Error("Piped input cannot be re-run".into()));
                self.effects.push(Effect::Beep);
            }
        }
    }

    fn load(&mut self, source: &MenuSource, kind: LoadKind) {
        let reset_pending = matches!(self.current.phase,
            Phase::AwaitingSource { selection: SelectionPolicy::Reset }
            | Phase::Matching(SearchRun { publication: Publication::Source(SelectionPolicy::Reset), .. }));
        let selection = if kind == LoadKind::Replace || source.format != InputFormat::Jsonl || reset_pending {
            SelectionPolicy::Reset
        } else {
            SelectionPolicy::Prune
        };
        self.current.input = Input::Command(source.id.clone());
        self.current.source_generation = SourceGeneration(self.fresh());
        self.current.phase = Phase::AwaitingSource { selection };
        self.current.store = Items::default();
        self.current.ingestion = Ingestion::Reading;
        self.current.message = None;
        self.effects.push(Effect::Cancel(TaskKind::Search));
        if kind == LoadKind::Replace {
            self.preview_text.clear();
        }
        self.update_preview();
        let context = ActionContext {
            action: "source".into(),
            query: self.query().to_owned(),
            source: Some(source.id.to_string()),
            items: self.levels.last().map(|l| l.chosen.clone()).unwrap_or_default(),
            path: self.ancestors(),
        };
        self.effects.push(Effect::Start(Task::Source {
            token: self.current.source_generation.0,
            argv: source.command.clone(),
            input: context.json(),
            env: context.environment(),
            format: source.format,
        }));
    }

    fn source_done(&mut self, token: Token, result: Result<Vec<MenuItem>, String>) {
        if SourceGeneration(token) != self.current.source_generation {
            return;
        }
        let Phase::AwaitingSource { selection } = self.current.phase else { return };
        match result {
            Ok(items) => {
                self.current.store = Items::from(items);
                self.current.ingestion = Ingestion::Complete;
                self.invalidate_preview();
            }
            Err(error) => {
                self.current.store = Items::default();
                self.current.ingestion = Ingestion::Failed;
                self.current.message = Some(Status::Error(error));
            }
        }
        self.start_search(Publication::Source(selection), None);
    }

    fn stdin_changed(&mut self) {
        match self.levels.first_mut() {
            Some(root) => root.stdin_changed = true,
            None => self.request_search(true),
        }
    }

    fn request_search(&mut self, data_changed: bool) {
        match &mut self.current.phase {
            Phase::AwaitingSource { .. } => return,
            Phase::Matching(run) if data_changed => {
                run.catch_up = true;
                return;
            }
            _ => {}
        }
        let (publication, intent) = match mem::replace(&mut self.current.phase, Phase::Ready) {
            Phase::Matching(run) => (run.publication, run.intent),
            _ => (Publication::Query, None),
        };
        self.start_search(publication, intent);
    }

    fn start_search(&mut self, publication: Publication, intent: Option<Intent>) {
        let token = self.fresh();
        let query = self.query().to_owned();
        self.current.phase = Phase::Matching(SearchRun {
            token,
            query: query.clone(),
            items: self.current.store.clone(),
            publication,
            catch_up: false,
            intent,
        });
        self.effects.push(Effect::Start(Task::Search {
            token,
            index: self.current.store.clone(),
            query,
            order: if self.options.sorted { Order::Ranked } else { Order::Input },
        }));
    }

    fn search_done(&mut self, token: Token, results: Vec<RowOffset>) {
        if !matches!(&self.current.phase, Phase::Matching(run) if run.token == token) {
            return;
        }
        let Phase::Matching(run) = mem::replace(&mut self.current.phase, Phase::Ready) else { unreachable!() };
        if run.catch_up {
            self.current.phase = Phase::CatchUp;
        }
        let shown = &mut self.current.shown;
        shown.items = run.items;
        shown.results = results;
        self.result_revision += 1;
        if matches!(run.publication, Publication::Source(SelectionPolicy::Reset)) {
            shown.selection = MenuSelection::default();
        }
        let marks = match run.publication {
            Publication::Source(_) => Marks::Prune,
            Publication::Query => Marks::Keep,
        };
        let listed = self.listed_count();
        let shown = &mut self.current.shown;
        shown.selection.reconcile(&shown.items, &shown.results[..listed], marks);
        shown.query = Some(run.query);
        shown.source_generation = Some(self.current.source_generation);
        self.update_preview();
        if self.can_act()
            && let Some(intent) = run.intent
            && self.apply_intent(intent) == IntentOutcome::Applied
        {
            return;
        }
        if run.catch_up {
            self.request_search(true);
        }
    }

    fn update_preview(&mut self) {
        let item = self.highlighted().cloned();
        let shown = self.preview_visible() && self.can_act();
        let identity = item.clone().filter(|_| shown).map(|i| (self.current.source_generation, i, self.query().to_owned()));
        if identity.is_none() && self.can_act() {
            self.preview_text.clear();
        }
        if identity == self.preview_identity {
            return;
        }
        self.preview_identity = identity;
        self.preview_generation = None;
        let (Some(argv), Some(item), true) = (self.preview_command(), item, shown) else {
            self.effects.push(Effect::Cancel(TaskKind::Preview));
            self.preview_text.clear();
            return;
        };
        let context = ActionContext {
            action: "preview".into(),
            query: self.query().to_owned(),
            source: self.source_id().map(ToString::to_string),
            items: vec![item.clone()],
            path: self.path(vec![item]),
        };
        let token = self.fresh();
        self.preview_generation = Some(token);
        self.effects.push(Effect::Start(Task::Preview {
            token,
            argv,
            env: context.environment(),
            delay: PREVIEW_DELAY,
        }));
    }

    fn clear_transient_message(&mut self) {
        if matches!(self.current.message, Some(Status::Ack(_))) {
            self.current.message = None;
        }
    }

    fn invalidate_preview(&mut self) {
        self.preview_generation = None;
        self.preview_identity = None;
        self.effects.push(Effect::Cancel(TaskKind::Preview));
    }

    fn run_operation(&mut self, argv: Vec<String>, action: String, reload: bool) {
        let chosen = self.chosen();
        let context = ActionContext {
            action: action.clone(),
            query: self.query().to_owned(),
            source: self.source_id().map(ToString::to_string),
            items: chosen.clone(),
            path: self.path(chosen),
        };
        let token = self.fresh();
        self.current.message = Some(Status::Operation(token));
        self.operation = Some(Operation {
            token,
            message: format!("Running {action}…"),
            action,
            reload,
            source_generation: self.current.source_generation,
        });
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
                self.current.message = Some(Status::Ack(format!("{} completed", operation.action)));
                if operation.reload && operation.source_generation == self.current.source_generation {
                    self.reload();
                }
            }
            Ok(r) => self.current.message = Some(Status::Error(command_failure(&r))),
            Err(error) => self.current.message = Some(Status::Error(error)),
        }
    }

    fn accept(&mut self, action: String, picked: Vec<MenuItem>) -> IntentOutcome {
        let items = match self.options.configuration.marks() {
            MarkScope::Level => picked.clone(),
            MarkScope::Global => {
                let marked: Vec<MenuItem> = self.levels.iter()
                    .map(|l| &l.level.shown).chain([&self.current.shown])
                    .filter(|s| s.selection.marked_count() != 0)
                    .flat_map(|s| s.selection.marked(&s.items)).collect();
                if marked.is_empty() { picked.clone() } else { marked }
            }
        };
        let output = if self.options.output == OutputFormat::Json {
            let context = ActionContext {
                action,
                query: self.query().to_owned(),
                source: self.source_id().map(ToString::to_string),
                items,
                path: self.path(picked),
            };
            let mut output = context.json();
            output.push(b'\n');
            output
        } else {
            let delimiter = if self.options.output == OutputFormat::Nul { '\0' } else { '\n' };
            if items.iter().any(|i| i.value().contains(delimiter)) {
                let hint = if self.options.output == OutputFormat::Nul { "" } else { " or --print0" };
                self.current.message = Some(Status::Error(format!("A selected value contains the output delimiter; use --json{hint}")));
                self.effects.push(Effect::Beep);
                return IntentOutcome::Refused;
            }
            items.iter().map(|i| format!("{}{delimiter}", i.value())).collect::<String>().into_bytes()
        };
        self.finish(Outcome::Chosen(output));
        IntentOutcome::Applied
    }

    fn cancel(&mut self) {
        let outcome = self.stdin_error.clone().map_or(Outcome::Cancelled, Outcome::Failed);
        self.finish(outcome);
    }

    fn finish(&mut self, outcome: Outcome) {
        self.closing = true;
        self.effects.push(Effect::Finish(outcome));
    }

    fn path(&self, current: Vec<MenuItem>) -> Vec<PathEntry> {
        let mut path = self.ancestors();
        path.push(PathEntry { source: self.source_id().map(ToString::to_string), query: self.query().to_owned(), items: current });
        path
    }

    fn ancestors(&self) -> Vec<PathEntry> {
        self.levels.iter().map(|l| PathEntry {
            source: l.level.input.source_id().map(ToString::to_string),
            query: l.level.query.clone(),
            items: l.chosen.clone(),
        }).collect()
    }
}

#[cfg(test)]
mod tests;
