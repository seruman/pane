use rune::Color;

use crate::{palette_matches, Palette, PaletteEntry, PaletteKey, Theme};

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
#[should_panic(expected = "index out of bounds")]
fn ansi_indices_outside_the_palette_are_rejected() {
    Theme::new(Palette::merino_dark()).ansi(16);
}

#[test]
fn text_on_a_fill_uses_white_or_the_palettes_darkest_ink() {
    for (background, foreground, expected_ink) in [
        (0xffffff, 0x555555, "#333333"),
        (0x101010, 0xeeeeee, "#101010"),
    ] {
        let palette = Palette::new(
            "Contrast fixture",
            crate::rgb(background),
            crate::rgb(foreground),
            crate::rgb(0x0066cc),
            Palette::merino_light().ansi,
        );
        let theme = Theme::new(palette);
        assert_eq!(theme.on(Color::BLACK), Color::WHITE);
        assert_eq!(crate::hex_code(theme.on(Color::WHITE)), expected_ink);
    }
}

#[test]
fn builtin_palettes_share_the_cursor_accent_and_status_colours_with_the_kit() {
    for palette in [
        Palette::merino_dark(),
        Palette::merino_light(),
        Palette::charcoal_atelier(),
    ] {
        let theme = Theme::new(palette.clone());
        assert_eq!(theme.background, palette.background);
        assert_eq!(theme.kit.c.page, palette.background, "{}", palette.name);
        assert_eq!(theme.accent, theme.kit.c.bg_accent);
        assert_eq!(theme.success, theme.kit.c.success.fg);
        assert_eq!(theme.warning, theme.kit.c.warning.fg);
        assert_eq!(theme.danger, theme.kit.c.danger.fg);
        assert_eq!(
            theme.kit.c.bg_accent, palette.cursor,
            "{}: the cursor is the accent",
            palette.name
        );
        assert_eq!(theme.kit.c.border_focus, palette.cursor, "{}", palette.name);
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
fn select_height_uses_the_control_size() {
    fn view(size: &crate::ControlSize, _: &mut rune::Cx<crate::ControlSize>) -> rune::El {
        rune::col().child(crate::Select::new(["Light", "Dark"]).size(*size).build(
            "select",
            &Theme::new(Palette::merino_dark()),
            &mut Vec::new(),
        ))
    }
    for size in [
        crate::ControlSize::Small,
        crate::ControlSize::Regular,
        crate::ControlSize::Large,
    ] {
        let mut rt = rune::Runtime::new(size, view, (400.0, 300.0), rune::Env::default());
        rt.frame(0.0);
        let select = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("select"))
            .unwrap();
        assert_eq!(select.rect[3], size.height());
    }
}

#[test]
fn select_toggle_opens_and_escape_closes_repeatedly() {
    #[derive(Default)]
    struct State {
        open: bool,
        toggles: usize,
    }
    fn view(s: &State, _: &mut rune::Cx<State>) -> rune::El {
        let theme = Theme::new(Palette::merino_dark());
        let mut layer = Vec::new();
        let select = crate::Select::new(["Light", "Dark"])
            .open(s.open)
            .on_toggle(|s: &mut State| {
                s.open = !s.open;
                s.toggles += 1;
            })
            .build("theme", &theme, &mut layer);
        crate::Page::new()
            .child(select)
            .overlays(layer)
            .build(&theme)
    }
    let mut rt = rune::Runtime::new(State::default(), view, (400.0, 300.0), rune::Env::default());
    for cycle in 0..2 {
        rt.frame(cycle as f64);
        let (x, y) = rt.find("theme").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(cycle as f64 + 0.1);
        assert!(rt.state.open);
        rt.key(rune::KeyEvent {
            key: rune::Key::Escape,
            mods: rune::Mods::default(),
        });
        rt.frame(cycle as f64 + 0.2);
        assert!(!rt.state.open);
        assert_eq!(rt.state.toggles, 2 * (cycle + 1));
    }
}

#[test]
#[should_panic(expected = "highlight must be a byte range within the label")]
fn menu_highlights_reject_split_unicode_characters() {
    crate::MenuItem::new("éclair").highlight(Some(1..2));
}

#[test]
fn invalid_empty_and_reversed_menu_highlights_are_rejected() {
    for range in [1..1, 99..99, std::ops::Range { start: 2, end: 0 }] {
        assert!(
            std::panic::catch_unwind(|| {
                crate::MenuItem::new("éclair").highlight(Some(range.clone()));
            })
            .is_err(),
            "invalid range {range:?}"
        );
    }
}

#[test]
fn empty_menu_highlights_keep_the_narrow_label_layout() {
    const LABEL: &str = "éclair with a long label that must truncate";
    fn view(
        range: &Option<std::ops::Range<usize>>,
        _: &mut rune::Cx<Option<std::ops::Range<usize>>>,
    ) -> rune::El {
        rune::col().child(
            crate::MenuItem::new(LABEL)
                .highlight(range.clone())
                .build("item", &Theme::new(Palette::merino_dark()))
                .w_full(),
        )
    }
    let layout = |range| {
        let mut rt = rune::Runtime::new(range, view, (120.0, 40.0), rune::Env::default());
        rt.frame(0.0);
        rt.elements()
            .into_iter()
            .filter_map(|e| e.text.filter(|t| !t.is_empty()).map(|text| (text, e.rect)))
            .collect::<Vec<_>>()
    };
    let normal = layout(None);
    assert_eq!(normal.len(), 1);
    assert!(
        normal[0].1[0] + normal[0].1[2] <= 120.0,
        "normal label stays within the row"
    );
    for offset in [LABEL.len(), 0, "é".len()] {
        assert_eq!(
            layout(Some(offset..offset)),
            normal,
            "empty highlight at {offset}"
        );
    }
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
fn the_terminal_constructor_keeps_pane_and_kit_colours_in_agreement() {
    for dark in [false, true] {
        for cursor in [None, Some(crate::rgb(0xcc44bb))] {
            let background = crate::rgb(if dark { 0x101010 } else { 0xf0f0f0 });
            let foreground = crate::rgb(if dark { 0xe0e0e0 } else { 0x202020 });
            let mut colors = rune::TerminalColors::default();
            colors.background = Some(background);
            colors.foreground = Some(foreground);
            colors.cursor = cursor;
            colors.dark = Some(!dark);
            colors.palette[1] = Some(crate::rgb(0xaa2211));
            colors.palette[2] = Some(crate::rgb(0x228833));
            colors.palette[3] = Some(crate::rgb(0x886622));
            colors.palette[4] = Some(crate::rgb(0x2255aa));
            colors.palette[9] = Some(crate::rgb(0xff7755));
            colors.palette[10] = Some(crate::rgb(0x66ee88));
            colors.palette[11] = Some(crate::rgb(0xffcc66));
            colors.palette[12] = Some(crate::rgb(0x88aaff));
            let accent = cursor.unwrap_or(crate::rgb(if dark { 0x88aaff } else { 0x2255aa }));
            let danger = crate::rgb(if dark { 0xff7755 } else { 0xaa2211 });
            let success = crate::rgb(if dark { 0x66ee88 } else { 0x228833 });
            let warning = crate::rgb(if dark { 0xffcc66 } else { 0x886622 });
            let theme = Theme::from_terminal_colors(colors).unwrap();
            assert_eq!(
                theme.palette.dark, dark,
                "colours override the reported scheme"
            );
            assert_eq!(theme.background, background);
            assert_eq!(theme.kit.c.page, background);
            assert_eq!(theme.text, foreground);
            assert_eq!(theme.accent, accent);
            assert_eq!(theme.kit.c.bg_accent, accent);
            assert_eq!(theme.kit.c.border_focus, accent);
            assert_eq!(theme.danger, danger);
            assert_eq!(theme.kit.c.danger.fg, danger);
            assert_eq!(theme.success, success);
            assert_eq!(theme.kit.c.success.fg, success);
            assert_eq!(theme.warning, warning);
            assert_eq!(theme.kit.c.warning.fg, warning);
        }
    }
}

#[test]
fn an_unreported_terminal_palette_uses_the_matching_merino_colours() {
    for (background, foreground, accent, danger, success, warning) in [
        (0xffffff, 0x202020, 0x0969da, 0xcf222e, 0x116329, 0x9d5c00),
        (0x101010, 0xeeeeee, 0x79c0ff, 0xffa198, 0x56d364, 0xe3b341),
    ] {
        let mut colors = rune::TerminalColors::default();
        colors.background = Some(crate::rgb(background));
        colors.foreground = Some(crate::rgb(foreground));
        let theme = Theme::from_terminal_colors(colors).unwrap();
        assert_eq!(theme.accent, crate::rgb(accent));
        assert_eq!(theme.kit.c.bg_accent, theme.accent);
        assert_eq!(theme.danger, crate::rgb(danger));
        assert_eq!(theme.kit.c.danger.fg, theme.danger);
        assert_eq!(theme.success, crate::rgb(success));
        assert_eq!(theme.kit.c.success.fg, theme.success);
        assert_eq!(theme.warning, crate::rgb(warning));
        assert_eq!(theme.kit.c.warning.fg, theme.warning);
    }
}

#[test]
fn a_terminal_theme_needs_both_background_and_foreground() {
    for (background, foreground) in [
        (None, None),
        (Some(Color::BLACK), None),
        (None, Some(Color::WHITE)),
    ] {
        let mut colors = rune::TerminalColors::default();
        colors.background = background;
        colors.foreground = foreground;
        colors.cursor = Some(Color::WHITE);
        assert!(Theme::from_terminal_colors(colors).is_none());
    }
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
