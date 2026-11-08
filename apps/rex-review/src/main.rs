//! Review a Markdown document in a terminal pane, the way a pull request is reviewed: select
//! text with Vim's keys, comment on it, and the comment shows under its block as a thread does on
//! GitHub. Quitting writes the review out as Markdown feedback, for the agent that wrote the
//! document (or anyone).
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p rex-review -- PLAN.md            # feedback to stdout
//! RUNE_TERMINAL=1 cargo run --release -p rex-review -- PLAN.md -o out.md  # or to a file
//! some-agent | RUNE_TERMINAL=1 rex-review -                               # read stdin
//! ```
//!
//! Read with `rex-vim`'s keys. Select with `v` or `V`, then:
//!
//! - `c` comments on the selection (in Normal mode, on the cursor's line), `d` marks it to be
//!   removed, `1`–`9` give it a label ("Clarify this", "Verify this", …);
//! - `C` writes the general comment, about the document as a whole;
//! - `]c` and `[c` go to the next and previous comment; on one, `ce` edits it and `dc` deletes it;
//! - `Tab` shows every comment in a side panel.
//!
//! In a comment, Enter saves (Shift-Enter starts a new line) and Escape cancels. `ZZ` quits
//! and sends the review, `ZQ` quits without it; Ctrl-C quits and sends it too.

mod page;
mod review;

use std::io::{Read as _, Write as _};
use std::ops::Range;

use rex_ui::{Badge, Button, Rex, Tone, icons, kbd};
use rex_vim::{Vim, VimEffect, VimMode};
use rune::{App, AppSpec, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, TextStyle, Tx, col, el, row, text, text_input};
use rune_kit::markdown::Highlight;
use rune_kit::{Markdown, MarkdownStyle};

use page::Page;
use review::{Comment, Kind, LABELS, Review};

const SAMPLE: &str = include_str!("sample.md");

/// What the composer is writing.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A new comment on this range of the page's text.
    New { kind: Kind, range: Range<usize> },
    /// Comment `n`, again.
    Edit(usize),
    /// The general comment.
    General,
}

/// The comment being written; its text is [`App_::draft`].
struct Composer {
    target: Target,
}

/// Keys after the first of a two-key command (`]c`, `ce`, `ZZ`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    Bracket(bool),
    C,
    D,
    Z,
}

struct App_ {
    title: String,
    /// Where the feedback goes: a file, or stdout when `None`.
    out: Option<String>,
    source: String,
    page: Page,
    vim: Vim,
    review: Review,
    composer: Option<Composer>,
    /// The composer's text. Kept while the composer is closed too: Rune reaches a field's
    /// `TextEdit` once more after it goes (to clear its focus).
    draft: TextEdit,
    prefix: Option<Prefix>,
    panel: bool,
    /// The comment the panel's cursor is on.
    panel_at: usize,
    /// `ZQ`: quit without sending.
    discard: bool,
    note: Option<String>,
}

impl Default for App_ {
    fn default() -> Self {
        let args = rune::extra_args();
        let mut path = None;
        let mut out = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-o" | "--output" => out = it.next().cloned(),
                _ => path = Some(a.clone()),
            }
        }
        let (title, source) = match path.as_deref() {
            Some("-") => ("stdin".to_owned(), read_stdin()),
            Some(p) => (
                p.to_owned(),
                std::fs::read_to_string(p).unwrap_or_else(|e| format!("Could not read `{p}`: {e}\n")),
            ),
            None => ("sample.md".to_owned(), SAMPLE.to_owned()),
        };
        App_::new(title, source, out)
    }
}

fn read_stdin() -> String {
    let mut s = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut s) {
        s = format!("Could not read stdin: {e}\n");
    }
    s
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
            prefix: None,
            panel: false,
            panel_at: 0,
            discard: false,
            note: None,
        }
    }

    /// What `c`, `d` and the labels act on: the selection, or the cursor's line.
    fn target_range(&self) -> Range<usize> {
        let text = self.page.text();
        if let Some(sel) = self.vim.selection(text) {
            return sel;
        }
        let at = self.vim.cursor(text).start;
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        start..end
    }

    fn open(&mut self, target: Target) {
        // Like a Vim operator, commenting ends the selection: the cursor goes to its start.
        if let Target::New { range, .. } = &target {
            self.vim.set_cursor(range.start, self.page.text());
        }
        let body = match &target {
            Target::Edit(n) => self.review.comments[*n].body.clone(),
            Target::General => self.review.general.clone(),
            Target::New { .. } => String::new(),
        };
        self.draft = TextEdit::new(body);
        self.draft.focused = true;
        self.composer = Some(Composer { target });
        self.note = None;
    }

    /// Saves what the composer holds. A new comment in words with nothing written is dropped.
    fn save(&mut self) {
        let Some(composer) = self.composer.take() else { return };
        let body = self.draft.text.trim().to_owned();
        match composer.target {
            Target::New { kind, range } => {
                if kind == Kind::Comment && body.is_empty() {
                    return;
                }
                let n = self.review.add(Comment { kind, range, body });
                self.panel_at = n;
                self.note = Some(format!("Comment {} saved", n + 1));
            }
            Target::Edit(n) => {
                let c = &mut self.review.comments[n];
                if c.kind == Kind::Comment && body.is_empty() {
                    self.review.comments.remove(n);
                    self.note = Some("Comment deleted".into());
                } else {
                    c.body = body;
                }
            }
            Target::General => self.review.general = body,
        }
    }

    /// Adds a comment that needs no words (a removal or a label), with a composer open on it
    /// for any words to go with it.
    fn quick(&mut self, kind: Kind) {
        let range = self.target_range();
        let n = self.review.add(Comment { kind, range: range.clone(), body: String::new() });
        self.vim.set_cursor(range.start, self.page.text());
        self.panel_at = n;
        self.note = Some(format!("{} — c e to add a note", self.review.comments[n].title()));
    }

    fn delete(&mut self, n: usize) {
        if n < self.review.comments.len() {
            self.review.comments.remove(n);
            self.panel_at = self.panel_at.min(self.review.comments.len().saturating_sub(1));
            self.note = Some("Comment deleted".into());
        }
    }

    fn jump(&mut self, n: usize, tx: &mut Tx) {
        let Some(c) = self.review.comments.get(n) else { return };
        self.vim.set_cursor(c.range.start, self.page.text());
        self.panel_at = n;
        tx.scroll_into_view(thread_key(n));
    }

    /// The comment under the cursor.
    fn here(&self) -> Option<usize> {
        self.review.at(self.vim.cursor(self.page.text()).start)
    }

    /// The feedback, when the review should go out.
    fn feedback(&self) -> Option<String> {
        if self.discard {
            return None;
        }
        self.review.feedback(&self.source, &self.page)
    }
}

/// The key a comment's thread is drawn under, to scroll to.
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

/// The tint a comment's text gets.
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
    let here = s.here();
    for (n, c) in s.review.comments.iter().enumerate() {
        let mut h = Highlight::new().bg(tint(c.kind, rex));
        if here == Some(n) {
            h = h.ring(tint(c.kind, rex).alpha(0.9), 1.0);
        }
        out.push((local(c.range.clone()), h));
    }
    if let Some(Composer { target: Target::New { range, kind }, .. }) = &s.composer {
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

/// The comment box, open under the block it's about.
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
    let save = Button::new("Comment")
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
        .child(text(title).font(rex.ty.strong).color(rex.text));
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
            .child(text("Enter saves · Shift-Enter new line · Esc cancels").font(rex.ty.caption).color(rex.text_tertiary).truncate().shrink())
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
        .child(text(q.trim().to_owned()).font(rex.ty.caption).color(rex.text_secondary).max_lines(2).shrink())
}

/// A saved comment, as a card in its thread.
fn comment_view(n: usize, c: &Comment, current: bool, rex: &Rex) -> El {
    let tone = match c.kind {
        Kind::Comment => Tone::Warning,
        Kind::Remove => Tone::Danger,
        Kind::Label(_) => Tone::Accent,
    };
    let edit = Button::icon_only(&icons::PENCIL, "Edit")
        .plain()
        .small()
        .on_press(move |s: &mut App_| s.open(Target::Edit(n)))
        .build(&format!("edit-{n}"), rex);
    let delete = Button::icon_only(&icons::TRASH, "Delete")
        .plain()
        .small()
        .on_press(move |s: &mut App_| s.delete(n))
        .build(&format!("delete-{n}"), rex);
    let mut card = col()
        .key(thread_key(n))
        .w_full()
        .gap(6.0)
        .px(12.0)
        .py(8.0)
        .radius(rex.radius.group)
        .fill(rex.group)
        .border(1.0, if current { rex.accent.alpha(0.7) } else { rex.separator })
        .hover_group()
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(text(format!("#{}", n + 1)).font(rex.ty.mono).color(rex.text_tertiary))
                .child(Badge::new(c.title()).tone(tone).build(rex))
                .child(el().grow(1.0))
                .child(row().gap(2.0).group_opacity(0.0, 1.0).child(edit).child(delete)),
        );
    if let Kind::Label(l) = c.kind
        && let Some(tip) = LABELS[l].tip
        && c.body.is_empty()
    {
        card = card.child(text(tip).font(rex.ty.caption).color(rex.text_tertiary).w_full());
    }
    if !c.body.is_empty() {
        card = card.child(text(c.body.clone()).font(rex.ty.body).color(rex.text).w_full());
    }
    card
}

/// The comments that end in block `index`, and the composer when it's for one there.
fn threads(s: &App_, index: usize, rex: &Rex) -> Option<El> {
    let page = &s.page;
    let ends_here = |range: &Range<usize>| page.block_at(range.end.saturating_sub(1).max(range.start)) == index;
    let here = s.here();
    let mut col_ = col().w_full().gap(8.0).mt(4.0).mb(12.0).pl(12.0);
    let mut any = false;
    for (n, c) in s.review.comments.iter().enumerate() {
        if !ends_here(&c.range) {
            continue;
        }
        any = true;
        if s.composer.as_ref().is_some_and(|k| k.target == Target::Edit(n)) {
            col_ = col_.child(composer_view(&s.draft, &format!("Edit #{} · {}", n + 1, c.title()), None, rex));
        } else {
            col_ = col_.child(comment_view(n, c, here == Some(n), rex));
        }
    }
    if let Some(composer) = &s.composer
        && let Target::New { kind, range } = &composer.target
        && ends_here(range)
    {
        any = true;
        let quote = page.text().get(range.clone()).unwrap_or_default();
        let title = match kind {
            Kind::Comment => "New comment",
            Kind::Remove => "Remove this",
            Kind::Label(l) => LABELS[*l].text,
        };
        col_ = col_.child(composer_view(&s.draft, title, Some(quote), rex));
    }
    any.then(|| {
        row()
            .w_full()
            .child(el().w(2.0).h_full().fill(rex.accent.alpha(0.35)))
            .child(col_.grow(1.0).min_w(0.0))
    })
}

/// The general comment: a card at the top once written, or its composer.
fn general_view(s: &App_, rex: &Rex) -> Option<El> {
    if s.composer.as_ref().is_some_and(|c| c.target == Target::General) {
        return Some(composer_view(&s.draft, "General comment", None, rex).mb(16.0));
    }
    let body = s.review.general.trim();
    if body.is_empty() {
        return None;
    }
    let edit = Button::icon_only(&icons::PENCIL, "Edit")
        .plain()
        .small()
        .on_press(|s: &mut App_| s.open(Target::General))
        .build("general-edit", rex);
    Some(
        col()
            .key("general")
            .w_full()
            .gap(6.0)
            .px(12.0)
            .py(8.0)
            .mb(16.0)
            .radius(rex.radius.group)
            .fill(rex.group)
            .border(1.0, rex.separator)
            .child(
                row()
                    .w_full()
                    .gap(8.0)
                    .center_y()
                    .child(rune::icon(&icons::INBOX, 14.0).color(rex.text_secondary))
                    .child(text("General comment").font(rex.ty.strong).color(rex.text))
                    .child(el().grow(1.0))
                    .child(edit),
            )
            .child(text(body.to_owned()).font(rex.ty.body).color(rex.text).w_full()),
    )
}

/// Every comment, in document order, beside the page.
fn panel_view(s: &App_, rex: &Rex) -> El {
    let mut list = col().w_full().gap(6.0);
    if s.review.comments.is_empty() {
        list = list.child(text("No comments yet. Select with v, then c.").font(rex.ty.caption).color(rex.text_tertiary).w_full());
    }
    for (n, c) in s.review.comments.iter().enumerate() {
        let quote = s.page.text().get(c.range.clone()).unwrap_or_default().trim().replace('\n', " ");
        let current = n == s.panel_at;
        list = list.child(
            col()
                .key(format!("panel-{n}"))
                .w_full()
                .gap(4.0)
                .padding(8.0)
                .radius(rex.radius.row)
                .fill(if current { rex.accent_soft } else { rune::Color::TRANSPARENT })
                .on_press_tx(move |s: &mut App_, tx: &mut Tx| s.jump(n, tx))
                .child(
                    row()
                        .gap(6.0)
                        .center_y()
                        .child(text(format!("#{}", n + 1)).font(rex.ty.mono).color(rex.text_tertiary))
                        .child(text(c.title()).font(rex.ty.label).color(rex.text_secondary)),
                )
                .child(text(quote).font(rex.ty.caption).color(rex.text_secondary).max_lines(1).w_full())
                .child(text(if c.body.is_empty() { "—".to_owned() } else { c.body.clone() }).font(rex.ty.body).color(rex.text).max_lines(2).w_full()),
        );
    }
    col()
        .key("panel")
        .w(300.0)
        .h_full()
        .shrink()
        .min_w(0.0)
        .padding(12.0)
        .gap(10.0)
        .fill(rex.group)
        .child(text(format!("Comments · {}", s.review.comments.len())).font(rex.ty.strong).color(rex.text))
        .child(col().key("panel-scroll").w_full().grow(1.0).min_h(0.0).child(list).scroll(ScrollSpec { stick_bottom: false }))
}

fn view(s: &App_, cx: &mut Cx<App_>) -> El {
    let rex = Rex::current(cx.system_dark());
    let md_style = style(&rex);
    let mut doc = col().w_full().max_w(880.0).self_center();
    if let Some(general) = general_view(s, &rex) {
        doc = doc.child(general);
    }
    for (i, block) in s.page.blocks.iter().enumerate() {
        let md = Markdown::new(block.markdown.as_str())
            .style(md_style.clone())
            .highlights(highlights(s, i, &rex))
            .tasks_read_only()
            .build(&rex.kit);
        doc = doc.child(el().key(format!("block-{i}")).w_full().mt(block.top).child(md));
        if let Some(thread) = threads(s, i, &rex) {
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
    if s.panel {
        body = body.child(el().w(1.0).h_full().fill(rex.separator)).child(panel_view(s, &rex));
    }
    col()
        .w_full()
        .h_full()
        .fill(rex.background)
        .child(bar(s, &rex))
        .child(el().h(1.0).w_full().fill(rex.separator))
        .child(body)
        .child(status_line(s, &rex))
}

fn bar(s: &App_, rex: &Rex) -> El {
    let hint = |k: &str, what: &str| row().gap(4.0).center_y().child(kbd(k, rex)).child(text(what.to_owned()).font(rex.ty.caption).color(rex.text_tertiary));
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
        .child(hint("c", "comment"))
        .child(hint("C", "general"))
        .child(hint("Tab", "list"))
        .child(hint("ZZ", "send"))
}

fn status_line(s: &App_, rex: &Rex) -> El {
    let page_text = s.page.text();
    let line = page_text[..s.vim.cursor(page_text).start].matches('\n').count() + 1;
    let lines = page_text.matches('\n').count() + 1;
    let (label, tone) = if s.composer.is_some() {
        ("COMMENT".to_owned(), Tone::Warning)
    } else {
        let tone = if s.vim.mode() == VimMode::Normal { Tone::Neutral } else { Tone::Accent };
        (s.vim.status(), tone)
    };
    let mode = if s.vim.mode() == VimMode::Search && s.composer.is_none() {
        text(label).font(rex.ty.mono).color(rex.text)
    } else {
        el().child(Badge::new(label).tone(tone).build(rex))
    };
    let count = s.review.comments.len();
    let summary = format!(
        "{count} comment{}{}",
        if count == 1 { "" } else { "s" },
        if s.review.general.trim().is_empty() { "" } else { " · general ✓" }
    );
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

fn on_key(s: &mut App_, k: &KeyEvent, tx: &mut Tx) -> bool {
    // The composer's field has the keys while it's open; one that reaches here missed it.
    if s.composer.is_some() {
        return false;
    }
    s.note = None;
    let c = ch(k);
    let searching = s.vim.mode() == VimMode::Search;
    let visual = matches!(s.vim.mode(), VimMode::Visual | VimMode::VisualLine);
    // A command Vim is in the middle of (`yi`, `f`, `2`) has its next key.
    let vim_busy = !s.vim.pending().is_empty();

    if let Some(prefix) = s.prefix.take() {
        match (prefix, c) {
            (Prefix::Bracket(forward), Some('c')) => {
                let at = s.vim.cursor(s.page.text()).start;
                match s.review.next(at, !forward) {
                    Some(n) => s.jump(n, tx),
                    None => s.note = Some("No comments".into()),
                }
            }
            (Prefix::C, Some('e')) => match s.here() {
                Some(n) => s.open(Target::Edit(n)),
                None => s.note = Some("No comment here".into()),
            },
            (Prefix::C, Some('c')) => s.open(Target::New { kind: Kind::Comment, range: s.target_range() }),
            (Prefix::D, Some('c')) => match s.here() {
                Some(n) => s.delete(n),
                None => s.note = Some("No comment here".into()),
            },
            (Prefix::D, Some('d')) => s.quick(Kind::Remove),
            (Prefix::Z, Some('Z')) => tx.quit(),
            (Prefix::Z, Some('Q')) => {
                s.discard = true;
                tx.quit();
            }
            _ => {}
        }
        return true;
    }

    if k.key == Key::Tab && !searching {
        s.panel = !s.panel;
        return true;
    }
    if !searching && !vim_busy && let Some(c) = c {
        match c {
            'c' if visual => {
                s.open(Target::New { kind: Kind::Comment, range: s.target_range() });
                return true;
            }
            'd' if visual => {
                s.quick(Kind::Remove);
                return true;
            }
            '1'..='9' if visual => {
                s.quick(Kind::Label(c as usize - '1' as usize));
                return true;
            }
            'c' => {
                s.prefix = Some(Prefix::C);
                return true;
            }
            'd' => {
                s.prefix = Some(Prefix::D);
                return true;
            }
            'C' => {
                s.open(Target::General);
                return true;
            }
            ']' | '[' => {
                s.prefix = Some(Prefix::Bracket(c == ']'));
                return true;
            }
            'Z' => {
                s.prefix = Some(Prefix::Z);
                return true;
            }
            _ => {}
        }
    }
    match s.vim.key(k, s.page.text()) {
        Some(VimEffect::Moved) => {
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

/// Sends the review on the way out: to the file `-o` names, or to stdout once the terminal is
/// the shell's again.
fn send(s: &mut App_, _: &mut Tx) {
    if s.composer.is_some() {
        s.save();
    }
    let Some(feedback) = s.feedback() else { return };
    match &s.out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &feedback) {
                eprintln!("could not write {path}: {e}");
            }
        }
        None => {
            let mut out = std::io::stdout();
            if out.write_all(feedback.as_bytes()).and_then(|()| out.flush()).is_err() {
                eprintln!("could not write the review to stdout");
            }
        }
    }
}

fn main() {
    App::new(AppSpec::<App_> {
        scenes: &[
            ("comments", |s| {
                let text = s.page.text().to_owned();
                let find = |needle: &str| {
                    let at = text.find(needle).unwrap_or(0);
                    at..at + needle.len()
                };
                s.review.general = "Good direction overall. Keep the first version small.".into();
                s.review.add(Comment {
                    kind: Kind::Comment,
                    range: find("stores the quote and its lines"),
                    body: "Lines drift when the agent rewrites the plan. Keep the quote as the anchor.".into(),
                });
                s.review.add(Comment { kind: Kind::Label(1), range: find("Escape cancels"), body: String::new() });
                s.review.add(Comment { kind: Kind::Remove, range: find("Sharing a review by link"), body: String::new() });
            }),
            ("composing", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("one general comment").unwrap_or(0);
                s.open(Target::New { kind: Kind::Comment, range: at..at + "one general comment".len() });
                s.draft = TextEdit::new("Should the general comment go first in the feedback?");
                s.draft.focused = true;
            }),
            ("panel", |s| {
                let text = s.page.text().to_owned();
                let at = text.find("stores the quote").unwrap_or(0);
                s.review.add(Comment { kind: Kind::Comment, range: at..at + 16, body: "Anchor on the quote.".into() });
                s.review.add(Comment { kind: Kind::Label(0), range: at + 20..at + 30, body: String::new() });
                s.panel = true;
            }),
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
