//! Markdown blocks with rendered-text offsets for review anchors.
//! Blocks render separately so comment threads can follow them.
//! Blank lines separate their text, matching [`RenderedText`] for the whole document.

use std::ops::Range;

use pane_ui::markdown::line_start;
pub use pane_ui::markdown::source_of;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rune_kit::markdown::RenderedText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Para,
    Heading(u8),
    Code,
    List,
    Quote,
    Rule,
    Table,
    /// A paragraph that holds only an image, which rune-kit draws as an image block.
    Image,
    /// `<details>` up to its `</details>`.
    Details,
    /// Footnote definitions in a row, which rune-kit draws under one rule.
    Footnotes,
    /// An HTML block shown as its source, drawn as code.
    Html,
}

impl BlockKind {
    // Keep in sync with rune-kit's private top-level block margins.
    fn margins(self) -> (f32, f32) {
        match self {
            BlockKind::Para => (0.0, 12.0),
            BlockKind::Heading(1) => (28.0, 12.0),
            BlockKind::Heading(2) => (24.0, 10.0),
            BlockKind::Heading(_) => (20.0, 8.0),
            BlockKind::Code | BlockKind::Html => (12.0, 24.0),
            BlockKind::List => (8.0, 12.0),
            BlockKind::Quote => (16.0, 16.0),
            BlockKind::Rule => (32.0, 32.0),
            BlockKind::Table => (12.0, 16.0),
            BlockKind::Image => (24.0, 24.0),
            BlockKind::Details => (8.0, 12.0),
            BlockKind::Footnotes => (32.0, 0.0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    /// Its bytes in the whole source, from the start of its first line.
    pub src: Range<usize>,
    /// Block source plus link-reference definitions from the document.
    pub markdown: String,
    rendered: RenderedText,
    /// Where its text starts in [`Page::text`].
    pub text_at: usize,
    pub kind: BlockKind,
    /// The space above it on the page.
    pub top: f32,
}

impl Block {
    pub fn text_range(&self) -> Range<usize> {
        self.text_at..self.text_at + self.rendered.text().len()
    }
}

/// A `<details>` of the page, which the reader folds.
#[derive(Clone, Debug)]
pub struct Fold {
    /// The block that holds it.
    pub block: usize,
    /// Its summary's bytes of [`Page::text`].
    pub summary: Range<usize>,
    /// Its body's bytes of [`Page::text`]; empty when it has none.
    pub body: Range<usize>,
    pub open: bool,
}

/// What the parser turns on, as rune-kit's does.
const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_GFM);

/// How many `<details>` an HTML block opens, less those it closes.
fn details_balance(html: &str) -> isize {
    let lower = html.to_ascii_lowercase();
    let opens = lower.match_indices("<details").filter(|(i, _)| {
        matches!(
            lower.as_bytes().get(i + 8),
            Some(b'>' | b' ' | b'\t' | b'\n')
        )
    });
    opens.count().cast_signed() - lower.matches("</details>").count().cast_signed()
}

/// An HTML block draws nothing when all it holds is comments.
fn only_comments(html: &str) -> bool {
    let mut rest = html.trim();
    while let Some(after) = rest.strip_prefix("<!--") {
        let Some(end) = after.find("-->") else {
            return false;
        };
        rest = after[end + 3..].trim_start();
    }
    rest.is_empty()
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub blocks: Vec<Block>,
    pub folds: Vec<Fold>,
    /// Each footnote's label and its body's bytes of [`Self::text`].
    footnotes: Vec<(String, Range<usize>)>,
    /// Each link's bytes of [`Self::text`] and where it goes.
    links: Vec<(Range<usize>, String)>,
    text: String,
    /// Where each selectable text the page draws sits in [`Self::text`], in tree order.
    pieces: Vec<Range<usize>>,
}

impl Page {
    pub fn new(source: &str) -> Page {
        let (found, refs) = split(source);
        Page::of_blocks(source, found, &refs)
    }

    fn of_blocks(source: &str, found: Vec<(BlockKind, Range<usize>)>, refs: &str) -> Page {
        let mut page = Page::default();
        let mut prev_bottom = 0.0f32;
        let mut after_heading = false;
        for (kind, range) in found {
            let start = line_start(source, range.start);
            let src = start..range.end;
            let markdown = format!("{}{refs}", source[src.clone()].trim_end());
            let rendered = RenderedText::new(&markdown);
            let (mt, mb) = kind.margins();
            let top = match kind {
                _ if page.blocks.is_empty() => 0.0,
                BlockKind::Heading(_) | BlockKind::Rule | BlockKind::Image => mt.max(prev_bottom),
                _ if after_heading => prev_bottom,
                _ => mt.max(prev_bottom),
            };
            prev_bottom = mb;
            after_heading = matches!(kind, BlockKind::Heading(_));
            if !rendered.text().is_empty() && !page.text.is_empty() {
                page.text.push_str("\n\n");
            }
            let text_at = page.text.len();
            page.text.push_str(rendered.text());
            let shift = |r: &Range<usize>| r.start + text_at..r.end + text_at;
            page.pieces.extend(rendered.pieces().iter().map(shift));
            let index = page.blocks.len();
            page.folds.extend(rendered.details().iter().map(|d| Fold {
                block: index,
                summary: shift(&rendered.pieces()[d.summary]),
                body: shift(&d.body),
                open: d.open,
            }));
            page.footnotes.extend(
                rendered
                    .footnotes()
                    .iter()
                    .map(|(label, r)| (label.clone(), shift(r))),
            );
            page.links.extend(
                rendered
                    .links()
                    .iter()
                    .map(|(r, url)| (shift(r), url.clone())),
            );
            page.blocks.push(Block {
                src,
                markdown,
                rendered,
                text_at,
                kind,
                top,
            });
        }
        page
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The bytes of [`Self::text`] that `t`, a piece of a mouse selection over the page's
    /// blocks drawn as one selection group, covers.
    pub fn selected(&self, t: &rune::SelectedText) -> Option<Range<usize>> {
        let piece = self.drawn().nth(t.index()?)?;
        let at = |b: usize| (piece.start + b).min(piece.end);
        Some(at(t.copied().start)..at(t.copied().end))
    }

    /// The byte of [`Self::text`] a click's caret in `t` is at: on the character after it, or
    /// on the last one when the click was past a text's end.
    pub fn clicked(&self, t: &rune::SelectedText) -> Option<usize> {
        let piece = self.drawn().nth(t.index()?)?.clone();
        let at = (piece.start + t.copied().start).min(piece.end);
        if at < piece.end || piece.is_empty() {
            return Some(at);
        }
        self.text[piece.clone()]
            .char_indices()
            .next_back()
            .map(|(i, _)| piece.start + i)
    }

    /// Where the link drawn at bytes `range` of [`Self::text`] goes: a Markdown link's
    /// destination (reference links resolved), or a bare web address the reader links.
    pub fn link_at(&self, range: Range<usize>) -> Option<String> {
        match self.links.iter().find(|(r, _)| r.contains(&range.start)) {
            // A bare `www.` address the parser linked over plain HTTP opens securely, as a
            // reader's own linking does.
            Some((r, url)) if self.text[r.clone()].starts_with("www.") => {
                bare_address(&self.text, range.start).or_else(|| Some(url.clone()))
            }
            Some((_, url)) => Some(url.clone()),
            None => bare_address(&self.text, range.start),
        }
    }

    /// Where footnote `label`'s body starts in [`Self::text`].
    pub fn footnote(&self, label: &str) -> Option<usize> {
        self.footnotes
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, r)| r.start)
    }

    /// The label of the footnote whose body holds byte `at`, and where the text first refers
    /// to it.
    pub fn footnote_reference(&self, at: usize) -> Option<usize> {
        let (label, _) = self.footnotes.iter().find(|(_, r)| r.contains(&at))?;
        let url = format!("{}{label}", rune_kit::markdown::FOOTNOTE_SCHEME);
        self.links
            .iter()
            .find(|(_, u)| *u == url)
            .map(|(r, _)| r.start)
    }

    /// The pieces drawn: those outside closed folds.
    fn drawn(&self) -> impl Iterator<Item = &Range<usize>> {
        self.pieces.iter().filter(|p| {
            !self
                .folds
                .iter()
                .any(|f| !f.open && !f.body.is_empty() && f.body.contains(&p.start))
        })
    }

    /// The innermost fold whose summary or body holds byte `at`.
    pub fn fold_at(&self, at: usize) -> Option<usize> {
        self.folds.iter().rposition(|f| {
            (f.summary.start..f.summary.end.max(f.summary.start + 1)).contains(&at)
                || f.body.contains(&at)
        })
    }

    /// Opens every fold around byte `at`, as Vim opens folds a jump lands in. Whether any was
    /// closed.
    pub fn open_around(&mut self, at: usize) -> bool {
        let mut opened = false;
        for f in &mut self.folds {
            if !f.open && f.body.contains(&at) {
                f.open = true;
                opened = true;
            }
        }
        opened
    }

    /// The text closed folds hide.
    pub fn closed(&self) -> Vec<Range<usize>> {
        self.folds
            .iter()
            .filter(|f| !f.open && !f.body.is_empty())
            .map(|f| f.body.clone())
            .collect()
    }

    /// Block `index`'s folds: the first one's place in [`Self::folds`], and each one's state.
    pub fn block_folds(&self, index: usize) -> (usize, Vec<bool>) {
        let first = self
            .folds
            .iter()
            .position(|f| f.block == index)
            .unwrap_or(0);
        let open = self
            .folds
            .iter()
            .filter(|f| f.block == index)
            .map(|f| f.open)
            .collect();
        (first, open)
    }

    /// The source byte ranges a range of [`Self::text`] was drawn from, in the whole source.
    pub fn source_ranges(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        for block in &self.blocks {
            let own = block.text_range();
            let (lo, hi) = (range.start.max(own.start), range.end.min(own.end));
            if lo >= hi {
                continue;
            }
            out.extend(
                block
                    .rendered
                    .source_ranges(lo - own.start..hi - own.start)
                    .into_iter()
                    .map(|r| r.start + block.src.start..r.end + block.src.start),
            );
        }
        out
    }

    /// `ranges` (of the whole source) as they fall in block `index`'s own Markdown.
    pub fn local_ranges(&self, index: usize, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        let src = &self.blocks[index].src;
        ranges
            .iter()
            .filter_map(|r| {
                let (lo, hi) = (r.start.max(src.start), r.end.min(src.end));
                (lo < hi).then(|| lo - src.start..hi - src.start)
            })
            .collect()
    }

    /// Nearest heading at or before a valid block index. `None` before the first heading.
    pub fn section(&self, index: usize) -> Option<&str> {
        self.blocks[..=index]
            .iter()
            .rev()
            .find(|b| matches!(b.kind, BlockKind::Heading(_)))
            .map(|b| b.rendered.text())
    }

    /// The block whose text holds byte `at` of [`Self::text`], or the last one before it.
    pub fn block_at(&self, at: usize) -> usize {
        self.blocks
            .iter()
            .rposition(|b| b.text_at <= at && !b.text_range().is_empty())
            .unwrap_or(0)
    }
}

/// The top-level blocks of `source` as mdreview draws them, and its link-reference definitions,
/// which each block's own Markdown needs. A `<details>` up to its `</details>` is one block, as
/// are footnote definitions in a row.
#[allow(
    clippy::too_many_lines,
    reason = "One pass over the parser's events, as rune-kit's own"
)]
fn split(source: &str) -> (Vec<(BlockKind, Range<usize>)>, String) {
    let mut found: Vec<(BlockKind, Range<usize>)> = Vec::new();
    let mut depth = 0usize;
    let mut only_image = None;
    // `<details>` left open, so the blocks until its `</details>` join its block.
    let mut details = 0isize;
    // The top-level block being read joins the last one found.
    let mut joining = false;
    let mut html = String::new();
    let mut events = Parser::new_ext(source, OPTIONS).into_offset_iter();
    for (event, range) in events.by_ref() {
        if depth == 1
            && let Some(only) = &mut only_image
        {
            let fits = match &event {
                Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => true,
                Event::End(TagEnd::Paragraph) => *only,
                Event::Text(t) | Event::Code(t) => t.trim().is_empty(),
                Event::SoftBreak | Event::HardBreak => true,
                _ => false,
            };
            *only = fits && (*only || matches!(event, Event::Start(Tag::Image { .. })));
            if matches!(event, Event::End(TagEnd::Paragraph))
                && only_image.take() == Some(true)
                && let Some(last) = found.last_mut()
            {
                last.0 = BlockKind::Image;
            }
        }
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let kind = kind_of(&tag);
                    joining = details > 0
                        || (kind == BlockKind::Footnotes
                            && found.last().is_some_and(|l| l.0 == BlockKind::Footnotes));
                    if matches!(tag, Tag::Paragraph) {
                        only_image = Some(false);
                    }
                    html.clear();
                    if !joining {
                        found.push((kind, range));
                    }
                }
                depth += 1;
            }
            Event::Html(h) if depth == 1 => html.push_str(&h),
            Event::End(end) => {
                depth -= 1;
                if depth == 0 {
                    let was_open = details > 0;
                    if end == TagEnd::HtmlBlock {
                        details = (details + details_balance(&html)).max(0);
                    }
                    if joining {
                        if let Some(last) = found.last_mut() {
                            last.1.end = range.end;
                        }
                    } else if end == TagEnd::HtmlBlock {
                        let kind = if details > 0
                            || (!was_open
                                && details_balance(&html) == 0
                                && html.to_ascii_lowercase().contains("<details"))
                        {
                            Some(BlockKind::Details)
                        } else if only_comments(&html) {
                            None
                        } else {
                            Some(BlockKind::Html)
                        };
                        match (kind, found.last_mut()) {
                            (Some(kind), Some(last)) => last.0 = kind,
                            (None, _) => {
                                found.pop();
                            }
                            (Some(_), None) => {}
                        }
                    }
                    joining = false;
                }
            }
            Event::Rule if depth == 0 => {
                if details > 0 {
                    if let Some(last) = found.last_mut() {
                        last.1.end = range.end;
                    }
                } else {
                    found.push((BlockKind::Rule, range));
                }
            }
            _ => {}
        }
    }
    let refs: String = events
        .reference_definitions()
        .iter()
        .filter_map(|(_, def)| source.get(def.span.clone()))
        .fold(String::new(), |mut refs, line| {
            refs.push_str("\n\n");
            refs.push_str(line.trim_end());
            refs
        });

    (found, refs)
}

fn kind_of(tag: &Tag<'_>) -> BlockKind {
    match tag {
        Tag::Heading { level, .. } => BlockKind::Heading(*level as u8),
        Tag::CodeBlock(_) => BlockKind::Code,
        Tag::List(_) => BlockKind::List,
        Tag::BlockQuote(_) => BlockKind::Quote,
        Tag::Table(_) => BlockKind::Table,
        Tag::FootnoteDefinition(_) => BlockKind::Footnotes,
        Tag::HtmlBlock => BlockKind::Html,
        _ => BlockKind::Para,
    }
}

/// The web address in the word of `text` holding byte `at`, as a reader links `https://…`,
/// `http://…` and `www.…`, without the punctuation that ends a sentence around it.
fn bare_address(text: &str, at: usize) -> Option<String> {
    let start = text[..at].rfind(char::is_whitespace).map_or(0, |i| i + 1);
    let end = text[at..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| at + i);
    let word = text[start..end]
        .trim_start_matches(['(', '<', '[', '"', '\''])
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', ']', '"', '\'']);
    if word.starts_with("https://") || word.starts_with("http://") {
        Some(word.to_owned())
    } else if word.starts_with("www.") {
        Some(format!("https://{word}"))
    } else {
        None
    }
}

/// The 1-based lines of `source` that `ranges` touch, first and last.
pub fn lines_of(source: &str, ranges: &[Range<usize>]) -> Option<(usize, usize)> {
    let first = ranges.first()?.start;
    let last = ranges.last()?.end.saturating_sub(1).max(first);
    let line = |at: usize| source[..at].matches('\n').count() + 1;
    Some((line(first), line(last)))
}
