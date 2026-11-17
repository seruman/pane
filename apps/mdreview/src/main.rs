//! Review Markdown with Vim keys and write feedback as Markdown.
//! Comments appear below the blocks they refer to.
//!
//! ```sh
//! RUNE_TERMINAL=1 cargo run --release -p mdreview -- README.md
//! RUNE_TERMINAL=1 cargo run --release -p mdreview -- README.md -o out.md
//! some-agent | RUNE_TERMINAL=1 mdreview -
//! ```
//!
//! `c` comments on a motion, text object or selection. `d` marks it for removal.
//! `cc` and `dd` act on the cursor's line. `C` adds a general comment.
//! `]c` and `[c` step through comments. Enter or `\e` edits a comment. `\x` deletes it.
//! Tab lists the comments. The key panel shows continuations for a prefix or selection.
//! `\?` shows every binding. `gx` opens the link under the cursor.
//!
//! The mouse works as in Vim: a click moves the cursor, a drag, double-click or triple-click
//! selects in Visual mode for `c`, `d` and `gy`, and a click on a link opens it.
//!
//! In the editor, Enter saves, Shift-Enter adds a line and Escape cancels.
//! `:wq`, `:x` and `ZZ` send the review and quit. `:w` and `\w` write it to the `-o` file and
//! keep reviewing. `:q` and `\q` quit when nothing is left unwritten. `:q!`, `ZQ` and Ctrl-C quit
//! without sending.

mod page;
mod review;

use std::cell::Cell;
use std::io::{IsTerminal as _, Write};
use std::ops::Range;
use std::rc::Rc;

use pane_ui::markdown::{self, Images, VIM_CURSOR, find_text, style, type_keys};
use pane_ui::{Badge, Button, Theme, Tone, icons};
use rune::{
    App, AppSpec, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input,
};
use rune_kit::Markdown;
use rune_kit::markdown::Highlight;
use vimkeys::{
    Hint, HintGroup, HintGroups, HintsFor, Keymap, Operator, Stroke, Table, Vim, VimEffect,
    VimMode, hint,
};

use page::Page;
use review::{Comment, CommentId, Kind, Review, Subject};

const SAMPLE: &str = include_str!("sample.md");

enum Origin {
    Sample,
    Stdin,
    File(String),
}

impl Origin {
    fn title(&self) -> &str {
        match self {
            Self::Sample => "sample.md",
            Self::Stdin => "stdin",
            Self::File(path) => path,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::File(path) => Some(path),
            Self::Sample | Self::Stdin => None,
        }
    }
}

struct Input {
    origin: Origin,
    source: String,
    out: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// `None` addresses the whole document.
    New {
        range: Option<Range<usize>>,
    },
    Edit(CommentId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Comment,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    General,
    NextComment,
    PreviousComment,
    Edit,
    Delete,
    List,
    Keys,
    CommandLine,
    SendAndQuit,
    QuitWithoutSending,
    OpenLink,
    Write,
    Quit,
}

fn keymap() -> Keymap<Act, Op> {
    use Stroke::{Char as C, Leader};
    Keymap::vim()
        .operator(&[C('g'), C('y')], Operator::YankSource, "Yank as Markdown")
        .operator(&[C('c')], Operator::App(Op::Comment), "Comment")
        .operator(&[C('d')], Operator::App(Op::Remove), "Remove this")
        .action(&[C('C')], Act::General, "General comment")
        .action(&[C(']'), C('c')], Act::NextComment, "Next comment")
        .action(&[C('['), C('c')], Act::PreviousComment, "Previous comment")
        .action(&[Stroke::Enter], Act::Edit, "Edit the comment")
        .action(&[Leader, C('e')], Act::Edit, None)
        .action(&[Leader, C('x')], Act::Delete, "Delete the comment")
        .action(&[Stroke::Tab], Act::List, "List the comments")
        .action(&[Leader, C('?')], Act::Keys, "Every key")
        .action(&[Leader, C('w')], Act::Write, "Write the review")
        .action(&[Leader, C('q')], Act::Quit, "Quit, once it's written")
        .action(&[C('g'), C('x')], Act::OpenLink, "Open the link")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quit {
    Send,
    Drop,
}

enum Command {
    Write,
    Quit,
    ForceQuit,
    SendAndQuit,
}

impl Command {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "w" | "write" => Some(Self::Write),
            "q" | "quit" => Some(Self::Quit),
            "q!" | "quit!" => Some(Self::ForceQuit),
            "wq" | "x" | "wq!" | "x!" => Some(Self::SendAndQuit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Normal,
    Composing(Target),
    Command(String),
    List(usize),
    AllKeys,
}

struct Reviewer {
    origin: Origin,
    /// `None` sends feedback to stdout.
    out: Option<String>,
    source: String,
    page: Page,
    vim: Vim<Act, Op>,
    review: Review,
    mode: Mode,
    // Rune clears focus through on_edit after the composer closes.
    draft: TextEdit,
    images: Images,
    /// Distinguishes comments sharing an anchor and selects general comments.
    selected: Option<CommentId>,
    quit: Quit,
    send_failed: Rc<Cell<bool>>,
    note: Option<String>,
    /// The review as `:w` last wrote it, so quitting knows whether anything is left unwritten.
    written: Option<String>,
}

impl Default for Reviewer {
    fn default() -> Self {
        Reviewer::new(Input {
            origin: Origin::Sample,
            source: SAMPLE.into(),
            out: None,
        })
    }
}

impl Reviewer {
    fn new(input: Input) -> Self {
        let Input {
            origin,
            source,
            out,
        } = input;
        let document = match &origin {
            Origin::File(path) => Some(std::path::Path::new(path)),
            Origin::Sample | Origin::Stdin => None,
        };
        Reviewer {
            images: Images::beside(document),
            origin,
            out,
            page: Page::new(&source),
            source,
            vim: Vim::new(keymap()).skip_blank_lines(true),
            review: Review::default(),
            mode: Mode::Normal,
            draft: TextEdit::default(),
            selected: None,
            quit: Quit::Drop,
            send_failed: Rc::new(Cell::new(false)),
            note: None,
            written: None,
        }
    }

    fn composer(&self) -> Option<&Target> {
        match &self.mode {
            Mode::Composing(target) => Some(target),
            _ => None,
        }
    }

    fn list(&self) -> Option<usize> {
        match self.mode {
            Mode::List(row) => Some(row),
            _ => None,
        }
    }

    fn cursor(&self) -> usize {
        self.vim.cursor(self.page.text()).start
    }

    fn current(&self) -> Option<CommentId> {
        if let Some(row) = self.list() {
            return self.review.at_position(row);
        }
        self.selected
            .filter(|id| self.review.get(*id).is_some())
            .or_else(|| self.review.at(self.cursor()))
    }

    /// Scrolls to the comment without moving Vim's cursor.
    fn show(&self, id: CommentId, tx: &mut Tx) {
        match self.review.get(id).map(|c| c.range().is_some()) {
            Some(true) => tx.scroll_center(CURRENT_MARK),
            Some(false) => tx.scroll_into_view(thread_key(id)),
            None => {}
        }
    }

    fn open(&mut self, target: Target) {
        if let Target::New {
            range: Some(range), ..
        } = &target
        {
            self.vim.set_cursor(range.start, self.page.text());
        }
        let body = match &target {
            Target::Edit(id) => match self.review.get(*id) {
                Some(c) => c.body.clone(),
                None => return,
            },
            Target::New { .. } => String::new(),
        };
        self.draft = TextEdit::new(body);
        self.draft.focused = true;
        self.mode = Mode::Composing(target);
        self.note = None;
    }

    /// Empty comments are discarded. Removals may have no body.
    fn save(&mut self) {
        let Some(target) = self.composer().cloned() else {
            return;
        };
        self.mode = Mode::Normal;
        let body = self.draft.text.trim().to_owned();
        match target {
            Target::New { range } => {
                if body.is_empty() {
                    return;
                }
                let subject = range.map_or(Subject::General, Subject::On);
                let id = self.review.add(Comment { subject, body });
                self.selected = Some(id);
                self.note = Some(format!("Comment #{} saved", self.number(id)));
            }
            Target::Edit(id) => {
                let Some(c) = self.review.get(id) else {
                    return;
                };
                if c.kind() == Kind::Comment && body.is_empty() {
                    self.delete(id);
                } else if self.review.set_body(id, body) {
                    self.note = Some(format!("Comment #{} saved", self.number(id)));
                }
            }
        }
    }

    fn remove(&mut self, range: Range<usize>) {
        self.vim.set_cursor(range.start, self.page.text());
        let id = self.review.add(Comment::remove(range));
        self.selected = Some(id);
        self.note = Some("Marked to remove. Enter adds a note.".into());
    }

    fn delete(&mut self, id: CommentId) {
        let number = self.number(id);
        if !self.review.remove(id) {
            return;
        }
        if self.composer() == Some(&Target::Edit(id)) {
            self.mode = Mode::Normal;
        } else if self.composer().is_some() {
            self.draft.focused = true;
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.note = Some(format!("Comment #{number} deleted"));
    }

    /// One-based display number.
    fn number(&self, id: CommentId) -> usize {
        self.review.position(id).map_or(0, |p| p + 1)
    }

    fn jump(&mut self, id: CommentId, tx: &mut Tx) {
        let Some(c) = self.review.get(id) else {
            return;
        };
        if let Some(range) = c.range() {
            self.vim.set_cursor(range.start, self.page.text());
        }
        self.selected = Some(id);
        self.note = Some(format!(
            "#{} of {} · Enter edits, \\x deletes",
            self.number(id),
            self.review.len()
        ));
        tx.scroll_into_view(thread_key(id));
    }

    fn open_list(&mut self, tx: &mut Tx) {
        let row = self
            .current()
            .and_then(|id| self.review.position(id))
            .unwrap_or(0);
        self.mode = Mode::List(row);
        tx.scroll_into_view(list_key(row));
        if let Some(id) = self.review.at_position(row) {
            self.show(id, tx);
        }
    }

    fn command(&mut self, cmd: &str, tx: &mut Tx) {
        let text = cmd.trim();
        if let Some(command) = Command::parse(text) {
            self.execute(command, tx);
        } else if !text.is_empty() {
            self.note = Some(format!("Not a command: {text}"));
        }
    }

    /// The review as it would be sent now, `None` when it's empty.
    fn feedback(&self) -> Option<String> {
        self.review
            .feedback(&self.source, &self.page, self.origin.name())
    }

    /// Whether quitting now would lose comments: the review isn't empty and isn't what `:w`
    /// last wrote.
    fn unwritten(&self) -> bool {
        self.feedback()
            .is_some_and(|f| self.written.as_ref() != Some(&f))
    }

    fn write(&mut self) {
        let Some(path) = self.out.clone() else {
            self.note =
                Some("No file to write to: start with -o FILE, or :wq sends to stdout".into());
            return;
        };
        let Some(feedback) = self.feedback() else {
            self.note = Some("Nothing to write yet".into());
            return;
        };
        match std::fs::write(&path, &feedback) {
            Ok(()) => {
                let n = self.review.len();
                self.note = Some(format!(
                    "Wrote {n} comment{} to {path}",
                    if n == 1 { "" } else { "s" }
                ));
                self.written = Some(feedback);
            }
            Err(e) => self.note = Some(format!("Could not write {path}: {e}")),
        }
    }

    fn execute(&mut self, command: Command, tx: &mut Tx) {
        match command {
            Command::Write => self.write(),
            Command::Quit if self.unwritten() => {
                self.note = Some(
                    "The review isn't written. \\w or :w writes it, :wq sends it, :q! drops it."
                        .into(),
                );
            }
            Command::Quit | Command::ForceQuit => {
                self.quit = Quit::Drop;
                tx.quit();
            }
            Command::SendAndQuit => {
                self.quit = Quit::Send;
                tx.quit();
            }
        }
    }
}

const CURRENT_MARK: &str = "current-comment";

fn thread_key(id: CommentId) -> String {
    format!("thread-{id}")
}

fn tone(c: &Comment) -> Tone {
    match c.subject {
        Subject::General => Tone::Neutral,
        Subject::On(_) => Tone::Warning,
        Subject::Remove(_) => Tone::Danger,
    }
}

fn tint(kind: Kind, theme: &Theme) -> rune::Color {
    match kind {
        Kind::Comment => theme.warning.alpha(0.22),
        Kind::Remove => theme.danger.alpha(0.22),
    }
}

/// A highlight's ranges in the whole source, and the block that carries its key.
type Mark = (Vec<Range<usize>>, Highlight, Option<usize>);

/// Every highlight of the page, mapped to the source once. Mapping walks all blocks, so
/// doing it per block made each keypress quadratic in the document's length.
fn marks(s: &Reviewer, theme: &Theme) -> Vec<Mark> {
    let page = &s.page;
    let text = page.text();
    let local = |range: Range<usize>| page.source_ranges(range);
    let mut out = Vec::new();
    let current = s.current();
    for (id, c) in s.review.iter() {
        let Some(range) = c.range().cloned() else {
            continue;
        };
        let mut h = Highlight::new().bg(tint(c.kind(), theme));
        let mut keyed = None;
        if current == Some(id) {
            h = h.ring(tint(c.kind(), theme).alpha(0.9), 1.0);
            keyed = Some(page.block_at(range.start));
        }
        out.push((local(range), h, keyed));
    }
    if let Some(Target::New { range: Some(range) }) = s.composer() {
        out.push((
            local(range.clone()),
            Highlight::new().bg(tint(Kind::Comment, theme)),
            None,
        ));
    }
    out.extend(
        markdown::vim_highlights(&s.vim, text, local, s.composer().is_none(), theme)
            .into_iter()
            .map(|(ranges, h)| (ranges, h, None)),
    );
    out.retain(|(ranges, ..)| !ranges.is_empty());
    out
}

/// The part of `marks` that falls in block `index`, in its own Markdown.
fn highlights(page: &Page, marks: &[Mark], index: usize) -> Vec<(Vec<Range<usize>>, Highlight)> {
    let mut out = Vec::new();
    for (ranges, h, keyed) in marks {
        let ranges = page.local_ranges(index, ranges);
        if ranges.is_empty() {
            continue;
        }
        let h = if *keyed == Some(index) {
            h.clone().key(CURRENT_MARK)
        } else {
            h.clone()
        };
        out.push((ranges, h));
    }
    out
}

fn composer_view(draft: &TextEdit, title: &str, quote: Option<&str>, theme: &Theme) -> El {
    let field = text_input(draft, theme.ty.body)
        .key("composer-field")
        .w_full()
        .color(theme.text)
        .placeholder("Leave a comment", theme.text_tertiary)
        .on_edit(|s: &mut Reviewer| &mut s.draft)
        .on_submit(|s: &mut Reviewer, tx: &mut Tx| {
            s.save();
            tx.blur();
        })
        .on_input_key(|s: &mut Reviewer, k: &KeyEvent, tx: &mut Tx| {
            if k.key != Key::Escape {
                return false;
            }
            s.mode = Mode::Normal;
            s.note = Some("Cancelled".into());
            tx.blur();
            true
        });
    let save = Button::new("Save")
        .small()
        .on_press_tx(|s: &mut Reviewer, tx: &mut Tx| {
            s.save();
            tx.blur();
        })
        .build("composer-save", theme);
    let cancel = Button::new("Cancel")
        .small()
        .gray()
        .on_press_tx(|s: &mut Reviewer, tx: &mut Tx| {
            s.mode = Mode::Normal;
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

fn comment_view(id: CommentId, number: usize, c: &Comment, current: bool, theme: &Theme) -> El {
    let edit = Button::icon_only(&icons::PENCIL, "Edit")
        .plain()
        .small()
        .on_press(move |s: &mut Reviewer| {
            s.selected = Some(id);
            s.open(Target::Edit(id));
        })
        .build(&format!("edit-{id}"), theme);
    let delete = Button::icon_only(&icons::TRASH, "Delete")
        .plain()
        .small()
        .on_press(move |s: &mut Reviewer| s.delete(id))
        .build(&format!("delete-{id}"), theme);
    let mut head = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(
            text(format!("#{number}"))
                .font(theme.ty.mono)
                .color(theme.text_tertiary),
        )
        .child(Badge::new(c.title()).tone(tone(c)).build(theme))
        .child(el().grow(1.0));
    if current {
        head = head.child(
            text("Enter edit · \\x delete")
                .font(theme.ty.caption)
                .color(theme.text_tertiary),
        );
    }
    let mut card = col()
        .key(thread_key(id))
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

fn thread_card(s: &Reviewer, id: CommentId, number: usize, c: &Comment, theme: &Theme) -> El {
    if s.composer() == Some(&Target::Edit(id)) {
        composer_view(
            &s.draft,
            &format!("Edit #{number} · {}", c.title()),
            None,
            theme,
        )
    } else {
        comment_view(id, number, c, s.current() == Some(id), theme)
    }
}

fn block_thread(s: &Reviewer, index: usize, theme: &Theme) -> Option<El> {
    let page = &s.page;
    let ends_here =
        |range: &Range<usize>| page.block_at(range.end.saturating_sub(1).max(range.start)) == index;
    let mut cards = Vec::new();
    for (number, (id, c)) in s.review.iter().enumerate() {
        if c.range().is_some_and(ends_here) {
            cards.push(thread_card(s, id, number + 1, c, theme));
        }
    }
    if let Some(Target::New { range: Some(range) }) = s.composer()
        && ends_here(range)
    {
        let quote = page.text().get(range.clone()).unwrap_or_default();
        cards.push(composer_view(&s.draft, "New comment", Some(quote), theme));
    }
    (!cards.is_empty()).then(|| thread(cards, theme))
}

fn general_thread(s: &Reviewer, theme: &Theme) -> Option<El> {
    let mut cards = Vec::new();
    for (number, (id, c)) in s.review.iter().enumerate() {
        if c.range().is_none() {
            cards.push(thread_card(s, id, number + 1, c, theme));
        }
    }
    if matches!(s.composer(), Some(Target::New { range: None })) {
        cards.push(composer_view(&s.draft, "General comment", None, theme));
    }
    (!cards.is_empty()).then(|| thread(cards, theme).mb(20.0))
}

fn list_view(s: &Reviewer, at: usize, theme: &Theme) -> El {
    let mut rows = col().w_full().gap(2.0);
    if s.review.is_empty() {
        rows = rows.child(
            text("No comments yet. Select with v, then c; C comments on the whole document.")
                .font(theme.ty.caption)
                .color(theme.text_tertiary)
                .wrap()
                .w_full(),
        );
    }
    for (n, (id, c)) in s.review.iter().enumerate() {
        rows = rows.child(list_item(s, n, id, c, n == at, theme));
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
            text(format!("Comments · {}", s.review.len()))
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

fn list_item(
    s: &Reviewer,
    n: usize,
    id: CommentId,
    comment: &Comment,
    selected: bool,
    theme: &Theme,
) -> El {
    let mut item = col()
        .key(list_key(n))
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(theme.radius.row)
        .fill(if selected {
            theme.accent_soft
        } else {
            rune::Color::TRANSPARENT
        })
        .on_press_tx(move |s: &mut Reviewer, tx: &mut Tx| {
            s.mode = Mode::Normal;
            s.jump(id, tx);
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
                .child(Badge::new(comment.title()).tone(tone(comment)).build(theme)),
        );
    if let Some(range) = comment.range() {
        let quote = s
            .page
            .text()
            .get(range.clone())
            .unwrap_or_default()
            .trim()
            .replace('\n', " ");
        item = item.child(quote_view(&quote, theme));
    }
    if !comment.body.is_empty() {
        item = item.child(
            text(comment.body.clone())
                .font(theme.ty.caption)
                .color(theme.text)
                .max_lines(2)
                .w_full(),
        );
    }
    item
}

fn list_key(n: usize) -> String {
    format!("list-{n}")
}

fn view(s: &Reviewer, cx: &mut Cx<Reviewer>) -> El {
    let theme = Theme::current(cx.system_dark());
    let md_style = style(&theme);
    let mut doc = col().w_full().max_w(880.0).self_center();
    if let Some(general) = general_thread(s, &theme) {
        doc = doc.child(general);
    }
    let marks = marks(s, &theme);
    for (i, block) in s.page.blocks.iter().enumerate() {
        let mut md = Markdown::new(block.markdown.as_str())
            .style(md_style.clone())
            .highlights(highlights(&s.page, &marks, i))
            .selectable(true)
            .join_selection_group()
            .on_link_tx(|s: &mut Reviewer, url, tx| open_link(s, url, tx))
            .tasks_read_only();
        // rune-kit rebuilds a block with an image loader every frame, so only image blocks get one.
        if block.markdown.contains("![") {
            let images = s.images.clone();
            md = md.images(move |src| images.get(src));
        }
        let md = md.build(&theme.kit);
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
        .child(
            doc.selection_group(true)
                .on_background_press_tx(pointed)
                .on_selection_change(
                    |s: &mut Reviewer, change: &rune::SelectionChange, tx: &mut Tx| {
                        // A drag selects once it lets go.
                        if !change.dragging() && change.selection().is_some() {
                            pointed(s, tx);
                        }
                    },
                ),
        )
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let mut body = el().w_full().grow(1.0).min_h(0.0).child(content);
    if let Some(at) = s.list() {
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

fn pending_keys(s: &Reviewer) -> Option<(String, HintGroups)> {
    if matches!(s.mode, Mode::Composing(_) | Mode::Command(_)) {
        return None;
    }
    if s.mode == Mode::AllKeys {
        let mut groups = s.vim.keymap().hints(Table::Normal);
        groups.push((
            HintGroup::App,
            vec![
                hint(":wq", "Send the review and quit"),
                hint(":w", "Write the review"),
                hint(":q", "Quit, once it's written"),
                hint(":q!", "Quit without sending"),
            ],
        ));
        return Some(("keys".into(), groups));
    }
    s.vim.hints().map(|(header, groups)| {
        let label = match header {
            HintsFor::Typed(keys) => keys,
            HintsFor::Mode(mode) => match mode {
                VimMode::Normal => "normal",
                VimMode::Visual => "visual",
                VimMode::VisualLine => "v-line",
                VimMode::Search => "search",
            }
            .to_owned(),
        };
        (label, groups)
    })
}

fn which_key(s: &Reviewer, theme: &Theme) -> Option<El> {
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
    for (i, (_, group)) in groups.iter().enumerate() {
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

fn bar(s: &Reviewer, theme: &Theme) -> El {
    markdown::title_bar(
        s.origin.title(),
        &[
            ("v", "select"),
            ("C", "general"),
            ("Tab", "comments"),
            ("\\w", "write"),
            ("\\q", "quit"),
            (":wq", "send"),
            ("\\?", "keys"),
        ],
        theme,
    )
}

fn status_line(s: &Reviewer, theme: &Theme) -> El {
    let mode = if let Mode::Command(cmd) = &s.mode {
        Some(
            text(format!(":{cmd}"))
                .font(theme.ty.mono)
                .color(theme.text),
        )
    } else if s.composer().is_some() {
        Some(el().child(Badge::new("COMMENT").tone(Tone::Warning).build(theme)))
    } else {
        None
    };
    let count = s.review.len();
    let summary = format!("{count} comment{}", if count == 1 { "" } else { "s" });
    let extra = text(summary)
        .font(theme.ty.caption)
        .color(theme.text_secondary);
    markdown::status_line(
        &s.vim,
        s.page.text(),
        s.note.as_deref(),
        mode,
        Some(extra),
        theme,
    )
}

fn cmdline_key(s: &mut Reviewer, mut cmd: String, k: &KeyEvent, tx: &mut Tx) {
    match &k.key {
        Key::Escape => return,
        Key::Backspace => {
            if cmd.pop().is_none() {
                return;
            }
        }
        Key::Enter => {
            s.command(&cmd, tx);
            return;
        }
        Key::Text(t) if !k.mods.cmd && !k.mods.ctrl => cmd.push_str(t),
        _ => {}
    }
    s.mode = Mode::Command(cmd);
}

fn on_key(s: &mut Reviewer, k: &KeyEvent, tx: &mut Tx) -> bool {
    match std::mem::take(&mut s.mode) {
        Mode::Composing(target) => {
            if k.key == Key::Escape {
                s.mode = Mode::Normal;
                s.note = Some("Cancelled".into());
            } else {
                // A pointer click can blur the field without ending composition.
                s.mode = Mode::Composing(target);
                s.draft.focused = true;
            }
            return true;
        }
        Mode::Command(cmd) => {
            cmdline_key(s, cmd, k, tx);
            return true;
        }
        Mode::List(at) => {
            s.note = None;
            list_key_press(s, at, k, tx);
            return true;
        }
        Mode::AllKeys => {
            s.mode = Mode::Normal;
            s.note = None;
            if k.key == Key::Escape {
                return true;
            }
        }
        Mode::Normal => s.note = None,
    }
    let before = (s.cursor(), s.vim.mode());
    let effect = s.vim.key(k, s.page.text());
    match effect {
        Some(VimEffect::Consumed) => {
            // Consuming a prefix must not change the selected comment.
            if (s.cursor(), s.vim.mode()) != before {
                s.selected = None;
            }
            tx.scroll_into_view(VIM_CURSOR);
        }
        Some(VimEffect::Copy(yanked)) => tx.copy(yanked),
        Some(VimEffect::CopySource { range, linewise }) => {
            let ranges = s.page.source_ranges(range);
            tx.copy(markdown::source_of(&s.source, &ranges, linewise));
        }
        Some(VimEffect::Operator { action, range, .. }) => match action {
            Op::Comment => s.open(Target::New { range: Some(range) }),
            Op::Remove => s.remove(range),
        },
        Some(VimEffect::Action { action, .. }) => act(s, action, tx),
        None => return false,
    }
    true
}

fn act(s: &mut Reviewer, action: Act, tx: &mut Tx) {
    match action {
        Act::General => s.open(Target::New { range: None }),
        Act::NextComment | Act::PreviousComment => {
            match s
                .review
                .step(s.current(), s.cursor(), action == Act::PreviousComment)
            {
                Some(id) => s.jump(id, tx),
                None => s.note = Some("No comments".into()),
            }
        }
        Act::Edit => match s.current() {
            Some(id) => s.open(Target::Edit(id)),
            None => s.note = Some("No comment here. v selects, c comments.".into()),
        },
        Act::Delete => match s.current() {
            Some(id) => s.delete(id),
            None => s.note = Some("No comment here".into()),
        },
        Act::List => s.open_list(tx),
        Act::Keys => s.mode = Mode::AllKeys,
        Act::CommandLine => s.mode = Mode::Command(String::new()),
        Act::SendAndQuit => s.execute(Command::SendAndQuit, tx),
        Act::QuitWithoutSending => s.execute(Command::ForceQuit, tx),
        Act::Write => s.execute(Command::Write, tx),
        Act::Quit => s.execute(Command::Quit, tx),
        Act::OpenLink => {
            let text = s.page.text();
            let at = s.vim.cursor(text);
            match s.page.link_at(at) {
                Some(url) => open_link(s, &url, tx),
                None => s.note = Some("No link under the cursor".into()),
            }
        }
    }
}

fn open_link(s: &mut Reviewer, url: &str, tx: &mut Tx) {
    s.note = Some(match tx.open_url(url) {
        Ok(()) => format!("Opening {url}"),
        Err(e) => format!("Can't open {url}: {e}"),
    });
}

/// The mouse as in Vim: a press that left a selection selects it in Visual mode, and one that
/// left a caret moves the cursor there.
fn pointed(s: &mut Reviewer, tx: &mut Tx) {
    if matches!(s.mode, Mode::Composing(_) | Mode::Command(_)) {
        return;
    }
    let text = s.page.text();
    let range = if let Some(selection) = tx.text_selection() {
        let mut ranges = selection.texts().iter().filter_map(|t| s.page.selected(t));
        let Some(first) = ranges.next() else { return };
        let end = ranges.next_back().map_or(first.end, |r| r.end);
        first.start..end
    } else if let Some(at) = tx.text_caret().and_then(|c| s.page.clicked(c)) {
        at..at
    } else {
        return;
    };
    s.mode = Mode::Normal;
    s.note = None;
    s.selected = None;
    s.vim.select(range, text);
    // The page shows the selection as Vim's, not as the pointer left it.
    tx.clear_text_selection();
}

fn list_key_press(s: &mut Reviewer, at: usize, k: &KeyEvent, tx: &mut Tx) {
    s.mode = Mode::List(at);
    let n = s.review.len();
    let step = |at: usize, by: isize| at.saturating_add_signed(by).min(n.saturating_sub(1));
    let to = match Stroke::from_event(k) {
        None if k.key == Key::Escape => {
            s.mode = Mode::Normal;
            tx.scroll_into_view(VIM_CURSOR);
            return;
        }
        Some(Stroke::Tab | Stroke::BackTab | Stroke::Char('q')) => {
            s.mode = Mode::Normal;
            tx.scroll_into_view(VIM_CURSOR);
            return;
        }
        Some(Stroke::Enter) => {
            s.mode = Mode::Normal;
            if let Some(id) = s.review.at_position(at) {
                s.jump(id, tx);
            }
            return;
        }
        Some(Stroke::Down | Stroke::Char('j')) => step(at, 1),
        Some(Stroke::Up | Stroke::Char('k')) => step(at, -1),
        Some(Stroke::Char('g')) => 0,
        Some(Stroke::Char('G')) => n.saturating_sub(1),
        _ => return,
    };
    s.mode = Mode::List(to);
    tx.scroll_into_view(list_key(to));
    if let Some(id) = s.review.at_position(to) {
        s.show(id, tx);
    }
}

fn send(s: &mut Reviewer, _: &mut Tx) {
    if s.quit == Quit::Drop {
        return;
    }
    s.save();
    let Some(feedback) = s.feedback() else {
        return;
    };
    // `:w` already wrote this to the file.
    if s.out.is_some() && s.written.as_ref() == Some(&feedback) {
        return;
    }
    if let Err(e) = deliver(&feedback, s.out.as_deref(), &mut std::io::stdout()) {
        eprintln!("mdreview: {e}");
        s.send_failed.set(true);
    }
}

/// A failed file write sends feedback to stdout before returning the error.
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
    let shown = if written.is_ok() {
        ", so it went to stdout"
    } else {
        ""
    };
    Err(format!("could not write the review to {path}: {e}{shown}"))
}

#[derive(Debug, Default, PartialEq)]
struct Args {
    path: Option<String>,
    out: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" | "--output" => match args.next() {
                Some(o) => parsed.out = Some(o.clone()),
                None => return Err("-o needs a file".into()),
            },
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(format!(
                    "unknown option {flag}. Usage: mdreview [FILE | -] [-o OUT]"
                ));
            }
            _ if parsed.path.is_some() => {
                return Err(format!(
                    "one document at a time, but {a} came after another"
                ));
            }
            _ => parsed.path = Some(a.clone()),
        }
    }
    Ok(parsed)
}

fn read_input() -> Input {
    let mut stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    load_input(rune::extra_args(), &mut stdin, terminal).unwrap_or_else(|error| fail(&error))
}

fn load_input(
    args: &[String],
    stdin: &mut impl std::io::Read,
    terminal: bool,
) -> Result<Input, String> {
    let Args { path, out } = parse_args(args)?;
    let (origin, source) = match path.as_deref() {
        Some("-") => {
            if terminal {
                return Err("`-` reads the document from stdin, but stdin is the terminal".into());
            }
            let mut source = String::new();
            stdin
                .read_to_string(&mut source)
                .map_err(|error| format!("could not read stdin: {error}"))?;
            (Origin::Stdin, source)
        }
        Some(path) => {
            let source = std::fs::read_to_string(path)
                .map_err(|error| format!("could not read {path}: {error}"))?;
            (Origin::File(path.to_owned()), source)
        }
        None => (Origin::Sample, SAMPLE.to_owned()),
    };
    Ok(Input {
        origin,
        source,
        out,
    })
}

fn fail(message: &str) -> ! {
    eprintln!("mdreview: {message}");
    std::process::exit(1);
}

fn main() {
    let state = Reviewer::new(read_input());
    let send_failed = state.send_failed.clone();
    App::new(AppSpec::<Reviewer> {
        scenes: &[
            ("comments", |s| {
                let text = s.page.text().to_owned();
                let find = |needle: &str| find_text(&text, needle);
                s.review.add(Comment::general(
                    "Good direction overall. Keep the first version small.",
                ));
                let picked = s.review.add(Comment::on(
                    find("stores the quote and its lines"),
                    "Lines drift when the agent rewrites the plan. Keep the quote as the anchor.",
                ));
                s.review.add(Comment::on(
                    find("Escape cancels"),
                    "Is Escape the only way out?",
                ));
                s.review
                    .add(Comment::remove(find("Sharing a review by link")));
                s.selected = Some(picked);
            }),
            ("composing", |s| {
                let text = s.page.text().to_owned();
                let range = find_text(&text, "one general comment");
                s.open(Target::New { range: Some(range) });
                s.draft = TextEdit::new("Should the general comment go first in the feedback?");
                s.draft.focused = true;
            }),
            ("panel", |s| {
                let text = s.page.text().to_owned();
                let find = |needle: &str| find_text(&text, needle);
                s.review
                    .add(Comment::general("Ship the first version small."));
                s.review.add(Comment::on(
                    find("stores the quote"),
                    "Anchor on the quote.",
                ));
                s.review
                    .add(Comment::remove(find("Sharing a review by link")));
                s.mode = Mode::List(1);
            }),
            ("visual", |s| {
                type_keys(&mut s.vim, "jjwwvee", s.page.text())
            }),
            ("keys", |s| s.mode = Mode::AllKeys),
            ("yank", |s| type_keys(&mut s.vim, "gy", s.page.text())),
            ("prefix", |s| {
                s.review.add(Comment::on(
                    find_text(s.page.text(), "stores the quote"),
                    "x",
                ));
                type_keys(&mut s.vim, "c", s.page.text());
            }),
        ],
        setup: Some(|s: &mut Reviewer, waker| s.images.start(waker)),
        poll: Some(|s: &mut Reviewer, _, _| s.images.poll()),
        on_key: Some(on_key),
        families: markdown::FONT_FAMILIES,
        ..AppSpec::new("Review", (1000.0, 900.0), Reviewer::default, view)
    })
    .init(move || state)
    .on_quit(send)
    .run();
    if send_failed.get() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
