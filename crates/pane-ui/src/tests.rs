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
    for palette in Palette::builtins() {
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
