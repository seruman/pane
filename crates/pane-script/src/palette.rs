//! Pane tokens for Luau, resolved from the terminal (Merino outside a terminal).

use std::{cell::RefCell, rc::Rc};

use pane_ui::{Palette, Theme};
use rune::{Color, TerminalColors};

struct Cached {
    dark: bool,
    colors: Option<TerminalColors>,
    theme: Theme,
}

pub(crate) fn current() -> rune_luau::Palette {
    let cached = RefCell::new(None::<Cached>);
    Rc::new(move |name, dark| {
        let colors = rune::terminal_colors();
        let mut cached = cached.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|c| c.dark != dark || c.colors != colors)
        {
            *cached = Some(Cached {
                dark,
                colors,
                theme: resolve(colors, dark),
            });
        }
        cached.as_ref().and_then(|c| color(name, &c.theme))
    })
}

fn resolve(colors: Option<TerminalColors>, dark: bool) -> Theme {
    colors
        .and_then(Theme::from_terminal_colors)
        .unwrap_or_else(|| {
            Theme::new(if dark {
                Palette::merino_dark()
            } else {
                Palette::merino_light()
            })
        })
}

fn color(name: &str, theme: &Theme) -> Option<Color> {
    Some(match name {
        "bg" => theme.background,
        "card" => theme.group,
        "card-border" => theme.border,
        "text" => theme.text,
        "text-secondary" => theme.text_secondary,
        "separator" => theme.separator,
        "hover" => theme.hover,
        "press" => theme.pressed,
        "control-off" | "control-track" => theme.field,
        "indicator" => theme.popover,
        "accent" => theme.accent,
        "accent-hover" => theme.accent_hover,
        "accent-text" => theme.accent_text,
        "on-accent" => theme.on_accent,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_background_foreground_and_cursor_drive_script_colors() {
        for dark in [false, true] {
            let mut colors = TerminalColors::default();
            colors.background = Some(pane_ui::rgb(if dark { 0x201c19 } else { 0xf5f0e8 }));
            colors.foreground = Some(pane_ui::rgb(if dark { 0xeee7dd } else { 0x332a22 }));
            colors.cursor = Some(pane_ui::rgb(0xb05223));
            let theme = resolve(Some(colors), !dark);
            assert_eq!(theme.palette.dark, dark);
            assert_eq!(color("bg", &theme), colors.background);
            assert_eq!(color("text", &theme), colors.foreground);
            assert_eq!(color("accent", &theme), colors.cursor);
            assert_eq!(color("on-accent", &theme), Some(theme.on(theme.accent)));
            assert_ne!(color("accent", &theme), Some(pane_ui::rgb(0x0969da)));
        }
    }

    #[test]
    fn luau_renders_terminal_accent_with_a_contrasting_label() {
        let theme = Theme::new(Palette::charcoal_atelier());
        let expected = theme.clone();
        let script = rune_luau::Script::with_options(
            "palette",
            rune_luau::Sources::single(
                "local ui = require('@rune/ui'); return { render = function() return ui.col({}, { ui.row({ key = 'swatch', fill = 'accent' }, { ui.text('label', { color = 'on-accent' }) }) }) end }",
            ),
            rune_luau::ScriptOptions::new().palette(Rc::new(move |name, _| color(name, &theme))),
        );
        let mut rt = rune::Runtime::new(
            script,
            |script, cx| script.view(cx),
            (200.0, 100.0),
            rune::Env::default(),
        );
        rt.frame(0.0);
        assert!(rt.state.error().is_none());
        let elements = rt.elements();
        assert!(
            elements
                .iter()
                .any(|e| e.key.as_deref() == Some("swatch") && e.fill == expected.accent)
        );
        assert!(elements.iter().any(
            |e| e.text.as_deref() == Some("label") && e.text_color == Some(expected.on_accent)
        ));
    }

    #[test]
    fn missing_terminal_colors_use_merino_in_the_requested_appearance() {
        for dark in [false, true] {
            let theme = resolve(None, dark);
            let fallback = if dark {
                Palette::merino_dark()
            } else {
                Palette::merino_light()
            };
            assert_eq!(color("bg", &theme), Some(fallback.background));
            assert_eq!(color("text", &theme), Some(fallback.foreground));
            assert_eq!(color("accent", &theme), Some(fallback.cursor));
            assert_eq!(color("not-a-token", &theme), None);
        }
    }
}
