use rune::{Key, KeyEvent, Mods};

use super::*;

const TEXT: &str = "fn main() {\n    let s = \"hello world\";\n    print(s);\n}\n\nsecond paragraph here\nand more";

fn press(vim: &mut Vim, keys: &str) -> Option<VimEffect<NoAction>> {
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

fn yanked(effect: Option<VimEffect<NoAction>>) -> String {
    match effect {
        Some(VimEffect::Copy(s)) => s,
        other => panic!("no yank: {other:?}"),
    }
}

#[test]
fn empty_text_still_dispatches_app_actions() {
    let keys = Keymap::<Act>::vim()
        .action(&[Stroke::Char('C')], Act::Comment, "General comment")
        .action(&[Stroke::Char(':')], Act::Next, "Command")
        .action(&[Stroke::Char('Z'), Stroke::Char('Z')], Act::Quit, "Quit");
    let mut v = Vim::new(keys);
    for (keys, action, count) in [
        ("C", Act::Comment, None),
        (":", Act::Next, None),
        ("ZZ", Act::Quit, None),
        ("2C", Act::Comment, Some(2)),
    ] {
        let mut effect = None;
        for c in keys.chars() {
            effect = v.key(
                &KeyEvent {
                    key: Key::Text(c.to_string()),
                    mods: Mods::default(),
                },
                "",
            );
        }
        assert_eq!(effect, Some(VimEffect::Action { action, count }), "{keys}");
        assert_eq!(v.cursor(""), 0..0);
        assert_eq!(v.selection(""), None);
        assert!(!v.is_pending());
    }
}

#[test]
fn empty_text_drops_text_commands_without_range_effects() {
    for pending in ["", "y2", "yi", "f", "v", "V", "/s"] {
        for keys in [
            "h", "l", "j", "k", "gg", "G", "$", "%", "w", "b", "e", "{", "}", "fX", "n", "*", "yy",
            "gygy", "yiw", "viw", "Vj", "Y", "/hello",
        ] {
            let mut v = Vim::default();
            press(&mut v, pending);
            for c in keys.chars() {
                let effect = v.key(
                    &KeyEvent {
                        key: Key::Text(c.to_string()),
                        mods: Mods::default(),
                    },
                    "",
                );
                assert!(
                    matches!(effect, None | Some(VimEffect::Consumed)),
                    "{pending} then {keys}: {effect:?}"
                );
            }
            assert_eq!(v.mode(), VimMode::Normal);
            assert_eq!(v.cursor(""), 0..0);
            assert_eq!(v.selection(""), None);
        }
    }
}

#[test]
fn named_and_control_leaders_resolve_and_render() {
    for (leader, event, label) in [
        (
            KeyStroke::Tab,
            KeyEvent {
                key: Key::Tab,
                mods: Mods::default(),
            },
            "tab q",
        ),
        (
            KeyStroke::Ctrl('q'),
            KeyEvent {
                key: Key::Text("q".into()),
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
            },
            "⌃qq",
        ),
    ] {
        let keys = Keymap::<bool>::empty().leader(leader).action(
            &[Stroke::Leader, Stroke::Char('q')],
            true,
            "Quit",
        );
        assert_eq!(
            keys.hints(Table::Normal),
            vec![(HintGroup::App, vec![hint(label, "Quit")])]
        );
        let mut v = Vim::new(keys);
        v.key(&event, TEXT);
        assert_eq!(
            v.key(
                &KeyEvent {
                    key: Key::Text("q".into()),
                    mods: Mods::default()
                },
                TEXT
            ),
            Some(VimEffect::Action {
                action: true,
                count: None
            })
        );
    }
}

#[test]
fn actions_and_operators_can_have_distinct_types() {
    #[derive(Clone, Debug, PartialEq)]
    enum Action {
        Quit,
    }
    #[derive(Clone, Debug, PartialEq)]
    enum Op {
        Remove,
    }
    let keys = Keymap::vim()
        .action(&[Stroke::Char('q')], Action::Quit, "Quit")
        .operator(&[Stroke::Char('d')], Operator::App(Op::Remove), "Remove");
    let mut v = Vim::new(keys);
    let key = |c: char| KeyEvent {
        key: Key::Text(c.to_string()),
        mods: Mods::default(),
    };
    assert_eq!(
        v.key(&key('q'), TEXT),
        Some(VimEffect::Action {
            action: Action::Quit,
            count: None
        })
    );
    v.key(&key('d'), TEXT);
    assert_eq!(
        v.key(&key('w'), TEXT),
        Some(VimEffect::Operator {
            action: Op::Remove,
            range: 0..3,
            linewise: false
        })
    );
}

#[test]
fn buffer_lines_match_character_boundaries() {
    for text in ["", "one", "\n", "αβ\n\n🙂z\n", "a\nb\nc"] {
        let b = Buf::new(text);
        let chars: Vec<_> = text.chars().collect();
        let starts: Vec<_> = std::iter::once(0)
            .chain(
                chars
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| (*c == '\n').then_some(i + 1)),
            )
            .collect();
        assert_eq!(b.line_count(), starts.len());
        for (line, start) in starts.iter().enumerate() {
            assert_eq!(b.nth_line(line), *start);
        }
        assert_eq!(b.nth_line(starts.len()), chars.len());
        for i in 0..=chars.len() {
            let line = chars[..i].iter().filter(|c| **c == '\n').count();
            assert_eq!(b.line_of(i), line);
            assert_eq!(b.line_start(i), starts[line]);
            assert_eq!(
                b.line_end(i),
                (i..chars.len())
                    .find(|j| chars[*j] == '\n')
                    .unwrap_or(chars.len())
            );
        }
    }
}

#[test]
fn word_motions() {
    let mut v = Vim::default();
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
    let mut v = Vim::default();
    press(&mut v, "j");
    assert_eq!(v.cursor(TEXT).start, TEXT.find('\n').unwrap() + 1);
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
fn find_counts_survive_the_character_prompt() {
    for keys in ["y2fn", "2yfn"] {
        let mut v = Vim::default();
        assert_eq!(yanked(press(&mut v, keys)), "fn main");
        assert!(!v.is_pending());
        press(&mut v, "G");
        assert!(at(&v).starts_with("and more"));
    }
}

#[test]
fn vertical_moves_keep_the_requested_column() {
    let text = "abcdef\nx\nuvwxyz";
    for keys in ["jj", "yjjj"] {
        let mut v = Vim::default();
        v.set_cursor(4, text);
        for c in keys.chars() {
            v.key(
                &KeyEvent {
                    key: Key::Text(c.to_string()),
                    mods: Mods::default(),
                },
                text,
            );
        }
        assert_eq!(v.cursor(text), 13..14, "{keys}");
    }
}

#[test]
fn find_and_repeat() {
    let mut v = Vim::default();
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
fn backward_finds_stop_on_or_after_the_target() {
    let text = "aXbcXde";
    for (keys, expected) in [("$FX", 4..5), ("$2FX", 1..2), ("$TX", 5..6), ("$2TX", 2..3)] {
        let mut v = Vim::default();
        press_on(&mut v, text, keys);
        assert_eq!(v.cursor(text), expected, "{keys}");
    }
    let mut v = Vim::default();
    press_on(&mut v, text, "$FX;");
    assert_eq!(v.cursor(text), 1..2);
    press_on(&mut v, text, ",");
    assert_eq!(v.cursor(text), 4..5);
}

#[test]
fn setting_the_cursor_clears_selection_and_pending_input() {
    let text = "αβ\nabcdef\nuvwxyz";
    for keys in ["v", "V", "y2", "g", "/hello"] {
        let mut v = Vim::default();
        press_on(&mut v, text, keys);
        let at = text.find('d').unwrap();
        v.set_cursor(at, text);
        assert_eq!(v.cursor(text), at..at + 1, "{keys}");
        assert_eq!(v.mode(), VimMode::Normal);
        assert_eq!(v.status(), "NORMAL");
        assert_eq!(v.selection(text), None);
        assert_eq!(v.pending(), "");
        assert!(!v.is_pending());
        press_on(&mut v, text, "j");
        let at = text.find('x').unwrap();
        assert_eq!(v.cursor(text), at..at + 1, "{keys}");
    }
    let mut v = Vim::default();
    v.set_cursor(text.len() + 10, text);
    assert_eq!(v.cursor(text), text.len() - 1..text.len());
    v.set_cursor("α".len(), text);
    assert_eq!(v.cursor(text), 2..4);
}

#[test]
fn paragraphs_and_brackets() {
    let mut v = Vim::default();
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
fn bracket_matching_handles_nested_pairs_in_both_directions() {
    for text in ["α(a(b)c)z", "α[a[b]c]z", "α{a{b}c}z"] {
        let mut v = Vim::default();
        press_on(&mut v, text, "%");
        assert_eq!(v.cursor(text), text.len() - 2..text.len() - 1);
        press_on(&mut v, text, "%");
        assert_eq!(v.cursor(text), 2..3);
    }
    for text in ["(missing", "missing)", "plain"] {
        let mut v = Vim::default();
        press_on(&mut v, text, "%");
        assert_eq!(v.cursor(text), 0..1);
        press_on(&mut v, text, "y%");
        assert!(!v.is_pending());
    }
}

#[test]
fn yank_motions_and_lines() {
    let mut v = Vim::default();
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
    let mut v = Vim::default();
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
    let mut v = Vim::default();
    press(&mut v, "vee");
    assert_eq!(v.mode(), VimMode::Visual);
    assert_eq!(v.status(), "VISUAL");
    assert_eq!(&TEXT[v.selection(TEXT).unwrap()], "fn main");
    press(&mut v, "o");
    assert!(at(&v).starts_with("fn"));
    assert_eq!(yanked(press(&mut v, "y")), "fn main");
    assert_eq!(v.mode(), VimMode::Normal);
    press(&mut v, "Vj");
    assert_eq!(v.mode(), VimMode::VisualLine);
    assert_eq!(v.status(), "V-LINE");
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
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert_eq!(v.mode(), VimMode::Normal);
    assert_eq!(v.status(), "NORMAL");
    assert_eq!(v.selection(TEXT), None);
}

#[test]
fn search_entry_restores_the_mode_and_discards_only_the_draft() {
    let key = |v: &mut Vim, key| {
        v.key(
            &KeyEvent {
                key,
                mods: Mods::default(),
            },
            TEXT,
        )
    };
    for (prefix, mode) in [
        ("", VimMode::Normal),
        ("v", VimMode::Visual),
        ("V", VimMode::VisualLine),
    ] {
        let mut v = Vim::default();
        press(&mut v, "/world");
        key(&mut v, Key::Enter);
        let committed = v.matches(TEXT);
        press(&mut v, prefix);
        let selection = v.selection(TEXT);
        let cursor = v.cursor(TEXT);
        press(&mut v, "/hello");
        assert_eq!(v.mode(), VimMode::Search);
        assert_eq!(v.status(), "/hello");
        assert_eq!(key(&mut v, Key::Up), None);
        assert_eq!(v.status(), "/hello");
        key(&mut v, Key::Escape);
        assert_eq!(v.mode(), mode);
        assert_eq!(v.cursor(TEXT), cursor);
        assert_eq!(v.selection(TEXT), selection);
        assert_eq!(v.matches(TEXT), committed);

        press(&mut v, "/x");
        key(&mut v, Key::Backspace);
        assert_eq!(v.status(), "/");
        key(&mut v, Key::Backspace);
        assert_eq!(v.mode(), mode);
        assert_eq!(v.selection(TEXT), selection);

        press(&mut v, "/hello");
        key(&mut v, Key::Enter);
        assert_eq!(v.mode(), mode);
        assert_eq!(v.cursor(TEXT).start, TEXT.find("hello").unwrap());
        assert_eq!(
            v.matches(TEXT),
            vec![TEXT.find("hello").unwrap()..TEXT.find("hello").unwrap() + 5]
        );
    }
}

#[test]
fn search_smart_case_and_star() {
    let mut v = Vim::default();
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
    assert_eq!(search_matches("Hello hello", "hello"), vec![0..5, 6..11]);
    assert_eq!(search_matches("Hello hello", "Hello"), vec![0..5]);
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
    let mut v = Vim::default();
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
fn digits_after_find_are_characters_not_counts() {
    let text = "a12b2";
    let mut vim = Vim::default();
    press_on(&mut vim, text, "f2");
    assert_eq!(vim.cursor(text), 2..3);
    vim.set_cursor(0, text);
    press_on(&mut vim, text, "2f2");
    assert_eq!(vim.cursor(text), 4..5);
    vim.set_cursor(0, text);
    press_on(&mut vim, text, "f");
    vim.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        text,
    );
    assert!(!vim.is_pending());
    assert_eq!(vim.cursor(text), 0..1);
}

#[test]
fn skipping_blank_lines() {
    let mut v = Vim::default().skip_blank_lines(true);
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
    let mut v = Vim::default();
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
        Some(VimEffect::Consumed)
    );
    assert!(v.matches(TEXT).is_empty());
    press(&mut v, "n");
    assert!(!v.matches(TEXT).is_empty());
}

fn source(effect: Option<VimEffect<NoAction>>) -> (std::ops::Range<usize>, bool) {
    match effect {
        Some(VimEffect::CopySource { range, linewise }) => (range, linewise),
        other => panic!("no source yank: {other:?}"),
    }
}

#[test]
fn gy_asks_for_the_source() {
    let mut v = Vim::default();
    let (range, linewise) = source(press(&mut v, "gyw"));
    assert_eq!((&TEXT[range], linewise), ("fn ", false));
    assert_eq!(v.message(), Some("3 characters yanked as source"));
    for keys in ["gyy", "gygy"] {
        let (range, linewise) = source(press(&mut v, keys));
        assert_eq!(
            (range, linewise),
            (0..TEXT.find('\n').unwrap(), true),
            "{keys}"
        );
    }
    let (range, linewise) = source(press(&mut v, "Vjgy"));
    assert_eq!(
        (&TEXT[range], linewise),
        ("fn main() {\n    let s = \"hello world\";", true)
    );
    // Plain `y` afterwards copies text again.
    assert_eq!(yanked(press(&mut v, "yw")), "fn ");
    assert_eq!(press(&mut v, "g"), Some(VimEffect::Consumed));
    assert_eq!(v.pending(), "g");
    press(&mut v, "y");
    assert_eq!(v.pending(), "gy");
}

#[test]
fn gy_text_object_after_a_visual_yank() {
    let mut v = Vim::default();
    yanked(press(&mut v, "vey"));
    let (range, _) = source(press(&mut v, "gyiw"));
    assert_eq!(&TEXT[range], "fn");
}

fn hints_for(groups: &HintGroups, kind: HintGroup) -> Vec<Hint> {
    groups
        .iter()
        .filter(|(group, _)| *group == kind)
        .flat_map(|(_, hints)| hints.iter().cloned())
        .collect()
}

#[test]
fn hints_follow_what_is_typed() {
    let mut v = Vim::default();
    assert_eq!(v.hints(), None, "nothing half typed");
    press(&mut v, "y");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, HintsFor::Typed("y".into()));
    assert_eq!(
        hints_for(&groups, HintGroup::Vim),
        vec![
            hint("y", "The line"),
            hint("i", "Inside an object…"),
            hint("a", "Around an object…"),
        ]
    );
    press(&mut v, "i");
    assert_eq!(v.hints().unwrap().0, HintsFor::Typed("yi".into()));
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    press(&mut v, "g");
    assert_eq!(v.hints().unwrap().0, HintsFor::Typed("g".into()));
    press(&mut v, "y");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(
        (typed, hints_for(&groups, HintGroup::Vim)),
        (
            HintsFor::Typed("gy".into()),
            vec![
                hint("gy", "The line"),
                hint("i", "Inside an object…"),
                hint("a", "Around an object…"),
            ]
        )
    );
    press(&mut v, "a");
    assert_eq!(v.hints().unwrap().0, HintsFor::Typed("gya".into()));
    press(&mut v, "w");
    assert_eq!(v.hints(), None);
    press(&mut v, "v");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, HintsFor::Mode(VimMode::Visual));
    assert!(
        groups
            .iter()
            .flat_map(|(_, hints)| hints)
            .any(|h| *h == hint("gy", "Yank as source"))
    );
    press(&mut v, "V");
    assert_eq!(v.hints().unwrap().0, HintsFor::Mode(VimMode::VisualLine));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Comment,
    Next,
    Quit,
}

fn app_vim() -> Vim<Act> {
    Vim::new(
        Keymap::vim()
            .operator(&[Stroke::Char('c')], Operator::App(Act::Comment), "Comment")
            .action(
                &[Stroke::Char(']'), Stroke::Char('c')],
                Act::Next,
                "Next comment",
            )
            .action(&[Stroke::Leader, Stroke::Char('q')], Act::Quit, "Quit"),
    )
}

fn press_app(vim: &mut Vim<Act>, keys: &str) -> Option<VimEffect<Act>> {
    let mut last = None;
    for c in keys.chars() {
        last = vim.key(
            &KeyEvent {
                key: Key::Text(c.to_string()),
                mods: Mods::default(),
            },
            TEXT,
        );
    }
    last
}

fn operated(effect: Option<VimEffect<Act>>) -> (&'static str, bool) {
    match effect {
        Some(VimEffect::Operator {
            action: Act::Comment,
            range,
            linewise,
        }) => (&TEXT[range], linewise),
        other => panic!("no operator: {other:?}"),
    }
}

#[test]
fn an_app_operator_takes_motions_objects_lines_and_selections() {
    let mut v = app_vim();
    assert_eq!(operated(press_app(&mut v, "cw")), ("fn ", false));
    assert_eq!(operated(press_app(&mut v, "c$")), ("fn main() {", false));
    assert_eq!(operated(press_app(&mut v, "cc")), ("fn main() {", true));
    assert_eq!(
        operated(press_app(&mut v, "2cc")),
        ("fn main() {\n    let s = \"hello world\";", true)
    );
    assert_eq!(
        operated(press_app(&mut v, "cj")),
        ("fn main() {\n    let s = \"hello world\";", true)
    );
    press_app(&mut v, "jfh");
    assert_eq!(operated(press_app(&mut v, "ciw")), ("hello", false));
    assert_eq!(operated(press_app(&mut v, "ci\"")), ("hello world", false));
    assert_eq!(operated(press_app(&mut v, "vec")), ("hello", false));
    assert_eq!(v.mode(), VimMode::Normal, "an operator ends the selection");
    press_app(&mut v, "G");
    assert_eq!(
        operated(press_app(&mut v, "cip")),
        ("second paragraph here\nand more", false)
    );
    // The cursor went to the paragraph's start, as an operator leaves it; yank works beside it.
    assert_eq!(
        yanked_app(press_app(&mut v, "yy")),
        "second paragraph here\n"
    );
}

fn yanked_app(effect: Option<VimEffect<Act>>) -> String {
    match effect {
        Some(VimEffect::Copy(s)) => s,
        other => panic!("no yank: {other:?}"),
    }
}

#[test]
fn app_actions_sequences_and_the_leader() {
    let mut v = app_vim();
    assert_eq!(press_app(&mut v, "]"), Some(VimEffect::Consumed));
    assert_eq!(v.pending(), "]");
    assert_eq!(
        press_app(&mut v, "c"),
        Some(VimEffect::Action {
            action: Act::Next,
            count: None
        })
    );
    assert_eq!(
        press_app(&mut v, "3]c"),
        Some(VimEffect::Action {
            action: Act::Next,
            count: Some(3)
        })
    );
    // The leader is `\` by default.
    assert_eq!(
        press_app(&mut v, "\\q"),
        Some(VimEffect::Action {
            action: Act::Quit,
            count: None
        })
    );
    // A sequence that goes nowhere is dropped; an unbound key is the app's.
    assert_eq!(press_app(&mut v, "]x"), Some(VimEffect::Consumed));
    assert!(!v.is_pending());
    assert_eq!(press_app(&mut v, "Q"), None);

    // Another leader. `,` is Vim's (find back), so it's unbound first.
    let comma = [Stroke::Char(',')];
    let keys = Keymap::vim()
        .action(&[Stroke::Leader, Stroke::Char('q')], Act::Quit, "Quit")
        .unbind(Table::Normal, &comma)
        .unbind(Table::Visual, &comma)
        .unbind_pending(&comma)
        .leader(KeyStroke::Char(','));
    let mut v = Vim::new(keys);
    assert_eq!(
        press_app(&mut v, ",q"),
        Some(VimEffect::Action {
            action: Act::Quit,
            count: None
        })
    );
}

#[test]
fn hint_groups_keep_their_kind_when_other_groups_are_empty() {
    let mut v = Vim::default();
    press(&mut v, "v");
    let groups = v.hints().unwrap().1;
    assert_eq!(groups, v.keymap().hints(Table::Visual));
    assert_eq!(
        groups.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        [HintGroup::Vim]
    );
    press(&mut v, "2");
    let groups = v.hints().unwrap().1;
    assert_eq!(
        groups.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        [HintGroup::Vim, HintGroup::Motion]
    );
}

#[test]
fn hints_come_from_the_keymap() {
    let mut v = app_vim();
    press_app(&mut v, "]");
    assert_eq!(
        v.hints().unwrap(),
        (
            HintsFor::Typed("]".into()),
            vec![(HintGroup::App, vec![hint("c", "Next comment")])]
        )
    );
    press_app(&mut v, "x");
    press_app(&mut v, "\\");
    assert_eq!(
        v.hints().unwrap().1,
        vec![(HintGroup::App, vec![hint("q", "Quit")])]
    );
    press_app(&mut v, "x");

    // In Visual mode the app's operator comes first, then Vim's.
    press_app(&mut v, "v");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, HintsFor::Mode(VimMode::Visual));
    assert_eq!(
        groups,
        vec![
            (HintGroup::App, vec![hint("c", "Comment")]),
            (
                HintGroup::Vim,
                vec![
                    hint("y", "Yank"),
                    hint("gy", "Yank as source"),
                    hint("o", "Go to the other end"),
                    hint("i", "Select inside an object…"),
                    hint("a", "Select around an object…"),
                ]
            ),
        ]
    );
    press_app(&mut v, "v");

    // After an operator: the line, then text objects and motions.
    press_app(&mut v, "c");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, HintsFor::Typed("c".into()));
    assert_eq!(
        hints_for(&groups, HintGroup::App),
        vec![hint("c", "The line")]
    );
    assert_eq!(
        hints_for(&groups, HintGroup::Vim),
        vec![
            hint("i", "Inside an object…"),
            hint("a", "Around an object…"),
        ]
    );
    assert_eq!(
        hints_for(&groups, HintGroup::Motion),
        vec![
            hint("w", "To the next word"),
            hint("b", "Back a word"),
            hint("e", "To the word's end"),
            hint("0", "From the line's start"),
            hint("$", "To the line's end"),
            hint("gg", "To the top"),
            hint("}", "To the paragraph's end"),
            hint("f", "To a character"),
            hint("t", "Up to a character"),
        ]
    );
    press_app(&mut v, "i");
    let (typed, groups) = v.hints().unwrap();
    assert_eq!(typed, HintsFor::Typed("ci".into()));
    assert_eq!(
        hints_for(&groups, HintGroup::Motion),
        vec![
            hint("w", "Word"),
            hint("W", "WORD, to spaces"),
            hint("p", "Paragraph"),
            hint("\"", "Double quotes"),
            hint("'", "Single quotes"),
            hint("`", "Backticks"),
            hint("(", "Parentheses"),
            hint("[", "Brackets"),
            hint("{", "Braces"),
            hint("<", "Angle brackets"),
        ]
    );
}

#[test]
fn objects_are_their_own_table() {
    // An app can add a text object, and it's offered after `i` and `a` only.
    let mut v = Vim::new(Keymap::<Act>::vim().object(
        &[Stroke::Char('q')],
        Object::Quotes('"'),
        "Any quotes",
    ));
    press_app(&mut v, "jfh");
    assert_eq!(yanked_app(press_app(&mut v, "yiq")), "hello world");
    assert_eq!(press_app(&mut v, "q"), None, "q alone is still unbound");
}

#[test]
#[should_panic(expected = "]c")]
fn a_binding_that_starts_another_panics() {
    let _ = Keymap::<Act>::vim()
        .action(&[Stroke::Char(']'), Stroke::Char('c')], Act::Next, None)
        .action(&[Stroke::Char(']')], Act::Quit, None);
}

#[test]
#[should_panic(expected = "starts with a digit")]
fn a_binding_on_a_digit_panics() {
    let _ = Keymap::<Act>::vim().action(&[Stroke::Char('1')], Act::Quit, None);
}

#[test]
fn rebinding_replaces_and_unbinding_drops() {
    let keys = Keymap::<Act>::vim()
        .action(&[Stroke::Char('n')], Act::Next, "Next comment")
        .unbind(Table::Normal, &[Stroke::Char('N')]);
    let mut v = Vim::new(keys);
    assert_eq!(
        press_app(&mut v, "n"),
        Some(VimEffect::Action {
            action: Act::Next,
            count: None
        })
    );
    assert_eq!(press_app(&mut v, "N"), None);
}

#[test]
#[should_panic(expected = ",q")]
fn a_leader_vim_already_uses_panics() {
    let _ = Keymap::<Act>::vim()
        .action(&[Stroke::Leader, Stroke::Char('q')], Act::Quit, None)
        .leader(KeyStroke::Char(','));
}

#[test]
fn a_huge_count_neither_panics_nor_hangs() {
    let mut v = Vim::default();
    let start = std::time::Instant::now();
    press(&mut v, &"9".repeat(25));
    press(&mut v, "w");
    assert_eq!(
        v.cursor(TEXT),
        TEXT.len() - 1..TEXT.len(),
        "a count past the end stops on the last character"
    );
    press(&mut v, "gg");
    press(&mut v, &"9".repeat(25));
    press(&mut v, "y");
    press(&mut v, &"9".repeat(25));
    assert_eq!(
        yanked(press(&mut v, "j")),
        format!("{TEXT}\n"),
        "a count past the end yanks to the last line"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}",
        start.elapsed()
    );
}

fn search_matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut vim = Vim::default();
    press_on(&mut vim, text, &format!("/{query}"));
    vim.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        text,
    );
    vim.matches(text)
}

#[test]
fn search_survives_characters_whose_lowercase_changes_length() {
    // `İ` lowercases to two characters, three bytes where it had two.
    let text = "İstanbul Hello hello";
    assert_eq!(search_matches(text, "hello"), vec![10..15, 16..21]);
    assert_eq!(search_matches(text, "i̇stanbul"), vec![0..9]);
    assert_eq!(search_matches("ẞtraße STRASSE", "sse"), vec![14..17]);
}

#[test]
fn pending_shows_the_keys_typed_with_their_counts() {
    let mut v = Vim::default();
    press(&mut v, "y2i");
    assert_eq!(
        v.pending(),
        "y2i",
        "the count between the operator and the object"
    );
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    press(&mut v, "3y2f");
    assert_eq!(v.pending(), "3y2f");
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );

    // Rebound keys show as typed, not as Vim's defaults.
    let keys = Keymap::<NoAction>::vim()
        .motion(&[Stroke::Char('x')], Motion::Find(Find::To), None)
        .bind_pending(&[Stroke::Char('I')], PendingCommand::Inner, None);
    let mut v = Vim::new(keys);
    press(&mut v, "x");
    assert_eq!(v.pending(), "x");
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    press(&mut v, "yI");
    assert_eq!(v.pending(), "yI");
}

#[test]
fn a_count_on_a_text_object_takes_that_many() {
    let mut v = Vim::default();
    assert_eq!(
        yanked(press(&mut v, "y2iw")),
        "fn ",
        "a space run counts as a word, as in Vim"
    );
    assert_eq!(yanked(press(&mut v, "y3iw")), "fn main");
    assert_eq!(
        yanked(press(&mut v, "2yiw")),
        "fn ",
        "the count may come before the operator"
    );
    assert_eq!(yanked(press(&mut v, "y2aw")), "fn main");
    press(&mut v, "v3iw");
    assert_eq!(&TEXT[v.selection(TEXT).unwrap()], "fn main");
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    assert_eq!(
        yanked(press(&mut v, "gg0y3ip")),
        TEXT,
        "both paragraphs and the blank line"
    );

    let text = "f(a, (b), c)";
    let mut v = Vim::default();
    press_on(&mut v, text, "fb");
    let yank = |v: &mut Vim, keys: &str| {
        let mut last = None;
        for c in keys.chars() {
            last = v.key(
                &KeyEvent {
                    key: Key::Text(c.to_string()),
                    mods: Mods::default(),
                },
                text,
            );
        }
        yanked(last)
    };
    assert_eq!(yank(&mut v, "yi("), "b");
    press_on(&mut v, text, "fb");
    assert_eq!(yank(&mut v, "y2i("), "a, (b), c", "the second pair out");
    press_on(&mut v, text, "fb");
    assert_eq!(yank(&mut v, "y2a("), "(a, (b), c)");
}

#[test]
fn a_typed_backslash_b_is_searched_as_typed() {
    let text = r"foobar barfoo \bfoo";
    let mut v = Vim::default();
    press_on(&mut v, text, r"/\bfoo");
    v.key(
        &KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        },
        text,
    );
    let found: Vec<&str> = v.matches(text).iter().map(|r| &text[r.clone()]).collect();
    assert_eq!(
        found,
        [r"\bfoo"],
        "a backslash the user typed is part of the query"
    );

    // `*` searches whole words.
    let mut v = Vim::default();
    press_on(&mut v, text, "*");
    let found: Vec<usize> = v.matches(text).iter().map(|r| r.start).collect();
    assert_eq!(found, [0], "foobar as a whole word only");
}

#[test]
fn a_one_character_yank_says_character() {
    let mut v = Vim::default();
    press(&mut v, "yl");
    assert_eq!(v.message(), Some("1 character yanked"));
    press(&mut v, "y2l");
    assert_eq!(v.message(), Some("2 characters yanked"));
}

#[test]
fn hints_show_what_a_count_can_take() {
    let mut v = Vim::default();
    press(&mut v, "3");
    let (typed, groups) = v.hints().expect("a count is half typed");
    assert_eq!(typed, HintsFor::Typed("3".into()));
    assert!(
        hints_for(&groups, HintGroup::Motion)
            .iter()
            .any(|h| h.keys == "w"),
        "motions take a count"
    );
    assert!(
        hints_for(&groups, HintGroup::Vim)
            .iter()
            .any(|h| h.keys == "y"),
        "so do operators"
    );
    v.key(
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        TEXT,
    );
    press(&mut v, "v3");
    assert_eq!(
        v.hints().expect("in visual mode too").0,
        HintsFor::Typed("3".into())
    );
}

#[test]
fn half_a_page_is_the_page_the_app_sets() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let line = |v: &Vim| text[..v.cursor(&text).start].matches('\n').count();
    let ctrl_d = |v: &mut Vim| {
        v.key(
            &KeyEvent {
                key: Key::Command('d'),
                mods: Mods {
                    ctrl: true,
                    cmd: true,
                    ..Mods::default()
                },
            },
            &text,
        );
    };
    let mut v = Vim::default();
    ctrl_d(&mut v);
    assert_eq!(line(&v), 15, "without a page height, 15 lines as before");
    let mut v = Vim::default().page_lines(40);
    ctrl_d(&mut v);
    assert_eq!(line(&v), 20, "half of 40");
}

#[test]
#[should_panic(expected = "OtherEnd")]
fn binding_other_end_outside_visual_mode_panics() {
    let _ = Keymap::<NoAction>::vim().bind(
        Table::Normal,
        &[Stroke::Char('o')],
        Command::OtherEnd,
        None,
    );
}

#[test]
#[should_panic(expected = "Lines")]
fn binding_a_lines_operator_in_visual_mode_panics() {
    let _ = Keymap::<NoAction>::vim().bind(
        Table::Visual,
        &[Stroke::Char('Y')],
        Command::Lines(Operator::Yank),
        None,
    );
}
