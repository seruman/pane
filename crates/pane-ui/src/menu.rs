//! Menus: a floating panel of rows with icons, shortcuts, checks and sections. The command
//! palette uses the same rows.

use std::ops::Range;

use rune::{El, Icon, col, el, icon, row, text};

use crate::{Theme, Transform, icons};

/// What sits at the end of a [`MenuItem`].
#[derive(Clone, Debug, Default)]
pub enum MenuTrailing {
    #[default]
    None,
    Shortcut(String),
    Check,
    Submenu,
    Icon(&'static Icon),
}

/// A menu or palette row. The selected row fills with the accent; `highlight` marks the part of
/// the label a search matched.
pub struct MenuItem {
    label: String,
    leading: Option<&'static Icon>,
    trailing: MenuTrailing,
    highlight: Option<Range<usize>>,
    /// `None` for an item that isn't a toggle.
    checked: Option<bool>,
    selected: bool,
    disabled: bool,
    handlers: Vec<Transform>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        MenuItem {
            label: label.into(),
            leading: None,
            trailing: MenuTrailing::None,
            highlight: None,
            checked: None,
            selected: false,
            disabled: false,
            handlers: Vec::new(),
        }
    }

    pub fn icon(mut self, glyph: &'static Icon) -> Self {
        self.leading = Some(glyph);
        self
    }
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        self.trailing = MenuTrailing::Shortcut(keys.into());
        self
    }
    /// Whether the item is ticked, as "Wrap lines" is. A ticked item shows a check at its end
    /// and reads as checked. Unticked, it reads as unchecked and shows what `trailing` set.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
    pub fn submenu(mut self) -> Self {
        self.trailing = MenuTrailing::Submenu;
        self
    }
    pub fn trailing(mut self, trailing: MenuTrailing) -> Self {
        self.trailing = trailing;
        self
    }
    /// Marks a byte range in the label. Panics if it is out of bounds or splits a character.
    pub fn highlight(mut self, range: Option<Range<usize>>) -> Self {
        if let Some(range) = &range {
            assert!(
                self.label.get(range.clone()).is_some(),
                "highlight must be a byte range within the label on UTF-8 boundaries"
            );
        }
        self.highlight = range;
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.push(Box::new(move |e: El| e.on_press(f)));
        self
    }

    /// Runs when the pointer moves onto the row, to move the selection with it.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.push(Box::new(move |e: El| e.on_hover(f)));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let (fg, secondary) = if self.selected {
            (theme.on_accent, theme.on_accent.alpha(0.8))
        } else {
            (theme.text, theme.text_secondary)
        };
        // The keyboard and the pointer share one highlight, as in a native menu.
        let mut item = row()
            .key(key)
            .role("menuitem")
            .label(self.label.clone())
            .h(28.0)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(theme.radius.row)
            .disabled(self.disabled)
            .focusable(!self.disabled);
        if let Some(checked) = self.checked {
            item = item.checked(checked);
        }
        item = if self.selected {
            item.fill(theme.accent)
        } else {
            item.focus_highlight(theme.hover)
        };
        if let Some(glyph) = self.leading {
            item = item.child(icon(glyph, 15.0).color(secondary));
        }
        let label = &self.label;
        let style = theme.ty.row;
        let label_el = match self.highlight.filter(|_| !self.selected) {
            Some(r) => row()
                .child(text(&label[..r.start]).font(style).color(fg))
                .child(
                    text(&label[r.clone()])
                        .font(rune::TextStyle {
                            weight: 600.0,
                            ..style
                        })
                        .color(theme.accent_text),
                )
                .child(
                    text(&label[r.end..])
                        .font(style)
                        .color(fg)
                        .truncate()
                        .shrink(),
                ),
            None => row().child(
                text(label.as_str())
                    .font(style)
                    .color(fg)
                    .truncate()
                    .shrink(),
            ),
        };
        item = item.child(label_el.grow(1.0).min_w(0.0));
        let trailing = match self.checked {
            Some(true) => MenuTrailing::Check,
            _ => self.trailing,
        };
        item = match trailing {
            MenuTrailing::None => item,
            MenuTrailing::Shortcut(keys) => {
                item.child(text(keys).font(theme.ty.body).color(secondary))
            }
            MenuTrailing::Check => item.child(icon(&icons::CHECK, 15.0).color(fg)),
            MenuTrailing::Submenu => item.child(icon(&icons::CHEVRON_RIGHT, 14.0).color(secondary)),
            MenuTrailing::Icon(glyph) => item.child(icon(glyph, 15.0).color(secondary)),
        };
        if self.disabled {
            return item.opacity(0.45);
        }
        self.handlers
            .into_iter()
            .fold(item, |item, attach| attach(item))
    }
}

/// A section heading inside a menu, as "Sort by".
pub fn menu_section(title: impl Into<String>, theme: &Theme) -> El {
    el().px(10.0).pt(8.0).pb(3.0).child(
        text(title.into())
            .font(theme.ty.label)
            .color(theme.text_tertiary),
    )
}

/// A hairline between groups of menu rows.
pub fn menu_separator(theme: &Theme) -> El {
    el().h(1.0).w_full().my(4.0).fill(theme.separator)
}

/// The floating panel menus, the palette and pickers sit on. Up and Down move between its rows.
pub fn popover(key: &str, theme: &Theme) -> El {
    col()
        .key(key)
        .role("menu")
        .focus_group()
        .radius(theme.radius.popover)
        .fill(theme.popover)
        .border(1.0, theme.popover_border)
        .shadow(12.0, 32.0, theme.shadow_strong)
        .padding(6.0)
        .gap(1.0)
}

/// A menu: rows, sections and separators on a [`popover`]. It is as wide as its parent allows,
/// up to 280 points.
///
/// ```no_run
/// # use pane_ui::{Menu, MenuItem, Theme, icons};
/// # struct App;
/// # impl App { fn open(&mut self) {} }
/// # fn view(theme: &Theme) -> rune::El {
/// Menu::new()
///     .item(MenuItem::new("Open").icon(&icons::FOLDER_OPEN).shortcut("⌃O").on_press(|s: &mut App| s.open()))
///     .section("Sort by")
///     .item(MenuItem::new("Name").checked(true))
///     .build("menu", &theme)
/// # }
/// ```
pub struct Menu {
    entries: Vec<Entry>,
    width: f32,
}

enum Entry {
    Item(MenuItem),
    Section(String),
    Separator,
    Other(Box<El>),
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            entries: Vec::new(),
            width: 280.0,
        }
    }
}

impl Menu {
    pub fn new() -> Self {
        Menu::default()
    }
    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }
    pub fn items(mut self, items: impl IntoIterator<Item = MenuItem>) -> Self {
        self.entries.extend(items.into_iter().map(Entry::Item));
        self
    }
    pub fn section(mut self, title: impl Into<String>) -> Self {
        self.entries.push(Entry::Section(title.into()));
        self
    }
    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }
    pub fn child(mut self, child: El) -> Self {
        self.entries.push(Entry::Other(Box::new(child)));
        self
    }
    /// The widest it gets, in points; 280 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let mut menu = popover(key, theme).w_full().max_w(self.width);
        for (i, entry) in self.entries.into_iter().enumerate() {
            menu = menu.child(match entry {
                Entry::Item(item) => item.build(&format!("item-{i}"), theme),
                Entry::Section(title) => menu_section(title, theme),
                Entry::Separator => menu_separator(theme),
                Entry::Other(other) => *other,
            });
        }
        menu
    }
}
