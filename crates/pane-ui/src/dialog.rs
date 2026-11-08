//! A dialog: a title, a message, any content, and actions at the bottom. Show it with
//! [`crate::modal_layer`], which handles Escape, a press outside and focus.

use rune::{El, Icon, col, icon, row, text};

use crate::{Theme, Tone};

/// ```ignore
/// let dialog = Dialog::new("Delete my-service?").message("This can't be undone.")
///     .action(Button::new("Cancel").gray().on_press(|s: &mut App| s.confirm = false).build("cancel", &theme))
///     .action(Button::new("Delete").destructive().on_press(|s: &mut App| s.delete()).build("delete", &theme))
///     .build("confirm", &theme);
/// page.overlay(modal_layer("confirm-layer", dialog, |s: &mut App| s.confirm = false, &theme))
/// ```
pub struct Dialog {
    title: String,
    message: Option<String>,
    glyph: Option<(&'static Icon, Tone)>,
    body: Vec<El>,
    actions: Vec<El>,
    width: f32,
}

impl Dialog {
    pub fn new(title: impl Into<String>) -> Self {
        Dialog {
            title: title.into(),
            message: None,
            glyph: None,
            body: Vec::new(),
            actions: Vec::new(),
            width: 400.0,
        }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
    /// An icon beside the title, in a tone's colour, as a warning triangle.
    pub fn icon(mut self, glyph: &'static Icon, tone: Tone) -> Self {
        self.glyph = Some((glyph, tone));
        self
    }
    pub fn child(mut self, child: El) -> Self {
        self.body.push(child);
        self
    }
    /// A button at the bottom; the last one is the default, which Return presses.
    pub fn action(mut self, action: El) -> Self {
        self.actions.push(action);
        self
    }
    /// The widest it gets, in points; 400 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let mut head = row().gap(10.0).items_start();
        if let Some((glyph, tone)) = self.glyph {
            head = head.child(icon(glyph, 20.0).color(tone.color(theme)));
        }
        let mut lines = col().gap(4.0).grow(1.0).min_w(0.0).child(
            text(self.title.clone())
                .font(theme.ty.title)
                .color(theme.text)
                .wrap(),
        );
        if let Some(message) = self.message {
            lines = lines.child(
                text(message)
                    .font(theme.ty.body)
                    .color(theme.text_secondary)
                    .wrap(),
            );
        }
        head = head.child(lines);
        let count = self.actions.len();
        let actions = self.actions.into_iter().enumerate().map(|(i, action)| {
            if i + 1 == count {
                action.default_button()
            } else {
                action
            }
        });
        col()
            .key(key)
            .role("dialog")
            .label(self.title)
            .w_full()
            .max_w(self.width)
            .padding(18.0)
            .gap(14.0)
            .radius(theme.radius.popover)
            .fill(theme.popover)
            .border(1.0, theme.popover_border)
            .shadow(12.0, 32.0, theme.shadow_strong)
            .child(head)
            .children(self.body)
            .child(
                row()
                    .gaps(8.0, 8.0)
                    .flex_wrap()
                    .justify_end()
                    .children(actions),
            )
    }
}
