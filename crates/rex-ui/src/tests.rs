use rune::Color;

use crate::{Palette, PaletteEntry, PaletteKey, Rex, palette_matches};

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
    let rex = Rex::new(Palette::merino_light());
    assert_eq!(rex.on(Color::BLACK), Color::WHITE);
    assert!(rex.on(Color::WHITE).luminance() < 0.2);
}
