//! A Markdown reader with Vim navigation and the terminal's colours.
//! It renders headings, tables, task lists, syntax-highlighted code, diff and Mermaid fences,
//! quotes, and links.
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p mdview -- README.md
//! ```
//!
//! With no file, mdview shows a sample. Move with `h j k l w b e 0 $ gg G { } f t %`.
//! Select characters with `v` or lines with `V`. Yank rendered text with `y`, as in `yy` or `yiw`.
//! Use `gy` to copy source Markdown, as in `gyy`, `gyip`, or `Vgy`.
//! Search with `/` and `?`. Repeat with `n` and `N`, or search the current word with `*` and `#`.
//! Escape hides the matches. Ctrl-D and Ctrl-U move 15 lines with the default page setting.
//! Space toggles the task on the cursor's line and writes the change to the open file.
//! Hover a code block for its copy button.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Options, Parser};

use pane_ui::markdown::{self, VIM_CURSOR, style, type_keys};
use pane_ui::{Button, Theme, icons};
use rune::{App, AppSpec, Cx, El, KeyEvent, ScrollSpec, Tx, col, el, row};
use rune_kit::markdown::RenderedText;
use rune_kit::{CodeFrame, Markdown};
use vimkeys::{Keymap, Operator, Stroke, Vim, VimEffect};

const SAMPLE: &str = include_str!("sample.md");
const COPIED_FOR: Duration = Duration::from_millis(1500);

/// What the viewer adds to Vim's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// Space: tick or clear the task on the cursor's line.
    Tick,
}

enum Origin {
    Sample,
    File(PathBuf),
}

impl Origin {
    fn title(&self) -> String {
        match self {
            Self::Sample => "sample.md".into(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

#[derive(Clone, Copy)]
struct Copied {
    block: usize,
    at: Instant,
}

struct Doc {
    origin: Origin,
    source: String,
    page: RenderedText,
    tasks: Vec<(usize, bool)>,
    vim: Vim<Act>,
    copied: Option<Copied>,
    /// A one-off note for the status line.
    note: Option<String>,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::with_source(None, SAMPLE.to_owned())
    }
}

impl Doc {
    /// The file at `path`, or the sample when there is none. A file that can't be read is an
    /// error.
    fn open(path: Option<String>) -> Result<Doc, String> {
        let source = match &path {
            Some(p) => {
                std::fs::read_to_string(p).map_err(|e| format!("could not read {p}: {e}"))?
            }
            None => SAMPLE.to_owned(),
        };
        Ok(Doc::with_source(path, source))
    }

    fn with_source(path: Option<String>, source: String) -> Doc {
        Doc {
            origin: path.map_or(Origin::Sample, |path| Origin::File(path.into())),
            page: RenderedText::new(&source),
            tasks: task_markers(&source),
            source,
            vim: Vim::new(
                Keymap::vim()
                    .operator(
                        &[Stroke::Char('g'), Stroke::Char('y')],
                        Operator::YankSource,
                        "Yank as Markdown",
                    )
                    .action(&[Stroke::Char(' ')], Act::Tick, "Tick the task"),
            )
            .skip_blank_lines(true),
            copied: None,
            note: None,
        }
    }
}

impl Doc {
    /// The `[` of the task list item the Vim cursor is on, and whether it is ticked.
    fn task_under_cursor(&self) -> Option<(usize, bool)> {
        let at = self
            .page
            .source_ranges(self.vim.cursor(self.page.text()))
            .first()?
            .start;
        let start = pane_ui::markdown::line_start(&self.source, at);
        self.tasks
            .iter()
            .copied()
            .find(|(offset, _)| pane_ui::markdown::line_start(&self.source, *offset) == start)
    }

    fn tick_and_return_keys_to_vim(&mut self, offset: usize, checked: bool, tx: &mut Tx) {
        self.tick_and_save(offset, checked);
        tx.blur();
    }

    fn tick_and_save(&mut self, offset: usize, checked: bool) {
        if !self.tasks.iter().any(|(at, _)| *at == offset) {
            return;
        }
        self.source
            .replace_range(offset + 1..offset + 2, if checked { "x" } else { " " });
        self.page = RenderedText::new(&self.source);
        self.tasks = task_markers(&self.source);
        if let Origin::File(path) = &self.origin
            && let Err(error) = std::fs::write(path, &self.source)
        {
            self.note = Some(format!("could not save {}: {error}", path.display()));
        }
    }
}

fn task_markers(source: &str) -> Vec<(usize, bool)> {
    Parser::new_ext(
        source,
        Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .into_offset_iter()
    .filter_map(|(event, range)| match event {
        Event::TaskListMarker(checked) => Some((range.start, checked)),
        _ => None,
    })
    .collect()
}

/// A code block with a copy button over its top right corner while the pointer is on it.
fn code_frame(theme: &Theme, copied: Option<usize>) -> impl Fn(El, &CodeFrame<'_>) -> El + 'static {
    let theme = theme.clone();
    move |block, frame| {
        let code = frame.source.to_owned();
        let index = frame.index;
        let done = copied == Some(index);
        let button = Button::icon_only(
            if done { &icons::CHECK } else { &icons::COPY },
            if done { "Copied" } else { "Copy" },
        )
        .gray()
        .small()
        .on_press_tx(move |s: &mut Doc, tx: &mut Tx| {
            tx.copy(code.clone());
            s.copied = Some(Copied {
                block: index,
                at: Instant::now(),
            });
            tx.wake_after(COPIED_FOR);
        })
        .build(&format!("copy-{index}"), &theme);
        el().key(format!("code-{index}"))
            .w_full()
            .hover_group()
            .child(block)
            .child(
                row()
                    .pin(None, Some(6.0), Some(6.0), None)
                    .group_opacity(if done { 1.0 } else { 0.0 }, 1.0)
                    .child(button),
            )
    }
}

fn view(s: &Doc, cx: &mut Cx<Doc>) -> El {
    let theme = Theme::current(cx.system_dark());
    let page = &s.page;
    let title = s.origin.title();
    let bar = markdown::title_bar(
        &title,
        &[
            ("/", "search"),
            ("v", "select"),
            ("y", "yank"),
            ("gy", "as md"),
        ],
        &theme,
    );
    let doc = Markdown::new(s.source.as_str())
        .style(style(&theme).selection(theme.accent_soft))
        .selectable(true)
        .highlights(markdown::vim_highlights(
            &s.vim,
            page.text(),
            |range| page.source_ranges(range),
            true,
            &theme,
        ))
        .code_frame(code_frame(&theme, s.copied.map(|copied| copied.block)))
        .on_task_tx(|s: &mut Doc, _, offset, checked, tx| {
            s.tick_and_return_keys_to_vim(offset, checked, tx);
        })
        .build(&theme.kit);
    let content = col()
        .key("page")
        .w_full()
        .h_full()
        .padding(24.0)
        .child(col().w_full().max_w(880.0).self_center().child(doc))
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    col()
        .w_full()
        .h_full()
        .fill(theme.background)
        .child(bar)
        .child(el().h(1.0).w_full().fill(theme.separator))
        .child(content.grow(1.0).min_h(0.0))
        .child(markdown::status_line(
            &s.vim,
            page.text(),
            s.note.as_deref(),
            None,
            None,
            &theme,
        ))
}

fn on_key(s: &mut Doc, k: &KeyEvent, tx: &mut Tx) -> bool {
    s.note = None;
    let page = &s.page;
    match s.vim.key(k, page.text()) {
        Some(VimEffect::Consumed) => {
            tx.scroll_into_view(VIM_CURSOR);
            true
        }
        Some(VimEffect::Copy(yanked)) => {
            tx.copy(yanked);
            tx.scroll_into_view(VIM_CURSOR);
            true
        }
        Some(VimEffect::CopySource { range, linewise }) => {
            tx.copy(markdown::source_of(
                &s.source,
                &page.source_ranges(range),
                linewise,
            ));
            tx.scroll_into_view(VIM_CURSOR);
            true
        }
        Some(VimEffect::Action {
            action: Act::Tick, ..
        }) => {
            if let Some((offset, checked)) = s.task_under_cursor() {
                s.tick_and_save(offset, !checked);
            }
            true
        }
        Some(VimEffect::Operator { .. }) => true,
        None => false,
    }
}

/// Puts a copy button back to its icon once the check has shown long enough.
fn poll(s: &mut Doc, _: f64, tx: &mut Tx) -> bool {
    let Some(Copied { at, .. }) = s.copied else {
        return false;
    };
    let left = COPIED_FOR.saturating_sub(at.elapsed());
    if !left.is_zero() {
        tx.wake_after(left);
        return false;
    }
    s.copied = None;
    true
}

fn main() {
    let doc = match Doc::open(rune::extra_args().first().cloned()) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("mdview: {e}");
            std::process::exit(1);
        }
    };
    App::new(AppSpec::<Doc> {
        scenes: &[
            ("visual", |s| {
                type_keys(&mut s.vim, "jjwwvee", s.page.text())
            }),
            ("search", |s| type_keys(&mut s.vim, "/rune", s.page.text())),
            ("copied", |s| {
                s.copied = Some(Copied {
                    block: 0,
                    at: Instant::now(),
                })
            }),
        ],
        on_key: Some(on_key),
        poll: Some(poll),
        families: markdown::FONT_FAMILIES,
        ..AppSpec::new("Markdown", (900.0, 900.0), Doc::default, view)
    })
    .init(move || doc)
    .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "## Tasks\n\nSome **bold phrase** and more.\n\n- [x] Port the shell\n- [ ] Copy\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    fn runtime(doc: Doc) -> rune::Runtime<Doc> {
        let mut rt = rune::Runtime::new(doc, view, (600.0, 400.0), rune::Env::default());
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt
    }

    fn press(rt: &mut rune::Runtime<Doc>, keys: &str) {
        for c in keys.chars() {
            rt.key(KeyEvent {
                key: rune::Key::Text(c.to_string()),
                mods: rune::Mods::default(),
            });
            rt.frame(1.0 / 60.0);
        }
    }

    fn yank(text: &str, linewise: bool) -> String {
        let mut rt = runtime(Doc::with_source(None, SRC.into()));
        let at = rt.state.page.text().find(text).expect("drawn");
        rt.state.vim.set_cursor(at, rt.state.page.text());
        if linewise {
            press(&mut rt, "V");
            for _ in 1..text.lines().count() {
                press(&mut rt, "j");
            }
        } else {
            press(&mut rt, "v");
            for _ in 1..text.chars().count() {
                press(&mut rt, "l");
            }
        }
        press(&mut rt, "gy");
        rt.copy_request
            .expect("gy copies Markdown")
            .content()
            .text()
            .to_owned()
    }

    #[test]
    fn space_toggles_only_parser_tasks_and_writes_the_file() {
        let path = std::env::temp_dir().join(format!("mdview-space-{}.md", std::process::id()));
        for (source, cursor, expected) in [
            ("- [ ] plain\n", "plain", "- [x] plain\n"),
            ("1. [x] ordered\n", "ordered", "1. [ ] ordered\n"),
            ("> - [ ] quoted\n", "quoted", "> - [x] quoted\n"),
            (
                "- outer\n  - [ ] nested\n",
                "nested",
                "- outer\n  - [x] nested\n",
            ),
            ("ordinary [ ] text\n", "ordinary", "ordinary [ ] text\n"),
            (
                "```text\n- [ ] code\n```\n",
                "code",
                "```text\n- [ ] code\n```\n",
            ),
        ] {
            std::fs::write(&path, source).unwrap();
            let mut rt = runtime(Doc::open(Some(path.to_str().unwrap().into())).unwrap());
            let at = rt.state.page.text().find(cursor).unwrap();
            rt.state.vim.set_cursor(at, rt.state.page.text());
            press(&mut rt, " ");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                expected,
                "Space on {cursor}"
            );
            assert_eq!(rt.state.source, expected, "visible source stays in sync");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn clicking_a_task_returns_space_to_the_vim_cursor() {
        let mut rt = runtime(Doc::with_source(None, "- [ ] one\n- [ ] two\n".into()));
        let (x, y) = rt.find("i1").expect("second task");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.1);
        assert_eq!(rt.state.source, "- [ ] one\n- [x] two\n");
        press(&mut rt, " ");
        assert_eq!(rt.state.source, "- [x] one\n- [x] two\n");
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_the_sample() {
        let dir = std::env::temp_dir().join(format!("mdview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.md");
        let missing_path = missing.to_str().unwrap().to_owned();
        assert!(
            Doc::open(Some(missing_path.clone())).is_err(),
            "a missing file is an error"
        );
        assert!(!missing.exists(), "and nothing is written to its path");

        let real = dir.join("plan.md");
        std::fs::write(&real, "- [ ] one\n").unwrap();
        let doc = Doc::open(Some(real.to_str().unwrap().to_owned())).unwrap();
        assert_eq!(doc.source, "- [ ] one\n");
        assert_eq!(
            Doc::open(None).unwrap().source,
            SAMPLE,
            "no file shows the sample"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_shows_in_the_status_line() {
        let dir = std::env::temp_dir().join(format!("mdview-save-{}", std::process::id()));
        let missing = dir.join("gone").join("plan.md");
        let mut doc = Doc::with_source(
            Some(missing.to_str().unwrap().to_owned()),
            "- [ ] one\n".to_owned(),
        );
        let (offset, checked) = doc.task_under_cursor().unwrap();
        doc.tick_and_save(offset, !checked);
        assert_eq!(doc.source, "- [x] one\n");
        let note = doc.note.clone().unwrap_or_default();
        assert!(
            note.starts_with("could not save"),
            "the user sees the failure: {note:?}"
        );

        let mut rt = rune::Runtime::new(doc, view, (600.0, 400.0), rune::Env::default());
        rt.frame(0.0);
        let shown: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(
            shown.iter().any(|t| t.starts_with("could not save")),
            "in the status line: {shown:?}"
        );
    }

    #[test]
    fn tasks_follow_the_parser_and_refresh_after_toggling() {
        let source = "> - [x] quoted\n\n1. [ ] ordered\n\n- [ ] outer\n  - [X] inner\n";
        let mut doc = Doc::with_source(None, source.into());
        assert_eq!(doc.tasks.len(), 4);
        for needle in ["quoted", "ordered", "outer", "inner"] {
            let at = doc.page.text().find(needle).unwrap();
            doc.vim.set_cursor(at, doc.page.text());
            let (offset, checked) = doc.task_under_cursor().unwrap();
            doc.tick_and_save(offset, !checked);
            assert_eq!(doc.task_under_cursor(), Some((offset, !checked)));
            assert_eq!(doc.page.text(), RenderedText::new(&doc.source).text());
        }
        let before = doc.source.clone();
        doc.tick_and_save(0, true);
        assert_eq!(doc.source, before);
    }

    #[test]
    fn task_markers_in_code_are_not_interactive() {
        let doc = Doc::with_source(None, "```text\n- [ ] not a task\n```\n".into());
        assert_eq!(doc.task_under_cursor(), None);
    }

    #[test]
    fn linewise_takes_whole_source_lines() {
        assert_eq!(yank("Port the shell", true), "- [x] Port the shell\n");
        assert_eq!(
            yank("Port the shell\nCopy", true),
            "- [x] Port the shell\n- [ ] Copy\n"
        );
        assert_eq!(yank("Tasks", true), "## Tasks\n");
        assert_eq!(yank("let x = 1;", true), "let x = 1;\n");
        assert_eq!(yank("1\t2", true), "| 1 | 2 |\n");
    }

    #[test]
    fn charwise_is_the_raw_source_slice() {
        assert_eq!(yank("bold phrase", false), "bold phrase");
        assert_eq!(yank("phrase and", false), "phrase** and");
        assert_eq!(yank("Some bold", false), "Some **bold");
    }
}
