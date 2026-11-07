//! A Markdown file viewed in a terminal pane, GitHub style, in the terminal's colours, read with
//! Vim's keys: headings, tables, task lists you can tick, code with syntax colours and a copy
//! button, diff and Mermaid fences, quotes and links.
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p rex-md -- README.md
//! ```
//!
//! With no file it shows a sample. Move with `h j k l w b e 0 $ gg G { } f t %`, select with `v`
//! and `V`, yank with `y` (`yy`, `yiw`, `ya"`, `yip`, …), or with `gy` to copy the Markdown
//! source instead of the text (`gyy`, `gyip`, `V…gy`), search with `/` and `?`, `n` and `N`,
//! `*` and `#`; Escape hides the matches. Ctrl-D and Ctrl-U page. Space on a task's line ticks
//! it, and with a file open writes it back. Hover a code block for its copy button.

use std::time::{Duration, Instant};

use rex_ui::{Badge, Button, Rex, icons, kbd};
use rex_vim::{Vim, VimEffect, VimMode};
use rune::{AppSpec, Cx, El, KeyEvent, ScrollSpec, TextStyle, Tx, col, el, row, text};
use rune_kit::markdown::{Highlight, RenderedText};
use rune_kit::{CodeFrame, Markdown, MarkdownStyle};

const SAMPLE: &str = include_str!("sample.md");
const COPIED_FOR: Duration = Duration::from_millis(1500);

struct Doc {
    path: Option<String>,
    source: String,
    vim: Vim,
    /// The code block whose copy button shows a check, and since when.
    copied: Option<(usize, Instant)>,
}

impl Default for Doc {
    fn default() -> Self {
        let path = rune::extra_args().first().cloned();
        let source = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_else(|| SAMPLE.to_owned());
        Doc {
            path,
            source,
            vim: Vim::new().skip_blank_lines(true),
            copied: None,
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
            eprintln!("could not save {path}: {error}");
        }
    }
}

/// GitHub's reading width and type, in the theme's colours.
fn style(rex: &Rex) -> MarkdownStyle {
    MarkdownStyle::from_kit(&rex.kit)
        .text(rex.text)
        .muted(rex.text_secondary)
        .link(rex.accent_text)
        .link_underline(rex.accent_text.alpha(0.4))
        .quote_rule(rex.separator)
        .rule(rex.separator)
        .code_block(rex.group, rex.separator)
        .inline_code_bg(rex.field)
        .selection(rex.accent_soft)
        // GitHub's heading scale: 2, 1.5 and 1.25 times the body.
        .heading_font(
            1,
            TextStyle {
                size: 26.0,
                weight: 600.0,
                line_height: 34.0,
                ..rex.ty.body
            },
        )
        .heading_font(
            2,
            TextStyle {
                size: 20.0,
                weight: 600.0,
                line_height: 28.0,
                ..rex.ty.body
            },
        )
        .heading_font(
            3,
            TextStyle {
                size: 16.0,
                weight: 600.0,
                line_height: 24.0,
                ..rex.ty.body
            },
        )
}

/// The Vim cursor, selection and search matches as highlights on the page.
fn vim_highlights(
    vim: &Vim,
    page: &RenderedText,
    rex: &Rex,
) -> Vec<(Vec<std::ops::Range<usize>>, Highlight)> {
    let text = page.text();
    let mut out = Vec::new();
    for range in vim.matches(text) {
        out.push((page.source_ranges(range), Highlight::new()));
    }
    if let Some(range) = vim.selection(text) {
        out.push((
            page.source_ranges(range),
            Highlight::new().bg(rex.accent_soft).fg(rex.text),
        ));
    }
    let cursor = vim.cursor(text);
    out.push((
        page.source_ranges(cursor),
        Highlight::new()
            .bg(rex.accent)
            .fg(rex.on_accent)
            .key("vim-cursor"),
    ));
    out.retain(|(ranges, _)| !ranges.is_empty());
    out
}

/// A code block with a copy button over its top right corner while the pointer is on it.
fn code_frame(rex: &Rex, copied: Option<usize>) -> impl Fn(El, &CodeFrame<'_>) -> El + 'static {
    let rex = rex.clone();
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
        .build(&format!("copy-{index}"), &rex);
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
    let rex = Rex::current(cx.system_dark());
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
        .child(rune::icon(&icons::FILE_TEXT, 16.0).color(rex.text_secondary))
        .child(
            text(title)
                .font(rex.ty.strong)
                .color(rex.text)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(kbd("/", &rex))
        .child(text("search").font(rex.ty.caption).color(rex.text_tertiary))
        .child(kbd("v", &rex))
        .child(text("select").font(rex.ty.caption).color(rex.text_tertiary))
        .child(kbd("y", &rex))
        .child(text("yank").font(rex.ty.caption).color(rex.text_tertiary))
        .child(kbd("gy", &rex))
        .child(text("as md").font(rex.ty.caption).color(rex.text_tertiary));
    let doc = Markdown::new(s.source.as_str())
        .style(style(&rex))
        .selectable(true)
        .highlights(vim_highlights(&s.vim, &page, &rex))
        .code_frame(code_frame(&rex, s.copied.map(|(i, _)| i)))
        // A clicked box would keep focus, and Space would go on ticking it rather than the task
        // under the cursor; drop focus so keys come back to Vim.
        .on_task_tx(|s: &mut Doc, _, offset, checked, tx| {
            s.set_task(offset, checked);
            tx.blur();
        })
        .build(&rex.kit);
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
        .fill(rex.background)
        .child(bar)
        .child(el().h(1.0).w_full().fill(rex.separator))
        .child(content.grow(1.0).min_h(0.0))
        .child(status_line(&s.vim, &page, &rex))
}

/// Vim's status line: the mode (or the search being typed), a note, half-typed keys and the
/// cursor's line.
fn status_line(vim: &Vim, page: &RenderedText, rex: &Rex) -> El {
    let text_of = page.text();
    let line = text_of[..vim.cursor(text_of).start].matches('\n').count() + 1;
    let lines = text_of.matches('\n').count() + 1;
    let searching = vim.mode() == VimMode::Search;
    let mode = if searching {
        text(vim.status()).font(rex.ty.mono).color(rex.text)
    } else {
        el().child(
            Badge::new(vim.status())
                .tone(match vim.mode() {
                    VimMode::Normal => rex_ui::Tone::Neutral,
                    _ => rex_ui::Tone::Accent,
                })
                .build(rex),
        )
    };
    row()
        .key("status")
        .w_full()
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .fill(rex.group)
        .child(mode)
        .child(
            text(vim.message().unwrap_or_default().to_owned())
                .font(rex.ty.caption)
                .color(rex.text_secondary)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(
            text(vim.pending())
                .font(rex.ty.mono)
                .color(rex.text_secondary),
        )
        .child(
            text(format!("{line}:{lines}"))
                .font(rex.ty.mono)
                .color(rex.text_tertiary),
        )
}

fn on_key(s: &mut Doc, k: &KeyEvent, tx: &mut Tx) -> bool {
    let page = RenderedText::new(&s.source);
    // Space on a task's line ticks it, as a click on its box does.
    if k.key == rune::Key::Text(" ".into()) && s.vim.mode() == VimMode::Normal {
        let Some((offset, checked)) = s.task_under_cursor(&page) else {
            return false;
        };
        s.set_task(offset, !checked);
        return true;
    }
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
