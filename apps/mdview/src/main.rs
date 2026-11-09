//! A Markdown file viewed in a terminal pane, GitHub style, in the terminal's colours, read with
//! Vim's keys: headings, tables, task lists you can tick, code with syntax colours and a copy
//! button, diff and Mermaid fences, quotes and links.
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p mdview -- README.md
//! ```
//!
//! With no file it shows a sample. Move with `h j k l w b e 0 $ gg G { } f t %`, select with `v`
//! and `V`, yank with `y` (`yy`, `yiw`, `ya"`, `yip`, …), or with `gy` to copy the Markdown
//! source instead of the text (`gyy`, `gyip`, `V…gy`), search with `/` and `?`, `n` and `N`,
//! `*` and `#`; Escape hides the matches. Ctrl-D and Ctrl-U page. Space on a task's line ticks
//! it, and with a file open writes it back. Hover a code block for its copy button.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use pane_ui::{Badge, Button, Theme, icons, kbd};
use rune::{AppSpec, Cx, El, KeyEvent, ScrollSpec, TextStyle, Tx, col, el, row, text};
use rune_kit::markdown::{Highlight, RenderedText};
use rune_kit::{CodeFrame, Markdown, MarkdownStyle};
use vimkeys::{Keymap, Operator, Stroke, Vim, VimEffect, VimMode};

const SAMPLE: &str = include_str!("sample.md");
const COPIED_FOR: Duration = Duration::from_millis(1500);

/// What the viewer adds to Vim's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// Space: tick or clear the task on the cursor's line.
    Tick,
}

#[derive(Clone)]
struct Doc {
    path: Option<String>,
    source: String,
    vim: Vim<Act>,
    /// The code block whose copy button shows a check, and since when.
    copied: Option<(usize, Instant)>,
    /// A one-off note for the status line.
    note: Option<String>,
}

/// The document `main` opened before the app starts, so a file it can't read is an error in
/// the shell rather than the sample shown under its name.
static OPENED: OnceLock<Doc> = OnceLock::new();

impl Default for Doc {
    fn default() -> Self {
        OPENED
            .get()
            .cloned()
            .unwrap_or_else(|| Doc::with_source(None, SAMPLE.to_owned()))
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
            path,
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
    fn task_under_cursor(&self, page: &RenderedText) -> Option<(usize, bool)> {
        let at = page
            .source_ranges(self.vim.cursor(page.text()))
            .first()?
            .start;
        let line_start = self.source[..at].rfind('\n').map_or(0, |i| i + 1);
        let line = &self.source[line_start..];
        let rest = line.trim_start_matches([' ', '\t', '>']);
        let rest = rest.strip_prefix(['-', '*', '+']).or_else(|| {
            let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            (digits.len() < rest.len())
                .then(|| digits.strip_prefix(['.', ')']))
                .flatten()
        })?;
        let box_at = rest.len() - rest.trim_start().len();
        let mark = rest.trim_start().get(..3)?;
        let checked = match mark {
            "[ ]" => false,
            "[x]" | "[X]" => true,
            _ => return None,
        };
        Some((line_start + (line.len() - rest.len()) + box_at, checked))
    }

    /// Ticks or clears the task whose `[` is at `offset`, and writes the file back.
    fn set_task(&mut self, offset: usize, checked: bool) {
        let mark = if checked { "x" } else { " " };
        if self.source.get(offset + 1..offset + 2).is_none() {
            return;
        }
        self.source.replace_range(offset + 1..offset + 2, mark);
        if let Some(path) = &self.path
            && let Err(error) = std::fs::write(path, &self.source)
        {
            // stderr is the terminal the page is drawn on, so the note goes in the status line.
            self.note = Some(format!("could not save {path}: {error}"));
        }
    }
}

/// GitHub's reading width and type, in the theme's colours.
fn style(theme: &Theme) -> MarkdownStyle {
    MarkdownStyle::from_kit(&theme.kit)
        .text(theme.text)
        .muted(theme.text_secondary)
        .link(theme.accent_text)
        .link_underline(theme.accent_text.alpha(0.4))
        .quote_rule(theme.separator)
        .rule(theme.separator)
        .code_block(theme.group, theme.separator)
        .inline_code_bg(theme.field)
        .selection(theme.accent_soft)
        // GitHub's heading scale: 2, 1.5 and 1.25 times the body.
        .heading_font(
            1,
            TextStyle {
                size: 26.0,
                weight: 600.0,
                line_height: 34.0,
                ..theme.ty.body
            },
        )
        .heading_font(
            2,
            TextStyle {
                size: 20.0,
                weight: 600.0,
                line_height: 28.0,
                ..theme.ty.body
            },
        )
        .heading_font(
            3,
            TextStyle {
                size: 16.0,
                weight: 600.0,
                line_height: 24.0,
                ..theme.ty.body
            },
        )
}

/// The Vim cursor, selection and search matches as highlights on the page.
fn vim_highlights(
    vim: &Vim<Act>,
    page: &RenderedText,
    theme: &Theme,
) -> Vec<(Vec<std::ops::Range<usize>>, Highlight)> {
    let text = page.text();
    let mut out = Vec::new();
    for range in vim.matches(text) {
        out.push((page.source_ranges(range), Highlight::new()));
    }
    if let Some(range) = vim.selection(text) {
        out.push((
            page.source_ranges(range),
            Highlight::new().bg(theme.accent_soft).fg(theme.text),
        ));
    }
    let cursor = vim.cursor(text);
    out.push((
        page.source_ranges(cursor),
        Highlight::new()
            .bg(theme.accent)
            .fg(theme.on_accent)
            .key("vim-cursor"),
    ));
    out.retain(|(ranges, _)| !ranges.is_empty());
    out
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
            s.copied = Some((index, Instant::now()));
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
    let page = RenderedText::new(&s.source);
    let title = s.path.as_deref().unwrap_or("sample.md");
    let bar = row()
        .key("bar")
        .w_full()
        .min_h(40.0)
        .px(20.0)
        .py(6.0)
        .gap(10.0)
        .center_y()
        .child(rune::icon(&icons::FILE_TEXT, 16.0).color(theme.text_secondary))
        .child(
            text(title)
                .font(theme.ty.strong)
                .color(theme.text)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(kbd("/", &theme))
        .child(
            text("search")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
        .child(kbd("v", &theme))
        .child(
            text("select")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
        .child(kbd("y", &theme))
        .child(
            text("yank")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
        .child(kbd("gy", &theme))
        .child(
            text("as md")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        );
    let doc = Markdown::new(s.source.as_str())
        .style(style(&theme))
        .selectable(true)
        .highlights(vim_highlights(&s.vim, &page, &theme))
        .code_frame(code_frame(&theme, s.copied.map(|(i, _)| i)))
        // A clicked box would keep focus, and Space would go on ticking it rather than the task
        // under the cursor; drop focus so keys come back to Vim.
        .on_task_tx(|s: &mut Doc, _, offset, checked, tx| {
            s.set_task(offset, checked);
            tx.blur();
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
        .child(status_line(&s.vim, s.note.as_deref(), &page, &theme))
}

/// Vim's status line: the mode (or the search being typed), a note, half-typed keys and the
/// cursor's line.
fn status_line(vim: &Vim<Act>, note: Option<&str>, page: &RenderedText, theme: &Theme) -> El {
    let text_of = page.text();
    let line = text_of[..vim.cursor(text_of).start].matches('\n').count() + 1;
    let lines = text_of.matches('\n').count() + 1;
    let searching = vim.mode() == VimMode::Search;
    let mode = if searching {
        text(vim.status()).font(theme.ty.mono).color(theme.text)
    } else {
        el().child(
            Badge::new(vim.status())
                .tone(match vim.mode() {
                    VimMode::Normal => pane_ui::Tone::Neutral,
                    _ => pane_ui::Tone::Accent,
                })
                .build(theme),
        )
    };
    row()
        .key("status")
        .w_full()
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .fill(theme.group)
        .child(mode)
        .child(
            text(note.or(vim.message()).unwrap_or_default().to_owned())
                .font(theme.ty.caption)
                .color(theme.text_secondary)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(
            text(vim.pending())
                .font(theme.ty.mono)
                .color(theme.text_secondary),
        )
        .child(
            text(format!("{line}:{lines}"))
                .font(theme.ty.mono)
                .color(theme.text_tertiary),
        )
}

fn on_key(s: &mut Doc, k: &KeyEvent, tx: &mut Tx) -> bool {
    s.note = None;
    let page = RenderedText::new(&s.source);
    match s.vim.key(k, page.text()) {
        Some(VimEffect::Moved) => {
            tx.scroll_into_view("vim-cursor");
            true
        }
        Some(VimEffect::Copy(yanked)) => {
            tx.copy(yanked);
            tx.scroll_into_view("vim-cursor");
            true
        }
        Some(VimEffect::CopySource { range, linewise }) => {
            tx.copy(source_of(&s.source, &page, range, linewise));
            tx.scroll_into_view("vim-cursor");
            true
        }
        // Space on a task's line ticks it, as a click on its box does.
        Some(VimEffect::Action {
            action: Act::Tick, ..
        }) => {
            if let Some((offset, checked)) = s.task_under_cursor(&page) {
                s.set_task(offset, !checked);
            }
            true
        }
        Some(VimEffect::Operator { .. }) => true,
        None => false,
    }
}

/// The Markdown behind `range` of the page's text: from the first source byte it was drawn from
/// to the last, as written, so a range starting inside bold text keeps its closing `**` and
/// drops the opening one. A linewise yank takes the whole source lines, with list markers,
/// heading hashes and code fences that fall on them.
fn source_of(
    source: &str,
    page: &RenderedText,
    range: std::ops::Range<usize>,
    linewise: bool,
) -> String {
    let ranges = page.source_ranges(range);
    let (Some(first), Some(last)) = (ranges.first(), ranges.last()) else {
        return String::new();
    };
    let (mut start, mut end) = (first.start, last.end);
    if linewise {
        start = source[..start].rfind('\n').map_or(0, |i| i + 1);
        end = source[end..].find('\n').map_or(source.len(), |i| end + i);
        return format!("{}\n", &source[start..end]);
    }
    source[start..end].to_owned()
}

/// Puts a copy button back to its icon once the check has shown long enough.
fn poll(s: &mut Doc, _: f64, tx: &mut Tx) -> bool {
    let Some((_, at)) = s.copied else {
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
    match Doc::open(rune::extra_args().first().cloned()) {
        Ok(doc) => {
            if OPENED.set(doc).is_err() {
                unreachable!("main opens the document once");
            }
        }
        Err(e) => {
            eprintln!("mdview: {e}");
            std::process::exit(1);
        }
    }
    rune::run(AppSpec::<Doc> {
        scenes: &[
            ("visual", |s| {
                let page = RenderedText::new(&s.source);
                for c in "jjwwvee".chars() {
                    s.vim.key(
                        &KeyEvent {
                            key: rune::Key::Text(c.to_string()),
                            mods: rune::Mods::default(),
                        },
                        page.text(),
                    );
                }
            }),
            ("search", |s| {
                let page = RenderedText::new(&s.source);
                for c in "/rune".chars() {
                    s.vim.key(
                        &KeyEvent {
                            key: rune::Key::Text(c.to_string()),
                            mods: rune::Mods::default(),
                        },
                        page.text(),
                    );
                }
            }),
            ("copied", |s| s.copied = Some((0, Instant::now()))),
        ],
        on_key: Some(on_key),
        poll: Some(poll),
        // Code blocks need a monospaced face; the default family pair is Inter for both.
        families: ("Inter", "ui-monospace, Menlo, Inter"),
        ..AppSpec::new("Markdown", (900.0, 900.0), Doc::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "## Tasks\n\nSome **bold phrase** and more.\n\n- [x] Port the shell\n- [ ] Copy\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    fn yank(text: &str, linewise: bool) -> String {
        let page = RenderedText::new(SRC);
        let at = page.text().find(text).expect("drawn");
        source_of(SRC, &page, at..at + text.len(), linewise)
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
        let page = RenderedText::new(&doc.source);
        let (offset, checked) = doc.task_under_cursor(&page).unwrap();
        doc.set_task(offset, !checked);
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
