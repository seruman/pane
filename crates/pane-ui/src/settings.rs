//! Rex's settings layout: inset groups of rows, each a title and description with a control.

use rune::{El, col, row, text};

use crate::{Theme, hairline, with_separator};

/// A row of a [`SettingsGroup`]: title and description on the left, the control on the right.
/// When the row is too narrow for both, the control moves under the title.
pub struct SettingsRow {
    title: String,
    description: Option<String>,
    control: Option<El>,
}

impl SettingsRow {
    pub fn new(title: impl Into<String>) -> Self {
        SettingsRow {
            title: title.into(),
            description: None,
            control: None,
        }
    }
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
    pub fn control(mut self, control: El) -> Self {
        self.control = Some(control);
        self
    }

    fn build(self, separator: bool, theme: &Theme) -> El {
        let mut lines = col()
            .gap(2.0)
            .grow(1.0)
            .min_w(100.0)
            .child(text(self.title).font(theme.ty.row).color(theme.text));
        if let Some(description) = self.description {
            lines = lines.child(
                text(description)
                    .font(theme.ty.caption)
                    .color(theme.text_secondary)
                    .wrap(),
            );
        }
        let mut r = row()
            .min_h(46.0)
            .py(8.0)
            .gaps(8.0, 16.0)
            .flex_wrap()
            .center_y()
            .child(lines);
        if let Some(control) = self.control {
            r = r.child(control.shrink());
        }
        with_separator(r, separator.then(|| hairline(theme).w_full()))
    }
}

/// An inset group a shade off the pane, holding [`SettingsRow`]s with hairlines between them.
/// `.child(..)` adds anything else, such as a heading or a note.
#[derive(Default)]
pub struct SettingsGroup {
    items: Vec<Item>,
}

enum Item {
    Row(SettingsRow),
    Other(El),
}

impl SettingsGroup {
    pub fn new() -> Self {
        SettingsGroup::default()
    }
    pub fn row(mut self, row: SettingsRow) -> Self {
        self.items.push(Item::Row(row));
        self
    }
    pub fn child(mut self, child: El) -> Self {
        self.items.push(Item::Other(child));
        self
    }

    pub fn build(self, theme: &Theme) -> El {
        let count = self.items.len();
        let mut group = col()
            .w_full()
            .radius(theme.radius.group)
            .fill(theme.group)
            .px(16.0)
            .py(6.0);
        for (i, item) in self.items.into_iter().enumerate() {
            let has_next = i + 1 < count;
            group = group.child(match item {
                Item::Row(row) => row.build(has_next, theme),
                Item::Other(other) => other,
            });
        }
        group
    }
}
