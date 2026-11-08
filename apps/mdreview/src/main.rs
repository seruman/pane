//! Review a Markdown document in a terminal pane, the way a pull request is reviewed: select
//! text with Vim's keys, comment on it, and the comment shows under its block as a thread does on
//! GitHub. `:wq` writes the review out as Markdown feedback, for the agent that wrote the
//! document (or anyone).
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p mdreview -- PLAN.md            # feedback to stdout
//! RUNE_TERMINAL=1 cargo run --release -p mdreview -- PLAN.md -o out.md  # or to a file
//! some-agent | RUNE_TERMINAL=1 mdreview -                               # read stdin
//! ```
//!
//! Read with `vimkeys`'s keys. Select with `v` or `V`, then `c` comments on the selection, `d`
//! marks it to be removed; `cc`
//! and `dd` do the same for the cursor's line. `C` adds a general comment, about the document
//! as a whole. `]c` and `[c` step through the comments; on one, Enter or `ce` edits it and `dc`
//! deletes it. `Tab` lists every comment beside the page. While keys are half typed (`y`,
//! `gy`, `c`, a selection) a panel shows what can follow; Space shows every key.
//!
//! In a comment, Enter saves (Shift-Enter starts a new line) and Escape cancels. `:wq`, `:x`
//! or `ZZ` quit and send the review; `:q` quits when there's nothing to send, `:q!` or `ZQ`
//! quit without it. Ctrl-C quits without sending too.

mod page;
mod review;

use std::io::{IsTerminal as _, Read as _, Write};
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use pane_ui::{Badge, Button, Theme, Tone, icons, kbd};
use rune::{
    App, AppSpec, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, TextStyle, Tx, col, el, row, text,
    text_input,
};
use rune_kit::markdown::Highlight;
use rune_kit::{Markdown, MarkdownStyle};
use vimkeys::{Hint, Keymap, Operator, Stroke, Table, Vim, VimEffect, VimMode, hint};

use page::Page;
use review::{Comment, Kind, Review};

const SAMPLE: &str = include_str!("sample.md");

/// The document and where its review goes, read from the command line before the terminal is
/// taken over, so a bad path is an error in the shell rather than a page.
struct Input {
    title: String,
    /// The path, for the feedback's title; `None` for stdin and the sample.
    name: Option<String>,
    source: String,
    out: Option<String>,
}

static INPUT: OnceLock<Input> = OnceLock::new();

/// What the composer is writing.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A new comment on this range of the page's text, or with `None`, a general one.
    New {
        kind: Kind,
        range: Option<Range<usize>>,
    },
    /// Comment `n`, again.
    Edit(usize),
}

/// What the review adds to Vim's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// `c{motion}`, `cc`, `v…c`: comment on what it covers.
    Comment,
    /// `d{motion}`, `dd`, `v…d`: mark it to be removed.
    Remove,
    /// `C`: a comment on the whole document.
    General,
    /// `]c`, `[c`.
    NextComment,
    PreviousComment,
    /// `\e`, Enter: edit the comment picked, or under the cursor.
    Edit,
    /// `\x`: delete it.
    Delete,
    /// Tab: the comment list.
    List,
    /// `\?`: every key.
    Keys,
    /// `:`: a command line.
    CommandLine,
    /// `ZZ`, `ZQ`.
    SendAndQuit,
    QuitWithoutSending,
}

/// Vim's keys, and the review's on top of them.
fn keymap() -> Keymap<Act> {
    use Stroke::{Char as C, Leader};
    Keymap::vim()
        .operator(&[C('c')], Operator::App(Act::Comment), "Comment")
        .operator(&[C('d')], Operator::App(Act::Remove), "Remove this")
        .action(&[C('C')], Act::General, "General comment")
        .action(&[C(']'), C('c')], Act::NextComment, "Next comment")
        .action(&[C('['), C('c')], Act::PreviousComment, "Previous comment")
        .action(&[Stroke::Enter], Act::Edit, "Edit the comment")
        .action(&[Leader, C('e')], Act::Edit, None)
        .action(&[Leader, C('x')], Act::Delete, "Delete the comment")
        .action(&[Stroke::Tab], Act::List, "List the comments")
        .action(&[Leader, C('?')], Act::Keys, "Every key")
        .action(&[C(':')], Act::CommandLine, None)
        .action(
            &[C('Z'), C('Z')],
            Act::SendAndQuit,
            "Send the review and quit",
        )
        .action(
            &[C('Z'), C('Q')],
            Act::QuitWithoutSending,
            "Quit without sending",
        )
}

struct App_ {
    title: String,
    /// The document's name for the feedback's title: the path given, or `None` for stdin and
    /// the sample.
    name: Option<String>,
    /// Where the feedback goes: a file, or stdout when `None`.
    out: Option<String>,
    source: String,
    page: Page,
    vim: Vim<Act>,
    review: Review,
    /// The comment being written. Its text is `draft`, kept while the composer is closed too:
    /// Rune reaches a field's `TextEdit` once more after it goes, to clear its focus.
    composer: Option<Target>,
    draft: TextEdit,
    /// The comment `]c`, `[c` or a click went to, until the cursor moves; it tells comments
    /// apart that start at the same place, and it's how a general comment is picked.
    selected: Option<usize>,
    /// `\\?` asked for every key; the next key closes the list.
    all_keys: bool,
    /// The `:` command being typed.
    cmdline: Option<String>,
    /// The comment list floating over the page, and the row its cursor is on; `None` while
    /// it's closed. It takes no focus: while it's open, keys move its cursor.
    list: Option<usize>,
    /// Quitting sends the review: `ZZ`, `:wq` or `:x`. Any other way out sends nothing.
    send: bool,
    note: Option<String>,
}

impl Default for App_ {
    fn default() -> Self {
        let input = INPUT.get_or_init(|| Input {
            title: "sample.md".into(),
            name: None,
            source: SAMPLE.into(),
            out: None,
        });
        let mut app = App_::new(input.title.clone(), input.source.clone(), input.out.clone());
        app.name = input.name.clone();
        app
    }
}

impl App_ {
    fn new(title: String, source: String, out: Option<String>) -> Self {
        App_ {
            name: None,
            title,
            out,
            page: Page::new(&source),
            source,
            vim: Vim::new(keymap()).skip_blank_lines(true),
            review: Review::default(),
            composer: None,
            draft: TextEdit::default(),
            selected: None,
            all_keys: false,
            cmdline: None,
            list: None,
            send: false,
            note: None,
        }
    }

    fn cursor(&self) -> usize {
        self.vim.cursor(self.page.text()).start
    }

    /// The comment Enter, `ce` and `dc` act on, and the one the page marks: the comment list's
    /// row while it's open, else the one stepped to, or the one under the cursor.
    fn current(&self) -> Option<usize> {
        let n = self.review.comments.len();
        if let Some(at) = self.list {
            return (at < n).then_some(at);
        }
        self.selected
            .filter(|i| *i < n)
            .or_else(|| self.review.at(self.cursor()))
    }

    /// Shows comment `n` on the page without moving Vim's cursor: its text centred, or for a
    /// general comment, its card.
    fn show(&self, n: usize, tx: &mut Tx) {
        match self.review.comments.get(n).map(|c| c.range.is_some()) {
            Some(true) => tx.scroll_center(CURRENT_MARK),
            Some(false) => tx.scroll_into_view(thread_key(n)),
            None => {}
        }
    }

    fn open(&mut self, target: Target) {
        // Like a Vim operator, commenting ends the selection: the cursor goes to its start.
        if let Target::New {
            range: Some(range), ..
        } = &target
        {
            self.vim.set_cursor(range.start, self.page.text());
        }
        let body = match &target {
            Target::Edit(n) => self.review.comments[*n].body.clone(),
            Target::New { .. } => String::new(),
        };
        self.draft = TextEdit::new(body);
        self.draft.focused = true;
        self.composer = Some(target);
        self.note = None;
    }

    /// Saves what the composer holds. A comment in words with nothing written is dropped.
    fn save(&mut self) {
        let Some(target) = self.composer.take() else {
            return;
        };
        let body = self.draft.text.trim().to_owned();
        match target {
            Target::New { kind, range } => {
                if kind == Kind::Comment && body.is_empty() {
                    return;
                }
                let n = self.review.add(Comment { kind, range, body });
                self.selected = Some(n);
                self.note = Some(format!("Comment #{} saved", n + 1));
            }
            Target::Edit(n) => {
                let c = &mut self.review.comments[n];
                if c.kind == Kind::Comment && body.is_empty() {
                    self.delete(n);
                } else {
                    c.body = body;
                    self.note = Some(format!("Comment #{} saved", n + 1));
                }
            }
        }
    }

    /// Marks the selection (or the line) to be removed. Enter adds words to it.
    fn remove(&mut self, range: Range<usize>) {
        self.vim.set_cursor(range.start, self.page.text());
        let n = self.review.add(Comment {
            kind: Kind::Remove,
            range: Some(range),
            body: String::new(),
        });
        self.selected = Some(n);
        self.note = Some("Marked to remove — Enter adds a note".into());
    }

    fn delete(&mut self, n: usize) {
        if n < self.review.comments.len() {
            self.review.comments.remove(n);
            self.selected = None;
            self.note = Some(format!("Comment #{} deleted", n + 1));
        }
    }

    /// Goes to comment `n`: the cursor to its start, and its card into view.
    fn jump(&mut self, n: usize, tx: &mut Tx) {
        let Some(c) = self.review.comments.get(n) else {
            return;
        };
        if let Some(range) = &c.range {
            self.vim.set_cursor(range.start, self.page.text());
        }
        self.selected = Some(n);
        self.note = Some(format!(
            "#{} of {} · Enter edits, dc deletes",
            n + 1,
            self.review.comments.len()
        ));
        tx.scroll_into_view(thread_key(n));
    }

    /// The feedback, when quitting sends it.
    fn feedback(&self) -> Option<String> {
        if !self.send {
            return None;
        }
        self.review
            .feedback(&self.source, &self.page, self.name.as_deref())
    }

    /// Opens the comment list on the comment picked or under the cursor, or the first, and
    /// shows that comment on the page.
    fn open_list(&mut self, tx: &mut Tx) {
        let at = self.current().unwrap_or(0);
        self.list = Some(at);
        tx.scroll_into_view(list_key(at));
        self.show(at, tx);
    }

    /// Runs a `:` command.
    fn command(&mut self, cmd: &str, tx: &mut Tx) {
        match cmd.trim() {
            "q" | "quit" if !self.review.is_empty() => {
                self.note = Some("The review isn't sent: :wq sends it, :q! drops it".into());
            }
            "q" | "quit" | "q!" | "quit!" => tx.quit(),
            "wq" | "x" | "wq!" | "x!" => {
                self.send = true;
                tx.quit();
            }
            "" => {}
            other => self.note = Some(format!("Not a command: {other}")),
        }
    }
}

/// The key of the current comment's mark on the page, to scroll to.
const CURRENT_MARK: &str = "current-comment";

/// The key a comment's card is drawn under, to scroll to.
fn thread_key(n: usize) -> String {
    format!("thread-{n}")
}

/// GitHub's reading width and type, in the theme's colours, as `mdview` draws.
fn style(theme: &Theme) -> MarkdownStyle {
    let heading = |size: f32, line_height: f32| TextStyle {
        size,
        weight: 600.0,
        line_height,
        ..theme.ty.body
    };
    MarkdownStyle::from_kit(&theme.kit)
        .text(theme.text)
        .muted(theme.text_secondary)
        .link(theme.accent_text)
        .link_underline(theme.accent_text.alpha(0.4))
        .quote_rule(theme.separator)
        .rule(theme.separator)
        .code_block(theme.group, theme.separator)
        .inline_code_bg(theme.field)
        .heading_font(1, heading(26.0, 34.0))
        .heading_font(2, heading(20.0, 28.0))
        .heading_font(3, heading(16.0, 24.0))
}

/// The colour that marks a kind of comment: its text's tint and its badge.
fn tone(c: &Comment) -> Tone {
    match (c.kind, &c.range) {
        (_, None) => Tone::Neutral,
        (Kind::Comment, _) => Tone::Warning,
        (Kind::Remove, _) => Tone::Danger,
    }
}

fn tint(kind: Kind, theme: &Theme) -> rune::Color {
    match kind {
        Kind::Comment => theme.warning.alpha(0.22),
        Kind::Remove => theme.danger.alpha(0.22),
    }
}

/// What's painted on block `index`: comments' ranges, then search matches, the selection and
/// the cursor on top.
fn highlights(s: &App_, index: usize, theme: &Theme) -> Vec<(Vec<Range<usize>>, Highlight)> {
    let page = &s.page;
    let text = page.text();
    let local = |range: Range<usize>| page.local_ranges(index, &page.source_ranges(range));
    let mut out = Vec::new();
    let current = s.current();
    for (n, c) in s.review.comments.iter().enumerate() {
        let Some(range) = c.range.clone() else {
            continue;
        };
        let mut h = Highlight::new().bg(tint(c.kind, theme));
        if current == Some(n) {
            // The same ring whether the cursor or the list picks it.
            h = h.ring(tint(c.kind, theme).alpha(0.9), 1.0);
            // Keyed in the block it starts in, for the list to scroll to.
            if page.block_at(range.start) == index {
                h = h.key(CURRENT_MARK);
            }
        }
        out.push((local(range), h));
    }
    if let Some(Target::New {
        range: Some(range),
        kind,
    }) = &s.composer
    {
        out.push((
            local(range.clone()),
            Highlight::new().bg(tint(*kind, theme)),
        ));
    }
    for range in s.vim.matches(text) {
        out.push((local(range), Highlight::new()));
    }
    if let Some(range) = s.vim.selection(text) {
        out.push((
            local(range),
            Highlight::new().bg(theme.accent_soft).fg(theme.text),
        ));
    }
    if s.composer.is_none() {
        let cursor = local(s.vim.cursor(text));
        out.push((
            cursor,
            Highlight::new()
                .bg(theme.accent)
                .fg(theme.on_accent)
                .key("vim-cursor"),
        ));
    }
    out.retain(|(ranges, _)| !ranges.is_empty());
    out
}

/// The comment box.
fn composer_view(draft: &TextEdit, title: &str, quote: Option<&str>, theme: &Theme) -> El {
    let field = text_input(draft, theme.ty.body)
        .key("composer-field")
        .w_full()
        .color(theme.text)
        .placeholder("Leave a comment", theme.text_tertiary)
        .on_edit(|s: &mut App_| &mut s.draft)
        .on_submit(|s: &mut App_, tx: &mut Tx| {
            s.save();
            tx.blur();
        })
        .on_input_key(|s: &mut App_, k: &KeyEvent, tx: &mut Tx| {
            if k.key != Key::Escape {
                return false;
            }
            s.composer = None;
            s.note = Some("Cancelled".into());
            tx.blur();
            true
        });
    let save = Button::new("Save")
        .small()
        .on_press_tx(|s: &mut App_, tx: &mut Tx| {
            s.save();
            tx.blur();
        })
        .build("composer-save", theme);
    let cancel = Button::new("Cancel")
        .small()
        .gray()
        .on_press_tx(|s: &mut App_, tx: &mut Tx| {
            s.composer = None;
            tx.blur();
        })
        .build("composer-cancel", theme);
    let mut card = col()
        .key("composer")
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(theme.radius.group)
        .fill(theme.group)
        .border(1.0, theme.accent.alpha(0.6))
        .child(
            text(title.to_owned())
                .font(theme.ty.strong)
                .color(theme.text),
        );
    if let Some(q) = quote {
        card = card.child(quote_view(q, theme));
    }
    card.child(
        el().w_full()
            .padding(8.0)
            .radius(theme.radius.field)
            .fill(theme.field)
            .child(rune::auto_grow(field, 2, 8)),
    )
    .child(
        row()
            .w_full()
            .gap(8.0)
            .center_y()
            .child(
                text("Enter saves · Shift-Enter new line · Esc cancels")
                    .font(theme.ty.caption)
                    .color(theme.text_tertiary)
                    .truncate()
                    .shrink(),
            )
            .child(el().grow(1.0))
            .child(cancel)
            .child(save),
    )
}

/// What a comment is about, a line or two of it.
fn quote_view(q: &str, theme: &Theme) -> El {
    row()
        .w_full()
        .gap(8.0)
        .child(el().w(2.0).h_full().fill(theme.separator))
        .child(
            text(q.trim().to_owned())
                .font(theme.ty.caption)
                .color(theme.text_secondary)
                .max_lines(2)
                .grow(1.0)
                .shrink(),
        )
}

/// A saved comment, as a card.
fn comment_view(n: usize, c: &Comment, current: bool, theme: &Theme) -> El {
    let edit = Button::icon_only(&icons::PENCIL, "Edit")
        .plain()
        .small()
        .on_press(move |s: &mut App_| {
            s.selected = Some(n);
            s.open(Target::Edit(n));
        })
        .build(&format!("edit-{n}"), theme);
    let delete = Button::icon_only(&icons::TRASH, "Delete")
        .plain()
        .small()
        .on_press(move |s: &mut App_| s.delete(n))
        .build(&format!("delete-{n}"), theme);
    let mut head = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(
            text(format!("#{}", n + 1))
                .font(theme.ty.mono)
                .color(theme.text_tertiary),
        )
        .child(Badge::new(c.title()).tone(tone(c)).build(theme))
        .child(el().grow(1.0));
    if current {
        head = head.child(
            text("Enter edit · dc delete")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        );
    }
    let mut card = col()
        .key(thread_key(n))
        .w_full()
        .gap(6.0)
        .px(12.0)
        .py(8.0)
        .radius(theme.radius.group)
        .fill(theme.group)
        .border(
            1.0,
            if current {
                theme.accent.alpha(0.7)
            } else {
                theme.separator
            },
        )
        .child(head.child(edit).child(delete));
    if !c.body.is_empty() {
        card = card.child(
            text(c.body.clone())
                .font(theme.ty.body)
                .color(theme.text)
                .wrap()
                .w_full(),
        );
    }
    card
}

/// A thread: a column of cards with a rule down its left, as GitHub sets review comments off.
fn thread(cards: Vec<El>, theme: &Theme) -> El {
    let mut c = col().w_full().gap(8.0).pl(12.0);
    for card in cards {
        c = c.child(card);
    }
    row()
        .w_full()
        .mt(4.0)
        .mb(12.0)
        .child(el().w(2.0).h_full().fill(theme.accent.alpha(0.35)))
        .child(c.grow(1.0).min_w(0.0))
}

/// The comments that end in block `index`, and the composer when it's for one there.
fn block_thread(s: &App_, index: usize, theme: &Theme) -> Option<El> {
    let page = &s.page;
    let ends_here =
        |range: &Range<usize>| page.block_at(range.end.saturating_sub(1).max(range.start)) == index;
    let current = s.current();
    let mut cards = Vec::new();
    for (n, c) in s.review.comments.iter().enumerate() {
        if !c.range.as_ref().is_some_and(ends_here) {
            continue;
        }
        if s.composer == Some(Target::Edit(n)) {
            cards.push(composer_view(
                &s.draft,
                &format!("Edit #{} · {}", n + 1, c.title()),
                None,
                theme,
            ));
        } else {
            cards.push(comment_view(n, c, current == Some(n), theme));
        }
    }
    if let Some(Target::New {
        kind,
        range: Some(range),
    }) = &s.composer
        && ends_here(range)
    {
        let quote = page.text().get(range.clone()).unwrap_or_default();
        let title = match kind {
            Kind::Comment => "New comment",
            Kind::Remove => "Remove this",
        };
        cards.push(composer_view(&s.draft, title, Some(quote), theme));
    }
    (!cards.is_empty()).then(|| thread(cards, theme))
}

/// The general comments, above the document, and the composer for a new one.
fn general_thread(s: &App_, theme: &Theme) -> Option<El> {
    let current = s.current();
    let mut cards = Vec::new();
    for (n, c) in s.review.comments.iter().enumerate() {
        if c.range.is_some() {
            continue;
        }
        if s.composer == Some(Target::Edit(n)) {
            cards.push(composer_view(
                &s.draft,
                &format!("Edit #{} · General", n + 1),
                None,
                theme,
            ));
        } else {
            cards.push(comment_view(n, c, current == Some(n), theme));
        }
    }
    if matches!(s.composer, Some(Target::New { range: None, .. })) {
        cards.push(composer_view(&s.draft, "General comment", None, theme));
    }
    (!cards.is_empty()).then(|| thread(cards, theme).mb(20.0))
}

/// Every comment, general ones first, in a panel floating over the page's right side. The row
/// `at` is on is tinted; Enter jumps to it.
fn list_view(s: &App_, at: usize, theme: &Theme) -> El {
    let mut rows = col().w_full().gap(2.0);
    if s.review.comments.is_empty() {
        rows = rows.child(
            text("No comments yet. Select with v, then c; C comments on the whole document.")
                .font(theme.ty.caption)
                .color(theme.text_tertiary)
                .wrap()
                .w_full(),
        );
    }
    for (n, c) in s.review.comments.iter().enumerate() {
        let quote = c.range.clone().map(|r| {
            s.page
                .text()
                .get(r)
                .unwrap_or_default()
                .trim()
                .replace('\n', " ")
        });
        let mut item = col()
            .key(list_key(n))
            .w_full()
            .gap(4.0)
            .padding(8.0)
            .radius(theme.radius.row)
            .fill(if n == at {
                theme.accent_soft
            } else {
                rune::Color::TRANSPARENT
            })
            .on_press_tx(move |s: &mut App_, tx: &mut Tx| {
                s.list = None;
                s.jump(n, tx);
            })
            .child(
                row()
                    .w_full()
                    .gap(6.0)
                    .center_y()
                    .child(
                        text(format!("#{}", n + 1))
                            .font(theme.ty.mono)
                            .color(theme.text_tertiary),
                    )
                    .child(Badge::new(c.title()).tone(tone(c)).build(theme)),
            );
        if let Some(q) = quote {
            item = item.child(quote_view(&q, theme));
        }
        if !c.body.is_empty() {
            item = item.child(
                text(c.body.clone())
                    .font(theme.ty.caption)
                    .color(theme.text)
                    .max_lines(2)
                    .w_full(),
            );
        }
        rows = rows.child(item);
    }
    let foot = row()
        .w_full()
        .gap(12.0)
        .center_y()
        .child(text("j k").font(theme.ty.mono).color(theme.text_tertiary))
        .child(
            text("move")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
        .child(text("enter").font(theme.ty.mono).color(theme.text_tertiary))
        .child(
            text("go to")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
        .child(el().grow(1.0))
        .child(text("esc").font(theme.ty.mono).color(theme.text_tertiary))
        .child(
            text("close")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        );
    col()
        .key("comment-list")
        .w(320.0)
        .shrink()
        .min_h(0.0)
        .gap(8.0)
        .padding(10.0)
        .radius(theme.radius.popover)
        .fill(theme.popover)
        .border(1.0, theme.popover_border)
        .shadow(12.0, 32.0, theme.shadow_strong)
        .child(
            text(format!("Comments · {}", s.review.comments.len()))
                .font(theme.ty.strong)
                .color(theme.text),
        )
        .child(
            col()
                .key("comment-list-scroll")
                .w_full()
                .shrink()
                .min_h(0.0)
                .child(rows)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                }),
        )
        .child(foot)
}

/// The key of the list's row for comment `n`, to scroll it into view.
fn list_key(n: usize) -> String {
    format!("list-{n}")
}

fn view(s: &App_, cx: &mut Cx<App_>) -> El {
    let theme = Theme::current(cx.system_dark());
    let md_style = style(&theme);
    let mut doc = col().w_full().max_w(880.0).self_center();
    if let Some(general) = general_thread(s, &theme) {
        doc = doc.child(general);
    }
    for (i, block) in s.page.blocks.iter().enumerate() {
        let md = Markdown::new(block.markdown.as_str())
            .style(md_style.clone())
            .highlights(highlights(s, i, &theme))
            .tasks_read_only()
            .build(&theme.kit);
        doc = doc.child(
            el().key(format!("block-{i}"))
                .w_full()
                .mt(block.top)
                .child(md),
        );
        if let Some(thread) = block_thread(s, i, &theme) {
            doc = doc.child(thread);
        }
    }
    let content = col()
        .key("page")
        .w_full()
        .h_full()
        .padding(24.0)
        .child(doc)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    // The comment list floats over the page's right side and the which-key panel over its
    // bottom right, as Rex's prefix panel does; neither moves the page.
    let mut body = el().w_full().grow(1.0).min_h(0.0).child(content);
    if let Some(at) = s.list {
        body = body.child(
            col()
                .pin(None, Some(16.0), Some(16.0), Some(16.0))
                .items_end()
                .child(list_view(s, at, &theme)),
        );
    }
    if let Some(keys) = which_key(s, &theme) {
        body = body.child(keys.pin(None, None, Some(16.0), Some(16.0)));
    }
    col()
        .w_full()
        .h_full()
        .fill(theme.background)
        .child(bar(s, &theme))
        .child(el().h(1.0).w_full().fill(theme.separator))
        .child(body)
        .child(status_line(s, &theme))
}

/// The keys that can follow what's been typed, for the which-key panel, all from the keymap:
/// what Vim has half typed (`y`, `gy`, `ci`, `]`, the leader) or the visual mode, or every key
/// after `\\?`. `None` when nothing is half typed.
fn pending_keys(s: &App_) -> Option<(String, Vec<Vec<Hint>>)> {
    if s.composer.is_some() || s.cmdline.is_some() {
        return None;
    }
    if s.all_keys {
        let mut groups = s.vim.keymap().hints(Table::Normal);
        groups.push(vec![
            hint(":wq", "Send the review and quit"),
            hint(":q", "Quit, with nothing to send"),
            hint(":q!", "Quit without sending"),
        ]);
        return Some(("keys".into(), groups));
    }
    s.vim.hints()
}

/// The keys that can follow, in a floating panel: the mode or prefix as a pill, Escape to
/// cancel, then each key's action with the key on the right.
fn which_key(s: &App_, theme: &Theme) -> Option<El> {
    let (prefix, groups) = pending_keys(s)?;
    let row_of = |r: &Hint| {
        row()
            .w_full()
            .h(24.0)
            .gap(24.0)
            .center_y()
            .child(
                text(r.what)
                    .font(theme.ty.caption)
                    .color(theme.text)
                    .grow(1.0)
                    .shrink(),
            )
            .child(
                text(r.keys.clone())
                    .font(theme.ty.mono)
                    .color(theme.text_secondary),
            )
    };
    let mut list = col().w_full();
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            list = list.child(el().h(1.0).w_full().my(6.0).fill(theme.separator));
        }
        for r in group {
            list = list.child(row_of(r));
        }
    }
    let pill = el()
        .px(9.0)
        .py(2.0)
        .radius(9.0)
        .fill(theme.accent_soft)
        .child(text(prefix).font(theme.ty.label).color(theme.accent_text));
    Some(
        col()
            .key("which-key")
            .w(260.0)
            .gap(8.0)
            .padding(12.0)
            .radius(theme.radius.popover)
            .fill(theme.popover)
            .border(1.0, theme.popover_border)
            .shadow(12.0, 32.0, theme.shadow_strong)
            .child(
                row()
                    .w_full()
                    .gap(8.0)
                    .center_y()
                    .child(pill)
                    .child(el().grow(1.0))
                    .child(text("esc").font(theme.ty.mono).color(theme.text_tertiary))
                    .child(
                        text("Cancel")
                            .font(theme.ty.caption)
                            .color(theme.text_tertiary),
                    ),
            )
            .child(list),
    )
}

fn bar(s: &App_, theme: &Theme) -> El {
    let hint = |k: &str, what: &str| {
        row().gap(4.0).center_y().child(kbd(k, theme)).child(
            text(what.to_owned())
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        )
    };
    row()
        .key("bar")
        .w_full()
        .min_h(40.0)
        .px(20.0)
        .py(6.0)
        .gap(10.0)
        .center_y()
        .child(rune::icon(&icons::FILE_TEXT, 16.0).color(theme.text_secondary))
        .child(
            text(s.title.clone())
                .font(theme.ty.strong)
                .color(theme.text)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(hint("v", "select"))
        .child(hint("C", "general"))
        .child(hint("Tab", "comments"))
        .child(hint(":wq", "send"))
        .child(hint(":q!", "quit"))
        .child(hint("\\?", "keys"))
}

fn status_line(s: &App_, theme: &Theme) -> El {
    let page_text = s.page.text();
    let line = page_text[..s.cursor()].matches('\n').count() + 1;
    let lines = page_text.matches('\n').count() + 1;
    let typed = if let Some(cmd) = &s.cmdline {
        Some(format!(":{cmd}"))
    } else if s.vim.mode() == VimMode::Search && s.composer.is_none() {
        Some(s.vim.status())
    } else {
        None
    };
    let mode = match typed {
        Some(t) => text(t).font(theme.ty.mono).color(theme.text),
        None => {
            let (label, tone) = if s.composer.is_some() {
                ("COMMENT".to_owned(), Tone::Warning)
            } else if s.vim.mode() == VimMode::Normal {
                (s.vim.status(), Tone::Neutral)
            } else {
                (s.vim.status(), Tone::Accent)
            };
            el().child(Badge::new(label).tone(tone).build(theme))
        }
    };
    let count = s.review.comments.len();
    let summary = format!("{count} comment{}", if count == 1 { "" } else { "s" });
    let note = s
        .note
        .clone()
        .or_else(|| s.vim.message().map(str::to_owned))
        .unwrap_or_default();
    let pending = s.vim.pending();
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
            text(note)
                .font(theme.ty.caption)
                .color(theme.text_secondary)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0))
        .child(
            text(summary)
                .font(theme.ty.caption)
                .color(theme.text_secondary),
        )
        .child(
            text(pending)
                .font(theme.ty.mono)
                .color(theme.text_secondary),
        )
        .child(
            text(format!("{line}:{lines}"))
                .font(theme.ty.mono)
                .color(theme.text_tertiary),
        )
}

/// The character a key types, without Cmd or Ctrl.
fn ch(k: &KeyEvent) -> Option<char> {
    match &k.key {
        Key::Text(t) if !k.mods.cmd && !k.mods.ctrl => {
            let mut chars = t.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(c)
        }
        _ => None,
    }
}

/// A key while a `:` command is being typed.
fn cmdline_key(s: &mut App_, k: &KeyEvent, tx: &mut Tx) {
    let Some(cmd) = s.cmdline.as_mut() else {
        return;
    };
    match &k.key {
        Key::Escape => s.cmdline = None,
        Key::Backspace => {
            if cmd.pop().is_none() {
                s.cmdline = None;
            }
        }
        Key::Enter => {
            let cmd = s.cmdline.take().unwrap_or_default();
            s.command(&cmd, tx);
        }
        Key::Text(t) if !k.mods.cmd && !k.mods.ctrl => cmd.push_str(t),
        _ => {}
    }
}

fn on_key(s: &mut App_, k: &KeyEvent, tx: &mut Tx) -> bool {
    // The composer's field has the keys while it's open; one that reaches here missed it.
    if s.composer.is_some() {
        return false;
    }
    if s.cmdline.is_some() {
        cmdline_key(s, k, tx);
        return true;
    }
    s.note = None;
    if s.list.is_some() {
        list_key_press(s, k, tx);
        return true;
    }
    // The list of every key only shows them: the next key closes it and does what it does.
    if std::mem::take(&mut s.all_keys) && k.key == Key::Escape {
        return true;
    }
    let before = (s.cursor(), s.vim.mode());
    let effect = s.vim.key(k, s.page.text());
    match effect {
        Some(VimEffect::Moved) => {
            // The comment `]c` picked holds until the cursor moves; a key toward a command
            // (the `]` of `]c`) isn't a move.
            if (s.cursor(), s.vim.mode()) != before {
                s.selected = None;
            }
            tx.scroll_into_view("vim-cursor");
        }
        Some(VimEffect::Copy(yanked)) => tx.copy(yanked),
        Some(VimEffect::CopySource { range, linewise }) => {
            let ranges = s.page.source_ranges(range);
            let mut md = page::source_of(&s.source, &ranges, linewise);
            if linewise {
                md.push('\n');
            }
            tx.copy(md);
        }
        Some(VimEffect::Operator { action, range, .. }) => match action {
            Act::Comment => s.open(Target::New {
                kind: Kind::Comment,
                range: Some(range),
            }),
            Act::Remove => s.remove(range),
            _ => {}
        },
        Some(VimEffect::Action { action, .. }) => act(s, action, tx),
        None => return false,
    }
    true
}

/// An action the review bound.
fn act(s: &mut App_, action: Act, tx: &mut Tx) {
    match action {
        Act::General => s.open(Target::New {
            kind: Kind::Comment,
            range: None,
        }),
        Act::NextComment | Act::PreviousComment => {
            match s
                .review
                .step(s.current(), s.cursor(), action == Act::PreviousComment)
            {
                Some(n) => s.jump(n, tx),
                None => s.note = Some("No comments".into()),
            }
        }
        Act::Edit => match s.current() {
            Some(n) => s.open(Target::Edit(n)),
            None => s.note = Some("No comment here: v selects, c comments".into()),
        },
        Act::Delete => match s.current() {
            Some(n) => s.delete(n),
            None => s.note = Some("No comment here".into()),
        },
        Act::List => s.open_list(tx),
        Act::Keys => s.all_keys = true,
        Act::CommandLine => s.cmdline = Some(String::new()),
        Act::SendAndQuit => s.command("wq", tx),
        Act::QuitWithoutSending => s.command("q!", tx),
        // Operators come back as `VimEffect::Operator`.
        Act::Comment | Act::Remove => {}
    }
}

/// A key while the comment list is open: `j` `k` (or the arrows) move its cursor, and the page
/// marks that comment and scrolls to it. Enter puts Vim's cursor there and closes the list;
/// Escape, Tab or `q` close it and scroll back to the cursor, which never moved.
fn list_key_press(s: &mut App_, k: &KeyEvent, tx: &mut Tx) {
    let Some(at) = s.list else { return };
    let n = s.review.comments.len();
    let step =
        |at: usize, by: isize| (at as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
    let to = match (&k.key, ch(k)) {
        (Key::Escape | Key::Tab, _) | (_, Some('q')) => {
            s.list = None;
            tx.scroll_into_view("vim-cursor");
            return;
        }
        (Key::Enter, _) => {
            s.list = None;
            if at < n {
                s.jump(at, tx);
            }
            return;
        }
        (Key::Down, _) | (_, Some('j')) => step(at, 1),
        (Key::Up, _) | (_, Some('k')) => step(at, -1),
        (_, Some('g')) => 0,
        (_, Some('G')) => n.saturating_sub(1),
        _ => return,
    };
    s.list = Some(to);
    tx.scroll_into_view(list_key(to));
    s.show(to, tx);
}

/// Sends the review on the way out, when `:wq` or `ZZ` asked: to the file `-o` names, or to
/// stdout once the terminal is the shell's again. A review that can't be written sets
/// [`SEND_FAILED`], so `main` exits non-zero.
fn send(s: &mut App_, _: &mut Tx) {
    if s.send && s.composer.is_some() {
        s.save();
    }
    let Some(feedback) = s.feedback() else { return };
    if let Err(e) = deliver(&feedback, s.out.as_deref(), &mut std::io::stdout()) {
        eprintln!("mdreview: {e}");
        SEND_FAILED.store(true, Ordering::Relaxed);
    }
}

/// The review couldn't be written. `on_quit` can't return an exit code, so it tells `main`.
static SEND_FAILED: AtomicBool = AtomicBool::new(false);

/// Writes `feedback` to the file at `out`, or to `stdout` when there's none. When the file
/// can't be written the review goes to `stdout` instead, so it isn't lost, and the error
/// comes back.
fn deliver(feedback: &str, out: Option<&str>, stdout: &mut impl Write) -> Result<(), String> {
    let to_stdout = |stdout: &mut dyn Write| {
        stdout
            .write_all(feedback.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|e| format!("could not write the review to stdout: {e}"))
    };
    let Some(path) = out else {
        return to_stdout(stdout);
    };
    let Err(e) = std::fs::write(path, feedback) else {
        return Ok(());
    };
    let written = to_stdout(stdout);
    let shown = if written.is_ok() { ", so it went to stdout" } else { "" };
    Err(format!("could not write the review to {path}: {e}{shown}"))
}

/// Reads the command line: `[FILE | -] [-o OUT]`. Exits with a message when the document can't
/// be read.
fn read_input() -> Input {
    let mut path = None;
    let mut out = None;
    let mut args = rune::extra_args().iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" | "--output" => match args.next() {
                Some(o) => out = Some(o.clone()),
                None => fail("-o needs a file"),
            },
            _ => path = Some(a.clone()),
        }
    }
    let (title, source) = match path.as_deref() {
        Some("-") => {
            if std::io::stdin().is_terminal() {
                fail("`-` reads the document from stdin, but stdin is the terminal");
            }
            let mut s = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut s) {
                fail(&format!("could not read stdin: {e}"));
            }
            ("stdin".to_owned(), s)
        }
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => (p.to_owned(), s),
            Err(e) => fail(&format!("could not read {p}: {e}")),
        },
        None => ("sample.md".to_owned(), SAMPLE.to_owned()),
    };
    let name = path.filter(|p| p != "-");
    Input {
        title,
        name,
        source,
        out,
    }
}

fn fail(message: &str) -> ! {
    eprintln!("mdreview: {message}");
    std::process::exit(1);
}

fn main() {
    if INPUT.set(read_input()).is_err() {
        unreachable!("the input is read once, here");
    }
    App::new(AppSpec::<App_> {
        scenes: &[
            ("comments", |s| {
                let text = s.page.text().to_owned();
                let find = |needle: &str| {
                    let at = text.find(needle).unwrap_or(0);
                    at..at + needle.len()
                };
                s.review.add(Comment::general(
                    "Good direction overall. Keep the first version small.",
                ));
                s.review.add(Comment::on(
                    find("stores the quote and its lines"),
                    "Lines drift when the agent rewrites the plan. Keep the quote as the anchor.",
                ));
                s.review.add(Comment::on(
                    find("Escape cancels"),
                    "Is Escape the only way out?",
                ));
                s.review.add(Comment {
                    kind: Kind::Remove,
                    range: Some(find("Sharing a review by link")),
                    body: String::new(),
                });
                s.selected = Some(1);
            }),
            ("composing", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("one general comment").unwrap_or(0);
                s.open(Target::New {
                    kind: Kind::Comment,
                    range: Some(at..at + "one general comment".len()),
                });
                s.draft = TextEdit::new("Should the general comment go first in the feedback?");
                s.draft.focused = true;
            }),
            ("panel", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("stores the quote").unwrap_or(0);
                s.review
                    .add(Comment::general("Ship the first version small."));
                s.review
                    .add(Comment::on(at..at + 16, "Anchor on the quote."));
                s.review.add(Comment {
                    kind: Kind::Remove,
                    range: Some(at + 20..at + 30),
                    body: String::new(),
                });
                s.list = Some(1);
            }),
            ("visual", |s| {
                let text = s.page.text().to_owned();
                for c in "jjwwvee".chars() {
                    s.vim.key(
                        &KeyEvent {
                            key: Key::Text(c.to_string()),
                            mods: rune::Mods::default(),
                        },
                        &text,
                    );
                }
            }),
            ("keys", |s| s.all_keys = true),
            ("yank", |s| {
                let text = s.page.text().to_owned();
                for c in "gy".chars() {
                    s.vim.key(
                        &KeyEvent {
                            key: Key::Text(c.to_string()),
                            mods: rune::Mods::default(),
                        },
                        &text,
                    );
                }
            }),
            ("prefix", |s| {
                s.review.add(Comment::on(0..4, "x"));
                let text = s.page.text().to_owned();
                s.vim.key(
                    &KeyEvent {
                        key: Key::Text("c".into()),
                        mods: rune::Mods::default(),
                    },
                    &text,
                );
            }),
        ],
        on_key: Some(on_key),
        families: ("Inter", "ui-monospace, Menlo, Inter"),
        ..AppSpec::new("Review", (1000.0, 900.0), App_::default, view)
    })
    .on_quit(send)
    .run();
    if SEND_FAILED.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
