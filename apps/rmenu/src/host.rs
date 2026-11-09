//! Runs the model's effects: stdin and tasks on threads whose results come back as events
//! through a channel, drained on the UI thread in `poll`.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rmenu_core::command::{self, Cancel, CommandError, CommandResult};
use rmenu_core::config::InputFormat;
use rmenu_core::item::MenuItem;
use rmenu_core::model::{Effect, Event, Model, Outcome, Task, TaskKind, command_failure};
use rmenu_core::record::RecordDecoder;
use rune::{TextEdit, Tx, Waker};

pub static OUTCOME: Mutex<Option<Outcome>> = Mutex::new(None);

struct Running {
    cancel: Cancel,
    thread: JoinHandle<()>,
}

enum Message {
    Start(Vec<Effect>),
    Event(Event),
}

/// The window's state: the model, the query field mirroring it, and the work in flight.
pub struct Menu {
    /// Never freed: the menu lives until the process exits, and freeing millions of rows one
    /// by one as Rune drops its runtime would only delay that exit.
    pub model: std::mem::ManuallyDrop<Model>,
    pub field: TextEdit,
    pub appearance: crate::cli::Appearance,
    events: Receiver<Message>,
    sender: Sender<Message>,
    waker: Option<Waker>,
    tasks: HashMap<TaskKind, Running>,
    stdin: Option<Running>,
    /// Cancelled tasks still finishing; joined before exit so their process groups are reaped.
    retired: Vec<Running>,
    /// What the list last scrolled to, so it follows the highlight only when it moves.
    revealed: Option<(u64, usize)>,
}

impl Menu {
    pub fn new(launch: crate::cli::Launch) -> Menu {
        let (model, effects) = Model::new(launch.options);
        let (sender, events) = channel();
        sender.send(Message::Start(effects)).expect("receiver was just created");
        let mut field = TextEdit::new(model.query());
        field.focused = true;
        Menu { model: std::mem::ManuallyDrop::new(model), field, appearance: launch.appearance, events, sender, waker: None,
               tasks: HashMap::new(), stdin: None, retired: Vec::new(), revealed: None }
    }

    pub fn setup(&mut self, waker: Waker) {
        self.waker = Some(waker.clone());
        waker();
    }

    /// Drains finished work into the model; true when anything arrived.
    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        // A bounded slice of work per frame, so input queued behind a flood of stdin (Esc,
        // a quit) gets its turn; consecutive stdin batches become one append and one search.
        let deadline = Instant::now() + POLL_BUDGET;
        let mut batch: Vec<MenuItem> = Vec::new();
        loop {
            let next = loop {
                if Instant::now() >= deadline {
                    self.waker.as_ref().expect("Menu::setup runs before poll")();
                    break None;
                }
                match self.events.try_recv() {
                    Ok(Message::Event(Event::StdinItems(items))) => {
                        if batch.is_empty() { batch = items; } else { batch.extend(items); }
                        changed = true;
                    }
                    other => break other.ok(),
                }
            };
            if !batch.is_empty() { self.handle(Event::StdinItems(std::mem::take(&mut batch)), tx); }
            match next {
                Some(Message::Start(effects)) => self.apply(effects, tx),
                Some(Message::Event(event)) => self.handle(event, tx),
                None => break,
            }
            changed = true;
            if OUTCOME.lock().unwrap().is_some() { break; }
        }
        changed
    }

    pub fn handle(&mut self, event: Event, tx: &mut Tx) {
        tx.settle();
        let effects = self.model.handle(event);
        self.apply(effects, tx);
        if self.field.text != self.model.query() {
            self.field.replace_text(self.model.query());
        }
        self.reveal(tx);
    }

    /// Keeps the highlighted row in view when it or the results change.
    fn reveal(&mut self, tx: &mut Tx) {
        let Some(row) = self.model.highlighted_row() else { return };
        let at = (self.model.result_revision(), row);
        if self.revealed == Some(at) { return; }
        self.revealed = Some(at);
        tx.scroll_into_view_in(crate::view::ROWS_KEY, self.model.result(row).id.clone());
    }

    fn apply(&mut self, effects: Vec<Effect>, tx: &mut Tx) {
        for effect in effects {
            match effect {
                Effect::ReadStdin => {
                    let format = self.model.options().input;
                    self.stdin = Some(self.spawn(move |cancel, emit| read_stdin(format, cancel, emit)));
                }
                Effect::Start(task) => {
                    let kind = task.kind();
                    self.retire(kind);
                    let running = self.spawn(move |cancel, emit| run_task(task, cancel, emit));
                    self.tasks.insert(kind, running);
                }
                Effect::Cancel(kind) => self.retire(kind),
                Effect::Copy(text) => tx.copy(text),
                Effect::Beep => {}
                Effect::Finish(result) => {
                    self.stop();
                    let mut outcome = OUTCOME.lock().unwrap();
                    if outcome.is_none() { *outcome = Some(result); }
                    tx.quit();
                }
            }
        }
    }

    fn spawn(&self, work: impl FnOnce(&Cancel, &dyn Fn(Event)) + Send + 'static) -> Running {
        let cancel = Cancel::default();
        let sender = self.sender.clone();
        let waker = self.waker.as_ref().expect("Menu::setup runs before effects").clone();
        let flag = cancel.clone();
        let thread = std::thread::spawn(move || {
            let emit = |event: Event| {
                if flag.is_cancelled() { return; }
                // The receiver only goes away as the process exits.
                if sender.send(Message::Event(event)).is_ok() { waker(); }
            };
            work(&flag, &emit);
        });
        Running { cancel, thread }
    }

    fn retire(&mut self, kind: TaskKind) {
        if let Some(running) = self.tasks.remove(&kind) {
            running.cancel.cancel();
            self.retired.push(running);
        }
        self.retired.retain(|r| !r.thread.is_finished());
    }

    /// Cancels everything and waits for it, so no child process outlives the menu.
    pub fn stop(&mut self) {
        let all: Vec<Running> = self.tasks.drain().map(|(_, r)| r).chain(self.stdin.take()).chain(self.retired.drain(..)).collect();
        for running in &all { running.cancel.cancel(); }
        for running in all { let _ = running.thread.join(); }
    }

    /// The terminal went away or Ctrl-C quit before the model finished: cancel as Escape at
    /// the root would, with no output.
    pub fn quit(&mut self, tx: &mut Tx) {
        if OUTCOME.lock().unwrap().is_none() {
            self.handle(Event::Nav(rmenu_core::model::Nav::Cancel), tx);
        }
    }
}

/// Work `poll` does per frame before yielding to input and drawing.
const POLL_BUDGET: Duration = Duration::from_millis(8);
const STDIN_CHUNK: usize = 64 * 1024;
const STDIN_BATCH_INTERVAL: Duration = Duration::from_millis(16);

/// Emits the first decoded batch immediately, then batches at `STDIN_BATCH_INTERVAL`.
fn read_stdin(format: InputFormat, cancel: &Cancel, emit: &dyn Fn(Event)) {
    let mut decoder = RecordDecoder::new(format);
    let mut buffer = vec![0u8; STDIN_CHUNK];
    let mut batch: Vec<MenuItem> = Vec::new();
    let mut last: Option<Instant> = None;
    let flush = |batch: &mut Vec<MenuItem>, last: &mut Option<Instant>, force: bool| {
        if batch.is_empty() { return; }
        if force || last.is_none_or(|at| at.elapsed() >= STDIN_BATCH_INTERVAL) {
            emit(Event::StdinItems(std::mem::take(batch)));
            *last = Some(Instant::now());
        }
    };
    loop {
        if cancel.is_cancelled() { return; }
        let mut request = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
        // SAFETY: one live pollfd, its count passed.
        let ready = unsafe { libc::poll(&mut request, 1, STDIN_BATCH_INTERVAL.as_millis() as libc::c_int) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted { continue; }
            return emit(Event::StdinFailed(format!("Cannot poll stdin: {error}")));
        }
        if ready == 0 {
            flush(&mut batch, &mut last, true);
            continue;
        }
        // SAFETY: reads into a live buffer of the length passed.
        let count = unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 { break; }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) { continue; }
            return emit(Event::StdinFailed(format!("Cannot read stdin: {error}")));
        }
        match decoder.append(&buffer[..count as usize]) {
            Ok(items) => batch.extend(items),
            Err(error) => return emit(Event::StdinFailed(error.to_string())),
        }
        flush(&mut batch, &mut last, false);
    }
    match decoder.finish() {
        Ok(items) => batch.extend(items),
        Err(error) => return emit(Event::StdinFailed(error.to_string())),
    }
    flush(&mut batch, &mut last, true);
    emit(Event::StdinDone);
}

const SOURCE_LIMIT: usize = 16 * 1024 * 1024;
const PREVIEW_LIMIT: usize = 512 * 1024;
const OPERATION_LIMIT: usize = 1024 * 1024;

fn run_task(task: Task, cancel: &Cancel, emit: &dyn Fn(Event)) {
    let run = |argv: &[String], input: Option<&[u8]>, env: &[(String, String)], timeout: u64, limit: usize|
        command::run(argv, input, env, Duration::from_secs(timeout), limit, cancel);
    match task {
        Task::Search { token, index, query, sorted } => {
            if let Ok(results) = index.search(&query, sorted, &|| cancel.is_cancelled()) {
                emit(Event::SearchDone { token, results });
            }
        }
        Task::Source { token, argv, input, env, format } => {
            let result = match run(&argv, Some(&input), &env, 30, SOURCE_LIMIT) {
                Err(CommandError::Cancelled) => return,
                Err(error) => Err(error.to_string()),
                Ok(done) if !done.status.success() => Err(command_failure(&done)),
                Ok(done) => decode(format, &done),
            };
            emit(Event::SourceDone { token, result });
        }
        Task::Preview { token, argv, env, delay } => {
            let until = Instant::now() + delay;
            while Instant::now() < until {
                if cancel.is_cancelled() { return; }
                std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)));
            }
            let result = match run(&argv, None, &env, 5, PREVIEW_LIMIT) {
                Err(CommandError::Cancelled) => return,
                result => result.map_err(|e| e.to_string()),
            };
            emit(Event::PreviewDone { token, result });
        }
        Task::Operation { token, argv, input, env } => {
            let result = match run(&argv, Some(&input), &env, 30, OPERATION_LIMIT) {
                Err(CommandError::Cancelled) => return,
                result => result.map_err(|e| e.to_string()),
            };
            emit(Event::OperationDone { token, result });
        }
    }
}

fn decode(format: InputFormat, done: &CommandResult) -> Result<Vec<MenuItem>, String> {
    let mut decoder = RecordDecoder::new(format);
    let mut items = decoder.append(&done.stdout).map_err(|e| e.to_string())?;
    items.extend(decoder.finish().map_err(|e| e.to_string())?);
    Ok(items)
}
