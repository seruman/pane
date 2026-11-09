//! Ports of mmenu's MenuModelTests, MenuStackTests, OperationTests, PipedInputTests and the
//! model-level KeyHandlingTests. Where mmenu raced real processes, these deliver completion
//! events in the adversarial order directly.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::*;
use crate::record::RecordDecoder;
use crate::command::ExitStatus;

struct Harness {
    model: Model,
    /// The live task of each kind: starting one replaces the previous, Cancel removes it.
    tasks: HashMap<TaskKind, Task>,
    started: Vec<Task>,
    finished: Option<Outcome>,
    copied: Vec<String>,
    beeps: usize,
    reads_stdin: bool,
    stdin_decoder: RecordDecoder,
}

impl Harness {
    fn new(options: Options) -> Harness {
        let stdin_decoder = RecordDecoder::new(options.input);
        let (model, effects) = Model::new(options);
        let mut h = Harness {
            model,
            tasks: HashMap::new(),
            started: Vec::new(),
            finished: None,
            copied: Vec::new(),
            beeps: 0,
            reads_stdin: false,
            stdin_decoder,
        };
        h.apply(effects);
        h
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::ReadStdin => {
                    assert!(!self.reads_stdin, "stdin is read at most once");
                    self.reads_stdin = true;
                }
                Effect::Start(task) => {
                    self.started.push(task.clone());
                    self.tasks.insert(task.kind(), task);
                }
                Effect::Cancel(kind) => {
                    self.tasks.remove(&kind);
                }
                Effect::Copy(text) => self.copied.push(text),
                Effect::Beep => self.beeps += 1,
                Effect::Finish(outcome) => {
                    assert!(self.finished.is_none(), "finished twice");
                    self.finished = Some(outcome);
                }
            }
        }
    }

    fn send(&mut self, event: Event) { let effects = self.model.handle(event); self.apply(effects); }
    fn nav(&mut self, nav: Nav) { self.send(Event::Nav(nav)); }
    fn query(&mut self, q: &str) { self.send(Event::SetQuery(q.into())); }
    fn toggle(&mut self, row: usize) { self.send(Event::ToggleRow(row)); }
    fn highlight(&mut self, row: usize) { self.send(Event::Highlight(row)); }
    fn perform(&mut self, index: usize) { self.send(Event::Perform(index)); }
    fn switch(&mut self, id: &str) {
        let id = self.model.options().configuration.sources().iter().find(|source| source.id.as_str() == id).unwrap().id.clone();
        self.send(Event::SwitchSource(id));
    }

    fn take(&mut self, kind: TaskKind) -> Task {
        self.tasks.remove(&kind).unwrap_or_else(|| panic!("no running {kind:?} task"))
    }

    /// Runs the live search to completion and returns its event, undelivered.
    fn run_search(&mut self) -> Event {
        let Task::Search { token, index, query, order } = self.take(TaskKind::Search) else { unreachable!() };
        Event::SearchDone { token, results: index.search(&query, order, &|| false).unwrap() }
    }

    fn settle(&mut self) {
        while self.tasks.contains_key(&TaskKind::Search) {
            let done = self.run_search();
            self.send(done);
        }
    }

    /// Decodes `text` as the live source's format, delivers it, but does not search.
    fn source_event(&mut self, text: &str) -> Event {
        let Task::Source { token, format, .. } = self.take(TaskKind::Source) else { unreachable!() };
        let mut decoder = RecordDecoder::new(format);
        let result = decoder
            .append(text.as_bytes())
            .and_then(|mut items| { items.extend(decoder.finish()?); Ok(items) })
            .map_err(|e| e.to_string());
        Event::SourceDone { token, result }
    }

    fn load(&mut self, text: &str) {
        let done = self.source_event(text);
        self.send(done);
        self.settle();
    }

    fn fail_source(&mut self, message: &str) {
        let Task::Source { token, .. } = self.take(TaskKind::Source) else { unreachable!() };
        self.send(Event::SourceDone { token, result: Err(message.into()) });
        self.settle();
    }

    fn stdin(&mut self, text: &str) {
        match self.stdin_decoder.append(text.as_bytes()) {
            Ok(items) => self.send(Event::StdinItems(items)),
            Err(error) => self.send(Event::StdinFailed(error.to_string())),
        }
    }

    fn stdin_done(&mut self) {
        match self.stdin_decoder.finish() {
            Ok(items) => {
                self.send(Event::StdinItems(items));
                self.send(Event::StdinDone);
            }
            Err(error) => self.send(Event::StdinFailed(error.to_string())),
        }
    }

    fn preview_event(&mut self, text: &str) -> (Event, Vec<(String, String)>) {
        let Task::Preview { token, env, .. } = self.take(TaskKind::Preview) else { unreachable!() };
        let result = CommandResult { stdout: text.into(), stderr: Vec::new(), status: ExitStatus::Code(0) };
        (Event::PreviewDone { token, result: Ok(result) }, env)
    }

    fn finish_operation(&mut self, status: i32, stderr: &str) {
        let Task::Operation { token, .. } = self.take(TaskKind::Operation) else { unreachable!() };
        let result = CommandResult { stdout: Vec::new(), stderr: stderr.into(), status: ExitStatus::Code(status) };
        self.send(Event::OperationDone { token, result: Ok(result) });
    }

    fn source_runs(&self, id: &str) -> usize {
        self.started.iter().filter(|task| {
            let Task::Source { input, .. } = task else { return false };
            let context: ActionContext = serde_json::from_slice(input).expect("source action context");
            context.source.as_deref() == Some(id)
        }).count()
    }

    fn settled(&self) -> bool { self.model.can_act() && !self.model.is_searching() }
    fn titles(&self) -> Vec<String> { (0..self.model.result_count()).map(|r| self.model.result(r).title().to_owned()).collect() }
    fn values(&self) -> Vec<String> { self.model.chosen().into_iter().map(|i| i.value().to_owned()).collect() }
    fn marked(&self) -> HashSet<String> { self.model.shown.selection.checked.clone() }
    fn all_items(&self) -> Vec<String> { self.model.shown.items.iter().map(|i| i.title().to_owned()).collect() }

    fn is_open(&self) -> bool { self.finished.is_none() }

    fn output(&self) -> (i32, String) {
        let outcome = self.finished.as_ref().expect("not finished");
        let output = match outcome {
            Outcome::Chosen(output) => String::from_utf8(output.clone()).unwrap(),
            _ => String::new(),
        };
        (outcome.exit_code(), output)
    }

    fn context(&self) -> ActionContext {
        let (status, text) = self.output();
        assert_eq!(status, 0);
        assert!(text.ends_with('\n'));
        serde_json::from_str(&text).unwrap()
    }
}

fn src(id: &str, extra: Value) -> Value {
    let mut source = json!({"id": id, "label": capitalized(id), "command": ["src", id]});
    source.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    source
}

fn capitalized(id: &str) -> String { id[..1].to_uppercase() + &id[1..] }

fn options(config: Value) -> Options {
    Options {
        configuration: serde_json::from_value(config).unwrap(),
        input: InputFormat::Lines,
        multi: true,
        json: true,
        print0: false,
        sorted: false,
        prompt: "Root".into(),
        query: String::new(),
        rows: 12,
        list_cap: usize::MAX,
    }
}

fn start(config: Value) -> Harness { Harness::new(options(config)) }

/// JSONL rows whose value is `/v/<id>`.
fn jsonl(rows: &[(&str, &str)]) -> String {
    rows.iter().map(|(id, title)| format!("{}\n", json!({"id": id, "title": title, "value": format!("/v/{id}")}))).collect()
}

fn chord(raw: &str) -> KeyChord { KeyChord::parse(raw).unwrap() }

fn cancelled(h: &Harness, kind: TaskKind) -> bool { !h.tasks.contains_key(&kind) }

fn env<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
    &env.iter().find(|(k, _)| k == key).unwrap().1
}

#[test]
fn outcomes_define_the_process_exit_codes() {
    assert_eq!(Outcome::Chosen(b"picked\n".to_vec()).exit_code(), 0);
    assert_eq!(Outcome::Cancelled.exit_code(), 130);
    assert_eq!(Outcome::Failed("bad input".into()).exit_code(), 2);
}

#[test]
fn cancelling_carries_the_original_stdin_error_in_the_finish_effect() {
    let (mut model, _) = Model::new(options(json!({})));
    model.handle(Event::StdinFailed("invalid input record".into()));
    model.message = Some(Status::Ack("unrelated status".into()));
    let effects = model.handle(Event::Nav(Nav::Cancel));
    assert!(matches!(effects.as_slice(), [Effect::Finish(Outcome::Failed(message))]
        if message == "invalid input record"));
}

// MenuModelTests

#[test]
fn source_switch_keeps_presented_snapshot_through_command_and_matching() {
    let mut o = options(json!({"sources": [src("projects", json!({"format": "jsonl"})), src("files", json!({}))]}));
    o.query = "a".into();
    let mut h = Harness::new(o);
    h.load(r#"{"id":"a","title":"alpha","subtitle":"Project"}
{"id":"b","title":"beta"}
"#);
    h.toggle(1);
    let (items, titles, row, marks) = (h.all_items(), h.titles(), h.model.highlighted_row(), h.marked());
    let revision = h.model.result_revision();
    let unchanged = |h: &Harness| {
        assert_eq!(h.model.result_revision(), revision);
        assert_eq!((h.all_items(), h.titles(), h.model.highlighted_row(), h.marked()), (items.clone(), titles.clone(), row, marks.clone()));
        assert!(h.model.loading());
        assert!(!h.model.can_act());
    };

    h.switch("files");
    assert_eq!(h.model.query(), "a");
    unchanged(&h);
    h.query("al");
    assert!(cancelled(&h, TaskKind::Search), "no interim search over the empty replacement");
    unchanged(&h);

    let done = h.source_event("alpine\nomega\n");
    h.send(done);
    assert!(h.model.is_searching());
    unchanged(&h);
    h.settle();
    assert_eq!(h.model.result_revision(), revision + 1);
    assert_eq!(h.all_items(), ["alpine", "omega"]);
    assert_eq!(h.titles(), ["alpine"]);
    assert_eq!(h.model.highlighted_row(), Some(0));
    assert!(h.marked().is_empty());
    assert!(!h.model.loading());
    assert!(h.settled());
    assert_eq!(h.model.query(), "al");
}

#[test]
fn stale_source_snapshot_cannot_be_highlighted_marked_or_acted_on() {
    let mut h = start(json!({
        "sources": [src("a", json!({})), src("b", json!({}))],
        "preview": ["true"],
        "bindings": [{"key": "ctrl+d", "action": "run", "command": ["true"]}],
    }));
    h.load("alpha\nbeta\n");
    h.toggle(1);
    let marks = h.marked();
    assert_eq!(h.model.preview(), Some(""), "nothing to show until the first preview lands");

    h.switch("b");
    assert_eq!(h.model.preview(), Some(""), "the preview clears as the source switches");
    assert!(cancelled(&h, TaskKind::Preview));
    assert_eq!(h.model.source_id().map(SourceId::as_str), Some("b"));
    assert!(!h.model.can_act());
    h.highlight(0);
    assert_eq!(h.model.highlighted_row(), Some(1));
    h.nav(Nav::Up);
    assert_eq!(h.model.highlighted_row(), Some(1));
    h.toggle(0);
    h.nav(Nav::Tab);
    assert_eq!(h.marked(), marks);
    h.perform(0);
    assert!(!h.model.running_operation());
    h.nav(Nav::Accept);
    assert_eq!(h.beeps, 2, "the run and the accept are both refused");

    h.load("bravo\n");
    assert!(h.settled());
    assert!(h.is_open(), "the refused accept was not replayed");
    assert_eq!(h.values(), ["bravo"]);
    assert!(h.marked().is_empty());
}

#[test]
fn empty_and_failed_sources_commit_coherent_empty_snapshots() {
    for failure in [None, Some("Command exited 7: failure")] {
        let mut h = start(json!({"sources": [src("good", json!({"format": "jsonl"})), src("next", json!({"format": "jsonl"}))]}));
        h.load(&jsonl(&[("a", "alpha")]));
        h.toggle(0);
        let (revision, items, marks) = (h.model.result_revision(), h.all_items(), h.marked());
        h.switch("next");
        h.query("a");
        assert_eq!((h.model.result_revision(), h.all_items(), h.marked()), (revision, items.clone(), marks.clone()), "failure={failure:?}");
        match failure {
            None => h.load(""),
            Some(message) => h.fail_source(message),
        }
        assert_eq!(h.model.result_revision(), revision + 1, "failure={failure:?}");
        assert_eq!(h.model.result_count(), 0, "failure={failure:?}");
        assert!(h.all_items().is_empty(), "failure={failure:?}");
        assert!(h.model.chosen().is_empty(), "failure={failure:?}");
        assert_eq!(h.model.highlighted_row(), None, "failure={failure:?}");
        assert_eq!(h.model.shown.selection.highlighted_id, None, "failure={failure:?}");
        assert!(h.marked().is_empty(), "failure={failure:?}");
        assert!(!h.model.loading(), "failure={failure:?}");
        assert!(!h.model.is_searching(), "failure={failure:?}");
        assert_eq!(h.model.input_failed(), failure.is_some(), "failure={failure:?}");
        assert_eq!(h.model.message(), failure, "failure={failure:?}");
        assert_eq!(h.model.can_act(), failure.is_none(), "failure={failure:?}");

        h.query("al");
        h.settle();
        assert!(h.model.chosen().is_empty(), "failure={failure:?}");
        assert_eq!(h.model.can_act(), failure.is_none(), "failure={failure:?}");

        // Failure is local to this source generation, not a permanent disabled state.
        h.switch("good");
        h.load(&jsonl(&[("a", "alpha")]));
        h.query("");
        h.settle();
        assert!(h.settled(), "failure={failure:?}");
        assert_eq!(h.values(), ["/v/a"], "failure={failure:?}");
        assert!(h.marked().is_empty(), "failure={failure:?}");
        assert_eq!(h.model.message(), None, "failure={failure:?}");
    }
}

#[test]
fn superseded_source_cannot_publish() {
    for during_matching in [false, true] {
        let mut h = start(json!({"sources": [src("a", json!({})), src("b", json!({})), src("c", json!({}))]}));
        h.load("alpha\n");
        let revision = h.model.result_revision();
        h.switch("b");
        let late = h.source_event("bravo\n");
        let late = if during_matching {
            h.send(late);
            assert!(h.model.is_searching(), "during_matching={during_matching}");
            let search = h.run_search();
            h.switch("c");
            search
        } else {
            h.switch("c");
            late
        };
        h.send(late);
        assert_eq!(h.model.result_revision(), revision, "a superseded source published, during_matching={during_matching}");
        assert_eq!(h.values(), ["alpha"], "during_matching={during_matching}");
        assert!(h.model.loading(), "during_matching={during_matching}");
        assert!(!h.model.can_act(), "during_matching={during_matching}");
        h.load("charlie\n");
        assert_eq!(h.model.result_revision(), revision + 1, "during_matching={during_matching}");
        assert_eq!(h.model.source_id().map(SourceId::as_str), Some("c"), "during_matching={during_matching}");
        assert_eq!(h.values(), ["charlie"], "during_matching={during_matching}");
        assert!(h.settled(), "during_matching={during_matching}");
        assert!(!h.model.loading(), "during_matching={during_matching}");
    }
}

#[test]
fn query_edit_during_replacement_matching_publishes_only_latest_query() {
    let mut o = options(json!({"sources": [src("a", json!({})), src("b", json!({}))]}));
    o.query = "al".into();
    let mut h = Harness::new(o);
    h.load("alpha\n");
    let revision = h.model.result_revision();
    h.switch("b");
    let done = h.source_event("alpine\nomega\n");
    h.send(done);
    let stale = h.run_search();
    h.query("ome");
    h.send(stale);
    assert_eq!(h.model.result_revision(), revision);
    assert_eq!(h.values(), ["alpha"]);
    h.settle();
    assert_eq!(h.model.result_revision(), revision + 1);
    assert_eq!(h.model.query(), "ome");
    assert_eq!(h.values(), ["omega"]);
    assert!(!h.model.loading());
    assert!(h.settled());
}

#[test]
fn keys_during_a_slow_search_are_not_lost() {
    let mut h = start(json!({"sources": [src("big", json!({}))]}));
    let rows: String = (0..1000).map(|i| format!("item-{i}-some/path/segment\n")).collect();
    h.load(&rows);
    h.query("item 9");
    assert!(h.model.is_searching());
    // Navigation acts on the rows still shown.
    h.nav(Nav::Down);
    assert_eq!(h.model.highlighted_row(), Some(1));
    // Enter is kept and applied to the typed query's results, not dropped or applied to old rows.
    h.nav(Nav::Accept);
    assert!(h.is_open());
    h.settle();
    let context = h.context();
    assert_eq!(context.query, "item 9");
    assert_eq!(context.items.len(), 1);
    assert!(context.items[0].value().contains('9'), "accepted a row matching the query typed before Enter");
}

#[test]
fn pending_query_cannot_run_with_old_items() {
    let mut h = start(json!({
        "sources": [src("test", json!({}))],
        "bindings": [{"key": "ctrl+d", "action": "run", "command": ["touch"]}],
    }));
    h.load("alpha\nbeta\n");
    assert_eq!(h.values(), ["alpha"]);
    h.query("does-not-match");
    assert!(!h.model.can_act());
    h.perform(0);
    h.settle();
    assert_eq!(h.model.result_count(), 0);
    assert!(!h.started.iter().any(|t| t.kind() == TaskKind::Operation));
}

#[test]
fn reload_prunes_checks_even_when_query_changes_during_loading() {
    let mut h = start(json!({
        "sources": [src("test", json!({"format": "jsonl"}))],
        "bindings": [{"key": "ctrl+r", "action": "reload"}],
    }));
    h.load(&jsonl(&[("a", "alpha"), ("b", "beta")]));
    h.toggle(0);
    h.toggle(1);
    let (items, revision) = (h.all_items(), h.model.result_revision());
    assert_eq!(h.marked(), HashSet::from(["a".into(), "b".into()]));
    h.perform(0);
    h.query("alp");
    assert!(!h.model.can_act());
    assert!(h.model.loading());
    assert_eq!(h.model.result_revision(), revision);
    assert_eq!(h.all_items(), items);
    assert_eq!(h.marked().len(), 2);
    assert_eq!(h.model.highlighted_row(), Some(1));
    h.load(&jsonl(&[("a", "alpine")]));
    assert_eq!(h.model.result_revision(), revision + 1);
    assert_eq!(h.marked(), HashSet::from(["a".into()]));
    assert_eq!(h.model.chosen()[0].title(), "alpine");
    assert_eq!(h.model.highlighted_row(), Some(0));
    assert!(!h.model.loading());
    assert!(h.settled());
}

#[test]
fn plain_reload_cannot_transfer_checks_to_different_values() {
    let mut h = start(json!({
        "sources": [src("test", json!({}))],
        "bindings": [{"key": "ctrl+r", "action": "reload"}],
    }));
    h.load("alpha\nbeta\n");
    h.toggle(1);
    let revision = h.model.result_revision();
    h.perform(0);
    assert_eq!(h.marked().len(), 1);
    assert_eq!(h.all_items(), ["alpha", "beta"]);
    h.load("charlie\ndelta\n");
    assert_eq!(h.model.result_revision(), revision + 1);
    assert_eq!(h.all_items(), ["charlie", "delta"]);
    assert!(h.marked().is_empty());
    assert_eq!(h.model.highlighted_row(), Some(0));
}

#[test]
fn cancelling_after_a_failed_command_source_is_an_ordinary_cancel() {
    let mut h = start(json!({"sources": [src("bad", json!({}))]}));
    h.fail_source("Command exited 7");
    assert!(h.model.input_failed());
    assert!(!h.model.loading());
    h.nav(Nav::Back);
    assert_eq!(h.output(), (130, String::new()));
}

#[test]
fn values_containing_the_output_delimiter_are_never_written() {
    for print0 in [true, false] {
        let unsafe_value = if print0 { "a\0b" } else { "a\nb" };
        let mut o = options(json!({"sources": [src("s", json!({"format": "jsonl"}))]}));
        o.json = false;
        o.print0 = print0;
        let mut h = Harness::new(o);
        let rows = format!("{}\n{}\n", json!({"id": "1", "title": "bad", "value": unsafe_value}), json!({"id": "2", "title": "good", "value": "ok"}));
        h.load(&rows);
        h.nav(Nav::Accept);
        assert!(h.is_open(), "an ambiguous record must not be emitted, print0={print0}");
        assert_eq!(h.beeps, 1, "print0={print0}");
        let message = h.model.message().unwrap_or_else(|| panic!("missing refusal, print0={print0}")).to_owned();
        assert!(message.contains("--json"), "print0={print0}");
        assert_eq!(message.contains("--print0"), !print0, "print0={print0}");
        h.nav(Nav::Down);
        assert_eq!(h.model.message(), Some(message.as_str()), "the refusal survives a move, print0={print0}");
        assert!(h.model.message_is_error(), "print0={print0}");
        h.nav(Nav::Accept);
        assert_eq!(h.output(), (0, if print0 { "ok\0" } else { "ok\n" }.into()), "print0={print0}");
    }
}

#[test]
fn preview_commands_get_the_item_in_the_environment_after_a_delay() {
    let mut h = start(json!({"sources": [src("a", json!({}))], "preview": ["show"]}));
    h.load("alpha\n");
    let Task::Preview { argv, delay, .. } = &h.tasks[&TaskKind::Preview] else { unreachable!() };
    assert_eq!(argv, &["show"]);
    assert_eq!(*delay, Duration::from_millis(60));
    let (done, vars) = h.preview_event("value=alpha");
    assert_eq!(env(&vars, "MMENU_VALUE"), "alpha");
    assert_eq!(env(&vars, "MMENU_ACTION"), "preview");
    h.send(done);
    assert_eq!(h.model.preview(), Some("value=alpha"));
}

#[test]
fn preview_tracks_query_and_stops_when_hidden_or_source_changes() {
    let mut o = options(json!({
        "sources": [src("a", json!({})), src("b", json!({}))],
        "preview": ["show"],
        "bindings": [{"key": "alt+p", "action": "togglePreview"}],
    }));
    o.query = "a".into();
    let mut h = Harness::new(o);
    h.load("alpha\n");
    let (done, vars) = h.preview_event("a");
    assert_eq!(env(&vars, "MMENU_QUERY"), "a");
    h.send(done);
    assert_eq!(h.model.preview(), Some("a"));

    h.query("al");
    assert_eq!(h.model.preview(), Some("a"), "the last preview stays while the next is fetched");
    assert!(cancelled(&h, TaskKind::Preview));
    h.settle();
    assert_eq!(h.model.preview(), Some("a"));
    let (stale, vars) = h.preview_event("al");
    assert_eq!(env(&vars, "MMENU_QUERY"), "al");
    h.query("alp");
    h.send(stale);
    assert_eq!(h.model.preview(), Some("a"), "a preview for an older query cannot publish");
    h.settle();
    let (hidden, _) = h.preview_event("alp");

    h.perform(0);
    assert_eq!(h.model.preview(), None);
    assert!(cancelled(&h, TaskKind::Preview));
    h.send(hidden);
    assert_eq!(h.model.preview(), None);
    h.perform(0);
    assert_eq!(h.model.preview(), Some(""), "a hidden preview comes back empty, not with old text");

    h.switch("b");
    assert_eq!(h.model.preview(), Some(""));
    assert!(cancelled(&h, TaskKind::Preview));
    h.load("");
    assert_eq!(h.model.preview(), Some(""));
    assert_eq!(h.model.result_count(), 0);
}

// MenuStackTests

fn push_binding() -> Value {
    json!({"key": "ctrl+w", "id": "worktrees", "label": "Worktrees", "action": "push", "source": "child"})
}

const REPOS: [(&str, &str); 4] = [("a", "alpha"), ("b", "beta"), ("c", "gamma"), ("d", "delta")];

fn repos(extra: Value) -> Value {
    let mut extra = extra;
    extra.as_object_mut().unwrap().insert("format".into(), "jsonl".into());
    src("repos", extra)
}

fn hidden(id: &str, extra: Value) -> Value {
    let mut extra = extra;
    let object = extra.as_object_mut().unwrap();
    object.entry("format").or_insert("jsonl".into());
    object.insert("hidden".into(), true.into());
    src(id, extra)
}

#[test]
fn push_then_pop_restores_the_parent_without_rerunning_it() {
    let mut h = start(json!({
        "sources": [
            repos(json!({"prompt": "Repos", "command": ["root-program"]})),
            hidden("child", json!({"command": ["child-program"]})),
        ],
        "bindings": [push_binding()],
    }));
    h.load(&jsonl(&REPOS));
    h.query("a");
    h.settle();
    h.toggle(0);
    h.toggle(2);
    h.highlight(1);
    let (items, titles, row, marks, highlighted) =
        (h.all_items(), h.titles(), h.model.highlighted_row(), h.marked(), h.model.shown.selection.highlighted_id.clone());
    assert_eq!(h.model.prompt(), "Repos");
    assert_eq!((titles.len(), marks.len()), (4, 2));

    h.perform(0);
    assert_eq!(h.model.depth(), 1);
    assert_eq!(h.model.query(), "");
    assert_eq!(h.all_items(), items, "the parent's rows stay presented until the child commits");
    assert!(!h.model.can_act());
    h.load(&jsonl(&[("x", "xray"), ("y", "yankee")]));
    assert_eq!(h.titles(), ["xray", "yankee"]);
    assert!(h.marked().is_empty());
    assert_eq!(h.model.prompt(), "alpha +1 › Child");

    let revision = h.model.result_revision();
    h.nav(Nav::Back);
    assert_eq!(h.model.depth(), 0);
    assert!(h.model.can_act());
    assert_ne!(h.model.result_revision(), revision, "a host resets scrolling to the parent's");
    assert_eq!((h.all_items(), h.titles(), h.model.highlighted_row(), h.marked()), (items, titles, row, marks));
    assert_eq!(h.model.shown.selection.highlighted_id, highlighted);
    assert_eq!(h.model.query(), "a");
    assert_eq!(h.model.prompt(), "Repos");
    assert!(h.tasks.is_empty());
    assert_eq!(h.source_runs("repos"), 1, "popping must not re-run the parent's command");
    assert_eq!(h.source_runs("child"), 1);
}

#[test]
fn child_command_receives_the_parent_selection_as_data() {
    let hostile = "odd $(touch INJECTED) `touch INJECTED`; touch INJECTED";
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.load(&format!("{}\n{}", json!({"id": "o", "title": "odd", "value": hostile}), jsonl(&[("b", "beta")])));
    h.query("be");
    h.settle();
    h.perform(0);
    let Task::Source { argv, input, env: vars, .. } = h.tasks[&TaskKind::Source].clone() else { unreachable!() };
    assert_eq!(argv, ["src", "child"]);
    let context: ActionContext = serde_json::from_slice(&input).unwrap();
    assert_eq!((context.action.as_str(), context.source.as_deref()), ("source", Some("child")));
    assert_eq!(context.items.iter().map(|i| i.value()).collect::<Vec<_>>(), ["/v/b"]);
    assert_eq!(context.path, [PathEntry { source: Some("repos".into()), query: "be".into(), items: context.items.clone() }]);
    assert_eq!((env(&vars, "MMENU_VALUE"), env(&vars, "MMENU_ID")), ("/v/b", "b"));

    h.nav(Nav::Back);
    h.query("");
    h.settle();
    h.highlight(0);
    h.perform(0);
    let Task::Source { argv, env: vars, .. } = h.tasks[&TaskKind::Source].clone() else { unreachable!() };
    assert_eq!(env(&vars, "MMENU_VALUE"), hostile, "the child sees the exact bytes");
    assert_eq!(argv, ["src", "child"], "values are never substituted into the command");
}

#[test]
fn a_slow_child_popped_mid_load_never_publishes_over_the_parent() {
    for during_matching in [false, true] {
        let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
        h.load(&jsonl(&REPOS));
        h.toggle(1);
        let (items, marks) = (h.all_items(), h.marked());
        h.perform(0);
        let late = h.source_event(&jsonl(&[("l", "late-1")]));
        let late = if during_matching {
            h.send(late);
            assert!(h.model.is_searching(), "during_matching={during_matching}");
            let search = h.run_search();
            h.nav(Nav::Back);
            search
        } else {
            h.nav(Nav::Back);
            late
        };
        let revision = h.model.result_revision();
        h.send(late);
        h.settle();
        assert_eq!(h.model.result_revision(), revision, "a late child result was published, during_matching={during_matching}");
        assert_eq!((h.all_items(), h.marked()), (items, marks), "during_matching={during_matching}");
        assert!(h.model.can_act(), "during_matching={during_matching}");
        assert_eq!(h.model.depth(), 0, "during_matching={during_matching}");
    }
}

#[test]
fn rapid_push_pop_push_lands_on_the_last_pushed_level() {
    let slow = json!({"key": "ctrl+e", "id": "slow", "action": "push", "source": "slow"});
    let mut h = start(json!({"sources": [repos(json!({})), hidden("slow", json!({})), hidden("child", json!({}))], "bindings": [slow, push_binding()]}));
    h.load(&jsonl(&REPOS));
    h.perform(0);
    let stale = h.source_event(&jsonl(&[("s", "stale")]));
    h.nav(Nav::Back);
    h.perform(1);
    assert_eq!((h.model.depth(), h.model.source_id().map(SourceId::as_str)), (1, Some("child")));
    h.send(stale);
    h.load(&jsonl(&[("f", "fresh")]));
    assert_eq!((h.model.depth(), h.model.source_id().map(SourceId::as_str)), (1, Some("child")));
    assert_eq!(h.titles(), ["fresh"]);

    // A second push while the child loads is refused, not queued onto stale rows.
    h.nav(Nav::Back);
    h.perform(1);
    h.perform(0);
    assert_eq!(h.beeps, 1);
    h.load(&jsonl(&[("f", "fresh")]));
    assert!(h.settled());
    assert_eq!((h.model.depth(), h.model.source_id().map(SourceId::as_str)), (1, Some("child")));
    assert_eq!(h.titles(), ["fresh"]);
}

fn two_levels(marks: MarkScope, json_output: bool) -> Harness {
    let grand = json!({"key": "ctrl+g", "id": "deeper", "action": "push", "source": "grand"});
    let split = json!({"key": "ctrl+v", "id": "split-right", "action": "accept"});
    let mut o = options(json!({
        "sources": [repos(json!({})), hidden("child", json!({})), hidden("grand", json!({}))],
        "bindings": [push_binding(), grand, split],
        "marks": match marks { MarkScope::Level => "level", MarkScope::Global => "global" },
    }));
    o.json = json_output;
    let mut h = Harness::new(o);
    h.load(&jsonl(&REPOS));
    h.toggle(1);
    h.toggle(3);
    h.perform(0);
    h.load(&jsonl(&[("x", "xray"), ("y", "yankee"), ("z", "zulu")]));
    h.query("u");
    h.settle();
    assert_eq!(h.model.result_count(), 1);
    h.toggle(0);
    h.perform(1);
    h.load(&jsonl(&[("1", "one"), ("2", "two")]));
    assert_eq!(h.model.depth(), 2);
    h.highlight(1);
    h
}

#[test]
fn accept_at_depth_two_returns_the_path() {
    for marks in [MarkScope::Level, MarkScope::Global] {
        let mut h = two_levels(marks, true);
        h.perform(2);
        let context = h.context();
        assert_eq!((context.action.as_str(), context.source.as_deref()), ("split-right", Some("grand")), "marks={marks:?}");
        let ids = |items: &[MenuItem]| items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        assert_eq!(context.path.iter().map(|p| p.source.as_deref()).collect::<Vec<_>>(), [Some("repos"), Some("child"), Some("grand")], "marks={marks:?}");
        assert_eq!(context.path.iter().map(|p| p.query.as_str()).collect::<Vec<_>>(), ["", "u", ""], "marks={marks:?}");
        assert_eq!(context.path.iter().map(|p| ids(&p.items)).collect::<Vec<_>>(), [vec!["b", "d"], vec!["z"], vec!["2"]], "marks={marks:?}");
        let expected = match marks {
            MarkScope::Level => vec!["2"],
            MarkScope::Global => vec!["b", "d", "z"],
        };
        assert_eq!(ids(&context.items), expected, "marks={marks:?}");
    }
}

#[test]
fn global_marks_include_the_current_levels_marks_in_plain_output() {
    let mut h = two_levels(MarkScope::Global, false);
    h.toggle(0);
    h.nav(Nav::Accept);
    assert_eq!(h.output(), (0, "/v/b\n/v/d\n/v/z\n/v/1\n".into()));
}

#[test]
fn level_marks_plain_output_is_the_current_level() {
    let mut h = two_levels(MarkScope::Level, false);
    h.nav(Nav::Accept);
    assert_eq!(h.output(), (0, "/v/2\n".into()));
}

#[test]
fn push_with_no_results_is_a_no_op() {
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.load(&jsonl(&REPOS));
    h.query("does-not-match");
    h.perform(0); // queued until the query's results land, then refused
    h.settle();
    assert_eq!(h.model.result_count(), 0);
    h.perform(0);
    assert_eq!((h.model.depth(), h.model.source_id().map(SourceId::as_str)), (0, Some("repos")));
    assert_eq!(h.source_runs("child"), 0);
}

#[test]
fn push_pressed_before_the_query_results_land_uses_those_results() {
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.load(&jsonl(&REPOS));
    h.query("gam");
    h.perform(0);
    assert_eq!(h.model.depth(), 0);
    h.settle();
    assert_eq!(h.model.depth(), 1);
    let Task::Source { env: vars, .. } = &h.tasks[&TaskKind::Source] else { unreachable!() };
    assert_eq!(env(vars, "MMENU_VALUE"), "/v/c");
}

#[test]
fn escape_and_empty_backspace_pop_then_escape_cancels_at_the_root() {
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.load(&jsonl(&REPOS));
    assert_eq!(h.model.target(&chord("ctrl+w")), Some(Target::Perform(0)));
    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    assert_eq!(h.model.depth(), 1);
    h.nav(Nav::Back);
    assert_eq!(h.model.depth(), 0);

    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    h.query("x");
    h.nav(Nav::PopIfEmpty);
    assert_eq!(h.model.depth(), 1, "backspace edits a nonempty query");
    h.query("");
    h.nav(Nav::PopIfEmpty);
    assert_eq!(h.model.depth(), 0);
    assert_eq!(h.model.query(), "", "the parent's query is restored");
    h.nav(Nav::PopIfEmpty);
    assert_eq!(h.model.depth(), 0);
    assert!(h.is_open());
    h.nav(Nav::Back);
    assert_eq!(h.output(), (130, String::new()));
}

#[test]
fn cancel_at_depth_exits_130() {
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.load(&jsonl(&REPOS));
    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    h.nav(Nav::Cancel);
    assert_eq!(h.output(), (130, String::new()));
}

#[test]
fn level_restricted_bindings_fire_only_at_their_levels() {
    let root_only = json!({"key": "ctrl+w", "id": "worktrees", "action": "push", "source": "child", "sources": ["repos"]});
    let child_only = json!({"key": "ctrl+o", "id": "open-child", "action": "accept", "sources": ["child"]});
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [root_only, child_only]}));
    h.load(&jsonl(&REPOS));
    assert_eq!(h.model.target(&chord("ctrl+o")), None);
    h.perform(1);
    assert!(h.is_open());
    assert_eq!(h.model.target(&chord("ctrl+w")), Some(Target::Perform(0)));
    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    assert_eq!(h.model.depth(), 1);
    assert_eq!(h.model.target(&chord("ctrl+w")), None, "the repo-level binding is inert in the child");
    h.perform(0);
    assert_eq!((h.model.depth(), h.model.source_id().map(SourceId::as_str)), (1, Some("child")));
    assert_eq!(h.model.target(&chord("ctrl+o")), Some(Target::Perform(1)));
    h.perform(1);
    assert_eq!(h.context().action, "open-child");
}

#[test]
fn piped_stdin_keeps_streaming_into_the_root_behind_a_child() {
    let mut h = start(json!({"sources": [hidden("child", json!({"format": "lines"}))], "bindings": [push_binding()]}));
    assert!(h.reads_stdin);
    h.stdin("alpha\n");
    h.settle();
    assert!(h.settled());
    h.perform(0);
    h.load("in alpha\n");
    assert_eq!(h.titles(), ["in alpha"]);
    h.stdin("beta\n");
    h.stdin_done();
    assert!(cancelled(&h, TaskKind::Search), "root input never searches in the child");
    assert_eq!(h.titles(), ["in alpha"]);
    h.nav(Nav::Back);
    h.settle();
    assert_eq!(h.titles(), ["alpha", "beta"]);
    assert!(!h.model.loading());
}

#[test]
fn prompt_and_source_keys_follow_the_level() {
    let root_only = json!({"key": "ctrl+w", "id": "worktrees", "action": "push", "source": "child", "sources": ["repos"]});
    let everywhere = json!({"key": "ctrl+v", "id": "split", "action": "accept"});
    let mut h = start(json!({
        "sources": [repos(json!({"prompt": "Repos"})), src("files", json!({"key": "ctrl+2"})), hidden("child", json!({"prompt": "Worktrees"}))],
        "bindings": [root_only, everywhere],
    }));
    h.load(&jsonl(&REPOS));
    assert_eq!(h.model.prompt(), "Repos");
    assert!(matches!(h.model.target(&chord("ctrl+2")), Some(Target::SwitchSource(id)) if id.as_str() == "files"));
    h.highlight(1);
    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    assert_eq!(h.model.prompt(), "beta › Worktrees");
    assert_eq!(h.model.target(&chord("ctrl+2")), None, "sibling sources cannot replace a pushed level");
    h.switch("files");
    assert_eq!(h.model.source_id().map(SourceId::as_str), Some("child"));
    assert_eq!(h.model.target(&chord("ctrl+v")), Some(Target::Perform(1)));
    h.nav(Nav::Back);
    assert_eq!(h.model.prompt(), "Repos");
}

#[test]
fn a_child_operation_finishing_after_pop_does_not_reload_the_parent() {
    let star = json!({"key": "ctrl+d", "id": "star", "action": "run", "command": ["star"], "reload": true});
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))], "bindings": [push_binding(), star]}));
    h.load(&jsonl(&REPOS));
    h.perform(0);
    h.load(&jsonl(&[("x", "xray")]));
    h.perform(1);
    assert!(h.model.running_operation());
    h.nav(Nav::Back);
    h.finish_operation(0, "");
    assert!(!h.model.running_operation());
    assert_eq!((h.source_runs("repos"), h.source_runs("child")), (1, 1));
    assert_eq!(h.model.depth(), 0);
    assert!(h.model.can_act());
}

// OperationTests

fn operations(bindings: Value) -> Harness { start(json!({"sources": [src("s", json!({}))], "bindings": bindings})) }

#[test]
fn run_receives_the_chosen_items_and_reloads_on_success() {
    let mut h = operations(json!([{"key": "ctrl+d", "id": "star", "label": "Star", "action": "run", "command": ["star"], "reload": true}]));
    h.load("1\none\ntwo\n");
    h.toggle(2);
    h.toggle(1);
    h.perform(0);
    assert_eq!(h.model.message(), Some("Running star…"));
    let Task::Operation { argv, input, env: vars, .. } = h.tasks[&TaskKind::Operation].clone() else { unreachable!() };
    assert_eq!(argv, ["star"]);
    let context: ActionContext = serde_json::from_slice(&input).unwrap();
    assert_eq!((context.action.as_str(), context.source.as_deref()), ("star", Some("s")));
    assert_eq!(context.items.iter().map(|i| i.value()).collect::<Vec<_>>(), ["one", "two"]);
    assert_eq!(env(&vars, "MMENU_ACTION"), "star");
    h.finish_operation(0, "");
    assert_eq!(h.source_runs("s"), 2);
    h.load("2\none\ntwo\n");
    assert_eq!(h.titles()[0], "2");
    assert!(h.settled());
    assert_eq!(h.model.message(), None, "a successful operation reports no error after reloading");
}

#[test]
fn failed_run_reports_and_does_not_reload() {
    let mut h = operations(json!([{"key": "ctrl+d", "action": "run", "command": ["fail"], "reload": true}]));
    h.load("one\n");
    h.perform(0);
    h.finish_operation(3, "nope\n");
    assert!(!h.model.running_operation());
    assert_eq!(h.model.message(), Some("Command exited 3: nope"));
    assert_eq!(h.source_runs("s"), 1, "a failed operation must not re-run the source");
}

#[test]
fn accept_waits_for_a_running_operation() {
    let mut h = operations(json!([{"key": "ctrl+d", "action": "run", "command": ["wait"]}]));
    h.load("one\n");
    h.perform(0);
    assert!(h.model.running_operation());
    h.perform(0);
    assert_eq!(h.started.iter().filter(|t| t.kind() == TaskKind::Operation).count(), 1, "operations are serialized");
    h.nav(Nav::Accept);
    assert!(h.is_open(), "accept is refused while an operation runs");
    assert_eq!(h.beeps, 2, "the second run and the accept are both refused");
    h.finish_operation(0, "");
    h.nav(Nav::Accept);
    assert_eq!(h.context().items[0].value(), "one");
}

#[test]
fn copy_puts_chosen_values_on_the_clipboard_and_stays_open() {
    let mut h = operations(json!([{"key": "ctrl+alt+c", "action": "copy"}]));
    h.load("one\ntwo\nthree\n");
    h.toggle(2);
    h.toggle(0);
    h.perform(0);
    assert_eq!(h.copied, ["one\nthree"]);
    assert_eq!(h.model.message(), Some("Copied 2"));
    assert!(h.is_open(), "copy keeps the menu open");
}

#[test]
fn command_failure_trims_and_bounds_stderr() {
    let result = |stderr: Vec<u8>| CommandResult { stdout: Vec::new(), stderr, status: ExitStatus::Code(3) };
    assert_eq!(command_failure(&result(b"  \n".to_vec())), "Command exited 3");
    let long = command_failure(&result(vec![b'x'; 5000]));
    assert_eq!(long.len(), "Command exited 3: ".len() + 2048);
}

// PipedInputTests

fn piped(json_output: bool, print0: bool) -> Harness {
    let mut o = options(json!({}));
    o.json = json_output;
    o.print0 = print0;
    Harness::new(o)
}

#[test]
fn stdin_helper_assigns_distinct_ids_across_appends() {
    let mut h = piped(true, false);
    h.stdin("same\n");
    h.stdin("same\n");
    h.settle();
    h.toggle(0);
    h.toggle(1);
    h.nav(Nav::Accept);
    let context = h.context();
    assert_eq!(context.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["0", "1"]);
    assert_eq!(context.items.iter().map(|item| item.value()).collect::<Vec<_>>(), ["same", "same"]);
}

#[test]
fn stdin_helper_buffers_records_in_the_configured_format() {
    let mut o = options(json!({}));
    o.input = InputFormat::Nul;
    let mut h = Harness::new(o);
    h.stdin("first\n");
    h.settle();
    assert_eq!(h.model.result_count(), 0);
    h.stdin("value\0");
    h.settle();
    assert_eq!(h.titles(), ["first\nvalue"]);
    h.stdin("last");
    h.stdin_done();
    h.settle();
    assert_eq!(h.titles(), ["first\nvalue", "last"]);
    assert!(!h.model.loading());
}

#[test]
fn rows_arrive_before_eof_and_eof_ends_loading() {
    let mut h = piped(false, false);
    assert!(h.reads_stdin);
    assert!(h.tasks.is_empty());
    h.stdin("one\ntwo\n");
    h.settle();
    assert_eq!(h.model.result_count(), 2);
    assert!(h.model.loading(), "still streaming: the writer has not closed");
    assert!(h.model.can_act(), "selection works before EOF");
    h.stdin("three\n");
    h.stdin_done();
    h.settle();
    assert!(!h.model.loading());
    assert_eq!(h.titles(), ["one", "two", "three"]);
}

#[test]
fn data_arriving_during_a_search_is_published_before_eof() {
    let mut h = piped(false, false);
    h.stdin("row-1\n");
    let running = h.run_search();
    h.stdin("row-final-9\n");
    assert_eq!(h.started.iter().filter(|t| t.kind() == TaskKind::Search).count(), 1, "data arrival does not restart a running search");
    h.send(running);
    assert_eq!(h.titles(), ["row-1"]);
    assert!(h.model.is_searching(), "the coalesced catch-up search starts");
    h.settle();
    assert_eq!(h.titles(), ["row-1", "row-final-9"]);
    assert!(h.model.loading(), "converged while the writer is still open");
}

#[test]
fn malformed_input_fails_and_dismissal_exits_two() {
    let mut h = piped(false, false);
    h.send(Event::StdinFailed("Record 1 is not valid JSON".into()));
    assert!(h.model.input_failed());
    assert!(!h.model.can_act());
    assert_eq!(h.model.message(), Some("Record 1 is not valid JSON"));
    h.nav(Nav::Back);
    assert_eq!(h.output(), (2, String::new()));
}

#[test]
fn accept_writes_exactly_the_chosen_values_in_source_order() {
    for (json, print0, expected) in [
        (false, false, Some("first value\nthird\n")),
        (false, true, Some("first value\0third\0")),
        (true, false, None),
    ] {
        let mut h = piped(json, print0);
        let items = [("a", "first value"), ("b", "second"), ("c", "third")]
            .map(|(id, value)| MenuItem::new(id, id.to_uppercase()).with_value(value));
        h.send(Event::StdinItems(items.to_vec()));
        h.stdin_done();
        h.settle();
        // Mark out of order; output follows source order.
        h.toggle(2);
        h.toggle(0);
        h.nav(Nav::Accept);
        if let Some(expected) = expected {
            assert_eq!(h.output(), (0, expected.into()), "json={json}, print0={print0}");
        } else {
            let context = h.context();
            assert_eq!(context.action, "accept", "json={json}, print0={print0}");
            assert_eq!(context.source, None, "json={json}, print0={print0}");
            assert_eq!(context.items.iter().map(|i| i.value()).collect::<Vec<_>>(), ["first value", "third"], "json={json}, print0={print0}");
        }
    }
}

#[test]
fn reload_of_piped_input_says_it_cannot_be_rerun() {
    let mut o = options(json!({"bindings": [{"key": "ctrl+r", "action": "reload"}]}));
    o.json = false;
    let mut h = Harness::new(o);
    h.stdin("one\n");
    h.settle();
    h.perform(0);
    assert_eq!(h.model.message(), Some("Piped input cannot be re-run"));
    assert_eq!(h.beeps, 1);
    h.nav(Nav::Down);
    assert_eq!(h.model.message(), Some("Piped input cannot be re-run"), "the refusal survives a move");
    assert!(h.model.message_is_error());
    assert!(!h.started.iter().any(|t| t.kind() == TaskKind::Source));
}

#[test]
fn plain_cancel_exits_130_with_no_output() {
    let mut h = piped(false, false);
    h.stdin("one\n");
    h.settle();
    h.nav(Nav::Cancel);
    assert_eq!(h.output(), (130, String::new()));
    h.nav(Nav::Accept);
    h.stdin_done();
    assert_eq!(h.output().0, 130, "nothing happens after finishing");
}

// KeyHandlingTests (model level: the host maps keys to Nav and chords to `target`)

fn keys(multi: bool) -> Harness {
    let mut o = options(json!({
        "sources": [src("a", json!({"key": "ctrl+1"})), src("b", json!({"key": "ctrl+2"}))],
        "bindings": [{"key": "ctrl+w", "id": "worktrees", "action": "accept"}, {"key": "alt+n", "id": "memos", "action": "accept"}],
    }));
    o.multi = multi;
    o.rows = 2;
    let mut h = Harness::new(o);
    h.load("one\ntwo\nthree\nfour\nfive\n");
    h
}

#[test]
fn navigation_keys_move_the_highlight() {
    let mut h = keys(true);
    assert_eq!(h.model.highlighted_row(), Some(0));
    h.nav(Nav::Down);
    h.nav(Nav::Down);
    assert_eq!(h.model.highlighted_row(), Some(2));
    h.nav(Nav::Up);
    h.nav(Nav::BackTab);
    h.nav(Nav::Up);
    assert_eq!(h.model.highlighted_row(), Some(0), "moving stops at the first row");
    h.nav(Nav::PageDown);
    assert_eq!(h.model.highlighted_row(), Some(2));
    h.nav(Nav::PageDown);
    h.nav(Nav::PageDown);
    assert_eq!(h.model.highlighted_row(), Some(4), "moving stops at the last row");
    h.nav(Nav::PageUp);
    assert_eq!(h.model.highlighted_row(), Some(2));
}

#[test]
fn tab_marks_and_advances_in_multi_mode() {
    let mut h = keys(true);
    h.nav(Nav::Tab);
    assert_eq!(h.marked(), HashSet::from([h.model.result(0).id.clone()]));
    assert_eq!(h.model.highlighted_row(), Some(1));
}

#[test]
fn tab_only_advances_in_single_mode() {
    let mut h = keys(false);
    h.nav(Nav::Tab);
    h.toggle(3);
    assert!(h.marked().is_empty());
    assert_eq!(h.model.highlighted_row(), Some(1));
}

#[test]
fn source_binding_switches_to_its_parsed_target() {
    let mut h = start(json!({"sources": [src("a", json!({})), src("b", json!({}))],
        "bindings": [{"key": "ctrl+w", "action": "source", "source": "b"}]}));
    h.load("alpha\n");
    h.perform(0);
    assert_eq!(h.model.source_id().map(SourceId::as_str), Some("b"));
    h.load("beta\n");
    assert_eq!(h.titles(), ["beta"]);
}

#[test]
fn source_key_switches_source() {
    let mut h = keys(true);
    let Some(Target::SwitchSource(id)) = h.model.target(&chord("ctrl+2")) else { panic!("ctrl+2 is a source key") };
    h.send(Event::SwitchSource(id));
    assert_eq!(h.model.source_id().map(SourceId::as_str), Some("b"));
}

#[test]
fn control_and_option_bindings_report_their_id() {
    for (key, id) in [("ctrl+w", "worktrees"), ("alt+n", "memos")] {
        let mut h = keys(true);
        let Some(Target::Perform(index)) = h.model.target(&chord(key)) else { panic!("{key} is bound") };
        h.perform(index);
        assert_eq!(h.context().action, id, "key={key}, id={id}");
    }
}

#[test]
fn extra_modifier_or_no_modifier_does_not_match_a_binding() {
    let h = keys(true);
    assert_eq!(h.model.target(&chord("ctrl+shift+w")), None);
    assert_eq!(h.model.target(&chord("ctrl+alt+1")), None);
    let plain = KeyChord { key: "w".into(), command: false, control: false, option: false, shift: false };
    assert_eq!(h.model.target(&plain), None);
}

#[test]
fn escape_and_control_c_cancel() {
    for nav in [Nav::Back, Nav::Cancel] {
        let mut h = keys(true);
        h.nav(nav);
        assert_eq!(h.output(), (130, String::new()), "nav={nav:?}");
    }
}

#[test]
fn return_accepts() {
    let mut h = keys(true);
    h.nav(Nav::Accept);
    let context = h.context();
    assert_eq!(context.action, "accept");
    assert_eq!(context.items.iter().map(|i| i.value()).collect::<Vec<_>>(), ["one"]);
}

#[test]
fn the_highlight_stays_within_the_listed_rows() {
    let mut o = options(json!({}));
    o.list_cap = 3;
    let mut h = Harness::new(o);
    h.stdin("a\nb\nc\nd\ne\n");
    h.stdin_done();
    h.settle();
    for _ in 0..10 { h.nav(Nav::Down); }
    assert_eq!(h.model.highlighted_row(), Some(2));
    h.nav(Nav::PageDown);
    assert_eq!(h.model.highlighted_row(), Some(2));
    assert_eq!(h.model.result_count(), 5, "every match is still counted");
}

// Review fixes.

#[test]
fn a_query_with_no_matches_clears_the_preview() {
    let mut h = start(json!({"sources": [src("a", json!({}))], "preview": ["show"]}));
    h.load("alpha\nbeta\n");
    let (done, _) = h.preview_event("PREVIEW OF alpha");
    h.send(done);
    h.query("zzzz");
    h.settle();
    assert_eq!(h.model.result_count(), 0);
    assert_eq!(h.model.preview(), Some(""), "no row, so no preview");
}

#[test]
fn switching_source_after_typing_drops_the_old_sources_preview() {
    let mut h = start(json!({"sources": [src("a", json!({})), src("b", json!({}))], "preview": ["show"]}));
    h.load("alpha\nbeta\n");
    let (done, _) = h.preview_event("PREVIEW OF alpha");
    h.send(done);
    h.query("al");
    assert_eq!(h.model.preview(), Some("PREVIEW OF alpha"), "kept while the next search runs");
    h.switch("b");
    assert_eq!(h.model.preview(), Some(""));
    h.load("");
    assert_eq!(h.model.preview(), Some(""));
}

#[test]
fn going_back_drops_the_childs_preview() {
    let mut h = start(json!({
        "sources": [repos(json!({})), hidden("child", json!({}))],
        "bindings": [push_binding()], "preview": ["show"]}));
    h.load(r#"{"id":"a","title":"alpha"}"#);
    let (done, _) = h.preview_event("PARENT");
    h.send(done);
    h.perform(0);
    h.load(r#"{"id":"x","title":"xray"}"#);
    let (done, _) = h.preview_event("CHILD xray");
    h.send(done);
    h.nav(Nav::Back);
    assert_eq!(h.model.preview(), Some(""), "the parent's own preview comes, not the child's");
}

#[test]
fn an_operation_failure_survives_typing_and_moving() {
    let mut h = operations(json!([{"key": "ctrl+d", "action": "run", "command": ["fail"]}]));
    h.load("one\ntwo\n");
    h.perform(0);
    h.query("o");
    h.finish_operation(3, "permission denied\n");
    h.settle();
    h.query("on");
    h.nav(Nav::Down);
    assert_eq!(h.model.message(), Some("Command exited 3: permission denied"));
    h.settle();
    h.perform(0);
    assert!(h.model.message().unwrap().starts_with("Running"), "the next action replaces it");
    h.finish_operation(0, "");
    h.query("o");
    assert_eq!(h.model.message(), None, "a later success is an acknowledgement again");
}

#[test]
fn acknowledgements_retire_on_the_next_key() {
    let mut h = operations(json!([{"key": "ctrl+d", "action": "run", "command": ["ok"]}]));
    h.load("one\ntwo\n");
    h.perform(0);
    h.finish_operation(0, "");
    assert!(h.model.message().unwrap().ends_with("completed"));
    h.nav(Nav::Down);
    assert_eq!(h.model.message(), None);
}

#[test]
fn going_back_does_not_show_a_finished_operations_running_line() {
    let mut h = start(json!({
        "sources": [repos(json!({})), hidden("child", json!({}))],
        "bindings": [{"key": "ctrl+d", "id": "star", "action": "run", "command": ["star"]}, push_binding()]}));
    h.load(r#"{"id":"a","title":"alpha"}"#);
    h.perform(0);
    assert!(h.model.message().unwrap().starts_with("Running"));
    h.perform(1);
    h.load(r#"{"id":"x","title":"xray"}"#);
    h.finish_operation(0, "");
    h.nav(Nav::Back);
    assert!(!h.model.message().is_some_and(|m| m.starts_with("Running")), "{:?}", h.model.message());
}

#[test]
fn a_stdin_failure_is_reported_from_any_level() {
    let mut h = start(json!({"sources": [hidden("child", json!({}))], "bindings": [push_binding()]}));
    h.stdin("alpha\n");
    h.settle();
    h.perform(0);
    h.send(Event::StdinFailed("Record 2 contains invalid UTF-8.".into()));
    h.nav(Nav::Cancel);
    assert_eq!(h.output().0, 2);
    assert_eq!(h.model.stdin_error(), Some("Record 2 contains invalid UTF-8."));
}

#[test]
fn an_acknowledgement_after_a_failure_still_retires() {
    let mut h = start(json!({"sources": [src("s", json!({}))], "bindings": [
        {"key": "ctrl+d", "action": "run", "command": ["fail"]}, {"key": "ctrl+y", "action": "copy"}]}));
    h.load("one\ntwo\n");
    h.perform(0);
    h.finish_operation(3, "nope\n");
    assert!(h.model.message_is_error());
    h.perform(1);
    assert_eq!(h.model.message(), Some("Copied 1"));
    h.nav(Nav::Down);
    assert_eq!(h.model.message(), None);
}

#[test]
fn a_highlight_past_the_listed_rows_moves_to_the_first() {
    let mut o = options(json!({"sources": [src("s", json!({}))]}));
    o.list_cap = 2;
    o.sorted = true;
    let mut h = Harness::new(o);
    h.load("ab\nzzab\nb\n");
    h.nav(Nav::Down);
    assert_eq!(h.model.highlighted_row(), Some(1));
    h.query("ab");
    h.settle();
    h.query("b");
    h.settle();
    let row = h.model.highlighted_row().unwrap();
    assert!(row < 2, "highlight {row} is outside the 2 listed rows");
}

#[test]
fn a_click_past_the_listed_rows_marks_nothing() {
    let mut o = options(json!({"sources": [src("s", json!({}))]}));
    o.list_cap = 1;
    let mut h = Harness::new(o);
    h.load("a\nb\nc\n");
    h.toggle(2);
    assert!(h.marked().is_empty(), "row 2 is not listed, yet {:?} is marked", h.marked());
}

#[test]
fn a_refused_run_beeps_and_starts_nothing() {
    let mut h = operations(json!([{"key": "ctrl+d", "action": "run", "command": ["wait"]}]));
    h.load("a\nb\n");
    h.perform(0);
    h.perform(0);
    assert_eq!(h.beeps, 1, "a second run while one is running");
    h.finish_operation(0, "");
    h.query("zzz");
    h.settle();
    h.perform(0);
    assert_eq!(h.beeps, 2, "a run with nothing chosen");
    assert_eq!(h.started.iter().filter(|t| t.kind() == TaskKind::Operation).count(), 1);
}

#[test]
fn every_action_with_nothing_chosen_beeps() {
    let mut h = start(json!({"sources": [repos(json!({})), hidden("child", json!({}))],
        "bindings": [push_binding(), {"key": "ctrl+y", "action": "copy"}]}));
    h.load(&jsonl(&REPOS));
    h.query("zzz");
    h.settle();
    h.nav(Nav::Accept);
    h.perform(0);
    h.perform(1);
    assert_eq!(h.beeps, 3);
    assert!(h.is_open());
    assert_eq!(h.model.depth(), 0);
    assert!(h.copied.is_empty());
}
