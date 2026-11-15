//! Correlates worker requests with borrowed frames from Rune's live terminal host.

use super::{pixels::Pixels, tree};
use rune::{CaptureError, CaptureId, CapturedFrame, Runtime, TerminalCapture};
use serde_json::{Value, json};
use std::{
    sync::{Mutex, mpsc},
    time::Instant,
};

type Reply = Result<Content, String>;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Screenshot,
    Tree,
}

pub(super) struct Capture {
    pub(super) handle: TerminalCapture,
    pending: Mutex<Option<Pending>>,
}

struct Pending {
    id: CaptureId,
    deadline: Instant,
    kind: Kind,
    reply: mpsc::SyncSender<Reply>,
}

enum Content {
    Image { pixels: Pixels, metadata: Value },
    Tree(Value),
}

impl Capture {
    pub(super) fn new() -> Self {
        Self {
            handle: TerminalCapture::new(),
            pending: Mutex::new(None),
        }
    }

    fn begin(
        &self,
        kind: Kind,
        deadline: Instant,
        reply: mpsc::SyncSender<Reply>,
    ) -> Result<CaptureId, String> {
        // Registration and request are one critical section: even an immediate host wake
        // cannot deliver a frame before its correlation id is registered.
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "capture request lock poisoned")?;
        let id = self.handle.request();
        *pending = Some(Pending {
            id,
            deadline,
            kind,
            reply,
        });
        Ok(id)
    }

    pub(super) fn request(&self, kind: Kind, deadline: Instant) -> Result<Value, String> {
        if Instant::now() >= deadline {
            return Err("capture deadline expired".into());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let id = self.begin(kind, deadline, sender)?;
        let result = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        if result.is_err() {
            self.cancel(id);
        }
        match result.map_err(|_| "capture timed out or app stopped")?? {
            // Only this worker encodes; the UI callback merely copied borrowed RGBA.
            Content::Image { pixels, metadata } => {
                Ok(json!({ "image": pixels.encode()?, "capture": metadata }))
            }
            Content::Tree(value) => Ok(value),
        }
    }

    fn cancel(&self, id: CaptureId) {
        if let Ok(mut slot) = self.pending.lock()
            && slot.as_ref().is_some_and(|p| p.id == id)
        {
            *slot = None;
        }
        self.handle.cancel(id);
    }

    fn deliver(&self, id: CaptureId, build: impl FnOnce(Kind) -> Reply) {
        let pending = self.pending.lock().ok().and_then(|mut slot| {
            if slot.as_ref().is_some_and(|p| p.id == id) {
                slot.take()
            } else {
                None
            }
        });
        let Some(pending) = pending else {
            return;
        };
        if Instant::now() >= pending.deadline {
            return;
        }
        drop(pending.reply.try_send(build(pending.kind)));
    }

    pub(super) fn complete(
        &self,
        rt: &Runtime<crate::State>,
        id: CaptureId,
        result: Result<CapturedFrame<'_>, CaptureError>,
    ) {
        self.deliver(id, |kind| {
            let frame = result.map_err(|error| format!("terminal capture failed: {error:?}"))?;
            let metadata = json!({
                "source": "rune-terminal-offscreen", "frame": frame.frame,
                "clock": frame.clock, "stale": frame.stale,
                "width": frame.width, "height": frame.height, "scale": frame.scale,
                "presentationConfirmed": false,
            });
            match kind {
                Kind::Screenshot => Ok(Content::Image {
                    pixels: Pixels::copy(frame.width, frame.height, frame.stride, frame.rgba)?,
                    metadata,
                }),
                Kind::Tree => {
                    let mut value = tree::snapshot(rt, "terminal-runtime-tree")?;
                    value["matchesCapturedPixels"] = (!frame.stale).into();
                    value["capture"] = metadata;
                    Ok(Content::Tree(value))
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
