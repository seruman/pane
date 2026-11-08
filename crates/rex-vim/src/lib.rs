//! Vim's keys for reading text: a cursor and visual selections moved with Vim's motions,
//! text objects, search, and yank. It knows nothing about what draws the text: the app keeps a
//! [`Vim`] beside the text it shows (a page's [`RenderedText`](rune_kit::markdown::RenderedText),
//! a log, a file), feeds it keys, and paints [`Vim::cursor`], [`Vim::selection`] and
//! [`Vim::matches`] back onto the page.
//!
//! Read-only: `y` is the one operator, and `gy` yanks the same way but asks the app for the
//! source behind the text ([`VimEffect::CopySource`]). Motions: `h j k l`, `w b e W B E`, `0 ^ $`, `gg G`,
//! `{ }`, `f F t T ; ,`, `%`, Ctrl-D and Ctrl-U, with counts. Text objects after `y`, or in
//! visual mode: `iw aw iW aW ip ap`, quotes `i" a" i' a' i` a``, brackets `i( a( ib i[ a[ i{ a{
//! iB i< a<`. `v` and `V` select, `o` swaps the ends, `y` yanks, `yy` and `Y` yank lines. `/` and
//! `?` search (smart case), `n N` repeat it, `* #` search the word under the cursor.

use std::ops::Range;

use rune::{Key, KeyEvent};

/// A key that can come next and what it does, for a which-key panel ([`Vim::hints`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub keys: &'static str,
    pub what: &'static str,
}

pub const fn hint(keys: &'static str, what: &'static str) -> Hint {
    Hint { keys, what }
}

/// What a yank can run to, as [`Vim::hints`] lists them.
const MOTIONS: [Hint; 8] = [
    hint("w", "To the next word"),
    hint("e", "To the word's end"),
    hint("b", "Back a word"),
    hint("$", "To the line's end"),
    hint("0", "From the line's start"),
    hint("}", "To the paragraph's end"),
    hint("f", "To a character"),
    hint("t", "Up to a character"),
];

/// The text objects after `i` or `a`.
const OBJECTS: [Hint; 9] = [
    hint("w", "Word"),
    hint("W", "WORD, to spaces"),
    hint("p", "Paragraph"),
    hint("\"", "Double quotes"),
    hint("'", "Single quotes"),
    hint("`", "Backticks"),
    hint("( b", "Parentheses"),
    hint("[ {", "Brackets, braces"),
    hint("<", "Angle brackets"),
];

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
pub enum VimEffect {
    /// The cursor, selection or search changed: repaint, and scroll the cursor into view.
    Moved,
    /// Copy this text (a yank).
    Copy(String),
    /// Copy the source behind this range of the text (a `gy` yank), which only the app can
    /// find: for a rendered page, the Markdown it was drawn from. `linewise` yanks take whole
    /// source lines.
    CopySource { range: Range<usize>, linewise: bool },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pending {
    #[default]
    None,
    /// `g`, waiting for the second `g`.
    G,
    /// `f F t T`, waiting for the character.
    Find(char),
    /// `y`, waiting for a motion or text object.
    Yank,
    /// `i` or `a` (after `y` or in visual mode), waiting for the object.
    Object { around: bool },
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

/// The state: mode, cursor, selection anchor and whatever keys are half typed. Positions are
/// byte offsets into the text passed with each key.
#[derive(Clone, Debug, Default)]
pub struct Vim {
    mode: VimMode,
    cursor: usize,
    anchor: usize,
    /// The column `j` and `k` aim for.
    want_col: usize,
    count: Option<usize>,
    /// The count the last motion took, for `G`, which reads it as a line number.
    count_was: Option<usize>,
    pending: Pending,
    yank_count: Option<usize>,
    /// The yank under way started with `gy`, so it copies the source.
    source_yank: bool,
    last_find: Option<(char, char)>,
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

fn key_char(ev: &KeyEvent) -> Option<char> {
    match &ev.key {
        Key::Text(s) if !ev.mods.cmd => {
            let mut chars = s.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(c)
        }
        _ => None,
    }
}

impl Vim {
    pub fn new() -> Vim {
        Vim::default()
    }

    /// Never rest the cursor on an empty line: `j`, `k` and the other motions step over them,
    /// and `{` `}` go to the start of the previous and next paragraph. For text whose blank
    /// lines only separate blocks and draw nothing, such as a rendered page.
    pub fn skip_blank_lines(mut self, on: bool) -> Self {
        self.skip_blank = on;
        self
    }

    pub fn mode(&self) -> VimMode {
        self.mode
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

    /// Keys typed toward a command that isn't finished, as `2y` or `yi`.
    pub fn pending(&self) -> String {
        let mut s = self
            .yank_count
            .or(self.count)
            .map(|n| n.to_string())
            .unwrap_or_default();
        match self.pending {
            Pending::None => {}
            Pending::G => s.push('g'),
            Pending::Find(c) => s.push(c),
            Pending::Yank => s.push_str(if self.source_yank { "gy" } else { "y" }),
            Pending::Object { around } => s.push_str(if around { "ya" } else { "yi" }),
        }
        s
    }

    /// What can follow the keys typed so far, for a which-key panel: what's been typed (`y`,
    /// `gy`, `yi`, `g`), or the visual mode, and the keys that can come next in groups. `None`
    /// in Normal mode with nothing half typed, and while a search is being typed.
    pub fn hints(&self) -> Option<(String, Vec<Vec<Hint>>)> {
        let visual = matches!(self.mode, VimMode::Visual | VimMode::VisualLine);
        let gy = if self.source_yank { "gy" } else { "y" };
        match self.pending {
            Pending::G => Some(("g".into(), vec![vec![hint("g", "Go to the top"), hint("y", "Yank as Markdown…")]])),
            Pending::Find(kind) => Some((
                kind.to_string(),
                vec![vec![hint("any", if matches!(kind, 'f' | 'F') { "The character to go to" } else { "The character to stop before" })]],
            )),
            Pending::Object { around } => {
                let typed = match (visual, around) {
                    (true, false) => "i".to_owned(),
                    (true, true) => "a".to_owned(),
                    (false, false) => format!("{gy}i"),
                    (false, true) => format!("{gy}a"),
                };
                Some((typed, vec![OBJECTS.to_vec()]))
            }
            Pending::Yank => Some((
                gy.into(),
                vec![
                    vec![hint(gy, "The line"), hint("i", "Inside an object…"), hint("a", "Around an object…")],
                    MOTIONS.to_vec(),
                ],
            )),
            Pending::None if visual => Some((
                if self.mode == VimMode::Visual { "visual" } else { "v-line" }.into(),
                vec![vec![
                    hint("y", "Yank"),
                    hint("gy", "Yank as Markdown"),
                    hint("o", "Go to the other end"),
                    hint("i", "Select inside an object…"),
                    hint("a", "Select around an object…"),
                ]],
            )),
            Pending::None => None,
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

    /// Handles `ev` over `text`. None when the key means nothing here, so the app can use it.
    pub fn key(&mut self, ev: &KeyEvent, text: &str) -> Option<VimEffect> {
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
            let busy = self.pending != Pending::None
                || self.count.is_some()
                || self.mode != VimMode::Normal;
            // With nothing else to cancel, Escape hides the search's matches, as `:noh` does.
            let shown = !busy && !self.matches_hidden && !self.query.is_empty();
            if shown {
                self.matches_hidden = true;
            }
            self.reset_pending();
            self.mode = VimMode::Normal;
            return (busy || shown).then_some(VimEffect::Moved);
        }
        let ch = match (&ev.key, ev.mods.ctrl) {
            (Key::Command('d'), true) => 'D',
            (Key::Command('u'), true) => 'U',
            (Key::Left, _) => 'h',
            (Key::Right, _) => 'l',
            (Key::Up, _) => 'k',
            (Key::Down, _) => 'j',
            _ => key_char(ev)?,
        };
        // Ctrl-D and Ctrl-U, held apart from `D` and `U`, which do nothing here.
        let ctrl = ev.mods.ctrl && matches!(ch, 'D' | 'U');
        self.command(ch, ctrl, text, &b)
    }

    fn reset_pending(&mut self) {
        self.pending = Pending::None;
        self.count = None;
        self.yank_count = None;
        self.source_yank = false;
    }

    fn take_count(&mut self) -> usize {
        let n = self.count.take().unwrap_or(1) * self.yank_count.take().unwrap_or(1);
        n.max(1)
    }

    fn command(&mut self, ch: char, ctrl: bool, text: &str, b: &Buf) -> Option<VimEffect> {
        let cur = b.index(self.cursor);
        match self.pending {
            Pending::G => {
                self.pending = Pending::None;
                // `gy` yanks like `y`, but copies the source behind the text.
                if ch == 'y' {
                    self.source_yank = true;
                    if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
                        return self.command('y', false, text, b);
                    }
                    self.yank_count = self.count.take();
                    self.pending = Pending::Yank;
                    return Some(VimEffect::Moved);
                }
                if ch != 'g' {
                    self.reset_pending();
                    return Some(VimEffect::Moved);
                }
                let line = self.count.take().map_or(0, |n| n.saturating_sub(1));
                return self.motion(b.first_non_blank(b.nth_line(line)), Kind::Linewise, text, b);
            }
            Pending::Find(kind) => {
                self.pending = Pending::None;
                self.last_find = Some((kind, ch));
                let n = self.take_count_keep_yank();
                let target = find_char(b, cur, kind, ch, n)?;
                return self.motion(
                    target,
                    if matches!(kind, 'f' | 't') {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                    text,
                    b,
                );
            }
            Pending::Object { around } => {
                self.pending = Pending::None;
                let range = object(b, cur, ch, around, self.anchor_index(b))?;
                return self.object_done(range, text, b);
            }
            Pending::None | Pending::Yank => {}
        }
        if ch.is_ascii_digit() && !ctrl && (ch != '0' || self.count.is_some()) {
            let digit = ch as usize - '0' as usize;
            self.count = Some(self.count.unwrap_or(0) * 10 + digit);
            return Some(VimEffect::Moved);
        }
        let visual = matches!(self.mode, VimMode::Visual | VimMode::VisualLine);
        let yanking = self.pending == Pending::Yank;
        match ch {
            'g' => {
                self.pending = Pending::G;
                return Some(VimEffect::Moved);
            }
            'f' | 'F' | 't' | 'T' => {
                self.pending = Pending::Find(ch);
                return Some(VimEffect::Moved);
            }
            'i' | 'a' if yanking || visual => {
                self.pending = Pending::Object { around: ch == 'a' };
                return Some(VimEffect::Moved);
            }
            'v' | 'V' if !yanking => {
                let mode = if ch == 'v' {
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
                return Some(VimEffect::Moved);
            }
            'o' if visual => {
                std::mem::swap(&mut self.cursor, &mut self.anchor);
                return Some(VimEffect::Moved);
            }
            'y' if visual => {
                let range = self.selection(text)?;
                let lines = self.mode == VimMode::VisualLine;
                let source = self.source_yank;
                self.reset_pending();
                self.source_yank = source;
                return Some(self.yank(range, lines, text, b));
            }
            'y' if yanking => {
                // `yy`: whole lines from the cursor's.
                let n = self.take_count();
                self.pending = Pending::None;
                let first = b.line_start(cur);
                let last = b.line_end(b.nth_line((b.line_of(cur) + n - 1).min(b.line_count() - 1)));
                return Some(self.yank(b.byte(first)..b.byte(last), true, text, b));
            }
            'y' => {
                self.yank_count = self.count.take();
                self.pending = Pending::Yank;
                return Some(VimEffect::Moved);
            }
            'Y' if !visual => {
                self.pending = Pending::Yank;
                return self.command('y', false, text, b);
            }
            '/' | '?' if !yanking => {
                self.reset_pending();
                self.backward = ch == '?';
                self.typing.clear();
                self.anchor_search = self.mode;
                self.mode = VimMode::Search;
                return Some(VimEffect::Moved);
            }
            'n' | 'N' => {
                self.reset_pending();
                let backward = self.backward != (ch == 'N');
                return self.jump_to_match(text, b, backward);
            }
            '*' | '#' => {
                self.reset_pending();
                let range = word_at(b, cur, false)?;
                self.query = format!("\\b{}", b.chars[range].iter().collect::<String>());
                self.backward = ch == '#';
                return self.jump_to_match(text, b, self.backward);
            }
            ';' | ',' => {
                let (kind, c) = self.last_find?;
                let kind = if ch == ',' { reverse_find(kind) } else { kind };
                let n = self.take_count_keep_yank();
                let target = find_char(b, cur, kind, c, n)?;
                return self.motion(
                    target,
                    if matches!(kind, 'f' | 't') {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                    text,
                    b,
                );
            }
            _ => {}
        }
        let n = self.take_count_keep_yank();
        let (target, kind) = match ch {
            'h' => (
                (cur.saturating_sub(n)).max(b.line_start(cur)),
                Kind::Exclusive,
            ),
            'l' => ((cur + n).min(b.line_last(cur)), Kind::Exclusive),
            'j' | 'k' | 'D' | 'U' => {
                let step = if matches!(ch, 'D' | 'U') { 15 * n } else { n };
                let line = b.line_of(cur);
                let down = matches!(ch, 'j' | 'D');
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
                let col = self.want_col.min(b.line_last(start) - start);
                return self.motion_keep_col(start + col, text, b);
            }
            '0' => (b.line_start(cur), Kind::Exclusive),
            '^' => (b.first_non_blank(cur), Kind::Exclusive),
            '$' => {
                let line = (b.line_of(cur) + n - 1).min(b.line_count() - 1);
                (b.line_last(b.nth_line(line)), Kind::Inclusive)
            }
            'G' => {
                let line = self.count_was.take().map_or(b.line_count() - 1, |n| {
                    n.saturating_sub(1).min(b.line_count() - 1)
                });
                (b.first_non_blank(b.nth_line(line)), Kind::Linewise)
            }
            'w' | 'W' => (
                repeat(n, cur, |i| word_forward(b, i, ch == 'W')),
                Kind::Exclusive,
            ),
            'b' | 'B' => (
                repeat(n, cur, |i| word_backward(b, i, ch == 'B')),
                Kind::Exclusive,
            ),
            'e' | 'E' => (
                repeat(n, cur, |i| word_end(b, i, ch == 'E')),
                Kind::Inclusive,
            ),
            '}' if self.skip_blank => (repeat(n, cur, |i| next_paragraph(b, i)), Kind::Exclusive),
            '{' if self.skip_blank => (
                repeat(n, cur, |i| previous_paragraph(b, i)),
                Kind::Exclusive,
            ),
            '}' => (repeat(n, cur, |i| paragraph(b, i, true)), Kind::Exclusive),
            '{' => (repeat(n, cur, |i| paragraph(b, i, false)), Kind::Exclusive),
            '%' => (match_bracket(b, cur)?, Kind::Inclusive),
            _ => {
                let busy = self.pending != Pending::None;
                self.reset_pending();
                return busy.then_some(VimEffect::Moved);
            }
        };
        self.motion(target, kind, text, b)
    }

    /// The count for a motion: after `y`, `2y3w` moves six words.
    fn take_count_keep_yank(&mut self) -> usize {
        self.count_was = self.count;
        let n = self.count.take().unwrap_or(1) * self.yank_count.unwrap_or(1);
        n.max(1)
    }

    fn anchor_index(&self, b: &Buf) -> Option<usize> {
        matches!(self.mode, VimMode::Visual | VimMode::VisualLine).then(|| b.index(self.anchor))
    }

    fn motion_keep_col(&mut self, target: usize, text: &str, b: &Buf) -> Option<VimEffect> {
        let want = self.want_col;
        let effect = self.motion(target, Kind::Linewise, text, b);
        self.want_col = want;
        effect
    }

    /// Moves the cursor to `target`, or after `y` yanks from the cursor to it.
    fn motion(&mut self, target: usize, kind: Kind, text: &str, b: &Buf) -> Option<VimEffect> {
        let cur = b.index(self.cursor);
        if self.pending == Pending::Yank {
            self.pending = Pending::None;
            self.yank_count = None;
            let (lo, hi) = (cur.min(target), cur.max(target));
            let range = match kind {
                Kind::Exclusive => b.byte(lo)..b.byte(hi),
                Kind::Inclusive => b.byte(lo)..b.byte(hi + 1),
                Kind::Linewise => b.byte(b.line_start(lo))..b.byte(b.line_end(hi)),
            };
            return Some(self.yank(range, kind == Kind::Linewise, text, b));
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

    fn object_done(&mut self, range: Range<usize>, text: &str, b: &Buf) -> Option<VimEffect> {
        if matches!(self.mode, VimMode::Visual | VimMode::VisualLine) {
            self.mode = VimMode::Visual;
            self.anchor = b.byte(range.start);
            self.cursor = b.byte(range.end.saturating_sub(1).max(range.start));
            return Some(VimEffect::Moved);
        }
        self.yank_count = None;
        Some(self.yank(b.byte(range.start)..b.byte(range.end), false, text, b))
    }

    fn yank(&mut self, range: Range<usize>, lines: bool, text: &str, b: &Buf) -> VimEffect {
        let yanked = text.get(range.clone()).unwrap_or_default().to_owned();
        let count = yanked.lines().count().max(1);
        let source = std::mem::take(&mut self.source_yank);
        let what = if source { " as source" } else { "" };
        self.message = Some(if lines || count > 1 {
            format!(
                "{count} line{} yanked{what}",
                if count == 1 { "" } else { "s" }
            )
        } else {
            format!("{} characters yanked{what}", yanked.chars().count())
        });
        self.mode = VimMode::Normal;
        self.cursor = b.byte(b.index(range.start));
        if source {
            return VimEffect::CopySource {
                range,
                linewise: lines,
            };
        }
        VimEffect::Copy(if lines { format!("{yanked}\n") } else { yanked })
    }

    fn search_key(&mut self, ev: &KeyEvent, text: &str, b: &Buf) -> Option<VimEffect> {
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
                return self.jump_to_match(text, b, self.backward);
            }
            Key::Text(s) if !ev.mods.cmd => self.typing.push_str(s),
            _ => return None,
        }
        Some(VimEffect::Moved)
    }

    fn jump_to_match(&mut self, text: &str, b: &Buf, backward: bool) -> Option<VimEffect> {
        self.matches_hidden = false;
        let found = find_all(text, &self.query);
        if found.is_empty() {
            self.message = Some(format!(
                "Pattern not found: {}",
                self.query.trim_start_matches("\\b")
            ));
            return Some(VimEffect::Moved);
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
        self.motion(b.index(start), Kind::Exclusive, text, b)
    }
}

/// Every place `query` occurs, case-insensitive unless it has a capital (Vim's smartcase). A
/// leading `\b` matches only at the start of a word, as `*` searches.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let (word, query) = match query.strip_prefix("\\b") {
        Some(q) => (true, q),
        None => (false, query),
    };
    if query.is_empty() {
        return Vec::new();
    }
    let sensitive = query.chars().any(char::is_uppercase);
    let fold = |s: &str| {
        if sensitive {
            s.to_owned()
        } else {
            s.to_lowercase()
        }
    };
    let (hay, needle) = (fold(text), fold(query));
    if hay.len() != text.len() {
        return Vec::new();
    }
    let boundary = |i: usize| {
        i == 0
            || !text[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    };
    hay.match_indices(&needle)
        .map(|(i, m)| i..i + m.len())
        .filter(|r| !word || boundary(r.start))
        .collect()
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

fn reverse_find(kind: char) -> char {
    match kind {
        'f' => 'F',
        'F' => 'f',
        't' => 'T',
        _ => 't',
    }
}

fn find_char(b: &Buf, cur: usize, kind: char, c: char, n: usize) -> Option<usize> {
    let (s, e) = (b.line_start(cur), b.line_end(cur));
    let mut at = cur;
    for k in 0..n {
        // `t` again from just before a match moves on to the next one.
        let skip = usize::from(k == 0 && matches!(kind, 't' | 'T'));
        at = match kind {
            'f' | 't' => (at + 1 + skip..e).find(|&j| b.chars[j] == c)?,
            _ => (s..at.saturating_sub(skip))
                .rev()
                .find(|&j| b.chars[j] == c)?,
        };
    }
    Some(match kind {
        't' => at - 1,
        'T' => at + 1,
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

/// A text object at `cur`, as character indices. `anchor` is the visual selection's other end,
/// so a second `ip` in visual mode grows from it.
fn object(
    b: &Buf,
    cur: usize,
    ch: char,
    around: bool,
    _anchor: Option<usize>,
) -> Option<Range<usize>> {
    match ch {
        'w' | 'W' => {
            let big = ch == 'W';
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
        'p' => {
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
        '"' | '\'' | '`' => {
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
        _ => {
            let (open, close) = match ch {
                '(' | ')' | 'b' => ('(', ')'),
                '[' | ']' => ('[', ']'),
                '{' | '}' | 'B' => ('{', '}'),
                '<' | '>' => ('<', '>'),
                _ => return None,
            };
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
