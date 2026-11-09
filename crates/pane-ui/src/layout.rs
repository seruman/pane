//! Pages, columns that wrap in narrow panes, and layers for toasts and modals.
//! These layouts fit the available width.

use rune::{El, ScrollSpec, col, el, row, text};

use crate::Theme;

pub(crate) enum Width {
    Prefer(f32),
    Fill,
}

impl Width {
    pub(crate) fn apply(self, element: El) -> El {
        match self {
            Self::Prefer(width) => element.w(width).shrink(),
            Self::Fill => element.w_full(),
        }
    }
}

/// The whole pane: the terminal background, scrolling content with padding, and overlays
/// (toasts, modals) above it.
///
/// ```no_run
/// # use pane_ui::{Page, Theme, Toast, toast_region};
/// # fn view(theme: &Theme) -> rune::El {
/// Page::new()
///     .child(rune::text("Build output"))
///     .overlay(toast_region(Toast::new("Build finished").build("done", &theme)))
///     .build(&theme)
/// # }
/// ```
pub struct Page {
    children: Vec<El>,
    overlays: Vec<El>,
    padding: f32,
    gap: f32,
    max_width: Option<f32>,
}

impl Default for Page {
    fn default() -> Self {
        Page {
            children: Vec::new(),
            overlays: Vec::new(),
            padding: 20.0,
            gap: 12.0,
            max_width: None,
        }
    }
}

impl Page {
    pub fn new() -> Self {
        Page::default()
    }
    pub fn child(mut self, child: El) -> Self {
        self.children.push(child);
        self
    }
    pub fn children(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.children.extend(children);
        self
    }
    /// Drawn above the content: a [`toast_region`] or a [`modal_layer`].
    pub fn overlay(mut self, overlay: El) -> Self {
        self.overlays.push(overlay);
        self
    }
    /// Several overlays, as the layer a [`crate::Select`] fills.
    pub fn overlays(mut self, overlays: impl IntoIterator<Item = El>) -> Self {
        self.overlays.extend(overlays);
        self
    }
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }
    /// Centre the content and keep it at most this wide.
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn build(self, theme: &Theme) -> El {
        let mut content = col()
            .key("content")
            .w_full()
            .gap(self.gap)
            .children(self.children);
        if let Some(width) = self.max_width {
            content = content.max_w(width).self_center();
        }
        let scroll = col()
            .key("page")
            .w_full()
            .h_full()
            .padding(self.padding)
            .child(content)
            .scroll(ScrollSpec {
                stick_bottom: false,
            });
        el().key("pane")
            .w_full()
            .h_full()
            .fill(theme.background)
            .child(scroll)
            .children(self.overlays)
    }
}

/// A small heading above a block of controls, as "Buttons" or "Settings".
pub fn heading(title: impl Into<String>, theme: &Theme) -> El {
    text(title.into())
        .font(theme.ty.heading)
        .color(theme.text_secondary)
}

/// Columns side by side while each has at least `min_width`, stacked one above the other when
/// the pane is narrower. Columns share the extra width equally.
pub struct Columns {
    children: Vec<(El, f32)>,
    min_width: f32,
    gap: f32,
}

impl Columns {
    pub fn new(min_width: f32) -> Self {
        Columns {
            children: Vec::new(),
            min_width,
            gap: 24.0,
        }
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }
    pub fn child(mut self, child: El) -> Self {
        let w = self.min_width;
        self.children.push((child, w));
        self
    }
    /// A column with its own minimum width.
    pub fn child_min(mut self, child: El, min_width: f32) -> Self {
        self.children.push((child, min_width));
        self
    }

    pub fn build(self) -> El {
        // Each column's flex basis is its minimum, so a line holds as many as fit and the rest
        // wrap below; growing shares out what is left.
        row()
            .w_full()
            .flex_wrap()
            .items_start()
            .gaps(self.gap, self.gap)
            .children(
                self.children
                    .into_iter()
                    .map(|(child, min)| col().grow(1.0).min_w(min).child(child)),
            )
    }
}

/// Pins `toast` to the bottom of the [`Page`], centred.
pub fn toast_region(toast: El) -> El {
    row()
        .key("toasts")
        .pin(Some(12.0), None, Some(12.0), Some(16.0))
        .justify_center()
        .child(toast)
}

/// A modal over the [`Page`]: a scrim, and `content` near the top, centred, as wide as it allows.
/// Escape and a click outside call `dismiss`; focus stays inside until then.
pub fn modal_layer<S: 'static>(
    key: &str,
    content: El,
    dismiss: impl Fn(&mut S) + 'static,
    theme: &Theme,
) -> El {
    el().key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .child(
            el().key("scrim")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(theme.scrim)
                .a11y_hidden(),
        )
        .child(
            col()
                .key("place")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .pt(56.0)
                .px(12.0)
                .items_center()
                .child(content.modal(true, dismiss)),
        )
}
