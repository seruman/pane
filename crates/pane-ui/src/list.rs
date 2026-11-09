//! Lists: an inset group of rows, each with an optional leading element, a title and detail,
//! and a trailing element. Up and Down move between rows; the selected one is tinted.

use rune::{El, col, el, row, text};

use crate::{Theme, Transform};

/// A row of a [`List`].
pub struct ListRow {
    title: String,
    detail: Option<String>,
    leading: Option<El>,
    trailing: Option<El>,
    selected: bool,
    press: Option<Transform>,
}

impl ListRow {
    pub fn new(title: impl Into<String>) -> Self {
        ListRow {
            title: title.into(),
            detail: None,
            leading: None,
            trailing: None,
            selected: false,
            press: None,
        }
    }

    /// A second line under the title, or beside it in a wide list.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    /// An icon, a status mark or any small element before the title.
    pub fn leading(mut self, leading: El) -> Self {
        self.leading = Some(leading);
        self
    }
    /// A badge, a shortcut, a button: anything at the row's end.
    pub fn trailing(mut self, trailing: El) -> Self {
        self.trailing = Some(trailing);
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.press = Some(Box::new(move |e: El| e.on_press(f)));
        self
    }

    fn build(self, key: &str, separator: bool, theme: &Theme) -> El {
        let mut lines = col().gap(1.0).grow(1.0).min_w(0.0).child(
            text(self.title.clone())
                .font(theme.ty.row)
                .color(theme.text)
                .truncate(),
        );
        if let Some(detail) = self.detail {
            lines = lines.child(
                text(detail)
                    .font(theme.ty.caption)
                    .color(theme.text_secondary)
                    .truncate(),
            );
        }
        let mut r = row()
            .key(key)
            .role("listitem")
            .label(self.title)
            .checked(self.selected)
            .focus_selected(self.selected)
            .min_h(40.0)
            .px(10.0)
            .py(6.0)
            .gap(10.0)
            .center_y()
            .radius(theme.radius.row)
            .focusable(true);
        r = if self.selected {
            r.fill(theme.accent_soft)
        } else {
            r.focus_highlight(theme.hover)
        };
        if let Some(leading) = self.leading {
            r = r.child(leading);
        }
        r = r.child(lines);
        if let Some(trailing) = self.trailing {
            r = r.child(trailing);
        }
        if let Some(press) = self.press {
            r = press(r);
        }
        if !separator {
            return r;
        }
        col()
            .child(r)
            .child(el().h(1.0).mx(10.0).fill(theme.separator))
    }
}

/// An inset group of [`ListRow`]s with hairlines between them.
#[derive(Default)]
pub struct List {
    rows: Vec<ListRow>,
    empty: Option<El>,
}

impl List {
    pub fn new() -> Self {
        List::default()
    }
    pub fn row(mut self, row: ListRow) -> Self {
        self.rows.push(row);
        self
    }
    pub fn rows(mut self, rows: impl IntoIterator<Item = ListRow>) -> Self {
        self.rows.extend(rows);
        self
    }
    /// Shown instead of the rows when there are none, as an [`crate::EmptyState`].
    pub fn empty(mut self, empty: El) -> Self {
        self.empty = Some(empty);
        self
    }

    pub fn build(self, key: &str, theme: &Theme) -> El {
        let mut list = col()
            .key(key)
            .role("list")
            .focus_group()
            .w_full()
            .radius(theme.radius.group)
            .fill(theme.group)
            .padding(4.0);
        if self.rows.is_empty() {
            return match self.empty {
                Some(empty) => list.child(empty),
                None => list,
            };
        }
        let count = self.rows.len();
        for (i, r) in self.rows.into_iter().enumerate() {
            list = list.child(r.build(&format!("row-{i}"), i + 1 < count, theme));
        }
        list
    }
}

/// A thin bar filled to `value`, 0 to 1, in the accent.
pub fn progress_bar(value: f32, theme: &Theme) -> El {
    let v = value.clamp(0.0, 1.0);
    row()
        .role("progressbar")
        .range_value(f64::from(v), 0.0, 1.0)
        .w_full()
        .h(6.0)
        .radius(3.0)
        .fill(theme.field)
        .clip_rounded(3.0)
        .child(
            el().h_full()
                .grow(v.max(1e-4))
                .radius(3.0)
                .fill(theme.accent),
        )
        .child(el().h_full().grow((1.0 - v).max(1e-4)))
}
