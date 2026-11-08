//! Review a Markdown document in a terminal pane, the way a pull request is reviewed: select
//! text with Vim's keys, comment on it, and the comment shows under its block as a thread does on
//! GitHub. `:wq` writes the review out as Markdown feedback, for the agent that wrote the
//! document (or anyone).
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p rex-review -- PLAN.md            # feedback to stdout
//! RUNE_TERMINAL=1 cargo run --release -p rex-review -- PLAN.md -o out.md  # or to a file
//! some-agent | RUNE_TERMINAL=1 rex-review -                               # read stdin
//! ```
//!
//! Read with `rex-vim`'s keys. Select with `v` or `V`, then `c` comments on the selection, `d`
//! marks it to be removed, and `1`–`9` give it a label ("Clarify this", "Verify this", …); `cc`
//! and `dd` do the same for the cursor's line. `C` adds a general comment, about the document
//! as a whole. `]c` and `[c` step through the comments; on one, Enter or `ce` edits it and `dc`
//! deletes it. `Tab` lists every comment beside the page and `:h` shows every key.
//!
//! In a comment, Enter saves (Shift-Enter starts a new line) and Escape cancels. `:wq`, `:x`
//! or `ZZ` quit and send the review; `:q` quits when there's nothing to send, `:q!` or `ZQ`
//! quit without it. Ctrl-C quits without sending too.

mod page;
mod review;

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::ops::Range;
use std::sync::OnceLock;

use rex_ui::{Badge, Button, Rex, Tone, icons, kbd};
use rex_vim::{Vim, VimEffect, VimMode};
use rune::{App, AppSpec, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, TextStyle, Tx, col, el, row, text, text_input};
use rune_kit::markdown::Highlight;
use rune_kit::{Markdown, MarkdownStyle};

use page::Page;
use review::{Comment, Kind, LABELS, Review};

const SAMPLE: &str = include_str!("sample.md");

/// The document and where its review goes, read from the command line before the terminal is
/// taken over, so a bad path is an error in the shell rather than a page.
struct Input {
    title: String,
    source: String,
    out: Option<String>,
}

static INPUT: OnceLock<Input> = OnceLock::new();

/// What the composer is writing.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A new comment on this range of the page's text, or with `None`, a general one.
    New { kind: Kind, range: Option<Range<usize>> },
    /// Comment `n`, again.
    Edit(usize),
}

/// Keys after the first of a two-key command (`]c`, `ce`, `ZZ`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    Bracket(bool),
    C,
    D,
    Z,
}

/// What shows beside the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    None,
    Comments,
    Help,
}

struct App_ {
    title: String,
    /// Where the feedback goes: a file, or stdout when `None`.
    out: Option<String>,
    source: String,
    page: Page,
    vim: Vim,
    review: Review,
    /// The comment being written. Its text is `draft`, kept while the composer is closed too:
    /// Rune reaches a field's `TextEdit` once more after it goes, to clear its focus.
    composer: Option<Target>,
    draft: TextEdit,
    /// The comment `]c`, `[c` or a click went to, until the cursor moves; it tells comments
    /// apart that start at the same place, and it's how a general comment is picked.
    selected: Option<usize>,
    prefix: Option<Prefix>,
    /// The `:` command being typed.
    cmdline: Option<String>,
    side: Side,
    /// Quitting sends the review: `ZZ`, `:wq` or `:x`. Any other way out sends nothing.
    send: bool,
    note: Option<String>,
}

impl Default for App_ {
    fn default() -> Self {
        let input = INPUT.get_or_init(|| Input { title: "sample.md".into(), source: SAMPLE.into(), out: None });
        App_::new(input.title.clone(), input.source.clone(), input.out.clone())
    }
}

impl App_ {
    fn new(title: String, source: String, out: Option<String>) -> Self {
        App_ {
            title,
            out,
            page: Page::new(&source),
            source,
            vim: Vim::new().skip_blank_lines(true),
            review: Review::default(),
            composer: None,
            draft: TextEdit::default(),
            selected: None,
            prefix: None,
            cmdline: None,
            side: Side::None,
            send: false,
            note: None,
        }
    }

    fn cursor(&self) -> usize {
        self.vim.cursor(self.page.text()).start
    }

    /// What `c`, `d` and the labels act on: the selection, or the cursor's line.
    fn target_range(&self) -> Range<usize> {
        let text = self.page.text();
        if let Some(sel) = self.vim.selection(text) {
            return sel;
        }
        let at = self.cursor();
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        start..end
    }

    /// The comment Enter, `ce` and `dc` act on: the one stepped to, or the one under the
    /// cursor.
    fn current(&self) -> Option<usize> {
        self.selected.filter(|n| *n < self.review.comments.len()).or_else(|| self.review.at(self.cursor()))
    }

    fn open(&mut self, target: Target) {
        // Like a Vim operator, commenting ends the selection: the cursor goes to its start.
        if let Target::New { range: Some(range), .. } = &target {
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
        let Some(target) = self.composer.take() else { return };
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

    /// Adds a comment that needs no words: a removal or a label. Enter adds words to it.
    fn quick(&mut self, kind: Kind) {
        let range = self.target_range();
        self.vim.set_cursor(range.start, self.page.text());
        let n = self.review.add(Comment { kind, range: Some(range), body: String::new() });
        self.selected = Some(n);
        self.note = Some(format!("{} added — Enter adds a note", self.review.comments[n].title()));
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
        let Some(c) = self.review.comments.get(n) else { return };
        if let Some(range) = &c.range {
            self.vim.set_cursor(range.start, self.page.text());
        }
        self.selected = Some(n);
        self.note = Some(format!("#{} of {} · Enter edits, dc deletes", n + 1, self.review.comments.len()));
        tx.scroll_into_view(thread_key(n));
    }

    /// The feedback, when quitting sends it.
    fn feedback(&self) -> Option<String> {
        if !self.send {
            return None;
        }
        self.review.feedback(&self.source, &self.page)
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
            "h" | "help" => self.side = if self.side == Side::Help { Side::None } else { Side::Help },
            "" => {}
            other => self.note = Some(format!("Not a command: {other}")),
        }
    }
}

/// The key a comment's card is drawn under, to scroll to.
fn thread_key(n: usize) -> String {
    format!("thread-{n}")
}

/// GitHub's reading width and type, in the theme's colours, as `rex-md` draws.
fn style(rex: &Rex) -> MarkdownStyle {
    let heading = |size: f32, line_height: f32| TextStyle { size, weight: 600.0, line_height, ..rex.ty.body };
    MarkdownStyle::from_kit(&rex.kit)
        .text(rex.text)
        .muted(rex.text_secondary)
        .link(rex.accent_text)
        .link_underline(rex.accent_text.alpha(0.4))
        .quote_rule(rex.separator)
        .rule(rex.separator)
        .code_block(rex.group, rex.separator)
        .inline_code_bg(rex.field)
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
        (Kind::Label(_), _) => Tone::Accent,
    }
}

fn tint(kind: Kind, rex: &Rex) -> rune::Color {
    match kind {
        Kind::Comment => rex.warning.alpha(0.22),
        Kind::Remove => rex.danger.alpha(0.22),
        Kind::Label(_) => rex.accent.alpha(0.18),
    }
}

/// What's painted on block `index`: comments' ranges, then search matches, the selection and
/// the cursor on top.
fn highlights(s: &App_, index: usize, rex: &Rex) -> Vec<(Vec<Range<usize>>, Highlight)> {
    let page = &s.page;
    let text = page.text();
    let local = |range: Range<usize>| page.local_ranges(index, &page.source_ranges(range));
    let mut out = Vec::new();
    let current = s.current();
    for (n, c) in s.review.comments.iter().enumerate() {
        let Some(range) = c.range.clone() else { continue };
        let mut h = Highlight::new().bg(tint(c.kind, rex));
        if current == Some(n) {
            h = h.ring(tint(c.kind, rex).alpha(0.9), 1.0);
        }
        out.push((local(range), h));
    }
    if let Some(Target::New { range: Some(range), kind }) = &s.composer {
        out.push((local(range.clone()), Highlight::new().bg(tint(*kind, rex))));
    }
    for range in s.vim.matches(text) {
        out.push((local(range), Highlight::new()));
    }
    if let Some(range) = s.vim.selection(text) {
        out.push((local(range), Highlight::new().bg(rex.accent_soft).fg(rex.text)));
    }
    if s.composer.is_none() {
        let cursor = local(s.vim.cursor(text));
        out.push((cursor, Highlight::new().bg(rex.accent).fg(rex.on_accent).key("vim-cursor")));
    }
    out.retain(|(ranges, _)| !ranges.is_empty());
    out
}

/// The comment box.
fn composer_view(draft: &TextEdit, title: &str, quote: Option<&str>, rex: &Rex) -> El {
    let field = text_input(draft, rex.ty.body)
        .key("composer-field")
        .w_full()
        .color(rex.text)
        .placeholder("Leave a comment", rex.text_tertiary)
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
        .build("composer-save", rex);
    let cancel = Button::new("Cancel")
        .small()
        .gray()
        .on_press_tx(|s: &mut App_, tx: &mut Tx| {
            s.composer = None;
            tx.blur();
        })
        .build("composer-cancel", rex);
    let mut card = col()
        .key("composer")
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(rex.radius.group)
        .fill(rex.group)
        .border(1.0, rex.accent.alpha(0.6))
        .child(text(title.to_owned()).font(rex.ty.strong).color(rex.text));
    if let Some(q) = quote {
        card = card.child(quote_view(q, rex));
    }
    card.child(
        el().w_full()
            .padding(8.0)
            .radius(rex.radius.field)
            .fill(rex.field)
            .child(rune::auto_grow(field, 2, 8)),
    )
    .child(
        row()
            .w_full()
            .gap(8.0)
            .center_y()
            .child(
                text("Enter saves · Shift-Enter new line · Esc cancels")
                    .font(rex.ty.caption)
                    .color(rex.text_tertiary)
                    .truncate()
                    .shrink(),
            )
            .child(el().grow(1.0))
            .child(cancel)
            .child(save),
    )
}

/// What a comment is about, a line or two of it.
fn quote_view(q: &str, rex: &Rex) -> El {
    row()
        .w_full()
        .gap(8.0)
        .child(el().w(2.0).h_full().fill(rex.separator))
        .child(
            text(q.trim().to_owned())
                .font(rex.ty.caption)
                .color(rex.text_secondary)
                .max_lines(2)
                .grow(1.0)
                .shrink(),
        )
}

/// A saved comment, as a card.
fn comment_view(n: usize, c: &Comment, current: bool, rex: &Rex) -> El {
    let edit = Button::icon_only(&icons::PENCIL, "Edit")
        .plain()
        .small()
        .on_press(move |s: &mut App_| {
            s.selected = Some(n);
            s.open(Target::Edit(n));
        })
        .build(&format!("edit-{n}"), rex);
    let delete = Button::icon_only(&icons::TRASH, "Delete")
        .plain()
        .small()
        .on_press(move |s: &mut App_| s.delete(n))
        .build(&format!("delete-{n}"), rex);
    let mut head = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(text(format!("#{}", n + 1)).font(rex.ty.mono).color(rex.text_tertiary))
        .child(Badge::new(c.title()).tone(tone(c)).build(rex))
        .child(el().grow(1.0));
    if current {
        head = head.child(text("Enter edit · dc delete").font(rex.ty.caption).color(rex.text_tertiary));
    }
    let mut card = col()
        .key(thread_key(n))
        .w_full()
        .gap(6.0)
        .px(12.0)
        .py(8.0)
        .radius(rex.radius.group)
        .fill(rex.group)
        .border(1.0, if current { rex.accent.alpha(0.7) } else { rex.separator })
        .child(head.child(edit).child(delete));
    if let Kind::Label(l) = c.kind
        && let Some(tip) = LABELS[l].tip
        && c.range.is_some()
    {
        card = card.child(text(tip).font(rex.ty.caption).color(rex.text_tertiary).wrap().w_full());
    }
    if !c.body.is_empty() {
        card = card.child(text(c.body.clone()).font(rex.ty.body).color(rex.text).wrap().w_full());
    }
    card
}

/// A thread: a column of cards with a rule down its left, as GitHub sets review comments off.
fn thread(cards: Vec<El>, rex: &Rex) -> El {
    let mut c = col().w_full().gap(8.0).pl(12.0);
    for card in cards {
        c = c.child(card);
    }
    row()
        .w_full()
        .mt(4.0)
        .mb(12.0)
        .child(el().w(2.0).h_full().fill(rex.accent.alpha(0.35)))
        .child(c.grow(1.0).min_w(0.0))
}

/// The comments that end in block `index`, and the composer when it's for one there.
fn block_thread(s: &App_, index: usize, rex: &Rex) -> Option<El> {
    let page = &s.page;
    let ends_here = |range: &Range<usize>| page.block_at(range.end.saturating_sub(1).max(range.start)) == index;
    let current = s.current();
    let mut cards = Vec::new();
    for (n, c) in s.review.comments.iter().enumerate() {
        if !c.range.as_ref().is_some_and(ends_here) {
            continue;
        }
        if s.composer == Some(Target::Edit(n)) {
            cards.push(composer_view(&s.draft, &format!("Edit #{} · {}", n + 1, c.title()), None, rex));
        } else {
            cards.push(comment_view(n, c, current == Some(n), rex));
        }
    }
    if let Some(Target::New { kind, range: Some(range) }) = &s.composer
        && ends_here(range)
    {
        let quote = page.text().get(range.clone()).unwrap_or_default();
        let title = match kind {
            Kind::Comment => "New comment",
            Kind::Remove => "Remove this",
            Kind::Label(l) => LABELS[*l].text,
        };
        cards.push(composer_view(&s.draft, title, Some(quote), rex));
    }
    (!cards.is_empty()).then(|| thread(cards, rex))
}

/// The general comments, above the document, and the composer for a new one.
fn general_thread(s: &App_, rex: &Rex) -> Option<El> {
    let current = s.current();
    let mut cards = Vec::new();
    for (n, c) in s.review.comments.iter().enumerate() {
        if c.range.is_some() {
            continue;
        }
        if s.composer == Some(Target::Edit(n)) {
            cards.push(composer_view(&s.draft, &format!("Edit #{} · General", n + 1), None, rex));
        } else {
            cards.push(comment_view(n, c, current == Some(n), rex));
        }
    }
    if matches!(s.composer, Some(Target::New { range: None, .. })) {
        cards.push(composer_view(&s.draft, "General comment", None, rex));
    }
    (!cards.is_empty()).then(|| thread(cards, rex).mb(20.0))
}

/// Every comment beside the page, general ones first.
fn comments_view(s: &App_, rex: &Rex) -> El {
    let mut list = col().w_full().gap(4.0);
    if s.review.comments.is_empty() {
        list = list.child(
            text("No comments yet. Select with v, then c; C comments on the whole document.")
                .font(rex.ty.caption)
                .color(rex.text_tertiary)
                .wrap()
                .w_full(),
        );
    }
    let current = s.current();
    for (n, c) in s.review.comments.iter().enumerate() {
        let quote = c
            .range
            .clone()
            .map(|r| s.page.text().get(r).unwrap_or_default().trim().replace('\n', " "));
        let mut item = col()
            .key(format!("panel-{n}"))
            .w_full()
            .gap(4.0)
            .padding(8.0)
            .radius(rex.radius.row)
            .fill(if current == Some(n) { rex.accent_soft } else { rune::Color::TRANSPARENT })
            .on_press_tx(move |s: &mut App_, tx: &mut Tx| s.jump(n, tx))
            .child(
                row()
                    .w_full()
                    .gap(6.0)
                    .center_y()
                    .child(text(format!("#{}", n + 1)).font(rex.ty.mono).color(rex.text_tertiary))
                    .child(Badge::new(c.title()).tone(tone(c)).build(rex)),
            );
        if let Some(q) = quote {
            item = item.child(quote_view(&q, rex));
        }
        if !c.body.is_empty() {
            item = item.child(text(c.body.clone()).font(rex.ty.body).color(rex.text).max_lines(3).w_full());
        }
        list = list.child(item);
    }
    side_panel(&format!("Comments · {}", s.review.comments.len()), list, rex)
}

/// Every key, and the labels on the number keys.
fn help_view(rex: &Rex) -> El {
    let line = |keys: &str, what: &str| {
        row()
            .w_full()
            .gap(10.0)
            .center_y()
            .child(el().w(64.0).child(kbd(keys, rex)))
            .child(text(what.to_owned()).font(rex.ty.caption).color(rex.text_secondary).shrink())
    };
    let heading = |t: &str| text(t.to_owned()).font(rex.ty.label).color(rex.text_tertiary).mt(8.0);
    let mut c = col()
        .w_full()
        .gap(6.0)
        .child(heading("COMMENT"))
        .child(line("v V", "select, then:"))
        .child(line("c", "comment on it"))
        .child(line("d", "mark it to remove"))
        .child(line("1–9", "label it"))
        .child(line("cc dd", "the same for the line"))
        .child(line("C", "general comment"))
        .child(heading("COMMENTS"))
        .child(line("]c [c", "next, previous"))
        .child(line("Enter ce", "edit"))
        .child(line("dc", "delete"))
        .child(line("Tab", "list them"))
        .child(heading("QUIT"))
        .child(line(":wq ZZ", "send the review"))
        .child(line(":q", "quit when there's nothing"))
        .child(line(":q! ZQ", "quit without sending"))
        .child(heading("LABELS"));
    for (i, l) in LABELS.iter().enumerate() {
        c = c.child(line(&(i + 1).to_string(), l.text));
    }
    side_panel("Keys", c, rex)
}

fn side_panel(title: &str, body: El, rex: &Rex) -> El {
    col()
        .key("panel")
        .w(300.0)
        .h_full()
        .shrink()
        .min_w(0.0)
        .padding(12.0)
        .gap(10.0)
        .fill(rex.group)
        .child(text(title.to_owned()).font(rex.ty.strong).color(rex.text))
        .child(
            col()
                .key("panel-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(body)
                .scroll(ScrollSpec { stick_bottom: false }),
        )
}

fn view(s: &App_, cx: &mut Cx<App_>) -> El {
    let rex = Rex::current(cx.system_dark());
    let md_style = style(&rex);
    let mut doc = col().w_full().max_w(880.0).self_center();
    if let Some(general) = general_thread(s, &rex) {
        doc = doc.child(general);
    }
    for (i, block) in s.page.blocks.iter().enumerate() {
        let md = Markdown::new(block.markdown.as_str())
            .style(md_style.clone())
            .highlights(highlights(s, i, &rex))
            .tasks_read_only()
            .build(&rex.kit);
        doc = doc.child(el().key(format!("block-{i}")).w_full().mt(block.top).child(md));
        if let Some(thread) = block_thread(s, i, &rex) {
            doc = doc.child(thread);
        }
    }
    let content = col()
        .key("page")
        .w_full()
        .h_full()
        .padding(24.0)
        .child(doc)
        .scroll(ScrollSpec { stick_bottom: false });
    let mut body = row().w_full().grow(1.0).min_h(0.0).child(content.grow(1.0).min_w(0.0));
    let side = match s.side {
        Side::None => None,
        Side::Comments => Some(comments_view(s, &rex)),
        Side::Help => Some(help_view(&rex)),
    };
    if let Some(side) = side {
        body = body.child(el().w(1.0).h_full().fill(rex.separator)).child(side);
    }
    let mut page = col()
        .w_full()
        .h_full()
        .fill(rex.background)
        .child(bar(s, &rex))
        .child(el().h(1.0).w_full().fill(rex.separator))
        .child(body);
    if matches!(s.vim.mode(), VimMode::Visual | VimMode::VisualLine) && s.composer.is_none() {
        page = page.child(legend(&rex));
    }
    page.child(status_line(s, &rex))
}

/// What a selection can become, while there is one.
fn legend(rex: &Rex) -> El {
    let item = |k: String, what: &str| {
        row()
            .gap(4.0)
            .center_y()
            .child(kbd(k, rex))
            .child(text(what.to_owned()).font(rex.ty.caption).color(rex.text_secondary))
    };
    let mut r = row()
        .key("legend")
        .w_full()
        .flex_wrap()
        .gap(12.0)
        .px(16.0)
        .py(6.0)
        .fill(rex.group)
        .child(item("c".into(), "comment"))
        .child(item("d".into(), "remove"));
    for (i, l) in LABELS.iter().enumerate() {
        r = r.child(item((i + 1).to_string(), l.text));
    }
    col().w_full().child(el().h(1.0).w_full().fill(rex.separator)).child(r)
}

fn bar(s: &App_, rex: &Rex) -> El {
    let hint = |k: &str, what: &str| {
        row()
            .gap(4.0)
            .center_y()
            .child(kbd(k, rex))
            .child(text(what.to_owned()).font(rex.ty.caption).color(rex.text_tertiary))
    };
    row()
        .key("bar")
        .w_full()
        .min_h(40.0)
        .px(20.0)
        .py(6.0)
        .gap(10.0)
        .center_y()
        .child(rune::icon(&icons::FILE_TEXT, 16.0).color(rex.text_secondary))
        .child(text(s.title.clone()).font(rex.ty.strong).color(rex.text).truncate().shrink())
        .child(el().grow(1.0))
        .child(hint("v", "select"))
        .child(hint("C", "general"))
        .child(hint("Tab", "comments"))
        .child(hint(":wq", "send"))
        .child(hint(":q!", "quit"))
        .child(hint(":h", "keys"))
}

fn status_line(s: &App_, rex: &Rex) -> El {
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
        Some(t) => text(t).font(rex.ty.mono).color(rex.text),
        None => {
            let (label, tone) = if s.composer.is_some() {
                ("COMMENT".to_owned(), Tone::Warning)
            } else if s.vim.mode() == VimMode::Normal {
                (s.vim.status(), Tone::Neutral)
            } else {
                (s.vim.status(), Tone::Accent)
            };
            el().child(Badge::new(label).tone(tone).build(rex))
        }
    };
    let count = s.review.comments.len();
    let summary = format!("{count} comment{}", if count == 1 { "" } else { "s" });
    let note = s.note.clone().or_else(|| s.vim.message().map(str::to_owned)).unwrap_or_default();
    let pending = match s.prefix {
        Some(Prefix::Bracket(true)) => "]".to_owned(),
        Some(Prefix::Bracket(false)) => "[".to_owned(),
        Some(Prefix::C) => "c".to_owned(),
        Some(Prefix::D) => "d".to_owned(),
        Some(Prefix::Z) => "Z".to_owned(),
        None => s.vim.pending(),
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
        .child(text(note).font(rex.ty.caption).color(rex.text_secondary).truncate().shrink())
        .child(el().grow(1.0))
        .child(text(summary).font(rex.ty.caption).color(rex.text_secondary))
        .child(text(pending).font(rex.ty.mono).color(rex.text_secondary))
        .child(text(format!("{line}:{lines}")).font(rex.ty.mono).color(rex.text_tertiary))
}

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
    let Some(cmd) = s.cmdline.as_mut() else { return };
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
    let c = ch(k);
    let searching = s.vim.mode() == VimMode::Search;
    let visual = matches!(s.vim.mode(), VimMode::Visual | VimMode::VisualLine);
    // A command Vim is in the middle of (`yi`, `f`, `2`) has its next key.
    let vim_busy = !s.vim.pending().is_empty();

    if let Some(prefix) = s.prefix.take() {
        match (prefix, c) {
            (Prefix::Bracket(forward), Some('c')) => match s.review.step(s.current(), s.cursor(), !forward) {
                Some(n) => s.jump(n, tx),
                None => s.note = Some("No comments".into()),
            },
            (Prefix::C, Some('e')) => match s.current() {
                Some(n) => s.open(Target::Edit(n)),
                None => s.note = Some("No comment here".into()),
            },
            (Prefix::C, Some('c')) => s.open(Target::New { kind: Kind::Comment, range: Some(s.target_range()) }),
            (Prefix::D, Some('c')) => match s.current() {
                Some(n) => s.delete(n),
                None => s.note = Some("No comment here".into()),
            },
            (Prefix::D, Some('d')) => s.quick(Kind::Remove),
            (Prefix::Z, Some('Z')) => s.command("wq", tx),
            (Prefix::Z, Some('Q')) => s.command("q!", tx),
            _ => {}
        }
        return true;
    }

    if searching || vim_busy {
        return vim_key(s, k, tx);
    }
    if k.key == Key::Tab {
        s.side = if s.side == Side::Comments { Side::None } else { Side::Comments };
        return true;
    }
    if k.key == Key::Enter && !visual {
        match s.current() {
            Some(n) => s.open(Target::Edit(n)),
            None => s.note = Some("No comment here: v selects, c comments".into()),
        }
        return true;
    }
    let Some(c) = c else { return vim_key(s, k, tx) };
    match c {
        'c' if visual => s.open(Target::New { kind: Kind::Comment, range: Some(s.target_range()) }),
        'd' if visual => s.quick(Kind::Remove),
        '1'..='9' if visual => s.quick(Kind::Label(c as usize - '1' as usize)),
        'c' => s.prefix = Some(Prefix::C),
        'd' => s.prefix = Some(Prefix::D),
        'C' => s.open(Target::New { kind: Kind::Comment, range: None }),
        ']' | '[' => s.prefix = Some(Prefix::Bracket(c == ']')),
        'Z' => s.prefix = Some(Prefix::Z),
        ':' => s.cmdline = Some(String::new()),
        _ => return vim_key(s, k, tx),
    }
    true
}

fn vim_key(s: &mut App_, k: &KeyEvent, tx: &mut Tx) -> bool {
    match s.vim.key(k, s.page.text()) {
        Some(VimEffect::Moved) => {
            s.selected = None;
            tx.scroll_into_view("vim-cursor");
            true
        }
        Some(VimEffect::Copy(yanked)) => {
            tx.copy(yanked);
            true
        }
        Some(VimEffect::CopySource { range, linewise }) => {
            let ranges = s.page.source_ranges(range);
            let mut md = page::source_of(&s.source, &ranges, linewise);
            if linewise {
                md.push('\n');
            }
            tx.copy(md);
            true
        }
        None => false,
    }
}

/// Sends the review on the way out, when `:wq` or `ZZ` asked: to the file `-o` names, or to
/// stdout once the terminal is the shell's again.
fn send(s: &mut App_, _: &mut Tx) {
    if s.send && s.composer.is_some() {
        s.save();
    }
    let Some(feedback) = s.feedback() else { return };
    match &s.out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &feedback) {
                eprintln!("rex-review: could not write {path}: {e}");
            }
        }
        None => {
            let mut out = std::io::stdout();
            if out.write_all(feedback.as_bytes()).and_then(|()| out.flush()).is_err() {
                eprintln!("rex-review: could not write the review to stdout");
            }
        }
    }
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
    Input { title, source, out }
}

fn fail(message: &str) -> ! {
    eprintln!("rex-review: {message}");
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
                s.review.add(Comment::general("Good direction overall. Keep the first version small."));
                s.review.add(Comment::on(
                    find("stores the quote and its lines"),
                    "Lines drift when the agent rewrites the plan. Keep the quote as the anchor.",
                ));
                s.review.add(Comment { kind: Kind::Label(1), range: Some(find("Escape cancels")), body: String::new() });
                s.review.add(Comment { kind: Kind::Remove, range: Some(find("Sharing a review by link")), body: String::new() });
                s.selected = Some(1);
            }),
            ("composing", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("one general comment").unwrap_or(0);
                s.open(Target::New { kind: Kind::Comment, range: Some(at..at + "one general comment".len()) });
                s.draft = TextEdit::new("Should the general comment go first in the feedback?");
                s.draft.focused = true;
            }),
            ("panel", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("stores the quote").unwrap_or(0);
                s.review.add(Comment::general("Ship the first version without labels."));
                s.review.add(Comment::on(at..at + 16, "Anchor on the quote."));
                s.review.add(Comment { kind: Kind::Label(0), range: Some(at + 20..at + 30), body: String::new() });
                s.side = Side::Comments;
            }),
            ("visual", |s| {
                let text = s.page.text().to_owned();
                for c in "jjwwvee".chars() {
                    s.vim.key(&KeyEvent { key: Key::Text(c.to_string()), mods: rune::Mods::default() }, &text);
                }
            }),
            ("help", |s| s.side = Side::Help),
        ],
        on_key: Some(on_key),
        families: ("Inter", "ui-monospace, Menlo, Inter"),
        ..AppSpec::new("Review", (1000.0, 900.0), App_::default, view)
    })
    .on_quit(send)
    .run();
}

#[cfg(test)]
mod tests;
