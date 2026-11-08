use rune::{Key, KeyEvent, Mods};

use super::*;

const TEXT: &str = "fn main() {\n    let s = \"hello world\";\n    print(s);\n}\n\nsecond paragraph here\nand more";

fn press(vim: &mut Vim, keys: &str) -> Option<VimEffect> {
    let mut last = None;
    for c in keys.chars() {
        let ev = KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        };
        last = vim.key(&ev, TEXT);
    }
    last
}

fn ctrl(vim: &mut Vim, c: char) {
    let ev = KeyEvent {
        key: Key::Command(c),
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    };
    vim.key(&ev, TEXT);
}

fn at(vim: &Vim) -> &'static str {
    &TEXT[vim.cursor(TEXT).start..]
}

fn yanked(effect: Option<VimEffect>) -> String {
    match effect {
        Some(VimEffect::Copy(s)) => s,
        other => panic!("no yank: {other:?}"),
    }
}

#[test]
fn word_motions() {
    let mut v = Vim::new();
    press(&mut v, "w");
    assert!(at(&v).starts_with("main"));
    press(&mut v, "w");
    assert!(at(&v).starts_with("()"));
    press(&mut v, "W");
    assert!(at(&v).starts_with("{"));
    press(&mut v, "b");
    assert!(at(&v).starts_with("()"));
    press(&mut v, "e");
    assert!(at(&v).starts_with(") {"));
}

#[test]
fn lines_columns_and_counts() {
    let mut v = Vim::new();
    press(&mut v, "j");
    assert!(at(&v).starts_with(" "));
    press(&mut v, "^");
    assert!(at(&v).starts_with("let"));
    press(&mut v, "$");
    assert!(at(&v).starts_with(";\n"));
    press(&mut v, "gg");
    assert!(at(&v).starts_with("fn"));
    press(&mut v, "G");
    assert!(at(&v).starts_with("and more"));
    press(&mut v, "3G");
    assert!(at(&v).starts_with("print"));
    press(&mut v, "gg2w");
    assert!(at(&v).starts_with("()"));
}

#[test]
fn find_and_repeat() {
    let mut v = Vim::new();
    press(&mut v, "jfo");
    assert!(at(&v).starts_with("o world"));
    press(&mut v, ";");
    assert!(at(&v).starts_with("orld"));
    press(&mut v, ",");
    assert!(at(&v).starts_with("o world"));
    press(&mut v, "0t\"");
    assert!(at(&v).starts_with(" \"hello"));
}

#[test]
fn paragraphs_and_brackets() {
    let mut v = Vim::new();
    press(&mut v, "}");
    assert_eq!(v.cursor(TEXT).start, TEXT.find("\n\n").unwrap() + 1);
    press(&mut v, "{");
    assert_eq!(v.cursor(TEXT).start, 0);
    press(&mut v, "f{%");
    assert!(at(&v).starts_with("}\n\n"));
    press(&mut v, "%");
    assert!(at(&v).starts_with("{\n"));
}

#[test]
fn yank_motions_and_lines() {
    let mut v = Vim::new();
    assert_eq!(yanked(press(&mut v, "yw")), "fn ");
    assert_eq!(yanked(press(&mut v, "ye")), "fn");
    assert_eq!(yanked(press(&mut v, "y$")), "fn main() {");
    assert_eq!(yanked(press(&mut v, "yy")), "fn main() {\n");
    assert_eq!(
        yanked(press(&mut v, "2yy")),
        "fn main() {\n    let s = \"hello world\";\n"
    );
    assert_eq!(v.message(), Some("2 lines yanked"));
    assert_eq!(yanked(press(&mut v, "jy2w")), "    let ");
}

#[test]
fn text_objects() {
    let mut v = Vim::new();
    press(&mut v, "jfh");
    assert_eq!(yanked(press(&mut v, "yiw")), "hello");
    assert_eq!(yanked(press(&mut v, "yaw")), "hello ");
    assert_eq!(yanked(press(&mut v, "yi\"")), "hello world");
    assert_eq!(yanked(press(&mut v, "ya\"")), "\"hello world\"");
    press(&mut v, "j0fs");
    assert_eq!(yanked(press(&mut v, "yi(")), "s");
    assert_eq!(yanked(press(&mut v, "yab")), "(s)");
    assert_eq!(
        yanked(press(&mut v, "yi{")),
        "\n    let s = \"hello world\";\n    print(s);\n"
    );
    press(&mut v, "G");
    assert_eq!(
        yanked(press(&mut v, "yip")),
        "second paragraph here\nand more"
    );
}

#[test]
fn visual_selections() {
    let mut v = Vim::new();
    press(&mut v, "vee");
    assert_eq!(&TEXT[v.selection(TEXT).unwrap()], "fn main");
    press(&mut v, "o");
    assert!(at(&v).starts_with("fn"));
    assert_eq!(yanked(press(&mut v, "y")), "fn main");
    assert_eq!(v.mode(), VimMode::Normal);
    press(&mut v, "Vj");
    assert_eq!(
        &TEXT[v.selection(TEXT).unwrap()],
        "fn main() {\n    let s = \"hello world\";"
    );
    assert_eq!(
        yanked(press(&mut v, "y")),
        "fn main() {\n    let s = \"hello world\";\n"
    );
    press(&mut v, "jfhviw");
    assert_eq!(&TEXT[v.selection(TEXT).unwrap()], "hello");
    press(&mut v, "\u{1b}");
}

#[test]
fn search_smart_case_and_star() {
    let mut v = Vim::new();
    press(&mut v, "/s");
    assert_eq!(v.status(), "/s");
    v.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert!(at(&v).starts_with("s = "));
    press(&mut v, "n");
    assert!(at(&v).starts_with("s);"));
    press(&mut v, "N");
    assert!(at(&v).starts_with("s = "));
    assert_eq!(find_all("Hello hello", "hello").len(), 2);
    assert_eq!(find_all("Hello hello", "Hello").len(), 1);
    press(&mut v, "gg*");
    assert_eq!(v.message(), Some("[1/1]"));
    press(&mut v, "/zzz");
    v.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert_eq!(v.message(), Some("Pattern not found: zzz"));
}

#[test]
fn half_pages_and_escape() {
    let mut v = Vim::new();
    ctrl(&mut v, 'd');
    assert!(at(&v).starts_with("and more"));
    ctrl(&mut v, 'u');
    assert!(at(&v).starts_with("fn"));
    press(&mut v, "y");
    assert_eq!(v.pending(), "y");
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert_eq!(v.pending(), "");
    // Nothing to do with Escape in normal mode, so the app may use it.
    assert!(
        v.key(
            &KeyEvent {
                key: Key::Escape,
                mods: Mods::default()
            },
            TEXT
        )
        .is_none()
    );
}

const PAGE: &str = "Title\n\nFirst paragraph\n\none\ntwo\n\nlast";

fn press_on(vim: &mut Vim, text: &str, keys: &str) {
    for c in keys.chars() {
        vim.key(
            &KeyEvent {
                key: Key::Text(c.to_string()),
                mods: Mods::default(),
            },
            text,
        );
    }
}

#[test]
fn skipping_blank_lines() {
    let mut v = Vim::new().skip_blank_lines(true);
    let at = |v: &Vim| &PAGE[v.cursor(PAGE).start..];
    press_on(&mut v, PAGE, "j");
    assert!(at(&v).starts_with("First"));
    press_on(&mut v, PAGE, "j");
    assert!(at(&v).starts_with("one"));
    press_on(&mut v, PAGE, "k");
    assert!(at(&v).starts_with("First"));
    press_on(&mut v, PAGE, "}");
    assert!(at(&v).starts_with("one"));
    press_on(&mut v, PAGE, "}");
    assert!(at(&v).starts_with("last"));
    press_on(&mut v, PAGE, "{");
    assert!(at(&v).starts_with("one"));
    press_on(&mut v, PAGE, "j{");
    assert!(at(&v).starts_with("one"));
    press_on(&mut v, PAGE, "{");
    assert!(at(&v).starts_with("First"));
}

#[test]
fn escape_hides_search_matches() {
    let mut v = Vim::new();
    press(&mut v, "/s");
    v.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert!(!v.matches(TEXT).is_empty());
    assert_eq!(
        v.key(
            &KeyEvent {
                key: Key::Escape,
                mods: Mods::default()
            },
            TEXT
        ),
        Some(VimEffect::Moved)
    );
    assert!(v.matches(TEXT).is_empty());
    press(&mut v, "n");
    assert!(!v.matches(TEXT).is_empty());
}

fn source(effect: Option<VimEffect>) -> (std::ops::Range<usize>, bool) {
    match effect {
        Some(VimEffect::CopySource { range, linewise }) => (range, linewise),
        other => panic!("no source yank: {other:?}"),
    }
}

#[test]
fn gy_asks_for_the_source() {
    let mut v = Vim::new();
    let (range, linewise) = source(press(&mut v, "gyw"));
    assert_eq!((&TEXT[range], linewise), ("fn ", false));
    assert_eq!(v.message(), Some("3 characters yanked as source"));
    let (range, linewise) = source(press(&mut v, "gyy"));
    assert_eq!((&TEXT[range], linewise), ("fn main() {", true));
    let (range, linewise) = source(press(&mut v, "Vjgy"));
    assert_eq!(
        (&TEXT[range], linewise),
        ("fn main() {\n    let s = \"hello world\";", true)
    );
    // Plain `y` afterwards copies text again.
    assert_eq!(yanked(press(&mut v, "yw")), "fn ");
    assert_eq!(press(&mut v, "g"), Some(VimEffect::Moved));
    assert_eq!(v.pending(), "g");
    press(&mut v, "y");
    assert_eq!(v.pending(), "gy");
}

#[test]
fn gy_text_object_after_a_visual_yank() {
    let mut v = Vim::new();
    yanked(press(&mut v, "vey"));
    let (range, _) = source(press(&mut v, "gyiw"));
    assert_eq!(&TEXT[range], "fn");
}

#[test]
fn hints_follow_what_is_typed() {
    let mut v = Vim::new();
    assert_eq!(v.hints(), None, "nothing half typed");
    press(&mut v, "y");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, "y");
    assert_eq!(groups[0][0], hint("y", "The line"));
    press(&mut v, "i");
    assert_eq!(v.hints().unwrap().0, "yi");
    v.key(&KeyEvent { key: Key::Escape, mods: Mods::default() }, TEXT);
    press(&mut v, "g");
    assert_eq!(v.hints().unwrap().0, "g");
    press(&mut v, "y");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!((typed.as_str(), groups[0][0]), ("gy", hint("gy", "The line")));
    press(&mut v, "a");
    assert_eq!(v.hints().unwrap().0, "gya");
    press(&mut v, "w");
    assert_eq!(v.hints(), None);
    press(&mut v, "v");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, "visual");
    assert!(groups[0].contains(&hint("gy", "Yank as Markdown")));
}
