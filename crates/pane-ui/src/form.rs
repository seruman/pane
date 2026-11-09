//! Form controls: text fields, toggles, checkboxes, sliders and colour swatches.
//!
//! Each takes the width it prefers and shrinks to fit a narrow pane; `.fill()` makes it take the
//! whole width it is given instead.

use rune::{
    Color, Drag, El, Icon, KeyEvent, Phase, TextEdit, Tx, col, el, icon, row, text, text_input,
};

use crate::{ControlSize, Theme, Transform, hex_code, icons};

/// A one-line text field, optionally with a leading icon (a search field has the magnifier).
///
/// ```no_run
/// # use pane_ui::{TextField, Theme};
/// # struct App { name: rune::TextEdit, filter: rune::TextEdit }
/// # fn view(s: &App, theme: &Theme) -> rune::El {
/// TextField::new(&s.name, |s: &mut App| &mut s.name).placeholder("Name").build("name", &theme);
/// TextField::search(&s.filter, |s: &mut App| &mut s.filter).placeholder("Filter files").fill().build("filter", &theme)
/// # }
/// ```
pub struct TextField<'a> {
    edit: &'a TextEdit,
    bind: Transform,
    placeholder: String,
    leading: Option<&'static Icon>,
    size: ControlSize,
    width: crate::layout::Width,
    extra: Vec<Transform>,
}

impl<'a> TextField<'a> {
    pub fn new<S: 'static>(
        edit: &'a TextEdit,
        bind: impl for<'b> Fn(&'b mut S) -> &'b mut TextEdit + 'static,
    ) -> Self {
        TextField {
            edit,
            bind: Box::new(move |e: El| e.on_edit(bind)),
            placeholder: String::new(),
            leading: None,
            size: ControlSize::Regular,
            width: crate::layout::Width::Prefer(200.0),
            extra: Vec::new(),
        }
    }

    /// A field with the magnifier in front, for filtering and search.
    pub fn search<S: 'static>(
        edit: &'a TextEdit,
        bind: impl for<'b> Fn(&'b mut S) -> &'b mut TextEdit + 'static,
    ) -> Self {
        TextField::new(edit, bind).icon(&icons::SEARCH)
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
    pub fn icon(mut self, glyph: &'static Icon) -> Self {
        self.leading = Some(glyph);
        self
    }
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }
    pub fn large(self) -> Self {
        self.size(ControlSize::Large)
    }
    /// The width it prefers, in points; it shrinks below it to fit. 200 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = crate::layout::Width::Prefer(width);
        self
    }
    /// Take all the width the parent gives.
    pub fn fill(mut self) -> Self {
        self.width = crate::layout::Width::Fill;
        self
    }

    /// Runs on Enter.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.extra.push(Box::new(move |e: El| e.on_submit(f)));
        self
    }

    /// Sees keys before the field edits with them; return true to take one.
    pub fn on_key<S: 'static>(
        mut self,
        f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
    ) -> Self {
        self.extra.push(Box::new(move |e: El| e.on_input_key(f)));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let h = self.size.height();
        let style = rune::TextStyle::new(self.size.text_size() + 0.5, 400.0);
        let mut input = text_input(self.edit, style)
            .key(key)
            .single_line()
            .grow(1.0)
            .color(theme.text)
            .placeholder(self.placeholder, theme.text_tertiary);
        input = (self.bind)(input);
        for extra in self.extra {
            input = extra(input);
        }
        let mut f = row()
            .h(h)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(theme.radius.field)
            .fill(theme.field)
            .min_w(0.0);
        f = self.width.apply(f);
        if let Some(glyph) = self.leading {
            f = f.child(icon(glyph, h * 0.5).color(theme.text_tertiary));
        }
        f.child(input)
    }
}

/// Rex's toggle switch. `name` is read by screen readers; it isn't drawn, since a toggle sits
/// beside its row's title.
pub struct Toggle {
    on: bool,
    name: String,
    disabled: bool,
    toggle: Option<Transform<rune_kit::Switch>>,
}

impl Toggle {
    pub fn new(on: bool, name: impl Into<String>) -> Self {
        Toggle {
            on,
            name: name.into(),
            disabled: false,
            toggle: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.toggle = Some(Box::new(move |s: rune_kit::Switch| s.on_toggle(f)));
        self
    }

    pub fn build(self, theme: &Theme) -> El {
        let mut switch = rune_kit::Switch::new(self.on)
            .label(self.name)
            .hide_label()
            .control_size(rune_kit::ControlSize::Small)
            .disabled(self.disabled);
        if let Some(toggle) = self.toggle {
            switch = toggle(switch);
        }
        switch.build(&theme.kit)
    }
}

/// A checkbox with an optional label; the whole row toggles it.
pub struct Checkbox {
    checked: bool,
    label: Option<String>,
    name: Option<String>,
    disabled: bool,
    toggle: Option<Transform>,
}

impl Checkbox {
    pub fn new(checked: bool) -> Self {
        Checkbox {
            checked,
            label: None,
            name: None,
            disabled: false,
            toggle: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The name screen readers read when there is no label.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.toggle = Some(Box::new(move |e: El| e.on_press(f)));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let mut mark = el().size(15.0, 15.0).radius(4.0).center().border(
            1.0,
            if self.checked {
                theme.accent
            } else {
                theme.text_tertiary
            },
        );
        if self.checked {
            mark = mark
                .fill(theme.accent)
                .child(icon(&icons::CHECK, 11.0).color(theme.on_accent));
        }
        let name = self.name.or_else(|| self.label.clone()).unwrap_or_default();
        let mut r = row()
            .key(key)
            .role("checkbox")
            .label(name)
            .checked(self.checked)
            .gap(8.0)
            .center_y()
            .min_h(22.0)
            .focusable(!self.disabled)
            .focus_ring(2.0, 2.0, theme.focus_ring)
            .disabled(self.disabled)
            .child(mark);
        if let Some(label) = self.label {
            r = r.child(
                text(label)
                    .font(theme.ty.row)
                    .color(if self.checked {
                        theme.text_secondary
                    } else {
                        theme.text
                    })
                    .truncate()
                    .shrink(),
            );
        }
        if self.disabled {
            return r.opacity(0.45);
        }
        match self.toggle {
            Some(toggle) => toggle(r),
            None => r,
        }
    }
}

/// A slider from 0 to 1 with a gradient track, as Volume or Hue: an optional label and value
/// over the track. The arrow keys step it by 0.01.
pub struct Slider {
    value: f32,
    label: Option<String>,
    value_label: Option<String>,
    stops: Vec<Color>,
    width: crate::layout::Width,
    change: Option<Transform>,
}

const KNOB: f32 = 18.0;

impl Slider {
    pub fn new(value: f32) -> Self {
        Slider {
            value: value.clamp(0.0, 1.0),
            label: None,
            value_label: None,
            stops: Vec::new(),
            width: crate::layout::Width::Prefer(240.0),
            change: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The value as shown at the label's end, as "40%".
    pub fn value_label(mut self, value: impl Into<String>) -> Self {
        self.value_label = Some(value.into());
        self
    }
    /// The track's colours from 0 to 1; the thumb takes the colour under it. The accent by default.
    pub fn gradient(mut self, stops: impl Into<Vec<Color>>) -> Self {
        self.stops = stops.into();
        self
    }
    /// The width it prefers, in points; it shrinks below it to fit. 240 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = crate::layout::Width::Prefer(width);
        self
    }
    /// Take all the width the parent gives.
    pub fn fill(mut self) -> Self {
        self.width = crate::layout::Width::Fill;
        self
    }

    /// Receives the new value, 0 to 1, from the pointer or the arrow keys.
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, f32) + 'static) -> Self {
        let v = self.value;
        let change = std::rc::Rc::new(f);
        let step = change.clone();
        self.change = Some(Box::new(move |e: El| {
            e.on_step(move |s: &mut S, steps, _| step(s, (v + steps as f32 * 0.01).clamp(0.0, 1.0)))
                .on_drag(move |s: &mut S, d: &mut Drag, tx| {
                    let to_value =
                        |x: f32| ((x - KNOB / 2.0) / (d.w - KNOB).max(1.0)).clamp(0.0, 1.0);
                    match d.phase {
                        Phase::Down => {
                            d.data = v;
                            change(s, to_value(d.x));
                        }
                        Phase::Move => {
                            change(s, to_value(d.x));
                            tx.instant();
                        }
                        Phase::Up => {
                            if d.cancelled() {
                                change(s, d.data);
                            }
                        }
                    }
                })
        }));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let v = self.value;
        let gradient: Vec<(f32, Color)> = if self.stops.len() >= 2 {
            let n = (self.stops.len() - 1) as f32;
            self.stops
                .iter()
                .enumerate()
                .map(|(i, c)| (i as f32 / n, *c))
                .collect()
        } else {
            vec![(0.0, theme.accent.alpha(0.35)), (1.0, theme.accent)]
        };
        let thumb_color = if self.stops.len() >= 2 {
            self.stops[(v * (self.stops.len() - 1) as f32).round() as usize]
        } else {
            theme.accent
        };
        let track = el()
            .key("track")
            .pin(Some(0.0), Some(3.0), Some(0.0), None)
            .h(12.0)
            .radius(6.0)
            .linear_gradient(90.0, gradient)
            .a11y_hidden();
        let thumb = el()
            .key("thumb")
            .size(KNOB, KNOB)
            .radius(KNOB / 2.0)
            .border(2.5, Color::WHITE)
            .fill(thumb_color)
            .shadow(1.0, 3.0, Color::BLACK.alpha(0.3))
            .a11y_hidden();
        // The thumb sits between two spacers sharing the free width as v : 1 - v, so the track
        // follows whatever width the slider is given.
        let rail = row()
            .w_full()
            .h(KNOB)
            .center_y()
            .child(el().grow(v.max(1e-4)))
            .child(thumb)
            .child(el().grow((1.0 - v).max(1e-4)));
        let name = self.label.clone().unwrap_or_else(|| "Slider".to_owned());
        let mut slider = el()
            .key("slider")
            .role("slider")
            .label(name)
            .range_value(f64::from(v), 0.0, 1.0)
            .range_step(0.01)
            .w_full()
            .h(KNOB)
            .focusable(true)
            .focus_ring(2.0, 2.0, theme.focus_ring)
            .child(track)
            .child(rail);
        if let Some(value_label) = &self.value_label {
            slider = slider.a11y_value(value_label.clone());
        }
        if let Some(change) = self.change {
            slider = change(slider);
        }
        let mut c = col().key(key).gap(6.0).min_w(80.0);
        c = self.width.apply(c);
        if self.label.is_some() || self.value_label.is_some() {
            c = c.child(
                row()
                    .gap(8.0)
                    .justify_between()
                    .child(
                        text(self.label.unwrap_or_default())
                            .font(theme.ty.caption)
                            .color(theme.text_secondary)
                            .truncate()
                            .shrink(),
                    )
                    .child(
                        text(self.value_label.unwrap_or_default())
                            .font(theme.ty.mono)
                            .color(theme.text),
                    ),
            );
        }
        c.child(slider)
    }
}

/// A colour to pick: the colour over its hex code, ringed in the accent while selected.
/// `.compact()` drops the hex code for a small round swatch.
pub struct Swatch {
    color: Color,
    selected: bool,
    compact: bool,
    press: Option<Transform>,
}

impl Swatch {
    pub fn new(color: Color) -> Self {
        Swatch {
            color,
            selected: false,
            compact: false,
            press: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |e: El| e.on_press(f)));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let hex = hex_code(self.color);
        let base = el()
            .key(key)
            .role("radio")
            .label(hex.clone())
            .checked(self.selected)
            .focusable(true)
            .focus_ring(2.0, 2.0, theme.focus_ring);
        let swatch = if self.compact {
            let ring = if self.selected {
                theme.accent
            } else {
                Color::TRANSPARENT
            };
            base.padding(2.0).radius(14.0).border(2.0, ring).child(
                el().size(20.0, 20.0)
                    .radius(10.0)
                    .fill(self.color)
                    .border(1.0, theme.border),
            )
        } else {
            base.w(86.0)
                .radius(8.0)
                .clip_rounded(8.0)
                .border(
                    if self.selected { 2.0 } else { 1.0 },
                    if self.selected {
                        theme.accent
                    } else {
                        theme.border
                    },
                )
                .child(
                    col()
                        .w_full()
                        .child(el().h(22.0).w_full().fill(self.color))
                        .child(
                            el().h(22.0)
                                .w_full()
                                .center()
                                .fill(theme.group)
                                .child(text(hex).font(theme.ty.mono).color(theme.text)),
                        ),
                )
        };
        match self.press {
            Some(press) => press(swatch),
            None => swatch,
        }
    }
}
