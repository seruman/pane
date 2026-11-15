use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use rune::{App, Key, KeyEvent, Mods, Runtime, Tx, rewind::Rewind};
use serde_json::{Value, json};

use super::{
    capture::{Capture, Kind},
    commands::{self, Command},
    transport, tree,
};
use crate::State;

const WAIT: Duration = Duration::from_secs(2);
const MAX_ELEMENTS: usize = 10_000;
type Reply = Result<Value, String>;
pub(super) type Job = Box<dyn FnOnce(&mut State, &mut Tx) -> bool + Send>;
type Updater = Arc<dyn Fn(Job) + Send + Sync>;
type TreeReply = Option<(Instant, mpsc::SyncSender<Reply>)>;

pub(super) struct Bridge {
    update: Updater,
    recorder: Rewind,
    terminal: bool,
    outstanding: Arc<AtomicBool>,
    tree: Arc<Mutex<TreeReply>>,
    capture: Arc<Capture>,
}

pub(super) fn attach(
    app: App<State>,
    dir: &Path,
) -> Result<(App<State>, transport::Server), String> {
    let handle = app.handle();
    let terminal =
        std::env::var_os("RUNE_TERMINAL").is_some_and(|value| value != "0" && !value.is_empty());
    let bridge = Arc::new(Bridge::new(
        Arc::new(move |job| handle.update_if(job)),
        terminal,
    ));
    let worker = bridge.clone();
    let server = transport::serve(dir, move |value| worker.request(value))?;
    let recorder = bridge.recorder();
    (bridge.update)(Box::new(move |_, tx| {
        tx.record_rewind(Some(&recorder));
        true
    }));
    let capture = bridge.capture.clone();
    let callback = capture.clone();
    let app = app.terminal_capture(&capture.handle, move |rt, id, result| {
        callback.complete(rt, id, result)
    });
    let app = app.on_frame_drawn(move |rt| bridge.drawn(rt));
    Ok((app, server))
}

impl Bridge {
    pub(super) fn new(update: Updater, terminal: bool) -> Self {
        Self {
            update,
            // Keep only the latest runtime frame, not a history of user activity. Rune keeps
            // at least one frame even when that frame exceeds this approximate budget.
            recorder: Rewind::new().seconds(0.0).max_bytes(1024 * 1024),
            terminal,
            outstanding: Arc::new(AtomicBool::new(false)),
            tree: Arc::new(Mutex::new(None)),
            capture: Arc::new(Capture::new()),
        }
    }

    pub(super) fn recorder(&self) -> Rewind {
        self.recorder.clone()
    }

    pub(super) fn request(&self, request: Value) -> Reply {
        let command = commands::parse(request)?;
        let deadline = Instant::now() + WAIT;
        match command {
            Command::Info => Ok(json!({
                "protocol": 1, "pid": std::process::id(),
                "host": if self.terminal { "terminal" } else { "native" },
                "commonOperations": ["info", "elements", "pick", "focus", "activate", "text", "key", "scroll", "logs"],
                "tree": true, "screenshot": self.terminal,
                "elementsKind": "partial-keyed-geometry",
                "source": "same-process-runtime", "presentationConfirmed": false,
            })),
            Command::Elements => self.elements(),
            Command::Tree => self.tree(deadline),
            Command::Screenshot => {
                if !self.terminal { return Err("offscreen screenshot is currently supported in the terminal host only".into()); }
                self.capture.request(Kind::Screenshot, deadline)
            }
            Command::Logs => self.ui(deadline, |state, _| {
                let logs: Vec<_> = state.script.log().into_iter().rev().take(256)
                    .map(|line| json!({ "level": format!("{:?}", line.level), "message": line.message })).collect();
                Ok((json!({ "newestFirst": true, "logs": logs, "reloads": state.script.metrics().reloads }), false))
            }),
            Command::Focus(key) => self.focus(deadline, key, None),
            Command::Activate(key) => self.focus(deadline, key, Some(Key::Text(" ".into()))),
            Command::Text { key, text } => self.focus(deadline, key, Some(Key::Text(text))),
            Command::Key(event) => self.ui(deadline, move |_, tx| {
                tx.send_key(event);
                Ok((json!({ "dispatched": true }), true))
            }),
            Command::Scroll { key, x, y } => {
                self.unique(&key)?;
                self.ui(deadline, move |_, tx| {
                    tx.scroll_to_instant(key, x, y);
                    Ok((json!({ "dispatched": true }), true))
                })
            }
            Command::Pick { x, y } => self.pick(deadline, x, y),
        }
    }

    fn elements(&self) -> Reply {
        let clip = self.recorder.freeze();
        self.recorder.resume();
        if clip.is_empty() {
            return Err("no runtime frame recorded yet; retry after the app draws".into());
        }
        let frame = clip.frame(clip.len() - 1);
        if frame.len() > MAX_ELEMENTS {
            return Err("more than 10000 recorded elements".into());
        }
        let elements: Vec<_> = frame
            .into_iter()
            .map(|(id, shot)| {
                json!({
                    "name": clip.name(id), "rect": shot.rect, "opacity": shot.opacity,
                })
            })
            .collect();
        Ok(json!({
            "kind": "partial-keyed-geometry", "source": "latest-runtime-frame",
            "presentationConfirmed": false, "viewport": clip.viewport(), "elements": elements,
        }))
    }

    fn unique(&self, key: &str) -> Result<(), String> {
        let value = self.elements()?;
        let elements = value["elements"].as_array().ok_or("missing geometry")?;
        let mut matches = elements.iter().filter(|e| e["name"].as_str() == Some(key));
        let first = matches
            .next()
            .ok_or_else(|| format!("no recorded element named {key:?}"))?;
        if matches.next().is_some() {
            return Err(format!("ambiguous element name: {key:?}"));
        }
        if first["opacity"].as_f64().unwrap_or(0.0) <= 0.0 {
            return Err(format!("element is transparent: {key:?}"));
        }
        Ok(())
    }

    fn focus(&self, deadline: Instant, key: String, event: Option<Key>) -> Reply {
        self.unique(&key)?;
        let target = key.clone();
        self.ui(deadline, move |_, tx| {
            tx.focus(target, true);
            Ok((Value::Null, true))
        })?;
        // Focus applies during a frame, not in the update closure. Never activate the old
        // focus just because a focus request was enqueued. Each probe checks the deadline.
        loop {
            let target = key.clone();
            let key_event = event.clone();
            let answer = self.ui(deadline, move |_, tx| {
                let focused = tx.focused() == Some(target.as_str());
                if focused && let Some(key) = key_event {
                    tx.send_key(KeyEvent {
                        key,
                        mods: Mods::default(),
                    });
                    return Ok((json!({ "focused": target, "dispatched": true }), true));
                }
                Ok((
                    json!({ "focused": if focused { Some(target) } else { None } }),
                    false,
                ))
            })?;
            if answer["focused"].as_str() == Some(key.as_str()) {
                return Ok(answer);
            }
            if Instant::now() >= deadline {
                return Err(format!("element did not take focus: {key:?}"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ui(
        &self,
        deadline: Instant,
        action: impl FnOnce(&mut State, &mut Tx) -> Result<(Value, bool), String> + Send + 'static,
    ) -> Reply {
        if Instant::now() >= deadline {
            return Err("UI request timed out".into());
        }
        if self.outstanding.swap(true, Ordering::AcqRel) {
            return Err("a previous UI request is still pending".into());
        }
        let outstanding = self.outstanding.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        (self.update)(Box::new(move |state, tx| {
            // Clearing on drop also handles unwinding. One queued closure maximum even if
            // the UI is hung and callers keep connecting after timeouts.
            struct Pending(Arc<AtomicBool>);
            impl Drop for Pending {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let pending = Pending(outstanding);
            let result = if Instant::now() < deadline {
                action(state, tx)
            } else {
                Err("UI request expired before dispatch".into())
            };
            let changed = result.as_ref().is_ok_and(|(_, changed)| *changed);
            drop(pending);
            drop(sender.try_send(result.map(|(value, _)| value)));
            changed
        }));
        receive(receiver, deadline)
    }

    fn pick(&self, deadline: Instant, x: f32, y: f32) -> Reply {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.ui(deadline, move |_, tx| {
            tx.element_at::<State>(x, y, move |_, picked, _| {
                let value = picked.map(|p| json!({ "key": p.key(), "path": p.path() }));
                drop(sender.try_send(Ok(json!({ "picked": value }))));
            });
            Ok((Value::Null, false))
        })?;
        receive(receiver, deadline)
    }

    fn tree(&self, deadline: Instant) -> Reply {
        if self.terminal {
            return self.capture.request(Kind::Tree, deadline);
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let slot = self.tree.clone();
        self.ui(deadline, move |_, _| {
            *slot.lock().map_err(|_| "tree request lock poisoned")? = Some((deadline, sender));
            Ok((Value::Null, true))
        })?;
        let result = receive(receiver, deadline);
        if result.is_err()
            && let Ok(mut slot) = self.tree.lock()
        {
            *slot = None;
        }
        result
    }

    pub(super) fn drawn(&self, rt: &Runtime<State>) {
        let pending = self.tree.lock().ok().and_then(|mut slot| slot.take());
        let Some((deadline, sender)) = pending else {
            return;
        };
        if Instant::now() >= deadline {
            return;
        }
        let result = tree::snapshot(rt, "native-frame-tree");
        drop(sender.try_send(result));
    }
}

fn receive(receiver: mpsc::Receiver<Reply>, deadline: Instant) -> Reply {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| {
            "UI request timed out or app stopped; dispatched actions are not rolled back".to_owned()
        })?
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
