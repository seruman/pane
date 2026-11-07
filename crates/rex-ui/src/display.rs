//! Things that show state rather than take input: badges, chips, tokens, key caps, program
//! status, spinners, toasts and empty states.

use rune::{Color, El, Icon, col, el, icon, row, text};

use crate::{Rex, mix};

/// The colour role of a [`Badge`] or other status element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    /// The tone's colour; neutral is the secondary text colour.
    pub fn color(self, rex: &Rex) -> Color {
        match self {
            Tone::Neutral => rex.text_secondary,
            Tone::Accent => rex.accent_text,
            Tone::Success => rex.working,
            Tone::Warning => rex.warning,
            Tone::Danger => rex.danger,
        }
    }
}

/// A small label in a rounded box, as "beta" or "3 errors".
pub struct Badge {
    label: String,
    tone: Tone,
}

impl Badge {
    pub fn new(label: impl Into<String>) -> Self {
        Badge {
            label: label.into(),
            tone: Tone::Neutral,
        }
    }
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn build(self, rex: &Rex) -> El {
        let fg = self.tone.color(rex);
        let bg = if self.tone == Tone::Neutral {
            rex.field
        } else {
            fg.alpha(0.16)
        };
        el().px(6.0).py(1.0).radius(5.0).fill(bg).child(
            text(self.label)
                .font(rune::TextStyle::new(10.5, 600.0))
                .color(fg),
        )
    }
}

/// A capsule label with an optional icon, as a tag or a filter.
pub struct Chip {
    label: String,
    glyph: Option<&'static Icon>,
}

impl Chip {
    pub fn new(label: impl Into<String>) -> Self {
        Chip {
            label: label.into(),
            glyph: None,
        }
    }
    pub fn icon(mut self, glyph: &'static Icon) -> Self {
        self.glyph = Some(glyph);
        self
    }

    pub fn build(self, rex: &Rex) -> El {
        let mut c = row()
            .h(26.0)
            .px(11.0)
            .gap(6.0)
            .center_y()
            .radius(13.0)
            .fill(rex.field);
        if let Some(glyph) = self.glyph {
            c = c.child(icon(glyph, 13.0).color(rex.text_secondary));
        }
        c.child(
            text(self.label)
                .font(rune::TextStyle::new(12.5, 500.0))
                .color(rex.text_secondary),
        )
    }
}

/// A scope token, as "Theme" inside the palette's field or a branch name.
pub fn token(label: impl Into<String>, rex: &Rex) -> El {
    el().px(7.0).py(2.0).radius(6.0).fill(rex.pressed).child(
        text(label.into())
            .font(rune::TextStyle::new(12.5, 600.0))
            .color(rex.text),
    )
}

/// Shortcut glyphs in an outlined box, as "⌃P".
pub fn kbd(keys: impl Into<String>, rex: &Rex) -> El {
    let color = rex.text_tertiary;
    el().min_w(20.0)
        .h(18.0)
        .px(5.0)
        .center()
        .radius(5.0)
        .border(1.0, color.alpha(0.45))
        .child(
            text(keys.into())
                .font(rune::TextStyle::new(11.0, 400.0))
                .color(color),
        )
}

/// What a program is doing, after Rex's Program Status Protocol (OSC 7501).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Idle => "Idle",
            Status::Working => "Working",
            Status::Done => "Done",
            Status::Blocked => "Needs input",
            Status::Error => "Failed",
        }
    }

    pub fn color(self, rex: &Rex) -> Color {
        match self {
            Status::Idle => rex.idle,
            Status::Working => rex.working,
            Status::Done => rex.done,
            Status::Blocked => rex.warning,
            Status::Error => rex.danger,
        }
    }

    /// Rex's mark for the status: a spinner while working, a dot with a halo when done, blocked
    /// or failed, and an empty space of the same size when idle.
    pub fn mark(self, rex: &Rex) -> El {
        match self {
            Status::Idle => el().size(13.0, 13.0),
            Status::Working => el().size(13.0, 13.0).center().child(spinner(rex)),
            other => {
                let color = other.color(rex);
                el().size(13.0, 13.0)
                    .radius(6.5)
                    .center()
                    .fill(color.alpha(0.22))
                    .role("img")
                    .label(other.label())
                    .child(el().size(7.0, 7.0).radius(3.5).fill(color))
            }
        }
    }
}

/// A small spinner in the secondary text colour.
pub fn spinner(rex: &Rex) -> El {
    rune_kit::Spinner::new()
        .build(&rex.kit)
        .color(rex.text_secondary)
}

/// A capsule notice: a leading mark, bold text and an optional action, as "Run finished
/// [Dismiss]". Place it with [`crate::toast_region`].
pub struct Toast {
    message: String,
    leading: Option<El>,
    action: Option<El>,
}

impl Toast {
    pub fn new(message: impl Into<String>) -> Self {
        Toast {
            message: message.into(),
            leading: None,
            action: None,
        }
    }
    pub fn leading(mut self, leading: El) -> Self {
        self.leading = Some(leading);
        self
    }
    pub fn action(mut self, action: El) -> Self {
        self.action = Some(action);
        self
    }

    pub fn build(self, key: &str, rex: &Rex) -> El {
        let mut t = row()
            .key(key)
            .role("status")
            .label(self.message.clone())
            .min_h(38.0)
            .py(4.0)
            .pl(14.0)
            .pr(if self.action.is_some() { 6.0 } else { 16.0 })
            .gap(10.0)
            .center_y()
            .min_w(0.0)
            .radius(19.0)
            .fill(rex.popover)
            .border(1.0, rex.popover_border)
            .shadow(6.0, 18.0, rex.shadow);
        if let Some(leading) = self.leading {
            t = t.child(leading);
        }
        t = t.child(
            text(self.message)
                .font(rex.ty.strong)
                .color(rex.text)
                .truncate()
                .shrink(),
        );
        if let Some(action) = self.action {
            t = t.child(action);
        }
        t
    }
}

/// A faint icon, a title, a line of text and optional actions, for an empty list or pane.
pub struct EmptyState {
    glyph: &'static Icon,
    title: String,
    message: Option<String>,
    actions: Vec<El>,
}

impl EmptyState {
    pub fn new(glyph: &'static Icon, title: impl Into<String>) -> Self {
        EmptyState {
            glyph,
            title: title.into(),
            message: None,
            actions: Vec::new(),
        }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
    pub fn action(mut self, action: El) -> Self {
        self.actions.push(action);
        self
    }

    pub fn build(self, rex: &Rex) -> El {
        let mut c = col()
            .gap(8.0)
            .center()
            .padding(24.0)
            .w_full()
            .child(icon(self.glyph, 34.0).color(mix(rex.text_tertiary, rex.background, 0.2)))
            .child(
                text(self.title)
                    .font(rex.ty.title)
                    .color(rex.text)
                    .wrap()
                    .align(rune::Align::Center),
            );
        if let Some(message) = self.message {
            c = c.child(
                text(message)
                    .font(rex.ty.body)
                    .color(rex.text_secondary)
                    .wrap()
                    .align(rune::Align::Center)
                    .max_w(360.0),
            );
        }
        if !self.actions.is_empty() {
            c = c.child(
                row()
                    .gap(8.0)
                    .mt(6.0)
                    .flex_wrap()
                    .justify_center()
                    .children(self.actions),
            );
        }
        c
    }
}
