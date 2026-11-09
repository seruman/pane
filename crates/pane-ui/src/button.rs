//! Buttons: Rex's capsule button in five variants and three sizes, with or without a label.

use rune::{Color, El, Icon, Tx, icon, row, text};

use crate::{Theme, Transform, mix};

/// How a [`Button`] is filled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// The accent, for the one primary action ("Save & Apply").
    #[default]
    Filled,
    /// The accent at low opacity with accent text ("Cancel", "Revert…").
    Tinted,
    /// A neutral grey fill.
    Gray,
    /// Only text; a fill on hover.
    Plain,
    /// The danger colour, for destructive actions.
    Destructive,
}

/// How tall a control is: 22, 28 or 34 points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlSize {
    Small,
    #[default]
    Regular,
    Large,
}

impl ControlSize {
    pub fn height(self) -> f32 {
        match self {
            ControlSize::Small => 22.0,
            ControlSize::Regular => 28.0,
            ControlSize::Large => 34.0,
        }
    }

    pub fn text_size(self) -> f32 {
        match self {
            ControlSize::Small => 12.0,
            ControlSize::Regular => 13.0,
            ControlSize::Large => 14.5,
        }
    }

    fn padding(self) -> f32 {
        match self {
            ControlSize::Small => 10.0,
            ControlSize::Regular => 14.0,
            ControlSize::Large => 18.0,
        }
    }
}

/// Rex's capsule button.
///
/// ```no_run
/// # use pane_ui::{Button, Theme, icons};
/// # struct App;
/// # impl App { fn run(&mut self) {} }
/// # fn view(theme: &Theme) -> rune::El {
/// Button::new("Run").icon(&icons::PLAY).on_press(|s: &mut App| s.run()).build("run", &theme);
/// Button::icon_only(&icons::REFRESH_CW, "Refresh").plain().build("refresh", &theme)
/// # }
/// ```
pub struct Button {
    label: Option<String>,
    name: Option<String>,
    leading: Option<&'static Icon>,
    trailing: Option<&'static Icon>,
    variant: ButtonVariant,
    size: ControlSize,
    disabled: bool,
    press: Option<Transform>,
}

impl Button {
    fn base() -> Self {
        Button {
            label: None,
            name: None,
            leading: None,
            trailing: None,
            variant: ButtonVariant::default(),
            size: ControlSize::default(),
            disabled: false,
            press: None,
        }
    }

    pub fn new(label: impl Into<String>) -> Self {
        Button {
            label: Some(label.into()),
            ..Self::base()
        }
    }

    /// A round button showing only `glyph`, named `name` for screen readers and its tooltip.
    pub fn icon_only(glyph: &'static Icon, name: impl Into<String>) -> Self {
        Button {
            leading: Some(glyph),
            name: Some(name.into()),
            ..Self::base()
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn tinted(self) -> Self {
        self.variant(ButtonVariant::Tinted)
    }
    pub fn gray(self) -> Self {
        self.variant(ButtonVariant::Gray)
    }
    pub fn plain(self) -> Self {
        self.variant(ButtonVariant::Plain)
    }
    pub fn destructive(self) -> Self {
        self.variant(ButtonVariant::Destructive)
    }
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(ControlSize::Small)
    }
    pub fn large(self) -> Self {
        self.size(ControlSize::Large)
    }
    pub fn icon(mut self, glyph: &'static Icon) -> Self {
        self.leading = Some(glyph);
        self
    }
    pub fn trailing_icon(mut self, glyph: &'static Icon) -> Self {
        self.trailing = Some(glyph);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |e: El| e.on_press(f)));
        self
    }

    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.press = Some(Box::new(move |e: El| e.on_press_tx(f)));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let (bg, hover, fg) = match self.variant {
            ButtonVariant::Filled => (theme.accent, theme.accent_hover, theme.on_accent),
            ButtonVariant::Tinted => (
                theme.accent_soft,
                theme.accent_soft_hover,
                theme.accent_text,
            ),
            ButtonVariant::Gray => (theme.field, theme.pressed, theme.text),
            ButtonVariant::Plain => (
                Color::TRANSPARENT,
                theme.hover,
                if self.label.is_some() {
                    theme.accent_text
                } else {
                    theme.text_secondary
                },
            ),
            ButtonVariant::Destructive => (
                theme.danger,
                mix(theme.danger, Color::BLACK, 0.1),
                theme.on(theme.danger),
            ),
        };
        let h = self.size.height();
        let glyph_size = self.size.text_size() + 1.0;
        let name = self
            .name
            .clone()
            .or_else(|| self.label.clone())
            .unwrap_or_default();
        let mut b = row()
            .key(key)
            .role("button")
            .label(name.clone())
            .h(h)
            .gap(6.0)
            .center()
            .radius(h / 2.0)
            .fill(bg)
            .color(fg)
            .focusable(!self.disabled)
            .focus_ring(2.0, 2.0, theme.focus_ring)
            .disabled(self.disabled);
        b = if self.label.is_some() {
            b.px(self.size.padding())
        } else {
            b.w(h).help(name)
        };
        if self.disabled {
            b = b.opacity(0.45);
        } else {
            b = b.hover(hover).press(mix(hover, fg, 0.08));
            if let Some(press) = self.press {
                b = press(b);
            }
        }
        if let Some(glyph) = self.leading {
            b = b.child(icon(glyph, glyph_size).color(fg));
        }
        if let Some(label) = self.label {
            b = b.child(
                text(label)
                    .font(rune::TextStyle::new(self.size.text_size(), 500.0))
                    .color(fg),
            );
        }
        if let Some(glyph) = self.trailing {
            b = b.child(icon(glyph, glyph_size).color(fg));
        }
        b
    }
}
