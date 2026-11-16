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
}

impl BlockKind {
    // Keep in sync with rune-kit's private top-level block margins.
    fn margins(self) -> (f32, f32) {
        match self {
            BlockKind::Para => (0.0, 12.0),
            BlockKind::Heading(1) => (28.0, 12.0),
            BlockKind::Heading(2) => (24.0, 10.0),
            BlockKind::Heading(_) => (20.0, 8.0),
            BlockKind::Code => (12.0, 24.0),
            BlockKind::List => (8.0, 12.0),
            BlockKind::Quote => (16.0, 16.0),
            BlockKind::Rule => (32.0, 32.0),
            BlockKind::Table => (12.0, 16.0),
            BlockKind::Image => (24.0, 24.0),
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

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub blocks: Vec<Block>,
    text: String,
    /// Where each selectable text the page draws sits in [`Self::text`], in tree order.
    pieces: Vec<Range<usize>>,
}

impl Page {
    pub fn new(source: &str) -> Page {
        let options =
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
        let mut found: Vec<(BlockKind, Range<usize>)> = Vec::new();
        let mut depth = 0usize;
        let mut only_image = None;
        let mut events = Parser::new_ext(source, options).into_offset_iter();
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
                        if matches!(tag, Tag::Paragraph) {
                            only_image = Some(false);
                        }
                        found.push((kind_of(&tag), range));
                    }
                    depth += 1;
                }
                Event::End(_) => depth -= 1,
                Event::Rule if depth == 0 => found.push((BlockKind::Rule, range)),
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
            page.pieces.extend(
                rendered
                    .pieces()
                    .iter()
                    .map(|r| r.start + text_at..r.end + text_at),
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
        let piece = self.pieces.get(t.index()?)?;
        let at = |b: usize| (piece.start + b).min(piece.end);
        Some(at(t.copied().start)..at(t.copied().end))
    }

    /// The byte of [`Self::text`] a click's caret in `t` is at: on the character after it, or
    /// on the last one when the click was past a text's end.
    pub fn clicked(&self, t: &rune::SelectedText) -> Option<usize> {
        let piece = self.pieces.get(t.index()?)?;
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
        let block = &self.blocks[self.block_at(range.start)];
        let own = block.text_range();
        if range.start >= own.start && range.end <= own.end {
            let local = range.start - own.start..range.end - own.start;
            if let Some(at) = block.rendered.source_ranges(local).first().map(|r| r.start) {
                let options = Options::ENABLE_STRIKETHROUGH
                    | Options::ENABLE_TABLES
                    | Options::ENABLE_TASKLISTS;
                let link = Parser::new_ext(&block.markdown, options)
                    .into_offset_iter()
                    .find_map(|(event, span)| match event {
                        Event::Start(Tag::Link { dest_url, .. }) if span.contains(&at) => {
                            Some(dest_url.into_string())
                        }
                        _ => None,
                    });
                if link.is_some() {
                    return link;
                }
            }
        }
        bare_address(&self.text, range.start)
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

fn kind_of(tag: &Tag<'_>) -> BlockKind {
    match tag {
        Tag::Heading { level, .. } => BlockKind::Heading(*level as u8),
        Tag::CodeBlock(_) => BlockKind::Code,
        Tag::List(_) => BlockKind::List,
        Tag::BlockQuote(_) => BlockKind::Quote,
        Tag::Table(_) => BlockKind::Table,
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
