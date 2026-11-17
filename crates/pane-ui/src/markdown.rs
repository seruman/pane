//! Shared presentation and source mapping for the Markdown apps.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rune::{El, Image, ImageCache, Key, KeyEvent, Mods, TextStyle, Waker, el, row, text};
use rune_kit::{MarkdownStyle, markdown::Highlight};
use vimkeys::{Vim, VimMode};

use crate::{Badge, Theme, Tone, icons, kbd};

pub const VIM_CURSOR: &str = "vim-cursor";
pub const FONT_FAMILIES: (&str, &str) = ("Inter", "ui-monospace, Menlo, Inter");

/// The pictures a Markdown document names, read from files beside it.
///
/// Pass a clone to `Markdown::images`. Call [`Images::start`] from `AppSpec::setup` and
/// [`Images::poll`] from `AppSpec::poll`. Remote addresses are not fetched, so those images
/// keep showing their alt text.
#[derive(Clone)]
pub struct Images(Rc<ImagesInner>);

struct ImagesInner {
    base: PathBuf,
    cache: OnceCell<ImageCache>,
    // Rune's cache holds images weakly. Keeping them stops each frame from decoding again.
    seen: RefCell<HashMap<String, Image>>,
    settled: Cell<usize>,
}

impl Images {
    /// Relative paths start from `document`'s folder, or the working folder without one.
    pub fn beside(document: Option<&Path>) -> Self {
        let base = document
            .and_then(Path::parent)
            .map_or_else(PathBuf::new, Path::to_path_buf);
        Images(Rc::new(ImagesInner {
            base,
            cache: OnceCell::new(),
            seen: RefCell::new(HashMap::new()),
            settled: Cell::new(0),
        }))
    }

    /// Starts decoding. Until then every image shows its alt text.
    pub fn start(&self, waker: Waker) {
        self.0.cache.get_or_init(|| ImageCache::new(waker));
    }

    /// The image at `src`, decoding in the background. `None` for a remote address.
    pub fn get(&self, src: &str) -> Option<Image> {
        let cache = self.0.cache.get()?;
        let src = src.strip_prefix("file://").unwrap_or(src);
        if src.is_empty() || src.contains("://") || src.starts_with("data:") {
            return None;
        }
        let mut seen = self.0.seen.borrow_mut();
        let image = seen.entry(src.to_owned()).or_insert_with(|| {
            // The size only applies to an SVG, which fills the 880 pt page column.
            cache.file(self.0.base.join(src), 440.0, 2.0)
        });
        Some(image.clone())
    }

    /// Whether an image finished loading or failed since the last call.
    pub fn poll(&self) -> bool {
        let settled = self
            .0
            .seen
            .borrow()
            .values()
            .filter(|i| i.is_ready())
            .count();
        self.0.settled.replace(settled) != settled
    }
}

pub fn style(theme: &Theme) -> MarkdownStyle {
    let heading = |size, line_height| TextStyle {
        size,
        weight: 600.0,
        line_height,
        ..theme.ty.body
    };
    MarkdownStyle::from_kit(&theme.kit)
        .text(theme.text)
        .muted(theme.text_secondary)
        .link(theme.accent_text)
        .link_underline(theme.accent_text.alpha(0.4))
        .quote_rule(theme.separator)
        .rule(theme.separator)
        .code_block(theme.group, theme.separator)
        .inline_code_bg(theme.field)
        .heading_font(1, heading(26.0, 34.0))
        .heading_font(2, heading(20.0, 28.0))
        .heading_font(3, heading(16.0, 24.0))
}

pub fn vim_highlights<A: Clone, O: Clone>(
    vim: &Vim<A, O>,
    text: &str,
    map: impl Fn(Range<usize>) -> Vec<Range<usize>>,
    show_cursor: bool,
    theme: &Theme,
) -> Vec<(Vec<Range<usize>>, Highlight)> {
    let mut out: Vec<_> = vim
        .matches(text)
        .into_iter()
        .map(|range| (map(range), Highlight::new()))
        .collect();
    if let Some(range) = vim.selection(text) {
        out.push((
            map(range),
            Highlight::new().bg(theme.accent_soft).fg(theme.text),
        ));
    }
    if show_cursor {
        out.push((
            map(vim.cursor(text)),
            Highlight::new()
                .bg(theme.accent)
                .fg(theme.on_accent)
                .key(VIM_CURSOR),
        ));
    }
    out.retain(|(ranges, _)| !ranges.is_empty());
    out
}

pub fn key_hint(key: &str, what: &str, theme: &Theme) -> El {
    row().gap(4.0).center_y().child(kbd(key, theme)).child(
        text(what.to_owned())
            .font(theme.ty.caption)
            .color(theme.text_tertiary),
    )
}

/// The height [`title_bar`] takes, for `App::titlebar(Titlebar::new(TITLE_BAR_HEIGHT))`.
pub const TITLE_BAR_HEIGHT: f32 = 40.0;

pub fn title_bar(title: &str, hints: &[(&str, &str)], theme: &Theme) -> El {
    window_title_bar(title, hints, rune::ChromeInsets::default(), theme)
}

/// [`title_bar`] as the window's own titlebar (`App::titlebar` with [`TITLE_BAR_HEIGHT`]): it
/// moves the window and keeps clear of the window's controls, the macOS traffic lights or the
/// Windows caption buttons, which `insets` (`Cx::chrome_insets`) says take its ends. In a
/// terminal the insets are zero and it draws as [`title_bar`] does.
pub fn window_title_bar(
    title: &str,
    hints: &[(&str, &str)],
    insets: rune::ChromeInsets,
    theme: &Theme,
) -> El {
    let mut bar = row()
        .key("bar")
        .w_full()
        .min_h(TITLE_BAR_HEIGHT)
        .pl(20.0 + insets.leading)
        .pr(20.0 + insets.trailing)
        .window_drag()
        .py(6.0)
        .gap(10.0)
        .center_y()
        .child(rune::icon(&icons::FILE_TEXT, 16.0).color(theme.text_secondary))
        .child(
            text(title.to_owned())
                .font(theme.ty.strong)
                .color(theme.text)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0));
    for (key, what) in hints {
        bar = bar.child(key_hint(key, what, theme));
    }
    bar
}

pub fn vim_mode<A: Clone, O: Clone>(vim: &Vim<A, O>, theme: &Theme) -> El {
    if vim.mode() == VimMode::Search {
        text(vim.status()).font(theme.ty.mono).color(theme.text)
    } else {
        el().child(
            Badge::new(vim.status())
                .tone(if vim.mode() == VimMode::Normal {
                    Tone::Neutral
                } else {
                    Tone::Accent
                })
                .build(theme),
        )
    }
}

pub fn status_line<A: Clone, O: Clone>(
    vim: &Vim<A, O>,
    page: &str,
    note: Option<&str>,
    mode: Option<El>,
    extra: Option<El>,
    theme: &Theme,
) -> El {
    let line = page[..vim.cursor(page).start].matches('\n').count() + 1;
    let lines = page.matches('\n').count() + 1;
    let mut status = row()
        .key("status")
        .w_full()
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .fill(theme.group)
        .child(mode.unwrap_or_else(|| vim_mode(vim, theme)))
        .child(
            text(note.or(vim.message()).unwrap_or_default().to_owned())
                .font(theme.ty.caption)
                .color(theme.text_secondary)
                .truncate()
                .shrink(),
        )
        .child(el().grow(1.0));
    if let Some(extra) = extra {
        status = status.child(extra);
    }
    status
        .child(
            text(vim.pending())
                .font(theme.ty.mono)
                .color(theme.text_secondary),
        )
        .child(
            text(format!("{line}:{lines}"))
                .font(theme.ty.mono)
                .color(theme.text_tertiary),
        )
}

pub fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// Maps rendered ranges back to source, including a trailing newline for a linewise yank.
pub fn source_of(source: &str, ranges: &[Range<usize>], linewise: bool) -> String {
    let (Some(first), Some(last)) = (ranges.first(), ranges.last()) else {
        return String::new();
    };
    let (mut start, mut end) = (first.start, last.end);
    if linewise {
        start = line_start(source, start);
        end = source[end..].find('\n').map_or(source.len(), |i| end + i);
        return format!("{}\n", &source[start..end]);
    }
    source[start..end].to_owned()
}

/// Types plain characters into Vim for deterministic scenes.
pub fn type_keys<A: Clone, O: Clone>(vim: &mut Vim<A, O>, keys: &str, text: &str) {
    for c in keys.chars() {
        vim.key(
            &KeyEvent {
                key: Key::Text(c.to_string()),
                mods: Mods::default(),
            },
            text,
        );
    }
}

pub fn find_text(text: &str, needle: &str) -> Range<usize> {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("scene text {needle:?} must exist in the sample"));
    at..at + needle.len()
}
