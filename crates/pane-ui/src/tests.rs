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
    accessibility_node(&rt.accessibility_json(), key)
        .unwrap_or_else(|| panic!("no {key} in the accessibility tree"))
}

fn accessibility_node(node: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
    if node["key"] == key {
        return Some(node.clone());
    }
    node["children"]
        .as_array()?
        .iter()
        .find_map(|child| accessibility_node(child, key))
}

fn element<S: 'static>(rt: &rune::Runtime<S>, key: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no rendered element {key}"))
}

fn click<S: 'static>(rt: &mut rune::Runtime<S>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("no clickable element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
}

fn press<S: 'static>(rt: &mut rune::Runtime<S>, key: rune::Key) {
    rt.key(rune::KeyEvent {
        key,
        mods: rune::Mods::default(),
    });
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
fn select_overlays_show_choices_apply_them_and_dismiss() {
    #[derive(Default)]
    struct State {
        open: bool,
        current: usize,
    }
    fn view(s: &State, _: &mut rune::Cx<State>) -> rune::El {
        let theme = Theme::new(Palette::merino_dark());
        let mut layer = Vec::new();
        let select = crate::Select::new(["Light", "Dark"])
            .current(s.current)
            .open(s.open)
            .on_toggle(|s: &mut State| s.open = !s.open)
            .on_choose(|s: &mut State, index| {
                s.current = index;
                s.open = false;
            })
            .build("theme", &theme, &mut layer);
        crate::Page::new()
            .child(select)
            .overlays(layer)
            .build(&theme)
    }
    let mut rt = rune::Runtime::new(State::default(), view, (400.0, 300.0), rune::Env::default());
    rt.frame(0.0);
    assert_eq!(element(&rt, "theme").label.as_deref(), Some("Light"));
    assert_eq!(rt.find("theme-menu"), None);
    assert_eq!(rt.find("option-1"), None);
    click(&mut rt, "theme");
    rt.frame(0.1);
    assert_eq!(element(&rt, "theme").expanded, Some(true));
    assert!(rt.find("theme-menu").is_some());
    assert_eq!(element(&rt, "option-0").label.as_deref(), Some("Light"));
    assert_eq!(element(&rt, "option-1").label.as_deref(), Some("Dark"));
    click(&mut rt, "option-1");
    rt.frame(0.2);
    assert_eq!(element(&rt, "theme").label.as_deref(), Some("Dark"));
    assert_eq!(element(&rt, "theme").expanded, Some(false));
    assert_eq!(rt.find("theme-menu"), None);
    assert_eq!(rt.find("option-1"), None);
    for (cycle, outside) in [false, true].into_iter().enumerate() {
        let time = 1.0 + cycle as f64;
        click(&mut rt, "theme");
        rt.frame(time);
        assert!(rt.find("theme-menu").is_some());
        if outside {
            rt.pointer_down(390.0, 290.0);
            rt.pointer_up(390.0, 290.0);
        } else {
            press(&mut rt, rune::Key::Escape);
        }
        rt.frame(time + 0.1);
        assert_eq!(rt.find("theme-menu"), None);
        assert_eq!(rt.find("option-1"), None);
        assert_eq!(element(&rt, "theme").expanded, Some(false));
        assert_eq!(element(&rt, "theme").label.as_deref(), Some("Dark"));
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

#[test]
fn disabled_controls_ignore_pointer_and_keyboard_actions() {
    #[derive(Clone, Copy, Debug)]
    enum Control {
        Button,
        ButtonTx,
        Checkbox,
        Toggle,
        MenuItem,
    }
    struct State {
        control: Control,
        disabled: bool,
        presses: usize,
        hovers: usize,
    }
    fn view(s: &State, _: &mut rune::Cx<State>) -> rune::El {
        let theme = Theme::new(Palette::merino_dark());
        let action = |s: &mut State| s.presses += 1;
        let control = match s.control {
            Control::Button => crate::Button::new("Run")
                .disabled(s.disabled)
                .on_press(action)
                .build("control", &theme),
            Control::ButtonTx => crate::Button::new("Run")
                .disabled(s.disabled)
                .on_press_tx(|s: &mut State, _| s.presses += 1)
                .build("control", &theme),
            Control::Checkbox => crate::Checkbox::new(false)
                .label("Notify")
                .disabled(s.disabled)
                .on_toggle(action)
                .build("control", &theme),
            Control::Toggle => crate::Toggle::new(false, "Notify")
                .disabled(s.disabled)
                .on_toggle(action)
                .build(&theme)
                .key("control"),
            Control::MenuItem => crate::MenuItem::new("Run")
                .disabled(s.disabled)
                .on_press(action)
                .on_hover(|s: &mut State| s.hovers += 1)
                .build("control", &theme),
        };
        rune::col()
            .child(control)
            .child(rune::text(format!("Actions: {}", s.presses)).key("result"))
    }
    for control in [
        Control::Button,
        Control::ButtonTx,
        Control::Checkbox,
        Control::Toggle,
        Control::MenuItem,
    ] {
        for disabled in [false, true] {
            for keyboard in [false, true] {
                let mut rt = rune::Runtime::new(
                    State {
                        control,
                        disabled,
                        presses: 0,
                        hovers: 0,
                    },
                    view,
                    (400.0, 300.0),
                    rune::Env::default(),
                );
                rt.frame(0.0);
                let node = accessibility_node(&rt.accessibility_json(), "control").unwrap();
                assert_eq!(
                    states(&node).iter().any(|state| state == "disabled"),
                    disabled,
                    "{control:?}, disabled={disabled}: {node}"
                );
                assert_eq!(element(&rt, "control").focusable, !disabled);
                if keyboard {
                    press(&mut rt, rune::Key::Tab);
                    rt.frame(0.1);
                    assert_eq!(element(&rt, "control").focused, !disabled, "{control:?}");
                    press(&mut rt, rune::Key::Text(" ".to_owned()));
                } else {
                    let (x, y) = rt.find("control").unwrap();
                    rt.pointer_move(x, y);
                    rt.frame(0.1);
                    click(&mut rt, "control");
                }
                rt.frame(0.2);
                assert_eq!(
                    element(&rt, "result").text.as_deref(),
                    Some(if disabled { "Actions: 0" } else { "Actions: 1" }),
                    "{control:?}, disabled={disabled}, keyboard={keyboard}"
                );
                if !keyboard {
                    assert_eq!(
                        rt.state.hovers,
                        usize::from(!disabled && matches!(control, Control::MenuItem)),
                        "{control:?}, disabled={disabled}"
                    );
                }
            }
        }
    }
}

#[test]
fn controls_prefer_a_width_shrink_to_fit_and_fill_on_request() {
    #[derive(Clone, Copy, Debug)]
    enum Width {
        Default,
        Preferred,
        Fill,
        PreferredAfterFill,
        FillAfterPreferred,
    }
    struct State {
        width: Width,
        edit: rune::TextEdit,
    }
    fn view(s: &State, _: &mut rune::Cx<State>) -> rune::El {
        let theme = Theme::new(Palette::merino_dark());
        let field = crate::TextField::new(&s.edit, |s: &mut State| &mut s.edit);
        let select = crate::Select::new(["Light", "Dark"]);
        let slider = crate::Slider::new(0.5);
        let (field, select, slider) = match s.width {
            Width::Default => (field, select, slider),
            Width::Preferred => (field.width(160.0), select.width(160.0), slider.width(160.0)),
            Width::Fill => (field.fill(), select.fill(), slider.fill()),
            Width::PreferredAfterFill => (
                field.fill().width(160.0),
                select.fill().width(160.0),
                slider.fill().width(160.0),
            ),
            Width::FillAfterPreferred => (
                field.width(160.0).fill(),
                select.width(160.0).fill(),
                slider.width(160.0).fill(),
            ),
        };
        rune::col()
            .w_full()
            .child(
                rune::row()
                    .w_full()
                    .child(field.build("input", &theme).key("field")),
            )
            .child(
                rune::row()
                    .w_full()
                    .child(select.build("select", &theme, &mut Vec::new())),
            )
            .child(rune::row().w_full().child(slider.build("slider", &theme)))
    }
    for width in [
        Width::Default,
        Width::Preferred,
        Width::Fill,
        Width::PreferredAfterFill,
        Width::FillAfterPreferred,
    ] {
        for available in [120.0_f32, 600.0] {
            let mut rt = rune::Runtime::new(
                State {
                    width,
                    edit: rune::TextEdit::default(),
                },
                view,
                (available, 300.0),
                rune::Env::default(),
            );
            rt.frame(0.0);
            for (key, default) in [("field", 200.0_f32), ("select", 200.0), ("slider", 240.0)] {
                let expected = match width {
                    Width::Default => default.min(available),
                    Width::Preferred | Width::PreferredAfterFill => 160.0_f32.min(available),
                    Width::Fill | Width::FillAfterPreferred => available,
                };
                assert_eq!(
                    element(&rt, key).rect[2],
                    expected,
                    "{key}, {width:?}, available={available}"
                );
            }
        }
    }
}

#[test]
fn filled_segments_share_the_available_width() {
    fn view(fill: &bool, _: &mut rune::Cx<bool>) -> rune::El {
        let theme = Theme::new(Palette::merino_dark());
        let control = crate::SegmentedControl::new(["One", "Longer label"]);
        let control = if *fill { control.fill() } else { control };
        rune::col().w_full().child(control.build("tabs", &theme))
    }
    for fill in [false, true] {
        let mut rt = rune::Runtime::new(fill, view, (400.0, 100.0), rune::Env::default());
        rt.frame(0.0);
        let track = element(&rt, "tabs").rect;
        let first = element(&rt, "option-0").rect;
        let second = element(&rt, "option-1").rect;
        if fill {
            assert_eq!(track[2], 400.0);
            assert_eq!(first[2], second[2]);
            assert_eq!(first[0], 2.0);
            assert_eq!(second[0] + second[2], 398.0);
        } else {
            assert!(track[2] < 400.0, "natural segments do not stretch");
        }
    }
}
