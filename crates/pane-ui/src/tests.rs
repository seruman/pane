use rune::Color;

use crate::{Palette, PaletteEntry, PaletteKey, Theme, palette_matches};
use rune_kit::Kit;

const ENTRIES: &[PaletteEntry] = &[
    PaletteEntry::new("Dark Theme").section("Appearance"),
    PaletteEntry::new("Run Tests"),
    PaletteEntry::new("Follow Terminal Theme").section("Appearance"),
    PaletteEntry::new("Open Logs"),
];

#[test]
fn matches_list_unsectioned_entries_first() {
    let order: Vec<usize> = palette_matches(ENTRIES, "")
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(order, [1, 3, 0, 2]);
}

#[test]
fn matches_filter_case_insensitively_and_mark_the_match() {
    let found = palette_matches(ENTRIES, "TE");
    assert_eq!(found, [(1, Some(4..6)), (2, Some(7..9))]);
}

#[test]
fn up_and_down_wrap_at_the_ends() {
    assert_eq!(PaletteKey::Up.step(0, 3), 2);
    assert_eq!(PaletteKey::Down.step(2, 3), 0);
    assert_eq!(PaletteKey::Down.step(0, 0), 0);
    assert_eq!(PaletteKey::Confirm.step(1, 3), 1);
}

#[test]
fn text_on_a_fill_contrasts_with_it() {
    let theme = Theme::new(Palette::merino_light());
    assert_eq!(theme.on(Color::BLACK), Color::WHITE);
    assert!(theme.on(Color::WHITE).luminance() < 0.2);
}

#[test]
fn the_kit_is_rune_kits_terminal_kit() {
    for palette in [
        Palette::merino_dark(),
        Palette::merino_light(),
        Palette::charcoal_atelier(),
    ] {
        let theme = Theme::new(palette.clone());
        let terminal = Kit::from_terminal_colors(palette.terminal_colors()).unwrap();
        assert_eq!(theme.kit.c.page, terminal.c.page, "{}", palette.name);
        assert_eq!(
            theme.kit.c.bg_accent, palette.cursor,
            "{}: the cursor is the accent",
            palette.name
        );
        assert_eq!(
            theme.kit.c.border_focus, terminal.c.border_focus,
            "{}",
            palette.name
        );
        assert_eq!(
            theme.kit.c.danger.fg,
            palette.ansi[if palette.dark { 9 } else { 1 }],
            "{}: danger is ANSI red",
            palette.name
        );
        assert_eq!(
            theme.kit.c.knob,
            Color::WHITE,
            "{}: the knob stays white",
            palette.name
        );
    }
}

#[test]
fn matches_mark_the_title_even_when_lowercasing_changes_its_length() {
    const TITLES: &[PaletteEntry] = &[
        PaletteEntry::new("İstanbul Run"),
        PaletteEntry::new("\u{212A}elvin Run"),
    ];
    for (i, range) in palette_matches(TITLES, "run") {
        let range = range.expect("a match has a range");
        assert_eq!(&TITLES[i].title[range], "Run", "{}", TITLES[i].title);
    }
    assert_eq!(palette_matches(TITLES, "run").len(), 2);
}

/// The accessibility node keyed `key` in a one-frame render of what `content` builds, as
/// assistive technology reads it.
fn a11y(content: fn(&Theme) -> rune::El, key: &str) -> serde_json::Value {
    let theme = Theme::new(Palette::merino_dark());
    let mut rt = rune::Runtime::new(
        content(&theme),
        |e: &rune::El, _: &mut rune::Cx<rune::El>| e.clone(),
        (400.0, 300.0),
        rune::Env::default(),
    );
    rt.frame(0.0);
    fn find(v: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
        if v["key"] == key {
            return Some(v.clone());
        }
        v["children"].as_array()?.iter().find_map(|c| find(c, key))
    }
    find(&rt.accessibility_json(), key)
        .unwrap_or_else(|| panic!("no {key} in the accessibility tree"))
}

fn states(node: &serde_json::Value) -> Vec<String> {
    node["states"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_open_select_reads_as_expanded_not_checked() {
    let open = a11y(
        |t| {
            crate::Select::new(["Light", "Dark"])
                .open(true)
                .build("theme", t, &mut Vec::new())
        },
        "theme",
    );
    assert!(states(&open).contains(&"expanded".to_owned()), "{open}");
    assert!(
        !states(&open).iter().any(|s| s.contains("checked")),
        "a combobox is never checked: {open}"
    );
    let shut = a11y(
        |t| crate::Select::new(["Light", "Dark"]).build("theme", t, &mut Vec::new()),
        "theme",
    );
    assert!(states(&shut).contains(&"collapsed".to_owned()), "{shut}");
}

#[test]
fn a_menu_item_reads_as_checked_until_checked_false() {
    let on = a11y(
        |t| crate::MenuItem::new("Wrap").checked(true).build("wrap", t),
        "wrap",
    );
    assert!(states(&on).contains(&"checked".to_owned()), "{on}");
    let off = a11y(
        |t| {
            crate::MenuItem::new("Wrap")
                .checked(true)
                .checked(false)
                .build("wrap", t)
        },
        "wrap",
    );
    assert!(
        states(&off).contains(&"unchecked".to_owned()),
        "the later call wins: {off}"
    );
}

#[test]
fn every_status_mark_has_a_name() {
    type Build = fn(&Theme) -> rune::El;
    let marks: [(crate::Status, Build); 5] = [
        (crate::Status::Idle, |t| {
            crate::Status::Idle.mark(t).key("mark")
        }),
        (crate::Status::Working, |t| {
            crate::Status::Working.mark(t).key("mark")
        }),
        (crate::Status::Done, |t| {
            crate::Status::Done.mark(t).key("mark")
        }),
        (crate::Status::Blocked, |t| {
            crate::Status::Blocked.mark(t).key("mark")
        }),
        (crate::Status::Error, |t| {
            crate::Status::Error.mark(t).key("mark")
        }),
    ];
    for (status, mark) in marks {
        assert_eq!(a11y(mark, "mark")["name"], status.label(), "{status:?}");
    }
}

#[test]
fn the_theme_and_its_kit_agree_on_the_terminal_colours() {
    // A dark terminal that reports its text, background and ANSI table, but no cursor.
    let mut colors = rune::TerminalColors::default();
    colors.background = Some(crate::rgb(0x101010));
    colors.foreground = Some(crate::rgb(0xe0e0e0));
    colors.palette = Palette::merino_dark().ansi.map(Some);
    let theme = Theme::from_terminal_colors(colors).expect("text and background are reported");
    assert_eq!(
        theme.accent, theme.kit.c.bg_accent,
        "one accent when the cursor falls back"
    );
    assert_eq!(theme.danger, theme.kit.c.danger.fg, "one red");
    assert_eq!(theme.warning, theme.kit.c.warning.fg, "one yellow");
    assert_eq!(theme.working, theme.kit.c.success.fg, "one green");
}

#[test]
fn an_app_can_build_its_own_palette() {
    let ansi = Palette::merino_light().ansi;
    let palette = Palette::new(
        "Paper",
        crate::rgb(0xffffff),
        crate::rgb(0x202020),
        crate::rgb(0x0066cc),
        ansi,
    );
    assert!(!palette.dark, "dark comes from the colours");
    let theme = Theme::new(palette);
    assert_eq!(theme.accent, crate::rgb(0x0066cc));
}
