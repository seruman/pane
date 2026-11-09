#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rmenu_core::command::{self, Cancel, CommandError, CommandResult};
use rmenu_core::config::InputFormat;
use rmenu_core::item::MenuItem;
use rmenu_core::model::{
    Effect, Event, Model, OPERATION_LIMITS, Outcome, PREVIEW_LIMITS, SOURCE_LIMITS, Task, TaskKind, command_failure,
};
use rmenu_core::record::RecordDecoder;
use rune::{TextEdit, Tx, Waker};

struct Running {
    cancel: Cancel,
    thread: JoinHandle<()>,
}

enum Message {
    Start(Vec<Effect>),
    Event(Event),
}

pub struct Menu {
    // The process reclaims the rows at exit. Dropping each row would delay shutdown.
    pub model: std::mem::ManuallyDrop<Model>,
    pub field: TextEdit,
    pub appearance: crate::cli::Appearance,
    outcome: Rc<RefCell<Option<Outcome>>>,
    events: Receiver<Message>,
    sender: Sender<Message>,
    waker: Option<Waker>,
    tasks: HashMap<TaskKind, Running>,
    stdin: Option<Running>,
    retired: Vec<Running>,
    revealed: Option<(u64, usize)>,
}

impl Menu {
    pub fn new(launch: crate::cli::Launch, outcome: Rc<RefCell<Option<Outcome>>>) -> Menu {
        let (model, effects) = Model::new(launch.options);
        let (sender, events) = channel();
        sender.send(Message::Start(effects)).expect("receiver was just created");
        let mut field = TextEdit::new(model.query());
        field.focused = true;
        Menu {
            model: std::mem::ManuallyDrop::new(model),
            field,
            appearance: launch.appearance,
            outcome,
            events,
            sender,
            waker: None,
            tasks: HashMap::new(),
            stdin: None,
            retired: Vec::new(),
            revealed: None,
        }
    }

    pub fn setup(&mut self, waker: Waker) {
        self.waker = Some(waker.clone());
        waker();
    }

    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
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
                        if batch.is_empty() {
                            batch = items;
                        } else {
                            batch.extend(items);
                        }
                        changed = true;
                    }
                    other => break other.ok(),
                }
            };
            if !batch.is_empty() {
                self.handle(Event::StdinItems(std::mem::take(&mut batch)), tx);
            }
            match next {
                Some(Message::Start(effects)) => self.apply(effects, tx),
                Some(Message::Event(event)) => self.handle(event, tx),
                None => break,
            }
            changed = true;
            if self.outcome.borrow().is_some() {
                break;
            }
        }
        changed
    }

    pub fn handle(&mut self, event: Event, tx: &mut Tx) {
        // Reduced motion still fades colors. Selection changes must use their final colors now.
        tx.settle();
        let effects = self.model.handle(event);
        self.apply(effects, tx);
        if self.field.text != self.model.query() {
            self.field.replace_text(self.model.query());
        }
        self.reveal(tx);
    }

    fn reveal(&mut self, tx: &mut Tx) {
        let Some(row) = self.model.highlighted_row() else { return };
        let at = (self.model.result_revision(), row);
        if self.revealed == Some(at) {
            return;
        }
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
                    let mut outcome = self.outcome.borrow_mut();
                    debug_assert!(outcome.is_none(), "the model finishes only once");
                    *outcome = Some(result);
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
                if flag.is_cancelled() {
                    return;
                }
                if sender.send(Message::Event(event)).is_ok() {
                    waker();
                }
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

    pub fn stop(&mut self) {
        let all: Vec<Running> =
            self.tasks.drain().map(|(_, r)| r).chain(self.stdin.take()).chain(self.retired.drain(..)).collect();
        for running in &all {
            running.cancel.cancel();
        }
        for running in all {
            let _ = running.thread.join();
        }
    }

    pub fn quit(&mut self, tx: &mut Tx) {
        if self.outcome.borrow().is_none() {
            self.handle(Event::Nav(rmenu_core::model::Nav::Cancel), tx);
        }
    }
}

// The 16 ms publication target comes from mmenu 7c8f662, Sources/MMenu/MenuModel.swift.
// Target roughly one stdin publication per 60 Hz frame. Spend at most half that interval
// draining queued messages before yielding to input and drawing. A reducer call can overrun it.
// The ignored poll_budget_measurement test measures draining and verifies the follow-up wake.
const STDIN_BATCH_INTERVAL: Duration = Duration::from_millis(16);
const POLL_BUDGET: Duration = STDIN_BATCH_INTERVAL.checked_div(2).expect("nonzero divisor");
const STDIN_CHUNK: usize = 64 * 1024;

fn read_stdin(format: InputFormat, cancel: &Cancel, emit: &dyn Fn(Event)) {
    let mut decoder = RecordDecoder::new(format);
    let mut buffer = vec![0u8; STDIN_CHUNK];
    let mut batch: Vec<MenuItem> = Vec::new();
    let mut last: Option<Instant> = None;
    let flush = |batch: &mut Vec<MenuItem>, last: &mut Option<Instant>, force: bool| {
        if batch.is_empty() {
            return;
        }
        if force || last.is_none_or(|at| at.elapsed() >= STDIN_BATCH_INTERVAL) {
            emit(Event::StdinItems(std::mem::take(batch)));
            *last = Some(Instant::now());
        }
    };
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let mut request = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
        // SAFETY: one live pollfd, its count passed.
        let ready = unsafe {
            libc::poll(
                &raw mut request,
                1,
                STDIN_BATCH_INTERVAL.as_millis().try_into().expect("stdin poll interval fits c_int"),
            )
        };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return emit(Event::StdinFailed(format!("Cannot poll stdin: {error}")));
        }
        if ready == 0 {
            flush(&mut batch, &mut last, true);
            continue;
        }
        // SAFETY: reads into a live buffer of the length passed.
        let count = unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 {
            break;
        }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) {
                continue;
            }
            return emit(Event::StdinFailed(format!("Cannot read stdin: {error}")));
        }
        match decoder.append(&buffer[..count.cast_unsigned()]) {
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

fn run_task(task: Task, cancel: &Cancel, emit: &dyn Fn(Event)) {
    match task {
        Task::Search { token, index, query, order } => {
            if let Ok(results) = index.search(&query, order, &|| cancel.is_cancelled()) {
                emit(Event::SearchDone { token, results });
            }
        }
        Task::Source { token, argv, input, env, format } => {
            let result = match command::run(&argv, Some(&input), &env, SOURCE_LIMITS, cancel) {
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
                if cancel.is_cancelled() {
                    return;
                }
                std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)));
            }
            let result = match command::run(&argv, None, &env, PREVIEW_LIMITS, cancel) {
                Err(CommandError::Cancelled) => return,
                result => result.map_err(|e| e.to_string()),
            };
            emit(Event::PreviewDone { token, result });
        }
        Task::Operation { token, argv, input, env } => {
            let result = match command::run(&argv, Some(&input), &env, OPERATION_LIMITS, cancel) {
                Err(CommandError::Cancelled) => return,
                result => result.map_err(|e| e.to_string()),
            };
            emit(Event::OperationDone { token, result });
        }
    }
}

fn decode(format: InputFormat, done: &CommandResult) -> Result<Vec<MenuItem>, String> {
    RecordDecoder::decode_all(format, &done.stdout).map_err(|e| e.to_string())
}
