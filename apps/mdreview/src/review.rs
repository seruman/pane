//! The review: comments anchored to ranges of the page's text, general comments about the whole
//! document, and the feedback they make, as Markdown for whoever wrote the document (an agent,
//! most often).

use std::ops::Range;

use crate::page::{Page, lines_of, source_of};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A comment in words.
    Comment,
    /// "Remove this".
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub kind: Kind,
    /// What it's about, in the page's text; `None` for a general comment, about the whole
    /// document.
    pub range: Option<Range<usize>>,
    /// What the reviewer wrote; may be empty for a removal.
    pub body: String,
}

impl Comment {
    /// A comment in words on `range`.
    pub fn on(range: Range<usize>, body: impl Into<String>) -> Comment {
        Comment {
            kind: Kind::Comment,
            range: Some(range),
            body: body.into(),
        }
    }

    /// A general comment.
    pub fn general(body: impl Into<String>) -> Comment {
        Comment {
            kind: Kind::Comment,
            range: None,
            body: body.into(),
        }
    }

    /// The heading word for it: "General", "Comment" or "Remove".
    pub fn title(&self) -> &'static str {
        match (self.kind, &self.range) {
            (_, None) => "General",
            (Kind::Comment, _) => "Comment",
            (Kind::Remove, _) => "Remove",
        }
    }

    /// Where it sorts: general comments first, as written, then the rest in document order.
    fn order(&self) -> (bool, usize, usize) {
        self.range
            .as_ref()
            .map_or((false, 0, 0), |r| (true, r.start, r.end))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Review {
    /// General comments first, in the order they were written, then the rest in document order.
    pub comments: Vec<Comment>,
}

impl Review {
    /// Adds `comment` in its place and returns its index.
    pub fn add(&mut self, comment: Comment) -> usize {
        let key = comment.order();
        let at = self.comments.partition_point(|c| c.order() <= key);
        self.comments.insert(at, comment);
        at
    }

    /// The comments on the page, with their ranges, in document order.
    fn anchored(&self) -> Vec<(usize, Range<usize>)> {
        self.comments
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.range.clone().map(|r| (i, r)))
            .collect()
    }

    /// The comment `at` (a byte of the page's text) falls in; the innermost when they nest.
    pub fn at(&self, at: usize) -> Option<usize> {
        self.anchored()
            .into_iter()
            .filter(|(_, r)| r.contains(&at) || (r.is_empty() && r.start == at))
            .min_by_key(|(_, r)| r.len())
            .map(|(i, _)| i)
    }

    /// The comment after comment `from` in the list (general comments first, then the page's in
    /// document order), or with `back` the one before, wrapping round. With no comment to start
    /// from, the first on the page starting after `at` (or the last before it).
    pub fn step(&self, from: Option<usize>, at: usize, back: bool) -> Option<usize> {
        let n = self.comments.len();
        if n == 0 {
            return None;
        }
        if let Some(f) = from.filter(|f| *f < n) {
            return Some((f + if back { n - 1 } else { 1 }) % n);
        }
        let list = self.anchored();
        if list.is_empty() {
            return Some(if back { n - 1 } else { 0 });
        }
        let found = if back {
            list.iter()
                .rev()
                .find(|(_, r)| r.start < at)
                .or(list.last())
        } else {
            list.iter().find(|(_, r)| r.start > at).or(list.first())
        };
        found.map(|(i, _)| *i)
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    /// The review as Markdown feedback: the general comments, then each comment on the page in
    /// document order. Each says where it is (its lines, and the section's heading, which still
    /// finds it once the lines have moved) and what it's about: the text as drawn, or the
    /// Markdown source where that differs, and always for a removal, so it can be found and
    /// cut as written. `name` is the document's, for the title; `None` for one read from stdin.
    /// `None` when there's nothing to send.
    pub fn feedback(&self, source: &str, page: &Page, name: Option<&str>) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = match name {
            Some(name) => format!("# Review of {}\n\n", inline_code(name)),
            None => "# Review\n\n".to_owned(),
        };
        let general: Vec<&str> = self
            .comments
            .iter()
            .filter(|c| c.range.is_none())
            .map(|c| c.body.trim())
            .filter(|b| !b.is_empty())
            .collect();
        if !general.is_empty() {
            out.push_str("## General\n\n");
            for body in general {
                out.push_str(&quoted(body));
                out.push_str("\n\n");
            }
        }
        let anchored = self.anchored();
        let n = anchored.len();
        if n > 0 {
            out.push_str(&format!(
                "I've reviewed {} and have {n} piece{} of feedback:\n\n",
                name.map_or("the document".to_owned(), inline_code),
                if n == 1 { "" } else { "s" }
            ));
        }
        for (number, (i, range)) in anchored.into_iter().enumerate() {
            let c = &self.comments[i];
            let ranges = page.source_ranges(range.clone());
            let mut place = Vec::new();
            match lines_of(source, &ranges) {
                Some((a, b)) if a == b => place.push(format!("line {a}")),
                Some((a, b)) => place.push(format!("lines {a}–{b}")),
                None => {}
            }
            // The section the comment starts in, unless it's on that heading itself.
            let block = page.block_at(range.start);
            if page.blocks[block].heading.is_none()
                && let Some(section) = page.section(block)
            {
                place.push(format!("under {}", inline_code(section)));
            }
            let title = match c.kind {
                Kind::Comment => "Comment",
                Kind::Remove => "Remove this",
            };
            out.push_str(&format!("## {}. {title}", number + 1));
            if !place.is_empty() {
                out.push_str(&format!(" ({})", place.join(", ")));
            }
            out.push_str("\n\n");

            let full = page.text();
            // Whole lines of the page (a `V` selection, `cc`, `dd`) quote whole source lines,
            // with the list marker or heading hashes on them.
            let whole_lines = (range.start == 0 || full[..range.start].ends_with('\n'))
                && (range.end >= full.len() || full[range.end..].starts_with('\n'));
            let text = full.get(range).unwrap_or_default().trim();
            if text.contains('\n') || (whole_lines && c.kind == Kind::Remove) {
                // More than a line reads best as the Markdown it came from, whole lines of it.
                out.push_str(&fenced(
                    "markdown",
                    source_of(source, &ranges, true).trim_end(),
                ));
            } else {
                let md = source_of(source, &ranges, false);
                if c.kind == Kind::Remove || md.trim() != text {
                    out.push_str(&format!("On {}, written as:\n\n", inline_code(text)));
                    out.push_str(&fenced("markdown", md.trim()));
                } else {
                    out.push_str(&format!("On {}\n\n", inline_code(text)));
                }
            }
            if c.kind == Kind::Remove {
                out.push_str("I don't want this in the document.\n\n");
            }
            let body = c.body.trim();
            if !body.is_empty() {
                out.push_str(&quoted(body));
                out.push_str("\n\n");
            }
        }
        Some(format!("{}\n", out.trim_end()))
    }
}

/// The longest run of backticks in `text`.
fn longest_backticks(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// `text` as inline code, whatever it holds: the backticks around it outnumber any run inside,
/// and a space pads a backtick at either end, as CommonMark reads code spans. Newlines become
/// spaces.
pub fn inline_code(text: &str) -> String {
    let text = text.replace('\n', " ");
    let ticks = "`".repeat(longest_backticks(&text) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

/// `text` in a fenced code block whose fence no line inside can close.
fn fenced(lang: &str, text: &str) -> String {
    let fence = "`".repeat(longest_backticks(text).max(2) + 1);
    format!("{fence}{lang}\n{text}\n{fence}\n\n")
}

/// `text` as a Markdown quote, every line of it.
fn quoted(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_owned()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
