//! The review: comments anchored to ranges of the page's text, general comments about the whole
//! document, and the feedback they make, as Markdown for whoever wrote the document (an agent,
//! most often).

use std::ops::Range;

use crate::page::{Page, lines_of, source_of};

/// A preset comment applied with one key, after Plannotator's quick labels. `tip` goes out with
/// the label, telling the reader what to do about it.
pub struct Label {
    pub text: &'static str,
    pub tip: Option<&'static str>,
}

pub const LABELS: [Label; 9] = [
    Label { text: "Clarify this", tip: None },
    Label {
        text: "Verify this",
        tip: Some("This seems like an assumption. Verify it by reading the actual code before going on."),
    },
    Label {
        text: "Give me an example",
        tip: Some("This is too abstract. Show a before and after, a sample input and output, or a concrete scenario."),
    },
    Label {
        text: "Match existing patterns",
        tip: Some("Look for existing patterns, components or utilities that already solve this, and reuse them."),
    },
    Label {
        text: "Consider alternatives",
        tip: Some("Propose two or three other approaches, with their trade-offs."),
    },
    Label {
        text: "Ensure no regression",
        tip: Some("Make sure this doesn't break existing behaviour: say what could regress and how it's protected."),
    },
    Label {
        text: "Out of scope",
        tip: Some("This isn't part of the task. Leave it out and stay with what was asked."),
    },
    Label { text: "Needs tests", tip: None },
    Label { text: "Looks good", tip: None },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A comment in words.
    Comment,
    /// "Remove this".
    Remove,
    /// One of [`LABELS`], with optional words of its own.
    Label(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub kind: Kind,
    /// What it's about, in the page's text; `None` for a general comment, about the whole
    /// document.
    pub range: Option<Range<usize>>,
    /// What the reviewer wrote; may be empty for a removal or a label.
    pub body: String,
}

impl Comment {
    /// A comment in words on `range`.
    pub fn on(range: Range<usize>, body: impl Into<String>) -> Comment {
        Comment { kind: Kind::Comment, range: Some(range), body: body.into() }
    }

    /// A general comment.
    pub fn general(body: impl Into<String>) -> Comment {
        Comment { kind: Kind::Comment, range: None, body: body.into() }
    }

    /// The heading word for it: "General", "Comment", "Remove" or the label.
    pub fn title(&self) -> &'static str {
        match (self.kind, &self.range) {
            (_, None) => "General",
            (Kind::Comment, _) => "Comment",
            (Kind::Remove, _) => "Remove",
            (Kind::Label(i), _) => LABELS[i].text,
        }
    }

    /// Where it sorts: general comments first, as written, then the rest in document order.
    fn order(&self) -> (bool, usize, usize) {
        self.range.as_ref().map_or((false, 0, 0), |r| (true, r.start, r.end))
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
            list.iter().rev().find(|(_, r)| r.start < at).or(list.last())
        } else {
            list.iter().find(|(_, r)| r.start > at).or(list.first())
        };
        found.map(|(i, _)| *i)
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    /// The review as Markdown feedback: the general comments, then each comment on the page in
    /// document order with the lines it's on and what it quotes. `None` when there's nothing to
    /// send.
    pub fn feedback(&self, source: &str, page: &Page) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("# Review feedback\n\n");
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
                "I've reviewed this document and have {n} piece{} of feedback:\n\n",
                if n == 1 { "" } else { "s" }
            ));
        }
        for (number, (i, range)) in anchored.into_iter().enumerate() {
            let c = &self.comments[i];
            let ranges = page.source_ranges(range.clone());
            let lines = match lines_of(source, &ranges) {
                Some((a, b)) if a == b => format!("(line {a}) "),
                Some((a, b)) => format!("(lines {a}–{b}) "),
                None => String::new(),
            };
            let text = page.text().get(range).unwrap_or_default().trim();
            out.push_str(&format!("## {}. {lines}", number + 1));
            match c.kind {
                Kind::Comment => out.push_str("Comment\n\n"),
                Kind::Remove => out.push_str("Remove this\n\n"),
                Kind::Label(l) => out.push_str(&format!("{}\n\n", LABELS[l].text)),
            }
            // A quote of more than a line reads better as the Markdown it came from.
            if text.contains('\n') {
                let md = source_of(source, &ranges, true);
                let fence = if md.contains("```") { "````" } else { "```" };
                out.push_str(&format!("{fence}markdown\n{}\n{fence}\n\n", md.trim_end()));
            } else {
                out.push_str(&format!("On: \"{text}\"\n\n"));
            }
            if let Kind::Label(l) = c.kind
                && let Some(tip) = LABELS[l].tip
            {
                out.push_str(&format!("{tip}\n\n"));
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

/// `text` as a Markdown quote, every line of it.
fn quoted(text: &str) -> String {
    text.lines()
        .map(|l| if l.is_empty() { ">".to_owned() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}
