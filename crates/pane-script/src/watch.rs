//! Watch the parent directory so atomic editor saves do not detach the watch.

use std::{path::Path, sync::mpsc};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rune::{Tx, Waker};

const DEBOUNCE: f64 = 0.08;

pub(crate) struct FileWatch {
    _watcher: RecommendedWatcher,
    events: mpsc::Receiver<Result<(), String>>,
    due: Option<f64>,
}

impl FileWatch {
    pub(crate) fn start(path: &Path, wake: Waker) -> notify::Result<Self> {
        let (send, events) = mpsc::sync_channel(1);
        let target = path.to_owned();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let result = match event {
                Ok(event) if relevant(&event, &target) => Ok(()),
                Ok(_) => return,
                Err(error) => Err(error.to_string()),
            };
            // One pending notification is enough: reload reads the latest complete file.
            match send.try_send(result) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => wake(),
                Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        })?;
        watcher.watch(
            path.parent().unwrap_or(Path::new(".")),
            RecursiveMode::NonRecursive,
        )?;
        Ok(Self {
            _watcher: watcher,
            events,
            due: None,
        })
    }

    pub(crate) fn poll(&mut self, clock: f64, tx: &mut Tx) -> Option<Result<(), String>> {
        for event in self.events.try_iter() {
            if let Err(error) = event {
                self.due = None;
                return Some(Err(error));
            }
            self.due = Some(clock + DEBOUNCE);
        }
        let due = self.due?;
        if clock < due {
            tx.wake_after(std::time::Duration::from_secs_f64(due - clock));
            return None;
        }
        self.due = None;
        Some(Ok(()))
    }
}

fn relevant(event: &Event, target: &Path) -> bool {
    event.need_rescan()
        || (matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) && event.paths.iter().any(|path| path == target))
}
