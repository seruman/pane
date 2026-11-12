//! Last-good state and tree are committed together; failures remain visible until a save.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use pane_ui::{Button, Theme};
use rune::{AppSpec, Cx, El, ScrollSpec, Tx, Waker, col, row, text};

use crate::{
    vm::{Data, Node, Script},
    watch::FileWatch,
};

const MAX_SOURCE: u64 = 1024 * 1024;

#[derive(Default)]
pub(crate) struct State {
    path: PathBuf,
    script: Option<Script>,
    data: Option<Data>,
    tree: Option<Node>,
    source: Option<String>,
    error: Option<String>,
    watch: Option<FileWatch>,
}

impl State {
    pub(crate) fn open(path: PathBuf) -> Self {
        let path = absolute(&path);
        let mut state = Self {
            path,
            ..Self::default()
        };
        state.reload();
        state
    }

    fn fail(&mut self, error: impl std::fmt::Display) {
        // Don't let an error containing a huge script string overwhelm text layout.
        self.error = Some(error.to_string().chars().take(4096).collect());
    }

    fn reload(&mut self) {
        let source = match read_source(&self.path) {
            Ok(source) => source,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        if self.error.is_none() && self.source.as_ref() == Some(&source) {
            return;
        }
        match Script::load(&self.path.to_string_lossy(), &source, self.data.as_ref()) {
            Ok((script, data, tree)) => {
                self.source = Some(source);
                self.script = Some(script);
                self.data = Some(data);
                self.tree = Some(tree);
                self.error = None;
            }
            Err(error) => self.fail(error),
        }
    }

    fn dispatch(&mut self, message: &str) {
        // A failed script is paused until a successful reload, not repeatedly invoked.
        if self.error.is_some() {
            return;
        }
        let (Some(script), Some(data)) = (&self.script, &self.data) else {
            return;
        };
        match script.update(data, message) {
            Ok((data, tree)) => {
                self.data = Some(data);
                self.tree = Some(tree);
            }
            Err(error) => self.fail(error),
        }
    }

    fn setup(&mut self, wake: Waker) {
        match FileWatch::start(&self.path, wake) {
            Ok(watch) => {
                self.watch = Some(watch);
                // Close the gap between initial loading and installing the watcher.
                self.reload();
            }
            Err(error) => self.fail(format!("File watcher: {error}")),
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        match (parent.canonicalize(), path.file_name()) {
            (Ok(parent), Some(name)) => parent.join(name),
            _ => path.to_owned(),
        }
    })
}

fn read_source(path: &Path) -> std::io::Result<String> {
    let mut source = String::new();
    File::open(path)?
        .take(MAX_SOURCE + 1)
        .read_to_string(&mut source)?;
    if source.len() > usize::try_from(MAX_SOURCE).unwrap_or(usize::MAX) {
        return Err(std::io::Error::other(
            "script exceeds the 1 MiB source limit",
        ));
    }
    Ok(source)
}

fn poll(state: &mut State, clock: f64, tx: &mut Tx) -> bool {
    match state.watch.as_mut().and_then(|watch| watch.poll(clock, tx)) {
        Some(Ok(())) => {
            state.reload();
            true
        }
        Some(Err(error)) => {
            state.fail(format!("File watcher: {error}"));
            true
        }
        None => false,
    }
}

pub(crate) fn spec() -> AppSpec<State> {
    AppSpec {
        setup: Some(State::setup),
        poll: Some(poll),
        ..AppSpec::new("Pane Script", (640.0, 480.0), State::default, view)
    }
}

fn render(node: &Node, key: &str, theme: &Theme) -> El {
    match node {
        Node::Text(content) => text(content)
            .key(key)
            .font(theme.ty.body)
            .color(theme.text)
            .w_full(),
        Node::Button { label, message } => {
            let message = message.clone();
            Button::new(label)
                .on_press(move |state: &mut State| state.dispatch(&message))
                .build(key, theme)
        }
        Node::Row(children) | Node::Column(children) => {
            let mut element = if matches!(node, Node::Row(_)) {
                row()
            } else {
                col()
            };
            element = element.key(key).gap(10.0);
            for (index, child) in children.iter().enumerate() {
                element = element.child(render(child, &format!("{key}-{index}"), theme));
            }
            element
        }
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let theme = Theme::current(cx.system_dark());
    let mut body = col().w_full().gap(16.0).padding(24.0);
    if let Some(error) = &state.error {
        body = body.child(
            col()
                .key("script-error")
                .w_full()
                .gap(8.0)
                .padding(12.0)
                .child(
                    text("Script paused — fix and save to recover")
                        .font(theme.ty.strong)
                        .color(theme.text),
                )
                .child(
                    text(error)
                        .key("script-error-detail")
                        .font(theme.ty.body)
                        .color(theme.danger)
                        .w_full(),
                ),
        );
    }
    if let Some(tree) = &state.tree {
        let content = render(tree, "script", &theme);
        body = body.child(if state.error.is_some() {
            content.inert()
        } else {
            content
        });
    }
    col().w_full().h_full().fill(theme.background).child(
        body.scroll(ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0),
    )
}

#[cfg(test)]
mod tests;
