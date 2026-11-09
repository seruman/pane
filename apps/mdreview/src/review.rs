//! Comments anchored to rendered text and Markdown feedback for the document's author.

use std::fmt::Write as _;
use std::ops::Range;

use crate::page::{Page, lines_of, source_of};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Comment,
    Remove,
}

/// Stable identity independent of comment order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommentId(u32);

impl std::fmt::Display for CommentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    General,
    On(Range<usize>),
    Remove(Range<usize>),
}

impl Subject {
    pub fn kind(&self) -> Kind {
        match self {
            Self::General | Self::On(_) => Kind::Comment,
            Self::Remove(_) => Kind::Remove,
        }
    }

    pub fn range(&self) -> Option<&Range<usize>> {
        match self {
            Self::General => None,
            Self::On(range) | Self::Remove(range) => Some(range),
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::On(_) => "Comment",
            Self::Remove(_) => "Remove",
        }
    }

    pub fn feedback_title(&self) -> &'static str {
        match self {
            Self::Remove(_) => "Remove this",
            _ => self.title(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub subject: Subject,
    /// Empty for a removal without an explanatory note.
    pub body: String,
}

impl Comment {
    pub fn on(range: Range<usize>, body: impl Into<String>) -> Comment {
        Comment {
            subject: Subject::On(range),
            body: body.into(),
        }
    }

    pub fn general(body: impl Into<String>) -> Comment {
        Comment {
            subject: Subject::General,
            body: body.into(),
        }
    }

    pub fn remove(range: Range<usize>) -> Comment {
        Comment {
            subject: Subject::Remove(range),
            body: String::new(),
        }
    }

    pub fn kind(&self) -> Kind {
        self.subject.kind()
    }

    pub fn range(&self) -> Option<&Range<usize>> {
        self.subject.range()
    }

    pub fn title(&self) -> &'static str {
        self.subject.title()
    }

    /// General comments sort before anchored comments.
    fn order(&self) -> (bool, usize, usize) {
        self.range()
            .map_or((false, 0, 0), |r| (true, r.start, r.end))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Review {
    /// General comments in insertion order, then anchored comments in document order.
    comments: Vec<(CommentId, Comment)>,
    next_id: u32,
}

impl Review {
    pub fn add(&mut self, comment: Comment) -> CommentId {
        let id = CommentId(self.next_id);
        self.next_id += 1;
        let key = comment.order();
        let at = self.comments.partition_point(|(_, c)| c.order() <= key);
        self.comments.insert(at, (id, comment));
        id
    }

    pub fn iter(&self) -> impl Iterator<Item = (CommentId, &Comment)> {
        self.comments.iter().map(|(id, c)| (*id, c))
    }

    pub fn len(&self) -> usize {
        self.comments.len()
    }

    pub fn get(&self, id: CommentId) -> Option<&Comment> {
        self.comments.iter().find(|(i, _)| *i == id).map(|(_, c)| c)
    }

    pub fn set_body(&mut self, id: CommentId, body: String) -> bool {
        let Some((_, comment)) = self.comments.iter_mut().find(|(i, _)| *i == id) else {
            return false;
        };
        comment.body = body;
        true
    }

    /// Zero-based position in display order.
    pub fn position(&self, id: CommentId) -> Option<usize> {
        self.comments.iter().position(|(i, _)| *i == id)
    }

    pub fn at_position(&self, position: usize) -> Option<CommentId> {
        self.comments.get(position).map(|(id, _)| *id)
    }

    /// Returns false when the id is absent.
    pub fn remove(&mut self, id: CommentId) -> bool {
        let before = self.comments.len();
        self.comments.retain(|(i, _)| *i != id);
        self.comments.len() != before
    }

    fn anchored(&self) -> Vec<(CommentId, Range<usize>)> {
        self.comments
            .iter()
            .filter_map(|(id, c)| c.range().cloned().map(|r| (*id, r)))
            .collect()
    }

    /// Innermost comment containing rendered-text offset `at`.
    pub fn at(&self, at: usize) -> Option<CommentId> {
        self.anchored()
            .into_iter()
            .filter(|(_, r)| r.contains(&at) || (r.is_empty() && r.start == at))
            .min_by_key(|(_, r)| r.len())
            .map(|(i, _)| i)
    }

    /// Next comment in display order, wrapping at either end. `back` reverses direction.
    /// With no `from`, chooses the next anchor after `at` or the previous anchor when reversed.
    pub fn step(&self, from: Option<CommentId>, at: usize, back: bool) -> Option<CommentId> {
        let n = self.comments.len();
        if n == 0 {
            return None;
        }
        if let Some(f) = from.and_then(|id| self.position(id)) {
            return self.at_position((f + if back { n - 1 } else { 1 }) % n);
        }
        let list = self.anchored();
        if list.is_empty() {
            return self.at_position(if back { n - 1 } else { 0 });
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

    /// General feedback precedes anchored feedback in document order.
    /// Anchored feedback includes source lines and the nearest section.
    /// Quotes Markdown when it differs from rendered text, and always for removals.
    /// `name` supplies the document title. Returns `None` for an empty review.
    #[expect(clippy::expect_used, reason = "Formatting into a String cannot fail")]
    pub fn feedback(&self, source: &str, page: &Page, name: Option<&str>) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = match name {
            Some(name) => format!("# Review of {}\n\n", inline_code(name)),
            None => "# Review\n\n".to_owned(),
        };
        let general: Vec<&str> = self
            .iter()
            .map(|(_, c)| c)
            .filter(|c| c.range().is_none())
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
            write!(
                out,
                "I've reviewed {} and have {n} piece{} of feedback:\n\n",
                name.map_or("the document".to_owned(), inline_code),
                if n == 1 { "" } else { "s" }
            )
            .expect("String write");
        }
        for (number, (id, range)) in anchored.into_iter().enumerate() {
            let Some(c) = self.get(id) else { continue };
            let ranges = page.source_ranges(range.clone());
            let mut place = Vec::new();
            match lines_of(source, &ranges) {
                Some((a, b)) if a == b => place.push(format!("line {a}")),
                Some((a, b)) => place.push(format!("lines {a} to {b}")),
                None => {}
            }
            let block = page.block_at(range.start);
            if !matches!(page.blocks[block].kind, crate::page::BlockKind::Heading(_))
                && let Some(section) = page.section(block)
            {
                place.push(format!("under {}", inline_code(section)));
            }
            let title = c.subject.feedback_title();
            write!(out, "## {}. {title}", number + 1).expect("String write");
            if !place.is_empty() {
                write!(out, " ({})", place.join(", ")).expect("String write");
            }
            out.push_str("\n\n");

            let full = page.text();
            let ends_on_line_boundaries = (range.start == 0 || full[..range.start].ends_with('\n'))
                && (range.end >= full.len() || full[range.end..].starts_with('\n'));
            let text = full.get(range).unwrap_or_default().trim();
            if text.contains('\n') || (ends_on_line_boundaries && c.kind() == Kind::Remove) {
                out.push_str(&fenced(
                    "markdown",
                    source_of(source, &ranges, true).trim_end(),
                ));
            } else {
                let md = source_of(source, &ranges, false);
                if c.kind() == Kind::Remove || md.trim() != text {
                    write!(out, "On {}, written as:\n\n", inline_code(text)).expect("String write");
                    out.push_str(&fenced("markdown", md.trim()));
                } else {
                    write!(out, "On {}\n\n", inline_code(text)).expect("String write");
                }
            }
            if c.kind() == Kind::Remove {
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

fn longest_backticks(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// Delimiter backticks outnumber every run inside `text`.
/// `CommonMark` requires padding when the text starts or ends with a backtick.
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
