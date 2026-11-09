//! The theme: a terminal [`Palette`] and the [`Theme`] tokens derived from it. Components read
//! only these tokens, so changing a field restyles every component that uses it.

use rune::color::{srgb_decode, srgb_encode};
use rune::{Color, TerminalColors, TextStyle};
use rune_kit::Kit;

/// A terminal theme: the colours a Rex theme is defined by.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Palette {
    pub name: &'static str,
    pub background: Color,
    pub foreground: Color,
    /// The cursor colour, which Rex uses as the interface accent.
    pub cursor: Color,
    /// ANSI 0–7, then the bright 8–15.
    pub ansi: [Color; 16],
    pub dark: bool,
}

/// A colour from `0xRRGGBB`.
pub fn rgb(value: u32) -> Color {
    let channel = |shift: u32| srgb_decode(((value >> shift) & 0xff) as f32 / 255.0);
    Color {
        r: channel(16),
        g: channel(8),
        b: channel(0),
        a: 1.0,
    }
}

/// `a` moved `t` of the way toward `b`, mixed as sRGB values the way Rex's greys were measured.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: f32, y: f32| srgb_decode(srgb_encode(x) + (srgb_encode(y) - srgb_encode(x)) * t);
    Color {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: a.a + (b.a - a.a) * t,
    }
}

fn ansi(values: [u32; 16]) -> [Color; 16] {
    values.map(rgb)
}

impl Palette {
    /// Rex's default dark theme.
    pub fn merino_dark() -> Palette {
        Palette {
            name: "Merino Dark",
            background: rgb(0x0d1117),
            foreground: rgb(0xe6edf3),
            cursor: rgb(0x2f81f7),
            ansi: ansi([
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
                0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
            ]),
            dark: true,
        }
    }

    /// Rex's default light theme.
    pub fn merino_light() -> Palette {
        Palette {
            name: "Merino Light",
            background: rgb(0xffffff),
            foreground: rgb(0x1f2328),
            cursor: rgb(0x0969da),
            ansi: ansi([
                0x24292f, 0xcf222e, 0x116329, 0x9d5c00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
                0x57606a, 0xa40e26, 0x1a7f37, 0xc47a00, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
            ]),
            dark: false,
        }
    }

    /// A warm dark theme with an orange cursor.
    pub fn charcoal_atelier() -> Palette {
        Palette {
            name: "Charcoal Atelier",
            background: rgb(0x161616),
            foreground: rgb(0xd6d2cc),
            cursor: rgb(0xe05d3a),
            ansi: ansi([
                0x222222, 0xd96a4d, 0x9aab84, 0xcfae7a, 0x8aa0b4, 0xb193a8, 0x8fafa8, 0xb5b1ab,
                0x5f5c58, 0xe8886e, 0xb3c39d, 0xe0c493, 0xa4b9cc, 0xc8adc0, 0xa8c7c0, 0xefece7,
            ]),
            dark: true,
        }
    }

    /// The terminal's theme, once it has reported its text and background colours.
    pub fn from_terminal() -> Option<Palette> {
        Palette::from_terminal_colors(rune::terminal_colors()?)
    }

    /// The palette of a terminal that reported `colors`, `None` until it has reported its text
    /// and background. It decides what rune-kit's [`Kit::from_terminal_colors`] decides, from
    /// the same colours. The theme is dark when the text is lighter than the background. ANSI
    /// colours the terminal left out come from Merino. A missing cursor falls back to ANSI blue,
    /// the bright one on a dark background.
    pub fn from_terminal_colors(colors: TerminalColors) -> Option<Palette> {
        let (background, foreground) = (colors.background?, colors.foreground?);
        let dark = foreground.luminance() > background.luminance();
        let base = if dark {
            Palette::merino_dark()
        } else {
            Palette::merino_light()
        };
        let mut ansi = base.ansi;
        for (slot, reported) in ansi.iter_mut().zip(colors.palette) {
            if let Some(color) = reported {
                *slot = color;
            }
        }
        let blue = ansi[if dark { 12 } else { 4 }];
        Some(Palette {
            name: "Terminal",
            background,
            foreground,
            cursor: colors.cursor.unwrap_or(blue),
            ansi,
            dark,
        })
    }

    /// A palette of your own. It is dark when `foreground` is lighter than `background`.
    pub fn new(
        name: &'static str,
        background: Color,
        foreground: Color,
        cursor: Color,
        ansi: [Color; 16],
    ) -> Palette {
        let dark = foreground.luminance() > background.luminance();
        Palette {
            name,
            background,
            foreground,
            cursor,
            ansi,
            dark,
        }
    }

    /// The palette as a terminal would report it, for [`Kit::from_terminal_colors`].
    pub fn terminal_colors(&self) -> TerminalColors {
        let mut colors = TerminalColors::default();
        colors.background = Some(self.background);
        colors.foreground = Some(self.foreground);
        colors.cursor = Some(self.cursor);
        colors.palette = self.ansi.map(Some);
        colors.dark = Some(self.dark);
        colors
    }
}

/// Corner radii, in points. Capsule controls (buttons, chips, toggles) are always fully round.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Radius {
    /// Text fields, swatches and small boxes.
    pub field: f32,
    /// Menu and palette rows.
    pub row: f32,
    /// Settings groups and cards.
    pub group: f32,
    /// Menus, the palette and other floating panels.
    pub popover: f32,
}

/// Text styles: 13 pt body, 13.5 pt rows, 12 pt captions, 11.5 pt labels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Type {
    pub body: TextStyle,
    pub strong: TextStyle,
    /// A settings row's or menu row's title.
    pub row: TextStyle,
    pub caption: TextStyle,
    /// Small labels: menu sections, badges.
    pub label: TextStyle,
    /// A heading above a group of controls.
    pub heading: TextStyle,
    pub title: TextStyle,
    pub mono: TextStyle,
}

/// Every token a component reads: colours derived from a [`Palette`], radii, type, and
/// rune-kit's tokens in the same colours for the rune-kit components used inside.
///
/// Build one per view with [`Theme::current`] or [`Theme::new`], then change any field to restyle:
/// `let mut theme = Theme::current(dark); theme.radius.group = 8.0;`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Theme {
    pub palette: Palette,
    /// The pane: the terminal background.
    pub background: Color,
    /// Settings groups and cards, a shade off the pane.
    pub group: Color,
    /// Menus, the palette, toasts.
    pub popover: Color,
    pub popover_border: Color,
    /// Text fields, gray buttons and chips.
    pub field: Color,
    /// Behind a modal.
    pub scrim: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_tertiary: Color,
    pub separator: Color,
    pub border: Color,
    pub hover: Color,
    pub pressed: Color,
    /// The theme's cursor colour.
    pub accent: Color,
    pub accent_hover: Color,
    /// Text on an accent fill: white, or dark on a light accent.
    pub on_accent: Color,
    pub accent_soft: Color,
    pub accent_soft_hover: Color,
    pub accent_text: Color,
    pub focus_ring: Color,
    pub working: Color,
    pub done: Color,
    pub warning: Color,
    pub danger: Color,
    pub idle: Color,
    pub shadow: Color,
    pub shadow_strong: Color,
    pub radius: Radius,
    pub ty: Type,
    /// rune-kit's tokens in the same colours, for its components.
    pub kit: Kit,
}

impl Theme {
    /// The theme of a terminal that reported `colors`, `None` until it has reported its text
    /// and background.
    pub fn from_terminal_colors(colors: TerminalColors) -> Option<Theme> {
        Palette::from_terminal_colors(colors).map(Theme::new)
    }

    /// The terminal's theme inside a terminal, else Merino in `dark`'s appearance.
    pub fn current(dark: bool) -> Theme {
        rune::terminal_colors()
            .and_then(Theme::from_terminal_colors)
            .unwrap_or_else(|| {
                Theme::new(if dark {
                    Palette::merino_dark()
                } else {
                    Palette::merino_light()
                })
            })
    }

    /// `palette`'s theme, with the kit rune-kit derives from a terminal in those colours.
    pub fn new(palette: Palette) -> Theme {
        let kit = Kit::from_terminal_colors(palette.terminal_colors())
            .unwrap_or_else(|| Kit::neutral(palette.dark));
        Theme::with_kit(palette, kit)
    }

    fn with_kit(palette: Palette, mut kit: Kit) -> Theme {
        let (bg, fg, dark) = (palette.background, palette.foreground, palette.dark);
        let (black, white) = (Color::BLACK, Color::WHITE);
        let accent = palette.cursor;
        let on_accent = on_fill(accent, &palette);
        let pick = |d: f32, l: f32| if dark { d } else { l };
        let gray = |d: f32, l: f32| mix(fg, bg, pick(d, l));
        let ink = |d: f32, l: f32| fg.alpha(pick(d, l));
        let radius = Radius {
            field: 10.0,
            row: 8.0,
            group: 14.0,
            popover: 14.0,
        };
        // Rex's switches have a white knob in either appearance.
        kit.c.knob = white;
        kit.c.knob_on = white;
        kit.radius.xs = 4.0;
        kit.radius.sm = radius.row;
        kit.radius.md = radius.field;
        kit.radius.lg = radius.group;
        kit.radius.xl = radius.popover;
        Theme {
            background: bg,
            group: mix(bg, fg, pick(0.045, 0.035)),
            popover: mix(bg, fg, pick(0.075, 0.015)),
            popover_border: ink(0.12, 0.1),
            field: ink(0.08, 0.06),
            scrim: black.alpha(pick(0.35, 0.15)),
            text: fg,
            text_secondary: gray(0.42, 0.48),
            text_tertiary: gray(0.6, 0.66),
            separator: gray(0.84, 0.895),
            border: ink(0.12, 0.14),
            hover: ink(0.07, 0.05),
            pressed: ink(0.12, 0.09),
            accent,
            accent_hover: if dark {
                mix(accent, white, 0.1)
            } else {
                mix(accent, black, 0.08)
            },
            on_accent,
            accent_soft: accent.alpha(pick(0.2, 0.12)),
            accent_soft_hover: accent.alpha(pick(0.28, 0.18)),
            accent_text: if dark {
                mix(accent, white, 0.12)
            } else {
                accent
            },
            focus_ring: accent.alpha(0.9),
            working: kit.c.success.fg,
            done: accent,
            warning: kit.c.warning.fg,
            danger: kit.c.danger.fg,
            idle: gray(0.6, 0.66),
            shadow: black.alpha(pick(0.35, 0.08)),
            shadow_strong: black.alpha(pick(0.5, 0.18)),
            radius,
            ty: Type {
                body: TextStyle::new(13.0, 400.0),
                strong: TextStyle::new(13.0, 600.0),
                row: TextStyle::new(13.5, 400.0),
                caption: TextStyle::new(12.0, 400.0),
                label: TextStyle::new(11.5, 500.0),
                heading: TextStyle::new(13.0, 600.0),
                title: TextStyle::new(15.0, 600.0),
                mono: TextStyle::new(12.0, 500.0).mono(),
            },
            kit,
            palette,
        }
    }

    /// Text and icons on a `fill` of any colour: white on a dark fill, dark on a light one.
    pub fn on(&self, fill: Color) -> Color {
        on_fill(fill, &self.palette)
    }

    /// One of the theme's 16 ANSI colours.
    pub fn ansi(&self, index: usize) -> Color {
        self.palette.ansi[index % 16]
    }
}

/// White or the theme's darkest ink, whichever contrasts more with `fill` (WCAG contrast ratio).
fn on_fill(fill: Color, palette: &Palette) -> Color {
    let dark = if palette.dark {
        palette.background
    } else {
        mix(palette.foreground, Color::BLACK, 0.4)
    };
    let ratio = |a: Color, b: Color| {
        let (la, lb) = (a.luminance(), b.luminance());
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    };
    if ratio(fill, Color::WHITE) >= ratio(fill, dark) {
        Color::WHITE
    } else {
        dark
    }
}

/// `#RRGGBB`, the way Rex labels a colour.
pub fn hex_code(color: Color) -> String {
    color
        .alpha(1.0)
        .hex()
        .chars()
        .take(7)
        .collect::<String>()
        .to_uppercase()
}
