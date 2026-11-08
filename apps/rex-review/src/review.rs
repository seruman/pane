//! The review: comments anchored to ranges of the page's text, one general comment, and the
//! feedback they make, as Markdown for whoever wrote the document (an agent, most often).

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
    /// What it's about, in the page's text.
    pub range: Range<usize>,
    /// What the reviewer wrote; may be empty for a removal or a label.
    pub body: String,
}

impl Comment {
    /// The heading word for it: "Comment", "Remove" or the label.
    pub fn title(&self) -> &'static str {
        match self.kind {
            Kind::Comment => "Comment",
            Kind::Remove => "Remove",
            Kind::Label(i) => LABELS[i].text,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Review {
    /// In document order.
    pub comments: Vec<Comment>,
    pub general: String,
}

impl Review {
    /// Adds `comment` in document order and returns its index.
    pub fn add(&mut self, comment: Comment) -> usize {
        let at = self
            .comments
            .partition_point(|c| (c.range.start, c.range.end) <= (comment.range.start, comment.range.end));
        self.comments.insert(at, comment);
        at
    }

    /// The comment `at` (a byte of the page's text) falls in; the innermost when they nest.
    pub fn at(&self, at: usize) -> Option<usize> {
        self.comments
            .iter()
            .enumerate()
            .filter(|(_, c)| c.range.contains(&at) || (c.range.is_empty() && c.range.start == at))
            .min_by_key(|(_, c)| c.range.len())
            .map(|(i, _)| i)
    }

    /// The first comment starting after byte `at`, or before it with `back`, wrapping round.
    pub fn next(&self, at: usize, back: bool) -> Option<usize> {
        let n = self.comments.len();
        if n == 0 {
            return None;
        }
        if back {
            Some(self.comments.iter().rposition(|c| c.range.start < at).unwrap_or(n - 1))
        } else {
            Some(self.comments.iter().position(|c| c.range.start > at).unwrap_or(0))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.general.trim().is_empty()
    }

    /// The review as Markdown feedback: the general comment, then each comment in document
    /// order with the lines it's on and what it quotes. `None` when there's nothing to send.
    pub fn feedback(&self, source: &str, page: &Page) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("# Review feedback\n\n");
        let general = self.general.trim();
        if !general.is_empty() {
            out.push_str("## General\n\n");
            out.push_str(&quoted(general));
            out.push_str("\n\n");
        }
        let n = self.comments.len();
        if n > 0 {
            out.push_str(&format!(
                "I've reviewed this document and have {n} piece{} of feedback:\n\n",
                if n == 1 { "" } else { "s" }
            ));
        }
        for (i, c) in self.comments.iter().enumerate() {
            let ranges = page.source_ranges(c.range.clone());
            let lines = match lines_of(source, &ranges) {
                Some((a, b)) if a == b => format!("(line {a}) "),
                Some((a, b)) => format!("(lines {a}–{b}) "),
                None => String::new(),
            };
            let text = page.text().get(c.range.clone()).unwrap_or_default().trim();
            out.push_str(&format!("## {}. {lines}", i + 1));
            match c.kind {
                Kind::Comment => out.push_str("Comment\n\n"),
                Kind::Remove => out.push_str("Remove this\n\n"),
                Kind::Label(l) => out.push_str(&format!("{}\n\n", LABELS[l].text)),
            }
            // A quote of more than a line reads better as the Markdown it came from.
            if text.contains('\n') {
                let fence = if source_of(source, &ranges, true).contains("```") { "````" } else { "```" };
                out.push_str(&format!("{fence}markdown\n{}\n{fence}\n\n", source_of(source, &ranges, true).trim_end()));
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
