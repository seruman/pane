//! What keys do. A [`Keymap`] binds sequences of [`Stroke`]s to [`Command`]s, in the tables
//! Vim keeps for what's been typed:
//!
//! | Table | When | Holds |
//! |---|---|---|
//! | [`Table::Normal`] | nothing half typed | motions, operators, mode switches, app actions |
//! | [`Table::Visual`] | `v` or `V` | motions; operators act on the selection |
//! | [`Table::Pending`] | after an operator | motions, and `i` `a` to start a text object |
//! | objects ([`Keymap::object`]) | after `i` or `a` | text objects |
//!
//! [`Keymap::vim`] holds Vim's own keys; an app adds its actions and operators to it, or rebinds
//! and drops Vim's. Counts (`3w`, `2y3w`), the character after `f` `t` and a typed search are
//! read by [`Vim`](crate::Vim) itself, not looked up.
//!
//! There's no timeout: a sequence can't also start a longer one in the same table (`]c` and
//! `]cc` can't both be bound), so every key either finishes a binding or waits for the next.
//! Binding one that breaks this panics, naming both.

use rune::{Key, KeyEvent};

/// One key press, as a binding names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stroke {
    /// A typed character, Shift already in it: `Char('w')`, `Char('G')`, `Char('$')`,
    /// `Char(' ')`.
    Char(char),
    /// Control and a letter: `Ctrl('d')`.
    Ctrl(char),
    Enter,
    Tab,
    /// Shift-Tab.
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// The keymap's leader, whatever [`Keymap::leader`] sets (`\` by default).
    Leader,
}

impl Stroke {
    /// The stroke a key event is; `None` for one no binding can name (Escape, which cancels,
    /// a Cmd chord, a modifier on its own).
    pub fn from_event(ev: &KeyEvent) -> Option<Stroke> {
        let m = ev.mods;
        Some(match &ev.key {
            Key::Text(s) if !m.cmd => {
                let mut chars = s.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                if m.ctrl {
                    Stroke::Ctrl(c.to_ascii_lowercase())
                } else {
                    Stroke::Char(c)
                }
            }
            Key::Command(c) if m.ctrl => Stroke::Ctrl(c.to_ascii_lowercase()),
            Key::Enter => Stroke::Enter,
            Key::Tab if m.shift => Stroke::BackTab,
            Key::Tab => Stroke::Tab,
            Key::Backspace => Stroke::Backspace,
            Key::Delete => Stroke::Delete,
            Key::Up => Stroke::Up,
            Key::Down => Stroke::Down,
            Key::Left => Stroke::Left,
            Key::Right => Stroke::Right,
            Key::Home => Stroke::Home,
            Key::End => Stroke::End,
            Key::PageUp => Stroke::PageUp,
            Key::PageDown => Stroke::PageDown,
            _ => return None,
        })
    }

    fn resolve(self, leader: Stroke) -> Stroke {
        if self == Stroke::Leader { leader } else { self }
    }

    /// How a hint shows it: `w`, `⌃d`, `enter`, `space`.
    fn label(self, leader: Stroke) -> String {
        match self.resolve(leader) {
            Stroke::Char(' ') => "space".into(),
            Stroke::Char(c) => c.to_string(),
            Stroke::Ctrl(c) => format!("⌃{c}"),
            Stroke::Enter => "enter".into(),
            Stroke::Tab => "tab".into(),
            Stroke::BackTab => "⇧tab".into(),
            Stroke::Backspace => "⌫".into(),
            Stroke::Delete => "⌦".into(),
            Stroke::Up => "↑".into(),
            Stroke::Down => "↓".into(),
            Stroke::Left => "←".into(),
            Stroke::Right => "→".into(),
            Stroke::Home => "home".into(),
            Stroke::End => "end".into(),
            Stroke::PageUp => "pgup".into(),
            Stroke::PageDown => "pgdn".into(),
            Stroke::Leader => unreachable!("the leader is a real key"),
        }
    }
}

/// `keys` as a hint shows them: `gy`, `]c`, `\e`; spaced when a key's name is a word
/// (`g enter`).
pub(crate) fn render(keys: &[Stroke], leader: Stroke) -> String {
    let labels: Vec<String> = keys.iter().map(|k| k.label(leader)).collect();
    let spaced = keys
        .iter()
        .zip(&labels)
        .any(|(k, l)| !matches!(k.resolve(leader), Stroke::Ctrl(_)) && l.chars().count() > 1);
    labels.join(if spaced { " " } else { "" })
}

/// The action type of a [`Keymap`] with none of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoAction {}

/// Where a motion goes. Each takes a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Down a line, keeping the column.
    Down,
    Up,
    /// Down half a page (Ctrl-D), which is half of [`Vim::page_lines`](crate::Vim::page_lines).
    HalfPageDown,
    HalfPageUp,
    WordForward,
    /// To the next run after spaces (`W`).
    BigWordForward,
    WordBackward,
    BigWordBackward,
    WordEnd,
    BigWordEnd,
    LineStart,
    FirstNonBlank,
    LineEnd,
    /// The first line, or with a count that line (`gg`).
    Top,
    /// The last line, or with a count that line (`G`).
    Bottom,
    ParagraphForward,
    ParagraphBackward,
    /// The bracket matching the one at or after the cursor (`%`).
    MatchingBracket,
    /// To a character on the line, read after the key (`f F t T`).
    Find(Find),
    /// The last [`Motion::Find`] again (`;`), or the other way (`,`).
    RepeatFind,
    RepeatFindReverse,
    /// The search's next match (`n`), previous (`N`).
    SearchNext,
    SearchPrevious,
    /// Searches the word under the cursor, forward (`*`) or back (`#`).
    SearchWord,
    SearchWordBack,
}

/// Which way [`Motion::Find`] looks, and whether it stops on the character or before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    /// `f`: forward, onto it.
    To,
    /// `F`: back, onto it.
    ToBack,
    /// `t`: forward, up to it.
    Till,
    /// `T`: back, up to it.
    TillBack,
}

impl Find {
    pub(crate) fn reverse(self) -> Find {
        match self {
            Find::To => Find::ToBack,
            Find::ToBack => Find::To,
            Find::Till => Find::TillBack,
            Find::TillBack => Find::Till,
        }
    }
    pub(crate) fn forward(self) -> bool {
        matches!(self, Find::To | Find::Till)
    }
    pub(crate) fn till(self) -> bool {
        matches!(self, Find::Till | Find::TillBack)
    }
}

/// A text object, after `i` (inside) or `a` (around).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    Word,
    /// A run between spaces (`W`).
    BigWord,
    Paragraph,
    /// Between two of this quote on the line.
    Quotes(char),
    /// Between this opening and closing bracket, across lines.
    Pair(char, char),
}

/// What an operator does with the range its motion or object covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator<A> {
    /// Copy the text ([`VimEffect::Copy`](crate::VimEffect::Copy)).
    Yank,
    /// Copy the source behind the text, which the app finds
    /// ([`VimEffect::CopySource`](crate::VimEffect::CopySource)).
    YankSource,
    /// The app's own ([`VimEffect::Operator`](crate::VimEffect::Operator)).
    App(A),
}

/// What a binding does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<A> {
    Motion(Motion),
    /// Waits for a motion or object in Normal mode (`yw`, `yiw`); its own keys again take the
    /// line (`yy`, `gyy`). In Visual mode it takes the selection.
    Operator(Operator<A>),
    /// The operator on the cursor's line, or as many as the count says (`Y`).
    Lines(Operator<A>),
    /// A text object comes next, inside it (`i`) or around it (`a`).
    Inner,
    Around,
    /// Starts or ends a selection by characters (`v`) or lines (`V`).
    Visual,
    VisualLine,
    /// The selection's other end (`o`).
    OtherEnd,
    /// Starts typing a search (`/`, `?`).
    Search {
        backward: bool,
    },
    /// The app's own ([`VimEffect::Action`](crate::VimEffect::Action)).
    App(A),
}

impl<A> Command<A> {
    /// The variant's name, for a panic message.
    fn name(&self) -> &'static str {
        match self {
            Command::Motion(_) => "Motion",
            Command::Operator(_) => "Operator",
            Command::Lines(_) => "Lines",
            Command::Inner => "Inner",
            Command::Around => "Around",
            Command::Visual => "Visual",
            Command::VisualLine => "VisualLine",
            Command::OtherEnd => "OtherEnd",
            Command::Search { .. } => "Search",
            Command::App(_) => "App",
        }
    }

    /// Hints list the app's bindings first, then Vim's commands, then its motions.
    fn group(&self) -> usize {
        match self {
            Command::App(_)
            | Command::Operator(Operator::App(_))
            | Command::Lines(Operator::App(_)) => 0,
            Command::Motion(_) => 2,
            _ => 1,
        }
    }
}

/// A key that can come next and what it does, for a which-key panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub keys: String,
    pub what: &'static str,
}

pub fn hint(keys: &str, what: &'static str) -> Hint {
    Hint {
        keys: keys.to_owned(),
        what,
    }
}

/// A [`Keymap`] table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Normal,
    Visual,
    /// After an operator. It holds motions, [`Command::Inner`] and [`Command::Around`] only.
    Pending,
}

impl Table {
    /// Whether `command` means anything here. [`Command::OtherEnd`] needs a selection, so it
    /// belongs to Visual. [`Command::Lines`] acts on the cursor's lines, which a selection
    /// replaces, so it belongs to Normal. After an operator only what it can act over counts.
    fn takes<A>(self, command: &Command<A>) -> bool {
        match (self, command) {
            (Table::Pending, c) => {
                matches!(c, Command::Motion(_) | Command::Inner | Command::Around)
            }
            (Table::Normal, Command::OtherEnd) | (Table::Visual, Command::Lines(_)) => false,
            _ => true,
        }
    }
}

#[derive(Clone, Debug)]
struct Binding<C> {
    keys: Vec<Stroke>,
    command: C,
    /// What a hint calls it; `None` keeps it out of the hints.
    what: Option<&'static str>,
}

/// What a sequence typed so far comes to in a table.
pub(crate) enum Lookup<T> {
    /// No binding starts with it.
    None,
    /// Bindings start with it; another key comes.
    Prefix,
    Exact(T),
}

/// Key bindings. Build one from [`Keymap::vim`] and add to it:
///
/// ```ignore
/// let keys = Keymap::vim()
///     .operator(&[Stroke::Char('c')], Operator::App(Act::Comment), "Comment")
///     .action(&[Stroke::Char(']'), Stroke::Char('c')], Act::NextComment, "Next comment")
///     .action(&[Stroke::Leader, Stroke::Char('q')], Act::Quit, "Quit");
/// ```
#[derive(Clone, Debug)]
pub struct Keymap<A> {
    leader: Stroke,
    normal: Vec<Binding<Command<A>>>,
    visual: Vec<Binding<Command<A>>>,
    pending: Vec<Binding<Command<A>>>,
    objects: Vec<Binding<Object>>,
}

impl<A: Clone> Keymap<A> {
    /// No bindings, and `\` for the leader.
    pub fn empty() -> Self {
        Keymap {
            leader: Stroke::Char('\\'),
            normal: Vec::new(),
            visual: Vec::new(),
            pending: Vec::new(),
            objects: Vec::new(),
        }
    }

    /// Vim's keys for reading: motions (`h j k l`, `w b e W B E`, `0 ^ $`, `gg G`, `{ }`, `%`,
    /// `f F t T ; ,`, `n N * #`, Ctrl-D and Ctrl-U, the arrows), `y` and `gy` (yank as source),
    /// `Y`, `v` `V` `o`, `/` `?`, and the text objects `w W p " ' ` ( ) b [ ] { } B < >`.
    pub fn vim() -> Self {
        use Motion as M;
        use Stroke::Char as C;
        let motions: &[(&[Stroke], Motion, Option<&'static str>)] = &[
            (&[C('h')], M::Left, None),
            (&[Stroke::Left], M::Left, None),
            (&[C('l')], M::Right, None),
            (&[Stroke::Right], M::Right, None),
            (&[C('j')], M::Down, None),
            (&[Stroke::Down], M::Down, None),
            (&[C('k')], M::Up, None),
            (&[Stroke::Up], M::Up, None),
            (&[Stroke::Ctrl('d')], M::HalfPageDown, None),
            (&[Stroke::Ctrl('u')], M::HalfPageUp, None),
            (&[C('w')], M::WordForward, Some("To the next word")),
            (&[C('W')], M::BigWordForward, None),
            (&[C('b')], M::WordBackward, Some("Back a word")),
            (&[C('B')], M::BigWordBackward, None),
            (&[C('e')], M::WordEnd, Some("To the word's end")),
            (&[C('E')], M::BigWordEnd, None),
            (&[C('0')], M::LineStart, Some("From the line's start")),
            (&[Stroke::Home], M::LineStart, None),
            (&[C('^')], M::FirstNonBlank, None),
            (&[C('$')], M::LineEnd, Some("To the line's end")),
            (&[Stroke::End], M::LineEnd, None),
            (&[C('g'), C('g')], M::Top, Some("To the top")),
            (&[C('G')], M::Bottom, None),
            (
                &[C('}')],
                M::ParagraphForward,
                Some("To the paragraph's end"),
            ),
            (&[C('{')], M::ParagraphBackward, None),
            (&[C('%')], M::MatchingBracket, None),
            (&[C('f')], M::Find(Find::To), Some("To a character")),
            (&[C('F')], M::Find(Find::ToBack), None),
            (&[C('t')], M::Find(Find::Till), Some("Up to a character")),
            (&[C('T')], M::Find(Find::TillBack), None),
            (&[C(';')], M::RepeatFind, None),
            (&[C(',')], M::RepeatFindReverse, None),
            (&[C('n')], M::SearchNext, None),
            (&[C('N')], M::SearchPrevious, None),
            (&[C('*')], M::SearchWord, None),
            (&[C('#')], M::SearchWordBack, None),
        ];
        let mut k = Keymap::empty();
        for &(keys, motion, what) in motions {
            k = k.motion(keys, motion, what);
        }
        let objects: &[(&[Stroke], Object, Option<&'static str>)] = &[
            (&[C('w')], Object::Word, Some("Word")),
            (&[C('W')], Object::BigWord, Some("WORD, to spaces")),
            (&[C('p')], Object::Paragraph, Some("Paragraph")),
            (&[C('"')], Object::Quotes('"'), Some("Double quotes")),
            (&[C('\'')], Object::Quotes('\''), Some("Single quotes")),
            (&[C('`')], Object::Quotes('`'), Some("Backticks")),
            (&[C('(')], Object::Pair('(', ')'), Some("Parentheses")),
            (&[C(')')], Object::Pair('(', ')'), None),
            (&[C('b')], Object::Pair('(', ')'), None),
            (&[C('[')], Object::Pair('[', ']'), Some("Brackets")),
            (&[C(']')], Object::Pair('[', ']'), None),
            (&[C('{')], Object::Pair('{', '}'), Some("Braces")),
            (&[C('}')], Object::Pair('{', '}'), None),
            (&[C('B')], Object::Pair('{', '}'), None),
            (&[C('<')], Object::Pair('<', '>'), Some("Angle brackets")),
            (&[C('>')], Object::Pair('<', '>'), None),
        ];
        for &(keys, object, what) in objects {
            k = k.object(keys, object, what);
        }
        k.operator(&[C('y')], Operator::Yank, "Yank")
            .operator(&[C('g'), C('y')], Operator::YankSource, "Yank as Markdown")
            .bind(
                Table::Normal,
                &[C('Y')],
                Command::Lines(Operator::Yank),
                None,
            )
            .bind(Table::Normal, &[C('v')], Command::Visual, "Select")
            .bind(
                Table::Normal,
                &[C('V')],
                Command::VisualLine,
                "Select lines",
            )
            .bind(Table::Visual, &[C('v')], Command::Visual, None)
            .bind(Table::Visual, &[C('V')], Command::VisualLine, None)
            .bind(
                Table::Visual,
                &[C('o')],
                Command::OtherEnd,
                "Go to the other end",
            )
            .bind(
                Table::Visual,
                &[C('i')],
                Command::Inner,
                "Select inside an object…",
            )
            .bind(
                Table::Visual,
                &[C('a')],
                Command::Around,
                "Select around an object…",
            )
            .bind(
                Table::Pending,
                &[C('i')],
                Command::Inner,
                "Inside an object…",
            )
            .bind(
                Table::Pending,
                &[C('a')],
                Command::Around,
                "Around an object…",
            )
            .bind(
                Table::Normal,
                &[C('/')],
                Command::Search { backward: false },
                "Search",
            )
            .bind(
                Table::Normal,
                &[C('?')],
                Command::Search { backward: true },
                "Search back",
            )
            .bind(
                Table::Visual,
                &[C('/')],
                Command::Search { backward: false },
                None,
            )
            .bind(
                Table::Visual,
                &[C('?')],
                Command::Search { backward: true },
                None,
            )
    }

    /// The key [`Stroke::Leader`] stands for. Panics on [`Stroke::Leader`] itself, or when the
    /// new leader makes one binding start another.
    pub fn leader(mut self, key: Stroke) -> Self {
        assert!(key != Stroke::Leader, "the leader must be a real key");
        self.leader = key;
        for (name, table) in [
            ("Normal", &self.normal),
            ("Visual", &self.visual),
            ("Pending", &self.pending),
        ] {
            check(name, table, key);
        }
        check("objects", &self.objects, key);
        self
    }

    /// Binds `keys` in `table`, replacing a binding of the same keys. `what` names it in hints;
    /// `None` keeps it out of them. Panics when `keys` would start another binding there or
    /// another would start it, when it starts with a digit (`1`–`9` start a count), or when
    /// `table` is [`Table::Pending`] and `command` isn't a motion, `Inner` or `Around`.
    pub fn bind(
        mut self,
        table: Table,
        keys: &[Stroke],
        command: Command<A>,
        what: impl Into<Option<&'static str>>,
    ) -> Self {
        assert!(
            table.takes(&command),
            "{}: {} means nothing in the {table:?} table",
            render(keys, self.leader),
            command.name()
        );
        let leader = self.leader;
        let (name, list) = match table {
            Table::Normal => ("Normal", &mut self.normal),
            Table::Visual => ("Visual", &mut self.visual),
            Table::Pending => ("Pending", &mut self.pending),
        };
        insert(name, list, leader, keys, command, what.into());
        self
    }

    /// Drops the binding of `keys` in `table`, if there is one.
    pub fn unbind(mut self, table: Table, keys: &[Stroke]) -> Self {
        let leader = self.leader;
        let list = match table {
            Table::Normal => &mut self.normal,
            Table::Visual => &mut self.visual,
            Table::Pending => &mut self.pending,
        };
        list.retain(|b| !same(&b.keys, keys, leader));
        self
    }

    /// Binds a motion where motions go: Normal, Visual and after an operator.
    pub fn motion(
        self,
        keys: &[Stroke],
        motion: Motion,
        what: impl Into<Option<&'static str>>,
    ) -> Self {
        let what = what.into();
        self.bind(Table::Normal, keys, Command::Motion(motion), what)
            .bind(Table::Visual, keys, Command::Motion(motion), what)
            .bind(Table::Pending, keys, Command::Motion(motion), what)
    }

    /// Binds an operator in Normal mode, where it waits for a motion, and in Visual, where it
    /// takes the selection.
    pub fn operator(
        self,
        keys: &[Stroke],
        operator: Operator<A>,
        what: impl Into<Option<&'static str>>,
    ) -> Self {
        let what = what.into();
        self.bind(
            Table::Normal,
            keys,
            Command::Operator(operator.clone()),
            what,
        )
        .bind(Table::Visual, keys, Command::Operator(operator), what)
    }

    /// Binds an app action in Normal mode.
    pub fn action(self, keys: &[Stroke], action: A, what: impl Into<Option<&'static str>>) -> Self {
        self.bind(Table::Normal, keys, Command::App(action), what)
    }

    /// Binds a text object, for after `i` or `a`.
    pub fn object(
        mut self,
        keys: &[Stroke],
        object: Object,
        what: impl Into<Option<&'static str>>,
    ) -> Self {
        let leader = self.leader;
        insert(
            "objects",
            &mut self.objects,
            leader,
            keys,
            object,
            what.into(),
        );
        self
    }

    /// Every described binding in `table`, grouped as [`Vim::hints`](crate::Vim::hints) groups
    /// them: the app's, then Vim's commands. Motions are left out; they'd crowd a list of
    /// everything.
    pub fn hints(&self, table: Table) -> Vec<Vec<Hint>> {
        self.next(table, &[], false)
    }

    pub(crate) fn render(&self, keys: &[Stroke]) -> String {
        render(keys, self.leader)
    }

    fn list(&self, table: Table) -> &[Binding<Command<A>>] {
        match table {
            Table::Normal => &self.normal,
            Table::Visual => &self.visual,
            Table::Pending => &self.pending,
        }
    }

    pub(crate) fn lookup(&self, table: Table, typed: &[Stroke]) -> Lookup<Command<A>> {
        lookup(self.list(table), typed, self.leader)
    }

    pub(crate) fn lookup_object(&self, typed: &[Stroke]) -> Lookup<Object> {
        lookup(&self.objects, typed, self.leader)
    }

    /// The described bindings in `table` that `typed` starts, each by the keys still to come,
    /// grouped: the app's, Vim's commands, Vim's motions. With nothing typed, motions only when
    /// `motions` is set.
    pub(crate) fn next(&self, table: Table, typed: &[Stroke], motions: bool) -> Vec<Vec<Hint>> {
        let mut groups = vec![Vec::new(), Vec::new(), Vec::new()];
        for b in self.list(table) {
            let Some(what) = b.what else { continue };
            let group = b.command.group();
            if !starts(&b.keys, typed, self.leader) || (typed.is_empty() && group == 2 && !motions)
            {
                continue;
            }
            groups[group].push(hint(&self.render(&b.keys[typed.len()..]), what));
        }
        groups.retain(|g| !g.is_empty());
        groups
    }

    pub(crate) fn next_objects(&self, typed: &[Stroke]) -> Vec<Hint> {
        self.objects
            .iter()
            .filter(|b| starts(&b.keys, typed, self.leader))
            .filter_map(|b| Some(hint(&self.render(&b.keys[typed.len()..]), b.what?)))
            .collect()
    }
}

/// Whether `keys` (with the leader resolved) begin with `typed`.
fn starts(keys: &[Stroke], typed: &[Stroke], leader: Stroke) -> bool {
    keys.len() >= typed.len() && keys.iter().zip(typed).all(|(k, t)| k.resolve(leader) == *t)
}

fn same(a: &[Stroke], b: &[Stroke], leader: Stroke) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.resolve(leader) == y.resolve(leader))
}

fn lookup<C: Clone>(list: &[Binding<C>], typed: &[Stroke], leader: Stroke) -> Lookup<C> {
    let mut longer = false;
    for b in list {
        if !starts(&b.keys, typed, leader) {
            continue;
        }
        if b.keys.len() == typed.len() {
            return Lookup::Exact(b.command.clone());
        }
        longer = true;
    }
    if longer { Lookup::Prefix } else { Lookup::None }
}

fn insert<C>(
    name: &str,
    list: &mut Vec<Binding<C>>,
    leader: Stroke,
    keys: &[Stroke],
    command: C,
    what: Option<&'static str>,
) {
    assert!(!keys.is_empty(), "{name}: a binding needs a key");
    assert!(
        !matches!(keys[0], Stroke::Char('1'..='9')),
        "{name}: {} starts with a digit, which starts a count",
        render(keys, leader)
    );
    list.retain(|b| !same(&b.keys, keys, leader));
    list.push(Binding {
        keys: keys.to_vec(),
        command,
        what,
    });
    check(name, list, leader);
}

/// Panics when one binding's keys start another's: with no timeout, the shorter would always
/// win and the longer could never be typed.
fn check<C>(name: &str, list: &[Binding<C>], leader: Stroke) {
    for (i, a) in list.iter().enumerate() {
        for b in &list[i + 1..] {
            let (short, long) = if a.keys.len() <= b.keys.len() {
                (a, b)
            } else {
                (b, a)
            };
            let resolved: Vec<Stroke> = short.keys.iter().map(|k| k.resolve(leader)).collect();
            if starts(&long.keys, &resolved, leader) {
                panic!(
                    "{name}: {} starts {}; with no timeout, {} could never be typed",
                    render(&short.keys, leader),
                    render(&long.keys, leader),
                    render(&long.keys, leader)
                );
            }
        }
    }
}
