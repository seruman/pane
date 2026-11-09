//! The command palette: a search field over a filtered list of commands grouped in sections,
//! the typed text marked in each title, and key hints. Show it with [`crate::modal_layer`].

use std::ops::Range;

use rune::{El, Icon, Key, TextEdit, el, row, text};

use crate::{MenuItem, MenuTrailing, TextField, Theme, kbd, menu_section, popover};

/// One command in the palette.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaletteEntry {
    pub title: &'static str,
    /// Entries without a section come first, then each section in order of first appearance.
    pub section: Option<&'static str>,
    pub shortcut: Option<&'static str>,
    pub submenu: bool,
    pub icon: Option<&'static Icon>,
}

impl PaletteEntry {
    pub const fn new(title: &'static str) -> Self {
        PaletteEntry {
            title,
            section: None,
            shortcut: None,
            submenu: false,
            icon: None,
        }
    }
    pub const fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }
    pub const fn shortcut(mut self, keys: &'static str) -> Self {
        self.shortcut = Some(keys);
        self
    }
    pub const fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }
    pub const fn icon(mut self, glyph: &'static Icon) -> Self {
        self.icon = Some(glyph);
        self
    }
}

/// The entries matching `query` (case-insensitive substring), in display order, with the
/// matched byte range of each title.
pub fn palette_matches(
    entries: &[PaletteEntry],
    query: &str,
) -> Vec<(usize, Option<Range<usize>>)> {
    let query = query.trim();
    let mut sections: Vec<Option<&str>> = vec![None];
    for e in entries {
        if !sections.contains(&e.section) {
            sections.push(e.section);
        }
    }
    let mut out = Vec::new();
    for section in sections {
        for (i, e) in entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.section == section)
        {
            if query.is_empty() {
                out.push((i, None));
            } else if let Some(range) = vimkeys::find_folded(e.title, query, true)
                .into_iter()
                .next()
            {
                out.push((i, Some(range)));
            }
        }
    }
    out
}

/// What a key in the palette's field asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteKey {
    Up,
    Down,
    Confirm,
    Dismiss,
}

impl PaletteKey {
    /// The selected row after Up or Down among `count` matches, wrapping at the ends.
    pub fn step(self, selected: usize, count: usize) -> usize {
        match self {
            PaletteKey::Up => selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            PaletteKey::Down if count > 0 => (selected + 1) % count,
            _ => selected,
        }
    }
}

/// The palette. `selected` indexes [`palette_matches`]; the app keeps it and the query.
///
/// ```no_run
/// # use pane_ui::{CommandPalette, PaletteEntry, PaletteKey, Theme, palette_matches};
/// # struct App { query: rune::TextEdit, selected: usize, chosen: Option<usize>, open: bool }
/// # const COMMANDS: &[PaletteEntry] = &[PaletteEntry::new("Open"), PaletteEntry::new("Save")];
/// # fn view(s: &App, theme: &Theme) -> rune::El {
/// CommandPalette::new(&s.query, |s: &mut App| &mut s.query, COMMANDS)
///     .selected(s.selected)
///     .on_key(|s: &mut App, key| {
///         let matches = palette_matches(COMMANDS, &s.query.text);
///         match key {
///             PaletteKey::Confirm => s.chosen = matches.get(s.selected).map(|(i, _)| *i),
///             PaletteKey::Dismiss => s.open = false,
///             step => s.selected = step.step(s.selected, matches.len()),
///         }
///     })
///     .on_choose(|s: &mut App, index| s.chosen = Some(index))
///     .build("palette", &theme)
/// # }
/// ```
pub struct CommandPalette<'a> {
    query: &'a TextEdit,
    field: TextField<'a>,
    entries: &'a [PaletteEntry],
    selected: usize,
    hints: Vec<(String, String)>,
    width: f32,
    choose: Option<crate::IndexedPress>,
}

impl<'a> CommandPalette<'a> {
    pub fn new<S: 'static>(
        query: &'a TextEdit,
        bind: impl for<'b> Fn(&'b mut S) -> &'b mut TextEdit + 'static,
        entries: &'a [PaletteEntry],
    ) -> Self {
        CommandPalette {
            query,
            field: TextField::search(query, bind)
                .placeholder("Search commands…")
                .large()
                .fill(),
            entries,
            selected: 0,
            hints: Vec::new(),
            width: 520.0,
            choose: None,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.field = self.field.placeholder(placeholder);
        self
    }
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }
    /// A key hint in the footer, as ("Run", "↩").
    pub fn hint(mut self, label: impl Into<String>, keys: impl Into<String>) -> Self {
        self.hints.push((label.into(), keys.into()));
        self
    }
    /// The widest it gets, in points; 520 by default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Up, Down, Enter and Escape typed in the field.
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, PaletteKey) + 'static) -> Self {
        self.field = self.field.on_key(move |s: &mut S, k, _| {
            let key = match k.key {
                Key::Up => PaletteKey::Up,
                Key::Down => PaletteKey::Down,
                Key::Enter => PaletteKey::Confirm,
                Key::Escape => PaletteKey::Dismiss,
                _ => return false,
            };
            f(s, key);
            true
        });
        self
    }

    /// A row clicked, with its index in the entries.
    pub fn on_choose<S: 'static>(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.choose = Some(crate::indexed_press(f));
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let matches = palette_matches(self.entries, &self.query.text);
        let mut list =
            rune::col()
                .key("rows")
                .gap(1.0)
                .max_h(12.0 * 29.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                });
        let mut last_section = None;
        for (row_index, (entry, range)) in matches.iter().enumerate() {
            let e = &self.entries[*entry];
            if e.section != last_section {
                if let Some(title) = e.section {
                    list = list.child(menu_section(title, theme));
                }
                last_section = e.section;
            }
            let mut item = MenuItem::new(e.title)
                .highlight(range.clone())
                .selected(row_index == self.selected);
            if let Some(glyph) = e.icon {
                item = item.icon(glyph);
            }
            item = match (e.shortcut, e.submenu) {
                (Some(keys), _) => item.shortcut(keys),
                (None, true) => item.trailing(MenuTrailing::Submenu),
                (None, false) => item,
            };
            let mut built = item.build(&format!("entry-{entry}"), theme);
            if let Some(choose) = &self.choose {
                built = choose(*entry)(built);
            }
            list = list.child(built);
        }
        if matches.is_empty() {
            list = list.child(
                el().h(28.0).px(10.0).center_y().child(
                    text("No matches")
                        .font(theme.ty.body)
                        .color(theme.text_tertiary),
                ),
            );
        }
        let mut palette = popover(key, theme)
            .role("dialog")
            .label("Command palette")
            .w_full()
            .max_w(self.width)
            .child(self.field.build("query", theme).mb(4.0))
            .child(list);
        if !self.hints.is_empty() {
            let mut footer = row()
                .min_h(32.0)
                .px(8.0)
                .pt(6.0)
                .gaps(4.0, 14.0)
                .flex_wrap()
                .center_y()
                .justify_end();
            for (label, keys) in self.hints {
                footer = footer.child(
                    row()
                        .gap(6.0)
                        .center_y()
                        .shrink()
                        .child(
                            text(label)
                                .font(theme.ty.body)
                                .color(theme.text_tertiary)
                                .truncate()
                                .shrink(),
                        )
                        .child(kbd(keys, theme)),
                );
            }
            palette = palette
                .child(crate::hairline(theme).w_full().mt(4.0))
                .child(footer);
        }
        palette
    }
}
