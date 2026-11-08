//! Vim's keys for reading text: a cursor and visual selections moved with Vim's motions,
//! text objects, operators and search. It knows nothing about what draws the text: the app keeps
//! a [`Vim`] beside the text it shows (a page's [`RenderedText`](rune_kit::markdown::RenderedText),
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
    Command, Find, Hint, Keymap, Motion, NoAction, Object, Operator, Stroke, Table, hint,
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

/// What a key asked the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimEffect<A> {
    /// The cursor, selection or search changed, or a key was taken toward a command: repaint,
    /// and scroll the cursor into view.
    Moved,
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
        action: A,
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
        op: Operator<A>,
        keys: Vec<Stroke>,
        count: Option<usize>,
        typed: Vec<Stroke>,
    },
    /// `i` or `a`, waiting for an object; after an operator, or in Visual mode.
    Object {
        around: bool,
        op: Option<OpState<A>>,
        typed: Vec<Stroke>,
    },
    /// `f F t T`, waiting for the character; after an operator, its keys and count too.
    Find {
        find: Find,
        op: Option<OpState<A>>,
    },
}

/// The state: mode, cursor, selection anchor and whatever keys are half typed, with the keymap
/// that says what keys do. Positions are byte offsets into the text passed with each key.
#[derive(Clone, Debug)]
pub struct Vim<A = NoAction> {
    keys: Keymap<A>,
    mode: VimMode,
    cursor: usize,
    anchor: usize,
    /// The column `j` and `k` aim for.
    want_col: usize,
    count: Option<usize>,
    /// The count the last motion took, for `G`, which reads it as a line number.
    count_was: Option<usize>,
    pending: Pending<A>,
    last_find: Option<(Find, char)>,
    /// The character just typed after `f F t T`, for the motion to take.
    last_typed_find: Option<char>,
    typing: String,
    query: String,
    backward: bool,
    /// The mode a search returns to.
    anchor_search: VimMode,
    /// Escape hid the last search's matches; the next search shows them again.
    matches_hidden: bool,
    /// [`Vim::skip_blank_lines`].
    skip_blank: bool,
    message: Option<String>,
}

impl Default for Vim<NoAction> {
    fn default() -> Self {
        Vim::new(Keymap::vim())
    }
}

impl<A: Clone> Vim<A> {
    pub fn new(keys: Keymap<A>) -> Vim<A> {
        Vim {
            keys,
            mode: VimMode::Normal,
            cursor: 0,
            anchor: 0,
            want_col: 0,
            count: None,
            count_was: None,
            pending: Pending::None,
            last_find: None,
            last_typed_find: None,
            typing: String::new(),
            query: String::new(),
            backward: false,
            anchor_search: VimMode::Normal,
            matches_hidden: false,
            skip_blank: false,
            message: None,
        }
    }

    /// Never rest the cursor on an empty line: `j`, `k` and the other motions step over them,
    /// and `{` `}` go to the start of the previous and next paragraph. For text whose blank
    /// lines only separate blocks and draw nothing, such as a rendered page.
    pub fn skip_blank_lines(mut self, on: bool) -> Self {
        self.skip_blank = on;
        self
    }

    pub fn keymap(&self) -> &Keymap<A> {
        &self.keys
    }

    pub fn mode(&self) -> VimMode {
        self.mode
    }

    fn visual(&self) -> bool {
        matches!(self.mode, VimMode::Visual | VimMode::VisualLine)
    }

    /// The mode for a status line: `NORMAL`, `VISUAL`, `V-LINE`, or the search being typed.
    pub fn status(&self) -> String {
        match self.mode {
            VimMode::Normal => "NORMAL".to_owned(),
            VimMode::Visual => "VISUAL".to_owned(),
            VimMode::VisualLine => "V-LINE".to_owned(),
            VimMode::Search => format!("{}{}", if self.backward { '?' } else { '/' }, self.typing),
        }
    }

    /// Whether keys are half typed toward a command (a count, `g`, `y`, `yi`, `]`).
    pub fn is_pending(&self) -> bool {
        self.count.is_some() || !matches!(self.pending, Pending::None)
    }

    /// Keys typed toward a command that isn't finished, as `2y` or `yi`.
    pub fn pending(&self) -> String {
        let count = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
        let r = |k: &[Stroke]| self.keys.render(k);
        let ai = |around: bool| if around { "a" } else { "i" };
        match &self.pending {
            Pending::None => count(self.count),
            Pending::Keys(typed) => format!("{}{}", count(self.count), r(typed)),
            Pending::Operator {
                keys,
                count: c,
                typed,
                ..
            } => format!("{}{}{}{}", count(*c), r(keys), count(self.count), r(typed)),
            Pending::Object {
                around,
                op: Some((_, keys, c)),
                typed,
            } => format!("{}{}{}{}", count(*c), r(keys), ai(*around), r(typed)),
            Pending::Object {
                around,
                op: None,
                typed,
            } => format!("{}{}", ai(*around), r(typed)),
            Pending::Find { find, op } => {
                let f = match find {
                    Find::To => 'f',
                    Find::ToBack => 'F',
                    Find::Till => 't',
                    Find::TillBack => 'T',
                };
                match op {
                    Some((_, keys, c)) => format!("{}{}{f}", count(*c), r(keys)),
                    None => format!("{}{f}", count(self.count)),
                }
            }
        }
    }

    /// What can follow the keys typed so far, for a which-key panel: what's been typed (`y`,
    /// `gy`, `yi`, `]`, the leader) or the visual mode, and the bound keys that can come next,
    /// grouped: the app's, then Vim's commands, then motions. `None` in Normal mode with
    /// nothing half typed, and while a search is being typed.
    pub fn hints(&self) -> Option<(String, Vec<Vec<Hint>>)> {
        let typed = self.pending();
        match &self.pending {
            Pending::None if self.visual() && self.count.is_none() => Some((
                if self.mode == VimMode::Visual {
                    "visual"
                } else {
                    "v-line"
                }
                .into(),
                self.keys.next(Table::Visual, &[], false),
            )),
            Pending::None => None,
            Pending::Keys(keys) => {
                let table = if self.visual() {
                    Table::Visual
                } else {
                    Table::Normal
                };
                Some((typed, self.keys.next(table, keys, true)))
            }
            Pending::Operator {
                keys,
                typed: motion,
                ..
            } => {
                let mut groups = Vec::new();
                if motion.is_empty() {
                    groups.push(vec![hint(&self.keys.render(keys), "The line")]);
                }
                groups.extend(self.keys.next(Table::Pending, motion, true));
                Some((typed, groups))
            }
            Pending::Object { typed: keys, .. } => {
                Some((typed, vec![self.keys.next_objects(keys)]))
            }
            Pending::Find { find, .. } => Some((
                typed,
                vec![vec![hint(
                    "any",
                    if find.till() {
                        "The character to stop before"
                    } else {
                        "The character to go to"
                    },
                )]],
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
        let i = b.index(self.cursor.min(text.len()));
        b.byte(i)..b.byte(i + 1)
    }

    /// Puts the cursor on the character at byte `at` of `text` and drops back to Normal, as a
    /// jump the app makes (to a mark, a comment) does.
    pub fn set_cursor(&mut self, at: usize, text: &str) {
        let b = Buf::new(text);
        if b.n() == 0 {
            return;
        }
        let i = b.index(at.min(text.len())).min(b.n() - 1);
        self.cursor = b.byte(i);
        self.want_col = i - b.line_start(i);
        self.mode = VimMode::Normal;
        self.reset_pending();
    }

    /// The selected bytes in visual modes.
    pub fn selection(&self, text: &str) -> Option<Range<usize>> {
        let b = Buf::new(text);
        let (c, a) = (b.index(self.cursor), b.index(self.anchor));
        let (lo, hi) = (c.min(a), c.max(a));
        match self.mode {
            VimMode::Visual => Some(b.byte(lo)..b.byte(hi + 1)),
            VimMode::VisualLine => Some(b.byte(b.line_start(lo))..b.byte(b.line_end(hi))),
            _ => None,
        }
    }

    /// Where the last search matches, for painting.
    pub fn matches(&self, text: &str) -> Vec<Range<usize>> {
        if self.mode == VimMode::Search {
            return find_all(text, &self.typing);
        }
        if self.matches_hidden {
            return Vec::new();
        }
        find_all(text, &self.query)
    }

    /// Handles `ev` over `text`. `None` when the key means nothing here, so the app can use it.
    pub fn key(&mut self, ev: &KeyEvent, text: &str) -> Option<VimEffect<A>> {
        let b = Buf::new(text);
        if b.n() == 0 {
            return None;
        }
        self.cursor = b.byte(b.index(self.cursor));
        self.message = None;
        if self.mode == VimMode::Search {
            return self.search_key(ev, text, &b);
        }
        if ev.key == Key::Escape {
            let busy = self.is_pending() || self.visual();
            // With nothing else to cancel, Escape hides the search's matches, as `:noh` does.
            let shown = !busy && !self.matches_hidden && !self.query.is_empty();
            if shown {
                self.matches_hidden = true;
            }
            self.reset_pending();
            self.mode = VimMode::Normal;
            return (busy || shown).then_some(VimEffect::Moved);
        }
        let stroke = Stroke::from_event(ev)?;

        // The character after `f F t T` is read, not looked up.
        if matches!(self.pending, Pending::Find { .. })
            && let Pending::Find { find, op } = std::mem::replace(&mut self.pending, Pending::None)
        {
            let Stroke::Char(c) = stroke else {
                self.reset_pending();
                return Some(VimEffect::Moved);
            };
            self.last_find = Some((find, c));
            self.last_typed_find = Some(c);
            return self.finish_motion(Motion::Find(find), op, text, &b);
        }

        // Counts: 1–9 start one, 0 goes on one; not inside a key sequence or object.
        if let Stroke::Char(c @ '0'..='9') = stroke {
            let between = matches!(&self.pending, Pending::None)
                || matches!(&self.pending, Pending::Operator { typed, .. } if typed.is_empty());
            if between && (c != '0' || self.count.is_some()) {
                let digit = c as usize - '0' as usize;
                self.count = Some((self.count.unwrap_or(0) * 10 + digit).min(MAX_COUNT));
                return Some(VimEffect::Moved);
            }
        }

        match std::mem::replace(&mut self.pending, Pending::None) {
            Pending::None => self.lookup(stroke, Vec::new(), text, &b),
            Pending::Keys(typed) => self.lookup(stroke, typed, text, &b),
            Pending::Operator {
                op,
                keys,
                count,
                mut typed,
            } => {
                typed.push(stroke);
                // The operator's keys again, or its last key, take the line (`yy`, `cc`, `gyy`
                // and `gygy`), as Vim's `gUU` and `gUgU` do.
                let last = &keys[keys.len() - 1..];
                if typed == keys || typed == last {
                    return Some(self.operate_lines(op, count, text, &b));
                }
                let line_prefix = keys.starts_with(&typed);
                match self.keys.lookup(Table::Pending, &typed) {
                    Lookup::Exact(Command::Motion(m)) => {
                        self.finish_motion(m, Some((op, keys, count)), text, &b)
                    }
                    Lookup::Exact(Command::Inner) => {
                        self.pending = Pending::Object {
                            around: false,
                            op: Some((op, keys, count)),
                            typed: Vec::new(),
                        };
                        Some(VimEffect::Moved)
                    }
                    Lookup::Exact(Command::Around) => {
                        self.pending = Pending::Object {
                            around: true,
                            op: Some((op, keys, count)),
                            typed: Vec::new(),
                        };
                        Some(VimEffect::Moved)
                    }
                    Lookup::Prefix => {
                        self.pending = Pending::Operator {
                            op,
                            keys,
                            count,
                            typed,
                        };
                        Some(VimEffect::Moved)
                    }
                    _ if line_prefix => {
                        self.pending = Pending::Operator {
                            op,
                            keys,
                            count,
                            typed,
                        };
                        Some(VimEffect::Moved)
                    }
                    _ => {
                        self.reset_pending();
                        Some(VimEffect::Moved)
                    }
                }
            }
            Pending::Object {
                around,
                op,
                mut typed,
            } => {
                typed.push(stroke);
                match self.keys.lookup_object(&typed) {
                    Lookup::Exact(object) => self.finish_object(object, around, op, text, &b),
                    Lookup::Prefix => {
                        self.pending = Pending::Object { around, op, typed };
                        Some(VimEffect::Moved)
                    }
                    Lookup::None => {
                        self.reset_pending();
                        Some(VimEffect::Moved)
                    }
                }
            }
            Pending::Find { .. } => unreachable!("handled above"),
        }
    }

    /// Looks `typed` and `stroke` up in the mode's table.
    fn lookup(
        &mut self,
        stroke: Stroke,
        mut typed: Vec<Stroke>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A>> {
        let first = typed.is_empty();
        typed.push(stroke);
        let table = if self.visual() {
            Table::Visual
        } else {
            Table::Normal
        };
        match self.keys.lookup(table, &typed) {
            Lookup::Exact(command) => self.run(command, typed, text, b),
            Lookup::Prefix => {
                self.pending = Pending::Keys(typed);
                Some(VimEffect::Moved)
            }
            // An unbound first key is the app's; a sequence that went nowhere is dropped.
            Lookup::None if first && self.count.is_none() => None,
            Lookup::None => {
                self.reset_pending();
                Some(VimEffect::Moved)
            }
        }
    }

    fn run(
        &mut self,
        command: Command<A>,
        keys: Vec<Stroke>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A>> {
        let visual = self.visual();
        match command {
            Command::Motion(m) => self.finish_motion(m, None, text, b),
            Command::Operator(op) if visual => {
                let range = self.selection(text)?;
                let lines = self.mode == VimMode::VisualLine;
                self.count = None;
                Some(self.apply(op, range, lines, text, b))
            }
            Command::Operator(op) => {
                let count = self.count.take();
                self.pending = Pending::Operator {
                    op,
                    keys,
                    count,
                    typed: Vec::new(),
                };
                Some(VimEffect::Moved)
            }
            Command::Lines(op) => {
                let count = self.count.take();
                Some(self.operate_lines(op, count, text, b))
            }
            Command::Inner | Command::Around => {
                self.pending = Pending::Object {
                    around: matches!(command, Command::Around),
                    op: None,
                    typed: Vec::new(),
                };
                Some(VimEffect::Moved)
            }
            Command::Visual | Command::VisualLine => {
                let mode = if matches!(command, Command::Visual) {
                    VimMode::Visual
                } else {
                    VimMode::VisualLine
                };
                if self.mode == mode {
                    self.mode = VimMode::Normal;
                } else {
                    if !visual {
                        self.anchor = self.cursor;
                    }
                    self.mode = mode;
                }
                self.reset_pending();
                Some(VimEffect::Moved)
            }
            Command::OtherEnd => {
                std::mem::swap(&mut self.cursor, &mut self.anchor);
                self.reset_pending();
                Some(VimEffect::Moved)
            }
            Command::Search { backward } => {
                self.reset_pending();
                self.backward = backward;
                self.typing.clear();
                self.anchor_search = self.mode;
                self.mode = VimMode::Search;
                Some(VimEffect::Moved)
            }
            Command::App(action) => {
                let count = self.count.take();
                self.reset_pending();
                Some(VimEffect::Action { action, count })
            }
        }
    }

    fn reset_pending(&mut self) {
        self.pending = Pending::None;
        self.count = None;
    }

    /// The count for a motion, times the operator's: `2y3w` moves six words.
    fn motion_count(&mut self, op_count: Option<usize>) -> usize {
        self.count_was = self.count;
        let n = (self.count.take().unwrap_or(1) * op_count.unwrap_or(1)).min(MAX_COUNT);
        n.max(1)
    }

    /// Runs motion `m`: moves the cursor, or after an operator, applies it over what the motion
    /// covers. `f F t T` wait for their character first.
    fn finish_motion(
        &mut self,
        m: Motion,
        op: Option<OpState<A>>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A>> {
        let find = match m {
            Motion::Find(find) => match self.last_typed_find.take() {
                Some(c) => Some((find, c)),
                None => {
                    self.pending = Pending::Find { find, op };
                    return Some(VimEffect::Moved);
                }
            },
            Motion::RepeatFind | Motion::RepeatFindReverse => match self.last_find {
                Some((find, c)) => Some((
                    if m == Motion::RepeatFind {
                        find
                    } else {
                        find.reverse()
                    },
                    c,
                )),
                None => {
                    self.reset_pending();
                    return Some(VimEffect::Moved);
                }
            },
            _ => None,
        };
        let op_count = op.as_ref().and_then(|(_, _, c)| *c);
        let n = self.motion_count(op_count);
        let cur = b.index(self.cursor);
        let (target, kind) = match (m, find) {
            (_, Some((find, c))) => {
                let Some(t) = find_char(b, cur, find, c, n) else {
                    self.reset_pending();
                    return Some(VimEffect::Moved);
                };
                (
                    t,
                    if find.forward() {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            (Motion::Left, _) => (
                cur.saturating_sub(n).max(b.line_start(cur)),
                Kind::Exclusive,
            ),
            (Motion::Right, _) => ((cur + n).min(b.line_last(cur)), Kind::Exclusive),
            (Motion::Down | Motion::Up | Motion::HalfPageDown | Motion::HalfPageUp, _) => {
                let step = if matches!(m, Motion::HalfPageDown | Motion::HalfPageUp) {
                    15 * n
                } else {
                    n
                };
                let down = matches!(m, Motion::Down | Motion::HalfPageDown);
                let line = b.line_of(cur);
                let line = if self.skip_blank {
                    // Count only lines that draw something.
                    let solid: Vec<usize> = (0..b.line_count())
                        .filter(|&l| !b.blank_line(b.nth_line(l)))
                        .collect();
                    let here = solid.partition_point(|&l| l < line);
                    let to = if down {
                        (here + step).min(solid.len().saturating_sub(1))
                    } else {
                        here.saturating_sub(step)
                    };
                    solid.get(to).copied().unwrap_or(line)
                } else if down {
                    (line + step).min(b.line_count() - 1)
                } else {
                    line.saturating_sub(step)
                };
                let start = b.nth_line(line);
                let want = self.want_col;
                let effect = self.go(
                    start + want.min(b.line_last(start) - start),
                    Kind::Linewise,
                    op,
                    text,
                    b,
                );
                self.want_col = want;
                return effect;
            }
            (Motion::LineStart, _) => (b.line_start(cur), Kind::Exclusive),
            (Motion::FirstNonBlank, _) => (b.first_non_blank(cur), Kind::Exclusive),
            (Motion::LineEnd, _) => {
                let line = (b.line_of(cur) + n - 1).min(b.line_count() - 1);
                (b.line_last(b.nth_line(line)), Kind::Inclusive)
            }
            (Motion::Top | Motion::Bottom, _) => {
                let last = b.line_count() - 1;
                let line = match (self.count_was.take(), m) {
                    (Some(n), _) => n.saturating_sub(1).min(last),
                    (None, Motion::Top) => 0,
                    (None, _) => last,
                };
                (b.first_non_blank(b.nth_line(line)), Kind::Linewise)
            }
            (Motion::WordForward | Motion::BigWordForward, _) => {
                let big = m == Motion::BigWordForward;
                (repeat(n, cur, |i| word_forward(b, i, big)), Kind::Exclusive)
            }
            (Motion::WordBackward | Motion::BigWordBackward, _) => {
                let big = m == Motion::BigWordBackward;
                (
                    repeat(n, cur, |i| word_backward(b, i, big)),
                    Kind::Exclusive,
                )
            }
            (Motion::WordEnd | Motion::BigWordEnd, _) => {
                let big = m == Motion::BigWordEnd;
                (repeat(n, cur, |i| word_end(b, i, big)), Kind::Inclusive)
            }
            (Motion::ParagraphForward, _) if self.skip_blank => {
                (repeat(n, cur, |i| next_paragraph(b, i)), Kind::Exclusive)
            }
            (Motion::ParagraphBackward, _) if self.skip_blank => (
                repeat(n, cur, |i| previous_paragraph(b, i)),
                Kind::Exclusive,
            ),
            (Motion::ParagraphForward, _) => {
                (repeat(n, cur, |i| paragraph(b, i, true)), Kind::Exclusive)
            }
            (Motion::ParagraphBackward, _) => {
                (repeat(n, cur, |i| paragraph(b, i, false)), Kind::Exclusive)
            }
            (Motion::MatchingBracket, _) => match match_bracket(b, cur) {
                Some(t) => (t, Kind::Inclusive),
                None => {
                    self.reset_pending();
                    return Some(VimEffect::Moved);
                }
            },
            (Motion::SearchNext | Motion::SearchPrevious, _) => {
                let backward = self.backward != (m == Motion::SearchPrevious);
                match self.next_match(text, backward) {
                    Some(t) => (b.index(t), Kind::Exclusive),
                    None => {
                        self.reset_pending();
                        return Some(VimEffect::Moved);
                    }
                }
            }
            (Motion::SearchWord | Motion::SearchWordBack, _) => {
                let Some(word) = word_at(b, cur, false) else {
                    self.reset_pending();
                    return Some(VimEffect::Moved);
                };
                self.query = format!("\\b{}", b.chars[word].iter().collect::<String>());
                self.backward = m == Motion::SearchWordBack;
                match self.next_match(text, self.backward) {
                    Some(t) => (b.index(t), Kind::Exclusive),
                    None => {
                        self.reset_pending();
                        return Some(VimEffect::Moved);
                    }
                }
            }
            (Motion::Find(_) | Motion::RepeatFind | Motion::RepeatFindReverse, None) => {
                unreachable!("found above")
            }
        };
        self.go(target, kind, op, text, b)
    }

    /// Moves the cursor to `target`, or with an operator, applies it from the cursor to there.
    fn go(
        &mut self,
        target: usize,
        kind: Kind,
        op: Option<OpState<A>>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A>> {
        let cur = b.index(self.cursor);
        self.pending = Pending::None;
        if let Some((op, _, _)) = op {
            let (lo, hi) = (cur.min(target), cur.max(target));
            let range = match kind {
                Kind::Exclusive => b.byte(lo)..b.byte(hi),
                Kind::Inclusive => b.byte(lo)..b.byte(hi + 1),
                Kind::Linewise => b.byte(b.line_start(lo))..b.byte(b.line_end(hi)),
            };
            return Some(self.apply(op, range, kind == Kind::Linewise, text, b));
        }
        let mut target = target.min(b.n() - 1);
        if self.skip_blank && b.at(target) == '\n' && b.blank_line(target) {
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
        self.want_col = target - b.line_start(target);
        Some(VimEffect::Moved)
    }

    /// A text object at the cursor: selected in Visual mode, or what an operator acts on.
    fn finish_object(
        &mut self,
        object: Object,
        around: bool,
        op: Option<OpState<A>>,
        text: &str,
        b: &Buf,
    ) -> Option<VimEffect<A>> {
        self.reset_pending();
        let cur = b.index(self.cursor);
        let Some(range) = text_object(b, cur, object, around) else {
            return Some(VimEffect::Moved);
        };
        match op {
            Some((op, _, _)) => {
                Some(self.apply(op, b.byte(range.start)..b.byte(range.end), false, text, b))
            }
            None => {
                self.mode = VimMode::Visual;
                self.anchor = b.byte(range.start);
                self.cursor = b.byte(range.end.saturating_sub(1).max(range.start));
                Some(VimEffect::Moved)
            }
        }
    }

    /// The operator over whole lines from the cursor's: as many as `count` times the count
    /// typed after the operator (`2yy`, `y2y`).
    fn operate_lines(
        &mut self,
        op: Operator<A>,
        count: Option<usize>,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A> {
        let n = (count.unwrap_or(1) * self.count.take().unwrap_or(1)).clamp(1, MAX_COUNT);
        self.reset_pending();
        let cur = b.index(self.cursor);
        let first = b.line_start(cur);
        let last = b.line_end(b.nth_line((b.line_of(cur) + n - 1).min(b.line_count() - 1)));
        self.apply(op, b.byte(first)..b.byte(last), true, text, b)
    }

    fn apply(
        &mut self,
        op: Operator<A>,
        range: Range<usize>,
        lines: bool,
        text: &str,
        b: &Buf,
    ) -> VimEffect<A> {
        self.reset_pending();
        self.mode = VimMode::Normal;
        self.cursor = b.byte(b.index(range.start));
        self.want_col = b.index(range.start) - b.line_start(b.index(range.start));
        match op {
            Operator::Yank | Operator::YankSource => {
                let source = matches!(op, Operator::YankSource);
                let yanked = text.get(range.clone()).unwrap_or_default().to_owned();
                let count = yanked.lines().count().max(1);
                let what = if source { " as source" } else { "" };
                self.message = Some(if lines || count > 1 {
                    format!(
                        "{count} line{} yanked{what}",
                        if count == 1 { "" } else { "s" }
                    )
                } else {
                    format!("{} characters yanked{what}", yanked.chars().count())
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

    fn search_key(&mut self, ev: &KeyEvent, text: &str, b: &Buf) -> Option<VimEffect<A>> {
        match &ev.key {
            Key::Escape => {
                self.mode = self.anchor_search;
                self.typing.clear();
            }
            Key::Backspace => {
                if self.typing.pop().is_none() {
                    self.mode = self.anchor_search;
                }
            }
            Key::Enter => {
                self.mode = self.anchor_search;
                if !self.typing.is_empty() {
                    self.query = std::mem::take(&mut self.typing);
                }
                if let Some(t) = self.next_match(text, self.backward) {
                    return self.go(b.index(t), Kind::Exclusive, None, text, b);
                }
            }
            Key::Text(s) if !ev.mods.cmd => self.typing.push_str(s),
            _ => return None,
        }
        Some(VimEffect::Moved)
    }

    /// The byte where the search's next (or previous) match starts, wrapping round, with a note
    /// of which match it is; `None`, and a note, when there's none.
    fn next_match(&mut self, text: &str, backward: bool) -> Option<usize> {
        self.matches_hidden = false;
        let found = find_all(text, &self.query);
        if found.is_empty() {
            self.message = Some(format!(
                "Pattern not found: {}",
                self.query.trim_start_matches("\\b")
            ));
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

/// An operator waiting for its motion: the operator, the keys that started it, its count.
type OpState<A> = (Operator<A>, Vec<Stroke>, Option<usize>);

/// The text as characters, with each character's byte offset, for one key.
struct Buf {
    chars: Vec<char>,
    offs: Vec<usize>,
    len: usize,
}

impl Buf {
    fn new(text: &str) -> Buf {
        let (offs, chars) = text.char_indices().unzip();
        Buf {
            chars,
            offs,
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
        (0..i.min(self.n()))
            .rev()
            .find(|&j| self.chars[j] == '\n')
            .map_or(0, |j| j + 1)
    }
    /// The newline ending `i`'s line, or the end.
    fn line_end(&self, i: usize) -> usize {
        (i..self.n())
            .find(|&j| self.chars[j] == '\n')
            .unwrap_or(self.n())
    }
    /// The last character of `i`'s line that the cursor can sit on.
    fn line_last(&self, i: usize) -> usize {
        self.line_end(i).saturating_sub(1).max(self.line_start(i))
    }
    fn line_of(&self, i: usize) -> usize {
        self.chars[..i.min(self.n())]
            .iter()
            .filter(|&&c| c == '\n')
            .count()
    }
    fn line_count(&self) -> usize {
        self.chars.iter().filter(|&&c| c == '\n').count() + 1
    }
    fn nth_line(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        self.chars
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == '\n')
            .nth(line - 1)
            .map_or(self.n(), |(j, _)| j + 1)
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

/// 0 for space, 1 for word characters, 2 for punctuation; with `big`, anything but space is 1.
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Every place `query` occurs, case-insensitive unless it has a capital (Vim's smartcase). A
/// leading `\b` matches only at the start of a word, as `*` searches.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let (word, query) = match query.strip_prefix("\\b") {
        Some(q) => (true, q),
        None => (false, query),
    };
    let sensitive = query.chars().any(char::is_uppercase);
    let boundary = |i: usize| {
        i == 0
            || !text[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    };
    find_folded(text, query, !sensitive)
        .into_iter()
        .filter(|r| !word || boundary(r.start))
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

fn word_forward(b: &Buf, i: usize, big: bool) -> usize {
    let n = b.n();
    let mut j = i;
    let c = class(b.at(j), big);
    if c != 0 {
        while j < n && class(b.at(j), big) == c {
            j += 1;
        }
    }
    while j < n && class(b.at(j), big) == 0 {
        j += 1;
    }
    j.min(n - 1)
}

fn word_backward(b: &Buf, i: usize, big: bool) -> usize {
    let mut j = i;
    while j > 0 && class(b.at(j - 1), big) == 0 {
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

fn word_end(b: &Buf, i: usize, big: bool) -> usize {
    let n = b.n();
    let mut j = i + 1;
    while j < n && class(b.at(j), big) == 0 {
        j += 1;
    }
    if j >= n {
        return n - 1;
    }
    let c = class(b.at(j), big);
    while j + 1 < n && class(b.at(j + 1), big) == c {
        j += 1;
    }
    j
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

fn find_char(b: &Buf, cur: usize, find: Find, c: char, n: usize) -> Option<usize> {
    let (s, e) = (b.line_start(cur), b.line_end(cur));
    let mut at = cur;
    for k in 0..n {
        // `t` again from just before a match moves on to the next one.
        let skip = usize::from(k == 0 && find.till());
        at = if find.forward() {
            (at + 1 + skip..e).find(|&j| b.chars[j] == c)?
        } else {
            (s..at.saturating_sub(skip))
                .rev()
                .find(|&j| b.chars[j] == c)?
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
    let indices: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new(at..b.n())
    } else {
        Box::new((0..=at).rev())
    };
    for j in indices {
        let c = b.chars[j];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
        }
        if depth == 0 {
            return Some(j);
        }
        if !forward && depth > 0 {
            return None;
        }
    }
    None
}

/// The word (or space run) under `i`.
fn word_at(b: &Buf, i: usize, big: bool) -> Option<Range<usize>> {
    if b.at(i) == '\n' {
        return None;
    }
    let c = class(b.at(i), big);
    let (s, e) = (b.line_start(i), b.line_end(i));
    let mut lo = i;
    while lo > s && class(b.at(lo - 1), big) == c {
        lo -= 1;
    }
    let mut hi = i + 1;
    while hi < e && class(b.at(hi), big) == c {
        hi += 1;
    }
    Some(lo..hi)
}

/// A text object at `cur`, as character indices.
fn text_object(b: &Buf, cur: usize, object: Object, around: bool) -> Option<Range<usize>> {
    match object {
        Object::Word | Object::BigWord => {
            let big = object == Object::BigWord;
            let word = word_at(b, cur, big)?;
            if !around {
                return Some(word);
            }
            // `aw` takes the space after the word, or before it at the end of a line.
            let e = b.line_end(cur);
            let mut hi = word.end;
            while hi < e && b.at(hi).is_whitespace() {
                hi += 1;
            }
            if hi > word.end {
                return Some(word.start..hi);
            }
            let s = b.line_start(cur);
            let mut lo = word.start;
            while lo > s && b.at(lo - 1).is_whitespace() {
                lo -= 1;
            }
            Some(lo..word.end)
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
