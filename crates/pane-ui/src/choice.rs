//! Picking one of several: a segmented control (which also serves as tabs) and a select that
//! drops a menu from its trigger.

use std::rc::Rc;

use rune::{Align, El, Side, icon, row, text};

use crate::{ControlSize, MenuItem, Theme, Transform, icons, popover};

/// A capsule track of options with the selected one raised, for a few short choices or tabs.
/// Left and Right move between options and select them.
///
/// ```ignore
/// SegmentedControl::new(["Overview", "Settings"]).selected(s.tab).on_select(|s: &mut App, i| s.tab = i).build("tabs", &theme)
/// ```
pub struct SegmentedControl {
    options: Vec<String>,
    selected: usize,
    size: ControlSize,
    fill: bool,
    select: Option<Rc<dyn Fn(usize) -> Transform>>,
}

impl SegmentedControl {
    pub fn new<T: Into<String>>(options: impl IntoIterator<Item = T>) -> Self {
        SegmentedControl {
            options: options.into_iter().map(Into::into).collect(),
            selected: 0,
            size: ControlSize::Regular,
            fill: false,
            select: None,
        }
    }

    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(ControlSize::Small)
    }
    /// Take the whole width, the options sharing it equally.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub fn on_select<S: 'static>(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        let f = Rc::new(f);
        self.select = Some(Rc::new(move |index| {
            let f = f.clone();
            Box::new(move |e: El| e.on_press(move |s: &mut S| f(s, index)))
        }));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let h = self.size.height();
        let style = rune::TextStyle::new(self.size.text_size(), 500.0);
        let mut track = row()
            .key(key)
            .role("tablist")
            .focus_group()
            .h(h + 4.0)
            .padding(2.0)
            .gap(2.0)
            .center_y()
            .radius(h / 2.0 + 2.0)
            .fill(theme.field)
            .min_w(0.0);
        track = if self.fill {
            track.w_full()
        } else {
            track.self_start()
        };
        for (i, label) in self.options.into_iter().enumerate() {
            let on = i == self.selected;
            let mut option = row()
                .key(format!("option-{i}"))
                .role("tab")
                .label(label.clone())
                .checked(on)
                .focus_selected(on)
                .focusable(true)
                .h(h)
                .px(12.0)
                .center()
                .radius(h / 2.0)
                .min_w(0.0)
                .shrink()
                .child(
                    text(label)
                        .font(style)
                        .color(if on { theme.text } else { theme.text_secondary })
                        .truncate()
                        .shrink(),
                );
            option = if on {
                option
                    .fill(if theme.palette.dark {
                        crate::mix(theme.background, theme.text, 0.16)
                    } else {
                        rune::Color::WHITE
                    })
                    .shadow(1.0, 2.0, theme.shadow)
            } else {
                option.hover(theme.hover)
            };
            if self.fill {
                option = option.grow(1.0);
            }
            if let Some(select) = &self.select {
                option = select(i)(option);
            }
            track = track.child(option);
        }
        track
    }
}

/// A field showing the current choice that drops a menu of options, as a theme or sort order.
/// The app keeps whether it is open.
///
/// ```ignore
/// let mut layer = Vec::new();
/// let select = Select::new(["Follow terminal", "Light", "Dark"]).current(s.theme).open(s.theme_open)
///     .on_toggle(|s: &mut App| s.theme_open = !s.theme_open)
///     .on_choose(|s: &mut App, i| { s.theme = i; s.theme_open = false; })
///     .build("theme", &theme, &mut layer);
/// // … Page::new().child(select).overlays(layer)
/// ```
pub struct Select {
    options: Vec<String>,
    current: Option<usize>,
    placeholder: String,
    open: bool,
    size: ControlSize,
    width: Option<f32>,
    toggle: Option<(Transform, Transform)>,
    choose: Option<Rc<dyn Fn(usize) -> Transform>>,
}

impl Select {
    pub fn new<T: Into<String>>(options: impl IntoIterator<Item = T>) -> Self {
        Select {
            options: options.into_iter().map(Into::into).collect(),
            current: None,
            placeholder: "Choose…".to_owned(),
            open: false,
            size: ControlSize::Regular,
            width: Some(200.0),
            toggle: None,
            choose: None,
        }
    }

    pub fn current(mut self, current: usize) -> Self {
        self.current = Some(current);
        self
    }
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// The width it prefers, in points; it shrinks below it to fit. 200 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    pub fn fill(mut self) -> Self {
        self.width = None;
        self
    }

    /// A press on the trigger. Escape and a press outside the open menu also call it.
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        let f = Rc::new(f);
        let close = f.clone();
        self.toggle = Some((
            Box::new(move |e: El| e.on_press(move |s: &mut S| f(s))),
            Box::new(move |e: El| e.modal(true, move |s: &mut S| close(s))),
        ));
        self
    }

    /// An option chosen, with its index.
    pub fn on_choose<S: 'static>(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        let f = Rc::new(f);
        self.choose = Some(Rc::new(move |index| {
            let f = f.clone();
            Box::new(move |e: El| e.on_press(move |s: &mut S| f(s, index)))
        }));
        self
    }

    /// The trigger; while open, the menu goes into `layer`, which the app draws above its page.
    pub fn build(self, key: &str, theme: &Theme, layer: &mut Vec<El>) -> El {
        let label = self.current.and_then(|i| self.options.get(i).cloned());
        let shown = label.clone().unwrap_or_else(|| self.placeholder.clone());
        let mut trigger = row()
            .key(key)
            .role("combobox")
            .label(shown.clone())
            .expanded(self.open)
            .h(self.size.height())
            .pl(10.0)
            .pr(8.0)
            .gap(6.0)
            .center_y()
            .radius(theme.radius.field)
            .fill(theme.field)
            .hover(theme.pressed)
            .min_w(0.0)
            .focusable(true)
            .focus_ring(2.0, 2.0, theme.focus_ring)
            .child(
                text(shown)
                    .font(theme.ty.body)
                    .color(if label.is_some() {
                        theme.text
                    } else {
                        theme.text_tertiary
                    })
                    .truncate()
                    .shrink()
                    .grow(1.0),
            )
            .child(icon(&icons::CHEVRON_DOWN, 13.0).color(theme.text_secondary));
        trigger = match self.width {
            Some(w) => trigger.w(w).shrink(),
            None => trigger.w_full(),
        };
        let (trigger, close) = match self.toggle {
            Some((toggle, close)) => (toggle(trigger), Some(close)),
            None => (trigger, None),
        };
        if self.open {
            let mut menu = popover(&format!("{key}-menu"), theme)
                .min_w(180.0)
                .max_w(280.0)
                .anchored_fit(key, Side::Bottom, Align::Start, 4.0)
                .focus_return(key);
            for (i, option) in self.options.into_iter().enumerate() {
                let mut item = MenuItem::new(option)
                    .checked(self.current == Some(i))
                    .build(&format!("option-{i}"), theme)
                    .focus_selected(self.current == Some(i));
                if let Some(choose) = &self.choose {
                    item = choose(i)(item);
                }
                menu = menu.child(item);
            }
            if let Some(close) = close {
                menu = close(menu);
            }
            layer.push(menu);
        }
        trigger
    }
}
