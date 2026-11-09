//! Vim's keys for reading text: a cursor and visual selections moved with Vim's motions,
//! text objects, operators and search. It knows nothing about what draws the text: the app keeps
//! a [`Vim`] beside the text it shows (a page's `rune_kit::markdown::RenderedText`,
//! a log, a file), feeds it keys, and paints [`Vim::cursor`], [`Vim::selection`] and
//! [`Vim::matches`] back onto the page.
//!
//! What each key does is a [`Keymap`]: [`Keymap::vim`] has Vim's own keys, as bindings like any
//! other, and an app adds its actions and operators to it (or rebinds Vim's). An app operator
//! takes a motion, an object or the selection as `y` does (`cw`, `cip`, `cc`, `v…c`) and comes
//! back as [`VimEffect::Operator`] with the range; an app action as [`VimEffect::Action`].
//! [`Vim::hints`] lists what can follow the keys typed so far, from the same keymap.

mod keymap;

use std::ops::Range;

use rune::{Key, KeyEvent};

use keymap::Lookup;
pub use keymap::{
    Command, Find, Hint, HintGroup, HintGroups, KeyStroke, Keymap, Motion, NoAction, Object,
    Operator, PendingCommand, Stroke, Table, hint,
};

/// The largest count Vim takes, as Vim caps its own. A motion repeats once per count, so a
/// larger one would only spin, and two multiplied could overflow.
const MAX_COUNT: usize = 99_999;

/// What [`Vim`] is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VimMode {
    #[default]
    Normal,
    Visual,
    VisualLine,
    /// Typing a search after `/` or `?`.
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisualMode {
    Character,
    Line,
}

#[derive(Clone, Debug)]
enum Mode {
    Normal,
    Visual(VisualMode),
    Search {
        from: Option<VisualMode>,
        typing: String,
    },
}

#[derive(Clone, Debug, Default)]
struct Search {
    query: Query,
    backward: bool,
    hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintsFor {
    Typed(String),
    Mode(VimMode),
}

/// What a key asked the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimEffect<A, O = A> {
    /// The cursor, selection or search changed, or a key was taken toward a command: repaint,
    /// and scroll the cursor into view.
    Consumed,
    /// Copy this text (a yank).
    Copy(String),
    /// Copy the source behind this range of the text (a `gy` yank), which only the app can
    /// find: for a rendered page, the Markdown it was drawn from. `linewise` yanks take whole
    /// lines.
    CopySource { range: Range<usize>, linewise: bool },
    /// An app action ([`Command::App`]), with the count typed before it, if any.
    Action { action: A, count: Option<usize> },
    /// An app operator ([`Operator::App`]) over `range` of the text: what its motion or object
    /// covered, or the selection. `linewise` when it covers whole lines (`cc`, `cj`, `V…c`).
    Operator {
        action: O,
        range: Range<usize>,
        linewise: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Up to, not including, the target.
    Exclusive,
    /// Including the target.
    Inclusive,
    /// Whole lines.
    Linewise,
}

/// Keys typed toward a command that isn't finished.
#[derive(Clone, Debug, PartialEq)]
enum Pending<A> {
    None,
    /// Strokes toward a binding in the mode's table.
    Keys(Vec<Stroke>),
    /// An operator, waiting for a motion or object: its keys (`yy`, `gyy` take the line), its
    /// count, and the strokes toward the motion so far.
    Operator {
        op: OpState<A>,
        typed: Vec<Stroke>,
    },
    /// `i` or `a`, waiting for an object; after an operator, or in Visual mode. `opened` is
    /// the count and keys that asked for it (`2i`), `typed` the strokes toward the object.
    Object {
        around: bool,
        op: Option<OpState<A>>,
        opened: (Option<usize>, Vec<Stroke>),
        typed: Vec<Stroke>,
    },
    /// `f F t T`, waiting for the character; after an operator, its keys and count too.
    /// `opened` is the count and keys that asked for it.
    Find {
        find: Find,
        op: Option<OpState<A>>,
        opened: (Option<usize>, Vec<Stroke>),
    },
}

/// The state: mode, cursor, selection anchor and whatever keys are half typed, with the keymap
/// that says what keys do. Positions are byte offsets into the text passed with each key.
#[derive(Clone, Debug)]
#[must_use]
pub struct Vim<A = NoAction, O = A> {
    keymap: Keymap<A, O>,
    mode: Mode,
    cursor: usize,
    anchor: usize,
    goal_column: usize,
    count: Option<usize>,

    pending: Pending<O>,
    last_find: Option<(Find, char)>,

    search: Search,
    skip_blank_lines: bool,
    half_page_lines: usize,
    message: Option<String>,
}

impl Default for Vim<NoAction> {
    fn default() -> Self {
        Vim::new(Keymap::vim())
    }
}

impl<A: Clone, O: Clone> Vim<A, O> {
    pub fn new(keymap: Keymap<A, O>) -> Vim<A, O> {
        Vim {
            keymap,
            mode: Mode::Normal,
            cursor: 0,
            anchor: 0,
            goal_column: 0,
            count: None,
            pending: Pending::None,
            last_find: None,
            search: Search::default(),
            skip_blank_lines: false,
            half_page_lines: 15,
            message: None,
        }
    }

    /// Never rest the cursor on an empty line: `j`, `k` and the other motions step over them,
    /// and `{` `}` go to the start of the previous and next paragraph. For text whose blank
    /// lines only separate blocks and draw nothing, such as a rendered page.
    pub fn skip_blank_lines(mut self, on: bool) -> Self {
        self.skip_blank_lines = on;
        self
    }

    /// The page Ctrl-D and Ctrl-U move half of, in lines: the app's view of the text. Without
    /// one they move 15 lines.
    pub fn page_lines(mut self, lines: usize) -> Self {
        self.half_page_lines = (lines / 2).max(1);
        self
    }

    pub fn keymap(&self) -> &Keymap<A, O> {
        &self.keymap
    }

    pub fn mode(&self) -> VimMode {
        match self.mode {
            Mode::Normal => VimMode::Normal,
            Mode::Visual(VisualMode::Character) => VimMode::Visual,
            Mode::Visual(VisualMode::Line) => VimMode::VisualLine,
            Mode::Search { .. } => VimMode::Search,
        }
    }

    fn visual_mode(&self) -> Option<VisualMode> {
        match self.mode {
            Mode::Visual(visual) => Some(visual),
            _ => None,
        }
    }

    fn visual(&self) -> bool {
        self.visual_mode().is_some()
    }

    fn table(&self) -> Table {
        if self.visual() {
            Table::Visual
        } else {
            Table::Normal
        }
    }

    /// The mode for a status line: `NORMAL`, `VISUAL`, `V-LINE`, or the search being typed.
    pub fn status(&self) -> String {
        match &self.mode {
            Mode::Normal => "NORMAL".to_owned(),
            Mode::Visual(VisualMode::Character) => "VISUAL".to_owned(),
            Mode::Visual(VisualMode::Line) => "V-LINE".to_owned(),
            Mode::Search { typing, .. } => {
                format!("{}{}", if self.search.backward { '?' } else { '/' }, typing)
            }
        }
    }

    /// Whether keys are half typed toward a command (a count, `g`, `y`, `yi`, `]`).
    pub fn is_pending(&self) -> bool {
        self.count.is_some() || !matches!(self.pending, Pending::None)
    }

    /// Keys typed toward a command that isn't finished, as `2y` or `yi`.
    pub fn pending(&self) -> String {
        let count = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
        let r = |k: &[Stroke]| self.keymap.render(k);
        let op_keys = |op: &Option<OpState<O>>| match op {
            Some(op) => format!("{}{}", count(op.count), r(&op.keys)),
            None => String::new(),
        };
        match &self.pending {
            Pending::None => count(self.count),
            Pending::Keys(typed) => format!("{}{}", count(self.count), r(typed)),
            Pending::Operator { op, typed } => format!(
                "{}{}{}{}",
                count(op.count),
                r(&op.keys),
                count(self.count),
                r(typed)
            ),
            Pending::Object {
                op,
                opened: (c, keys),
                typed,
                ..
            } => format!("{}{}{}{}", op_keys(op), count(*c), r(keys), r(typed)),
            Pending::Find {
                op,
                opened: (c, keys),
                ..
            } => format!("{}{}{}", op_keys(op), count(*c), r(keys)),
        }
    }

    /// What can follow the keys typed so far, for a which-key panel: what's been typed (`y`,
    /// `gy`, `yi`, `]`, the leader) or the visual mode, and the bound keys that can come next,
    /// grouped: the app's, then Vim's commands, then motions. `None` in Normal mode with
    /// nothing half typed, and while a search is being typed.
    pub fn hints(&self) -> Option<(HintsFor, HintGroups)> {
        let header = HintsFor::Typed(self.pending());
        match &self.pending {
            Pending::None if self.visual() && self.count.is_none() => Some((
                HintsFor::Mode(self.mode()),
                self.keymap.hints(Table::Visual),
            )),
            Pending::None if self.count.is_some() => {
                Some((header, self.keymap.next(self.table(), &[])))
            }
            Pending::None => None,
            Pending::Keys(keys) => Some((header, self.keymap.next(self.table(), keys))),
            Pending::Operator { op, typed } => {
                let mut groups = Vec::new();
                if typed.is_empty() {
                    let group = match op.op {
                        Operator::App(_) => HintGroup::App,
                        _ => HintGroup::Vim,
                    };
                    groups.push((group, vec![hint(&self.keymap.render(&op.keys), "The line")]));
                }
                groups.extend(self.keymap.next_pending(typed));
                Some((header, groups))
            }
            Pending::Object { typed, .. } => Some((
                header,
                vec![(HintGroup::Motion, self.keymap.next_objects(typed))],
            )),
            Pending::Find { find, .. } => Some((
                header,
                vec![(
                    HintGroup::Motion,
                    vec![hint(
                        "any",
                        if find.till() {
                            "The character to stop before"
                        } else {
                            "The character to go to"
                        },
                    )],
                )],
            )),
        }
    }

    /// A one-off note from the last command, as "3 lines yanked" or "Pattern not found".
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The byte range of the character under the cursor.
    pub fn cursor(&self, text: &str) -> Range<usize> {
        let b = Buf::new(text);
        let i = b.index(self.cursor);
        b.byte(i)..b.byte(i + 1)
    }

    /// Puts the cursor on the character at byte `at` of `text` and drops back to Normal, as a
    /// jump the app makes (to a mark, a comment) does.
    pub fn set_cursor(&mut self, at: usize, text: &str) {
        let b = Buf::new(text);
        if b.n() == 0 {
            return;
        }
        let i = b.index(at);
        self.cursor = b.byte(i);
        self.goal_column = i - b.line_start(i);
        self.mode = Mode::Normal;
        self.reset_pending();
    }

    /// The selected bytes in visual modes.
    pub fn selection(&self, text: &str) -> Option<Range<usize>> {
        self.visual_mode()
            .map(|mode| self.visual_range(mode, &Buf::new(text)))
    }

    fn visual_range(&self, mode: VisualMode, b: &Buf) -> Range<usize> {
        let (cursor, anchor) = (b.index(self.cursor), b.index(self.anchor));
        let (lo, hi) = (cursor.min(anchor), cursor.max(anchor));
        match mode {
            VisualMode::Character => b.byte(lo)..b.byte(hi + 1),
            VisualMode::Line => b.byte(b.line_start(lo))..b.byte(b.line_end(hi)),
        }
    }

    /// Where the last search matches, for painting.
    pub fn matches(&self, text: &str) -> Vec<Range<usize>> {
        if let Mode::Search { typing, .. } = &self.mode {
            return find_all(text, &Query::typed(typing));
        }
        if self.search.hidden {
            return Vec::new();
        }
        find_all(text, &self.search.query)
    }

    /// Handles `ev` over `text`. `None` when the key means nothing here, so the app can use it.
    pub fn key(&mut self, ev: &KeyEvent, text: &str) -> Option<VimEffect<A, O>> {
        let b = Buf::new(text);
        if b.n() == 0
            && (self.mode() != VimMode::Normal
                || !matches!(self.pending, Pending::None | Pending::Keys(_)))
        {
            self.reset_pending();
            self.mode = Mode::Normal;
        }
        self.cursor = b.byte(b.index(self.cursor));
        self.message = None;
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Search { from, typing } => return self.search_key(ev, text, &b, from, typing),
            mode => self.mode = mode,
        }
        if ev.key == Key::Escape {
            let busy = self.is_pending() || self.visual();
            let hide_matches = !busy && !self.search.hidden && !self.search.query.text.is_empty();
            if hide_matches {
                self.search.hidden = true;
            }
            self.reset_pending();
            self.mode = Mode::Normal;
            return (busy || hide_matches).then_some(VimEffect::Consumed);
        }
        let stroke = Stroke::from_event(ev)?;

        match (std::mem::replace(&mut self.pending, Pending::None), stroke) {
            (
                Pending::Find {
                    find,
                    op,
                    opened: (count, _),
                },
                stroke,
            ) => {
                self.count = count;
                let Stroke::Char(c) = stroke else {
                    return Some(self.cancel());
                };
                self.last_find = Some((find, c));
                let (n, _) = self.motion_count(op.as_ref().and_then(|op| op.count));
                Some(self.finish_find((find, c), n, op, text, &b))
            }
            (pending, Stroke::Char(c @ '0'..='9'))
                if (c != '0' || self.count.is_some())
                    && (matches!(&pending, Pending::None)
                        || matches!(&pending, Pending::Operator { typed, .. } if typed.is_empty())) =>
            {
                let digit = c as usize - '0' as usize;
                self.count = Some((self.count.unwrap_or(0) * 10 + digit).min(MAX_COUNT));
                self.pending = pending;
                Some(VimEffect::Consumed)
            }
            (Pending::None, stroke) => self.lookup(stroke, Vec::new(), text, &b),
            (Pending::Keys(typed), stroke) => self.lookup(stroke, typed, text, &b),
            (Pending::Operator { op, mut typed }, stroke) => {
                typed.push(stroke);
                let last = &op.keys[op.keys.len() - 1..];
                let operator_takes_line = typed == op.keys || typed == last;
                if operator_takes_line {
                    return Some(self.operate_lines(op.op, op.count, text, &b));
                }
                let line_prefix = op.keys.starts_with(&typed);
                match self.keymap.lookup_pending(&typed) {
                    Lookup::Exact(PendingCommand::Motion(m)) => {
                        Some(self.finish_motion(m, typed, Some(op), text, &b))
                    }
                    Lookup::Exact(c @ (PendingCommand::Inner | PendingCommand::Around)) => {
                        self.pending = Pending::Object {
                            around: matches!(c, PendingCommand::Around),
                            op: Some(op),
                            opened: (self.count.take(), typed),
                            typed: Vec::new(),
                        };
                        Some(VimEffect::Consumed)
                    }
                    found if matches!(found, Lookup::Prefix) || line_prefix => {
                        self.pending = Pending::Operator { op, typed };
                        Some(VimEffect::Consumed)
                    }
                    _ => Some(self.cancel()),
                }
            }
            (
                Pending::Object {
                    around,
                    op,
                    opened,
                    mut typed,
                },
                stroke,
            ) => {
                typed.push(stroke);
                match self.keymap.lookup_object(&typed) {
                    Lookup::Exact(object) => {
                        self.count = opened.0;
                        Some(self.finish_object(object, around, op, text, &b))
                    }
                    Lookup::Prefix => {
                        self.pending = Pending::Object {
                            around,
                            op,
                            opened,
                            typed,
                        };
                        Some(VimEffect::Consumed)
                    }
                    Lookup::None => Some(self.cancel()),
                }
            }
        }
    }

    /// Looks `typed` and `stroke` up in the mode's table.
    fn lookup(
        &mut self,
        stroke: Stroke,
        mut typed: Vec<Stroke>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A, O>> {
        let first = typed.is_empty();
        typed.push(stroke);
        match self.keymap.lookup(self.table(), &typed) {
            Lookup::Exact(command) => Some(self.run(command, typed, text, b)),
            Lookup::Prefix => {
                self.pending = Pending::Keys(typed);
                Some(VimEffect::Consumed)
            }
            Lookup::None if first && self.count.is_none() => None,
            Lookup::None => Some(self.cancel()),
        }
    }

    fn run(
        &mut self,
        command: Command<A, O>,
        keys: Vec<Stroke>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A, O> {
        if b.n() == 0 && !matches!(command, Command::App(_)) {
            return self.cancel();
        }
        let visual = self.visual_mode();
        match command {
            Command::Motion(m) => self.finish_motion(m, keys, None, text, b),
            Command::Operator(op) => {
                if let Some(mode) = visual {
                    let range = self.visual_range(mode, b);
                    return self.apply(op, range, mode == VisualMode::Line, text, b, false);
                }
                let count = self.count.take();
                self.pending = Pending::Operator {
                    op: OpState { op, keys, count },
                    typed: Vec::new(),
                };
                VimEffect::Consumed
            }
            Command::Lines(op) => {
                let count = self.count.take();
                self.operate_lines(op, count, text, b)
            }
            Command::Inner | Command::Around => {
                self.pending = Pending::Object {
                    around: matches!(command, Command::Around),
                    op: None,
                    opened: (self.count.take(), keys),
                    typed: Vec::new(),
                };
                VimEffect::Consumed
            }
            Command::Visual | Command::VisualLine => {
                let mode = if matches!(command, Command::Visual) {
                    VisualMode::Character
                } else {
                    VisualMode::Line
                };
                if visual == Some(mode) {
                    self.mode = Mode::Normal;
                } else {
                    if visual.is_none() {
                        self.anchor = self.cursor;
                    }
                    self.mode = Mode::Visual(mode);
                }
                self.cancel()
            }
            Command::OtherEnd => {
                std::mem::swap(&mut self.cursor, &mut self.anchor);
                self.cancel()
            }
            Command::Search { backward } => {
                self.search.backward = backward;
                self.mode = Mode::Search {
                    from: visual,
                    typing: String::new(),
                };
                self.cancel()
            }
            Command::App(action) => {
                let count = self.count.take();
                self.reset_pending();
                VimEffect::Action { action, count }
            }
        }
    }

    fn reset_pending(&mut self) {
        self.pending = Pending::None;
        self.count = None;
    }

    fn cancel(&mut self) -> VimEffect<A, O> {
        self.reset_pending();
        VimEffect::Consumed
    }

    /// The count for a motion, times the operator's: `2y3w` moves six words.
    fn motion_count(&mut self, op_count: Option<usize>) -> (usize, Option<usize>) {
        let raw = self.count.take();
        let n = (raw.unwrap_or(1) * op_count.unwrap_or(1)).min(MAX_COUNT);
        (n, raw)
    }

    /// Runs motion `m`, typed as `keys`: moves the cursor, or after an operator, applies it over
    /// what the motion covers. `f F t T` wait for their character first.
    fn finish_motion(
        &mut self,
        m: Motion,
        keys: Vec<Stroke>,
        op: Option<OpState<O>>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A, O> {
        let op_count = op.as_ref().and_then(|op| op.count);
        let (n, raw_count) = self.motion_count(op_count);
        let cur = b.index(self.cursor);
        let keep_column = matches!(
            m,
            Motion::Down | Motion::Up | Motion::HalfPageDown | Motion::HalfPageUp
        );
        let (target, kind) = match m {
            Motion::Find(find) => {
                self.pending = Pending::Find {
                    find,
                    op,
                    opened: (raw_count, keys),
                };
                return VimEffect::Consumed;
            }
            Motion::RepeatFind | Motion::RepeatFindReverse => {
                let Some((find, c)) = self.last_find else {
                    return self.cancel();
                };
                let find = if m == Motion::RepeatFind {
                    find
                } else {
                    find.reverse()
                };
                return self.finish_find((find, c), n, op, text, b);
            }
            Motion::Left => (
                cur.saturating_sub(n).max(b.line_start(cur)),
                Kind::Exclusive,
            ),
            Motion::Right => ((cur + n).min(b.line_last(cur)), Kind::Exclusive),
            Motion::Down | Motion::Up | Motion::HalfPageDown | Motion::HalfPageUp => {
                let step = if matches!(m, Motion::HalfPageDown | Motion::HalfPageUp) {
                    self.half_page_lines * n
                } else {
                    n
                };
                let down = matches!(m, Motion::Down | Motion::HalfPageDown);
                let line = b.line_of(cur);
                let line = if self.skip_blank_lines {
                    let drawn_lines: Vec<usize> = (0..b.line_count())
                        .filter(|&line| !b.blank_line(b.nth_line(line)))
                        .collect();
                    let here = drawn_lines.partition_point(|&candidate| candidate < line);
                    let to = if down {
                        (here + step).min(drawn_lines.len().saturating_sub(1))
                    } else {
                        here.saturating_sub(step)
                    };
                    drawn_lines.get(to).copied().unwrap_or(line)
                } else if down {
                    (line + step).min(b.line_count() - 1)
                } else {
                    line.saturating_sub(step)
                };
                let start = b.nth_line(line);
                (
                    start + self.goal_column.min(b.line_last(start) - start),
                    Kind::Linewise,
                )
            }
            Motion::LineStart => (b.line_start(cur), Kind::Exclusive),
            Motion::FirstNonBlank => (b.first_non_blank(cur), Kind::Exclusive),
            Motion::LineEnd => {
                let line = (b.line_of(cur) + n - 1).min(b.line_count() - 1);
                (b.line_last(b.nth_line(line)), Kind::Inclusive)
            }
            Motion::Top | Motion::Bottom => {
                let last = b.line_count() - 1;
                let line = match (raw_count, m) {
                    (Some(n), _) => n.saturating_sub(1).min(last),
                    (None, Motion::Top) => 0,
                    (None, _) => last,
                };
                (b.first_non_blank(b.nth_line(line)), Kind::Linewise)
            }
            Motion::WordForward | Motion::BigWordForward => {
                let big = m == Motion::BigWordForward;
                (repeat(n, cur, |i| word_forward(b, i, big)), Kind::Exclusive)
            }
            Motion::WordBackward | Motion::BigWordBackward => {
                let big = m == Motion::BigWordBackward;
                (
                    repeat(n, cur, |i| word_backward(b, i, big)),
                    Kind::Exclusive,
                )
            }
            Motion::WordEnd | Motion::BigWordEnd => {
                let big = m == Motion::BigWordEnd;
                (repeat(n, cur, |i| word_end(b, i, big)), Kind::Inclusive)
            }
            Motion::ParagraphForward if self.skip_blank_lines => {
                (repeat(n, cur, |i| next_paragraph(b, i)), Kind::Exclusive)
            }
            Motion::ParagraphBackward if self.skip_blank_lines => (
                repeat(n, cur, |i| previous_paragraph(b, i)),
                Kind::Exclusive,
            ),
            Motion::ParagraphForward => {
                (repeat(n, cur, |i| paragraph(b, i, true)), Kind::Exclusive)
            }
            Motion::ParagraphBackward => {
                (repeat(n, cur, |i| paragraph(b, i, false)), Kind::Exclusive)
            }
            Motion::MatchingBracket => {
                let Some(target) = match_bracket(b, cur) else {
                    return self.cancel();
                };
                (target, Kind::Inclusive)
            }
            Motion::SearchNext | Motion::SearchPrevious => {
                let backward = self.search.backward != (m == Motion::SearchPrevious);
                let Some(target) = self.next_match(text, backward) else {
                    return self.cancel();
                };
                (b.index(target), Kind::Exclusive)
            }
            Motion::SearchWord | Motion::SearchWordBack => {
                let Some(word) = word_at(b, cur, false) else {
                    return self.cancel();
                };
                self.search.query = Query {
                    text: b.chars[word].iter().collect(),
                    whole_word: true,
                };
                self.search.backward = m == Motion::SearchWordBack;
                let Some(target) = self.next_match(text, self.search.backward) else {
                    return self.cancel();
                };
                (b.index(target), Kind::Exclusive)
            }
        };
        self.go(target, kind, op, text, b, keep_column)
    }

    fn finish_find(
        &mut self,
        (find, c): (Find, char),
        n: usize,
        op: Option<OpState<O>>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A, O> {
        let Some(target) = find_char(b, b.index(self.cursor), find, c, n) else {
            return self.cancel();
        };
        let kind = if find.forward() {
            Kind::Inclusive
        } else {
            Kind::Exclusive
        };
        self.go(target, kind, op, text, b, false)
    }

    /// Moves the cursor to `target`, or with an operator, applies it from the cursor to there.
    fn go(
        &mut self,
        target: usize,
        kind: Kind,
        op: Option<OpState<O>>,
        text: &str,
        b: &Buf,
        keep_column: bool,
    ) -> VimEffect<A, O> {
        let cur = b.index(self.cursor);
        if let Some(op) = op {
            let (lo, hi) = (cur.min(target), cur.max(target));
            let range = match kind {
                Kind::Exclusive => b.byte(lo)..b.byte(hi),
                Kind::Inclusive => b.byte(lo)..b.byte(hi + 1),
                Kind::Linewise => b.byte(b.line_start(lo))..b.byte(b.line_end(hi)),
            };
            return self.apply(op.op, range, kind == Kind::Linewise, text, b, keep_column);
        }
        let mut target = target.min(b.n() - 1);
        if self.skip_blank_lines && b.at(target) == '\n' && b.blank_line(target) {
            let solid = |j: &usize| b.chars[*j] != '\n';
            let ahead = (target..b.n()).find(solid);
            let behind = (0..target).rev().find(solid);
            target = if target >= cur {
                ahead.or(behind)
            } else {
                behind.or(ahead)
            }
            .unwrap_or(target);
        }
        self.cursor = b.byte(target);
        if !keep_column {
            self.goal_column = target - b.line_start(target);
        }
        self.cancel()
    }

    /// A text object at the cursor: selected in Visual mode, or what an operator acts on.
    fn finish_object(
        &mut self,
        object: Object,
        around: bool,
        op: Option<OpState<O>>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A, O> {
        let count =
            self.count.take().unwrap_or(1) * op.as_ref().and_then(|op| op.count).unwrap_or(1);
        let cur = b.index(self.cursor);
        let Some(range) = counted_object(b, cur, object, around, count.min(MAX_COUNT)) else {
            return self.cancel();
        };
        if let Some(op) = op {
            return self.apply(
                op.op,
                b.byte(range.start)..b.byte(range.end),
                false,
                text,
                b,
                false,
            );
        }
        self.mode = Mode::Visual(VisualMode::Character);
        self.anchor = b.byte(range.start);
        self.cursor = b.byte(range.end.saturating_sub(1).max(range.start));
        self.cancel()
    }

    /// The operator over whole lines from the cursor's: as many as `count` times the count
    /// typed after the operator (`2yy`, `y2y`).
    fn operate_lines(
        &mut self,
        op: Operator<O>,
        count: Option<usize>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A, O> {
        let n = (count.unwrap_or(1) * self.count.take().unwrap_or(1)).min(MAX_COUNT);
        let cur = b.index(self.cursor);
        let first = b.line_start(cur);
        let last = b.line_end(b.nth_line((b.line_of(cur) + n - 1).min(b.line_count() - 1)));
        self.apply(op, b.byte(first)..b.byte(last), true, text, b, false)
    }

    fn apply(
        &mut self,
        op: Operator<O>,
        range: Range<usize>,
        lines: bool,
        text: &str,
        b: &Buf,
        keep_column: bool,
    ) -> VimEffect<A, O> {
        self.reset_pending();
        self.mode = Mode::Normal;
        self.cursor = b.byte(b.index(range.start));
        if !keep_column {
            self.goal_column = b.index(range.start) - b.line_start(b.index(range.start));
        }
        match op {
            Operator::Yank | Operator::YankSource => {
                let source = matches!(op, Operator::YankSource);
                let yanked = text[range.clone()].to_owned();
                let count = yanked.lines().count().max(1);
                let what = if source { " as source" } else { "" };
                self.message = Some(if lines || count > 1 {
                    format!(
                        "{count} line{} yanked{what}",
                        if count == 1 { "" } else { "s" }
                    )
                } else {
                    let n = yanked.chars().count();
                    format!(
                        "{n} character{} yanked{what}",
                        if n == 1 { "" } else { "s" }
                    )
                });
                if source {
                    return VimEffect::CopySource {
                        range,
                        linewise: lines,
                    };
                }
                VimEffect::Copy(if lines { format!("{yanked}\n") } else { yanked })
            }
            Operator::App(action) => VimEffect::Operator {
                action,
                range,
                linewise: lines,
            },
        }
    }

    fn search_key(
        &mut self,
        ev: &KeyEvent,
        text: &str,
        b: &Buf,
        from: Option<VisualMode>,
        mut typing: String,
    ) -> Option<VimEffect<A, O>> {
        let effect = match &ev.key {
            Key::Escape => {
                self.mode = from.map_or(Mode::Normal, Mode::Visual);
                return Some(VimEffect::Consumed);
            }
            Key::Backspace => {
                if typing.pop().is_none() {
                    self.mode = from.map_or(Mode::Normal, Mode::Visual);
                    return Some(VimEffect::Consumed);
                }
                Some(VimEffect::Consumed)
            }
            Key::Enter => {
                self.mode = from.map_or(Mode::Normal, Mode::Visual);
                if !typing.is_empty() {
                    self.search.query = Query::typed(&typing);
                }
                if let Some(t) = self.next_match(text, self.search.backward) {
                    return Some(self.go(b.index(t), Kind::Exclusive, None, text, b, false));
                }
                return Some(VimEffect::Consumed);
            }
            Key::Text(s) if !ev.mods.cmd => {
                typing.push_str(s);
                Some(VimEffect::Consumed)
            }
            _ => None,
        };
        self.mode = Mode::Search { from, typing };
        effect
    }

    /// The byte where the search's next (or previous) match starts, wrapping round, with a note
    /// of which match it is; `None`, and a note, when there's none.
    fn next_match(&mut self, text: &str, backward: bool) -> Option<usize> {
        self.search.hidden = false;
        let found = find_all(text, &self.search.query);
        if found.is_empty() {
            self.message = Some(format!("Pattern not found: {}", self.search.query.text));
            return None;
        }
        let at = self.cursor;
        let next = if backward {
            found.iter().rev().find(|r| r.start < at).or(found.last())
        } else {
            found.iter().find(|r| r.start > at).or(found.first())
        };
        let start = next.map_or(at, |r| r.start);
        let i = found.iter().position(|r| r.start == start).unwrap_or(0);
        self.message = Some(format!("[{}/{}]", i + 1, found.len()));
        Some(start)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct OpState<A> {
    op: Operator<A>,
    keys: Vec<Stroke>,
    count: Option<usize>,
}

/// The text as characters, with each character's byte offset, for one key.
struct Buf {
    chars: Vec<char>,
    offs: Vec<usize>,
    lines: Vec<usize>,
    len: usize,
}

impl Buf {
    fn new(text: &str) -> Buf {
        let (offs, chars): (Vec<_>, Vec<_>) = text.char_indices().unzip();
        let lines = std::iter::once(0)
            .chain(
                chars
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| (*c == '\n').then_some(i + 1)),
            )
            .collect();
        Buf {
            chars,
            offs,
            lines,
            len: text.len(),
        }
    }
    fn n(&self) -> usize {
        self.chars.len()
    }
    fn byte(&self, i: usize) -> usize {
        self.offs.get(i).copied().unwrap_or(self.len)
    }
    fn index(&self, byte: usize) -> usize {
        self.offs
            .partition_point(|&o| o < byte)
            .min(self.n().saturating_sub(1))
    }
    fn at(&self, i: usize) -> char {
        self.chars.get(i).copied().unwrap_or('\n')
    }
    fn line_start(&self, i: usize) -> usize {
        self.lines[self.line_of(i)]
    }
    /// The newline ending `i`'s line, or the end.
    fn line_end(&self, i: usize) -> usize {
        self.lines
            .get(self.line_of(i) + 1)
            .map_or(self.n(), |start| start - 1)
    }
    /// The last character of `i`'s line that the cursor can sit on.
    fn line_last(&self, i: usize) -> usize {
        self.line_end(i).saturating_sub(1).max(self.line_start(i))
    }
    fn line_of(&self, i: usize) -> usize {
        self.lines
            .partition_point(|&start| start <= i)
            .saturating_sub(1)
    }
    fn line_count(&self) -> usize {
        self.lines.len()
    }
    fn nth_line(&self, line: usize) -> usize {
        self.lines.get(line).copied().unwrap_or(self.n())
    }
    fn first_non_blank(&self, i: usize) -> usize {
        let (s, e) = (self.line_start(i), self.line_end(i));
        (s..e)
            .find(|&j| !self.chars[j].is_whitespace())
            .unwrap_or(s)
    }
    fn blank_line(&self, i: usize) -> bool {
        self.line_start(i) == self.line_end(i)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punctuation,
}

fn class(c: char, big: bool) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if big || c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// A search: the text to find, and whether it must start a word (`*` and `#` search whole words).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Query {
    text: String,
    whole_word: bool,
}

impl Query {
    /// A search as typed after `/` or `?`.
    fn typed(text: &str) -> Query {
        Query {
            text: text.to_owned(),
            whole_word: false,
        }
    }
}

/// Every place `query` occurs, case-insensitive unless it has a capital (Vim's smartcase).
fn find_all(text: &str, query: &Query) -> Vec<Range<usize>> {
    let sensitive = query.text.chars().any(char::is_uppercase);
    let starts_word = |i: usize| {
        i == 0
            || !text[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    };
    find_folded(text, &query.text, !sensitive)
        .into_iter()
        .filter(|r| !query.whole_word || starts_word(r.start))
        .collect()
}

/// The byte ranges of `text` where `query` occurs, without overlaps. With `fold`, both are
/// compared lowercased, a character at a time, and the ranges are still `text`'s own: a
/// character whose lowercase is longer (`İ`, `ẞ`) doesn't shift them.
pub fn find_folded(text: &str, query: &str, fold: bool) -> Vec<Range<usize>> {
    let lower = |c: char| -> Vec<char> {
        if fold {
            c.to_lowercase().collect()
        } else {
            vec![c]
        }
    };
    let needle: Vec<char> = query.chars().flat_map(lower).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // Each folded character, with the byte range of the character of `text` it came from.
    let hay: Vec<(char, Range<usize>)> = text
        .char_indices()
        .flat_map(|(at, c)| {
            lower(c)
                .into_iter()
                .map(move |l| (l, at..at + c.len_utf8()))
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let window = &hay[i..i + needle.len()];
        // A match starts and ends on whole characters of `text`.
        let starts_whole = i == 0 || hay[i - 1].1 != window[0].1;
        let ends_whole = hay
            .get(i + needle.len())
            .is_none_or(|next| next.1 != window[needle.len() - 1].1);
        if starts_whole && ends_whole && window.iter().map(|(c, _)| *c).eq(needle.iter().copied()) {
            out.push(window[0].1.start..window[needle.len() - 1].1.end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn repeat(n: usize, from: usize, step: impl Fn(usize) -> usize) -> usize {
    (0..n).fold(from, |i, _| step(i))
}

fn word_forward(buffer: &Buf, start: usize, big: bool) -> usize {
    let end = buffer.n();
    let mut cursor = start;
    let word_class = class(buffer.at(cursor), big);
    if word_class != CharClass::Space {
        while cursor < end && class(buffer.at(cursor), big) == word_class {
            cursor += 1;
        }
    }
    while cursor < end && class(buffer.at(cursor), big) == CharClass::Space {
        cursor += 1;
    }
    cursor.min(end - 1)
}

fn word_backward(b: &Buf, i: usize, big: bool) -> usize {
    let mut j = i;
    while j > 0 && class(b.at(j - 1), big) == CharClass::Space {
        j -= 1;
    }
    if j == 0 {
        return 0;
    }
    let c = class(b.at(j - 1), big);
    while j > 0 && class(b.at(j - 1), big) == c {
        j -= 1;
    }
    j
}

fn word_end(buffer: &Buf, start: usize, big: bool) -> usize {
    let end = buffer.n();
    let mut cursor = start + 1;
    while cursor < end && class(buffer.at(cursor), big) == CharClass::Space {
        cursor += 1;
    }
    if cursor >= end {
        return end - 1;
    }
    let word_class = class(buffer.at(cursor), big);
    while cursor + 1 < end && class(buffer.at(cursor + 1), big) == word_class {
        cursor += 1;
    }
    cursor
}

/// The next (or previous) blank line.
fn paragraph(b: &Buf, i: usize, forward: bool) -> usize {
    let mut line = b.line_of(i);
    let count = b.line_count();
    loop {
        if forward {
            if line + 1 >= count {
                return b.n() - 1;
            }
            line += 1;
        } else {
            if line == 0 {
                return 0;
            }
            line -= 1;
        }
        let start = b.nth_line(line);
        if b.blank_line(start)
            && !b.blank_line(b.nth_line(if forward { line - 1 } else { line + 1 }))
        {
            return start;
        }
    }
}

/// The first line of the paragraph after `i`'s.
fn next_paragraph(b: &Buf, i: usize) -> usize {
    let (lines, mut line) = (b.line_count(), b.line_of(i));
    while line < lines && !b.blank_line(b.nth_line(line)) {
        line += 1;
    }
    while line < lines && b.blank_line(b.nth_line(line)) {
        line += 1;
    }
    if line >= lines {
        b.n() - 1
    } else {
        b.nth_line(line)
    }
}

/// The first line of `i`'s paragraph, or of the one before when `i` is already on it.
fn previous_paragraph(b: &Buf, i: usize) -> usize {
    let blank = |line: usize| b.blank_line(b.nth_line(line));
    let first_of = |mut line: usize| {
        while line > 0 && !blank(line - 1) {
            line -= 1;
        }
        line
    };
    let here = first_of(b.line_of(i));
    if b.nth_line(here) < i {
        return b.nth_line(here);
    }
    let mut line = here;
    while line > 0 && (line == here || blank(line)) {
        line -= 1;
    }
    b.nth_line(first_of(line))
}

fn find_char(
    buffer: &Buf,
    cursor: usize,
    find: Find,
    character: char,
    count: usize,
) -> Option<usize> {
    let (line_start, line_end) = (buffer.line_start(cursor), buffer.line_end(cursor));
    let mut at = cursor;
    for occurrence in 0..count {
        let skip_adjacent_match = usize::from(occurrence == 0 && find.till());
        at = if find.forward() {
            (at + 1 + skip_adjacent_match..line_end)
                .find(|&index| buffer.chars[index] == character)?
        } else {
            (line_start..at.saturating_sub(skip_adjacent_match))
                .rev()
                .find(|&index| buffer.chars[index] == character)?
        };
    }
    Some(match find {
        Find::Till => at - 1,
        Find::TillBack => at + 1,
        _ => at,
    })
}

fn match_bracket(b: &Buf, cur: usize) -> Option<usize> {
    let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
    let end = b.line_end(cur);
    let at = (cur..end).find(|&j| {
        pairs
            .iter()
            .any(|(o, c)| b.chars[j] == *o || b.chars[j] == *c)
    })?;
    let ch = b.chars[at];
    let (open, close, forward) = pairs.iter().find_map(|&(o, c)| {
        (ch == o)
            .then_some((o, c, true))
            .or((ch == c).then_some((o, c, false)))
    })?;
    let mut depth = 0i32;
    let closes_pair = |index: &usize| {
        let character = b.chars[*index];
        if character == open {
            depth += 1;
        } else if character == close {
            depth -= 1;
        }
        depth == 0
    };
    if forward {
        (at..b.n()).find(closes_pair)
    } else {
        (0..=at).rev().find(closes_pair)
    }
}

fn word_at(buffer: &Buf, cursor: usize, big: bool) -> Option<Range<usize>> {
    if buffer.at(cursor) == '\n' {
        return None;
    }
    let word_class = class(buffer.at(cursor), big);
    let (line_start, line_end) = (buffer.line_start(cursor), buffer.line_end(cursor));
    let mut start = cursor;
    while start > line_start && class(buffer.at(start - 1), big) == word_class {
        start -= 1;
    }
    let mut end = cursor + 1;
    while end < line_end && class(buffer.at(end), big) == word_class {
        end += 1;
    }
    Some(start..end)
}

/// A text object at `cur`, as character indices.
/// `count` text objects from `cur`, as Vim takes them. Words and paragraphs run on, each from the
/// end of the one before (a space run counts as a word, a blank run as a paragraph). Brackets
/// grow out a pair at a time. Quotes take no count.
fn counted_object(
    b: &Buf,
    cur: usize,
    object: Object,
    around: bool,
    count: usize,
) -> Option<Range<usize>> {
    let first = text_object(b, cur, object, around)?;
    match object {
        Object::Word | Object::BigWord | Object::Paragraph => {
            let mut range = first;
            for _ in 1..count {
                // A paragraph ends before its newline, so the next one starts past it.
                let from =
                    range.end + usize::from(object == Object::Paragraph && b.at(range.end) == '\n');
                if from >= b.n() {
                    break;
                }
                let Some(next) = text_object(b, from, object, around) else {
                    break;
                };
                range.end = next.end.max(range.end);
            }
            Some(range)
        }
        Object::Pair(..) => {
            let mut range = first;
            for _ in 1..count {
                // The pair around this one: start looking from just outside its opening bracket.
                let outer_at = if around { range.start } else { range.start - 1 };
                let Some(outer) = outer_at
                    .checked_sub(1)
                    .and_then(|at| text_object(b, at, object, around))
                else {
                    break;
                };
                if outer.start >= range.start {
                    break;
                }
                range = outer;
            }
            Some(range)
        }
        Object::Quotes(_) => Some(first),
    }
}

fn text_object(b: &Buf, cur: usize, object: Object, around: bool) -> Option<Range<usize>> {
    match object {
        Object::Word | Object::BigWord => {
            let big = object == Object::BigWord;
            let word = word_at(b, cur, big)?;
            if !around {
                return Some(word);
            }
            let line_end = b.line_end(cur);
            let mut trailing_space_end = word.end;
            while trailing_space_end < line_end && b.at(trailing_space_end).is_whitespace() {
                trailing_space_end += 1;
            }
            if trailing_space_end > word.end {
                return Some(word.start..trailing_space_end);
            }
            let line_start = b.line_start(cur);
            let mut leading_space_start = word.start;
            while leading_space_start > line_start && b.at(leading_space_start - 1).is_whitespace()
            {
                leading_space_start -= 1;
            }
            Some(leading_space_start..word.end)
        }
        Object::Paragraph => {
            let blank = b.blank_line(cur);
            let mut first = b.line_of(cur);
            while first > 0 && b.blank_line(b.nth_line(first - 1)) == blank {
                first -= 1;
            }
            let mut last = b.line_of(cur);
            while last + 1 < b.line_count() && b.blank_line(b.nth_line(last + 1)) == blank {
                last += 1;
            }
            if around && last + 1 < b.line_count() {
                while last + 1 < b.line_count() && b.blank_line(b.nth_line(last + 1)) != blank {
                    last += 1;
                }
            }
            let end = b.line_end(b.nth_line(last));
            Some(b.nth_line(first)..(end + usize::from(around && end < b.n())).min(b.n()))
        }
        Object::Quotes(ch) => {
            let (s, e) = (b.line_start(cur), b.line_end(cur));
            let quotes: Vec<usize> = (s..e).filter(|&j| b.chars[j] == ch).collect();
            let pair = quotes
                .chunks(2)
                .find(|p| p.len() == 2 && p[0] <= cur && cur <= p[1])
                .or_else(|| quotes.chunks(2).find(|p| p.len() == 2 && p[0] > cur))?;
            Some(if around {
                pair[0]..pair[1] + 1
            } else {
                pair[0] + 1..pair[1]
            })
        }
        Object::Pair(open, close) => {
            let mut depth = 0i32;
            let start = (0..=cur.min(b.n() - 1)).rev().find(|&j| {
                let c = b.chars[j];
                if c == close && j != cur {
                    depth += 1;
                }
                if c == open {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                false
            })?;
            let mut depth = 0i32;
            let end = (start..b.n()).find(|&j| {
                let c = b.chars[j];
                if c == open {
                    depth += 1;
                }
                if c == close {
                    depth -= 1;
                }
                depth == 0
            })?;
            Some(if around {
                start..end + 1
            } else {
                start + 1..end
            })
        }
    }
}

#[cfg(test)]
mod tests;
