//! A Markdown source cut into its top-level blocks, so the view can draw each block on its own
//! and put comment threads between them, as a GitHub review does under the lines it's about.
//!
//! The page's text is what [`RenderedText`] draws for each block, the blocks joined by a blank
//! line as one `RenderedText` of the whole source joins them: Vim moves over it, and comments
//! keep ranges of it.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};
use rune_kit::markdown::RenderedText;

/// What a top-level block is, for the space around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Para,
    Heading(u8),
    Code,
    List,
    Quote,
    Rule,
    Table,
}

impl BlockKind {
    /// The space above and below it, as `rune_kit`'s renderer puts between top-level blocks.
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
        }
    }
}

/// One top-level block.
#[derive(Clone, Debug)]
pub struct Block {
    /// Its bytes in the whole source, from the start of its first line.
    pub src: Range<usize>,
    /// The Markdown it draws from: its own lines, then the document's link reference
    /// definitions, so `[text][ref]` still links.
    pub markdown: String,
    rendered: RenderedText,
    /// Where its text starts in [`Page::text`].
    pub text_at: usize,
    /// The space above it on the page.
    pub top: f32,
}

impl Block {
    /// Its range of [`Page::text`].
    pub fn text_range(&self) -> Range<usize> {
        self.text_at..self.text_at + self.rendered.text().len()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub blocks: Vec<Block>,
    text: String,
}

impl Page {
    pub fn new(source: &str) -> Page {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
        let mut found: Vec<(BlockKind, Range<usize>)> = Vec::new();
        let mut depth = 0usize;
        let mut events = Parser::new_ext(source, options).into_offset_iter();
        for (event, range) in events.by_ref() {
            match event {
                Event::Start(tag) => {
                    if depth == 0 {
                        found.push((kind_of(&tag), range));
                    }
                    depth += 1;
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Rule if depth == 0 => found.push((BlockKind::Rule, range)),
                Event::Html(_) if depth == 0 => found.push((BlockKind::Para, range)),
                _ => {}
            }
        }
        let refs: String = events
            .reference_definitions()
            .iter()
            .filter_map(|(_, def)| source.get(def.span.clone()))
            .map(|line| format!("\n\n{}", line.trim_end()))
            .collect();

        let mut page = Page::default();
        let mut prev_bottom = 0.0f32;
        let mut after_heading = false;
        for (kind, range) in found {
            let start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
            let src = start..range.end;
            let markdown = format!("{}{refs}", source[src.clone()].trim_end());
            let rendered = RenderedText::new(&markdown);
            let (mt, mb) = kind.margins();
            // As the renderer collapses margins: the larger of the two meet, except that a
            // block under a heading keeps only the heading's.
            let top = match kind {
                _ if page.blocks.is_empty() => 0.0,
                BlockKind::Heading(_) | BlockKind::Rule => mt.max(prev_bottom),
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
            page.blocks.push(Block { src, markdown, rendered, text_at, top });
        }
        page
    }

    /// The text as drawn, as Vim reads it.
    pub fn text(&self) -> &str {
        &self.text
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

/// The 1-based lines of `source` that `ranges` touch, first and last.
pub fn lines_of(source: &str, ranges: &[Range<usize>]) -> Option<(usize, usize)> {
    let first = ranges.first()?.start;
    let last = ranges.last()?.end.saturating_sub(1).max(first);
    let line = |at: usize| source[..at.min(source.len())].matches('\n').count() + 1;
    Some((line(first), line(last)))
}

/// The Markdown behind `ranges`: from the first source byte drawn to the last, or with
/// `linewise`, the whole source lines they fall on.
pub fn source_of(source: &str, ranges: &[Range<usize>], linewise: bool) -> String {
    let (Some(first), Some(last)) = (ranges.first(), ranges.last()) else {
        return String::new();
    };
    let (mut start, mut end) = (first.start, last.end);
    if linewise {
        start = source[..start].rfind('\n').map_or(0, |i| i + 1);
        end = source[end..].find('\n').map_or(source.len(), |i| end + i);
    }
    source[start..end].to_owned()
}
