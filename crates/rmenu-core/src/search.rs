//! Fuzzy-ranked matching on nucleo-matcher.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str, Utf32String};
use unicode_segmentation::UnicodeSegmentation;

use crate::item::MenuItem;

const CHUNK: usize = 8_192;
const MATCHER_CONFIG: Config = Config::DEFAULT;

fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)
}

/// Prepared search text for a list of rows. Cloning is cheap (shared; append makes a new
/// generation), so a host can search a snapshot on another thread while input keeps arriving.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    // Chunked so an append while a snapshot is alive copies at most one chunk, not every row.
    chunks: Arc<Vec<Arc<Vec<Utf32String>>>>,
    len: usize,
}

/// Byte ranges of `text` the query matched, merged and in order.
fn highlights(query: &str, text: &str) -> Vec<Range<usize>> {
    let pattern = pattern(query);
    if pattern.atoms.is_empty() { return Vec::new(); }
    let mut matcher = Matcher::new(MATCHER_CONFIG);
    let mut buf = Vec::new();
    let haystack = Utf32Str::new(text, &mut buf);
    let mut hits = Vec::new();
    // Each token matches on its own here, so a row whose title holds only some of the
    // query's tokens still shows which ones.
    for atom in &pattern.atoms {
        let mut found = Vec::new();
        if atom.indices(haystack, &mut matcher, &mut found).is_some() { hits.extend(found); }
    }
    let graphemes: Vec<Range<usize>> =
        text.grapheme_indices(true).map(|(start, g)| start..start + g.len()).collect();
    // `Utf32Str::new` keeps a text whose graphemes all start with ASCII as its bytes, so a hit
    // is a byte offset there. Otherwise nucleo keeps one char per grapheme and a hit counts graphemes.
    let mut marked: Vec<usize> = hits.into_iter().map(|hit| match haystack {
        Utf32Str::Ascii(_) => graphemes.partition_point(|g| g.end <= hit as usize),
        Utf32Str::Unicode(_) => hit as usize,
    }).collect();
    marked.sort_unstable();
    marked.dedup();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for grapheme in marked {
        let range = graphemes[grapheme].clone();
        match ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => ranges.push(range),
        }
    }
    ranges
}

/// Byte ranges to mark in a row's title and in its subtitle. Without a `search` override the
/// query runs on the same title and subtitle text the row was searched by, so a token spanning
/// both fields marks both. A `search` override isn't shown, so each field is matched on its own.
pub fn item_highlights(query: &str, item: &MenuItem) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let subtitle = item.subtitle.as_deref().unwrap_or("");
    if item.search.is_some() { return (highlights(query, item.title()), highlights(query, subtitle)); }
    let ranges = highlights(query, &item.search_text());
    let split = |field: Range<usize>| -> Vec<Range<usize>> {
        ranges.iter().map(|r| r.start.max(field.start)..r.end.min(field.end)).filter(|r| !r.is_empty())
            .map(|r| r.start - field.start..r.end - field.start).collect()
    };
    let title = item.title().len();
    (split(0..title), split(title + 1..title + 1 + subtitle.len()))
}

/// Search was cancelled before finishing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl SearchIndex {
    pub fn new(items: &[MenuItem]) -> Self {
        let mut index = SearchIndex::default();
        index.append(items);
        index
    }

    pub fn append(&mut self, items: &[MenuItem]) {
        let chunks = Arc::make_mut(&mut self.chunks);
        let mut pending = items;
        while !pending.is_empty() {
            if chunks.last().is_none_or(|c| c.len() == CHUNK) {
                chunks.push(Arc::new(Vec::with_capacity(CHUNK.min(pending.len()))));
            }
            let chunk = Arc::make_mut(chunks.last_mut().expect("pushed above"));
            let (taken, rest) = pending.split_at((CHUNK - chunk.len()).min(pending.len()));
            chunk.extend(taken.iter().map(|i| Utf32String::from(i.search_text())));
            self.len += taken.len();
            pending = rest;
        }
    }

    /// Offsets of matching rows, with nucleo's pattern syntax and smart case.
    /// Sorted: summed score, then shorter text, then input order. Unsorted: input order.
    /// Blocks are matched on all cores; `cancelled` is polled on the calling thread every 0.2 ms.
    pub fn search(&self, query: &str, sorted: bool, cancelled: &dyn Fn() -> bool) -> Result<Vec<u32>, Cancelled> {
        Ok(self.ranked(query, sorted, cancelled)?.into_iter().map(|m| m.offset).collect())
    }

    fn ranked(&self, query: &str, sorted: bool, cancelled: &dyn Fn() -> bool) -> Result<Vec<Ranked>, Cancelled> {
        if cancelled() { return Err(Cancelled); }
        let pattern = pattern(query);
        if pattern.atoms.is_empty() {
            return Ok((0..self.len as u32).map(|offset| Ranked { score: 0, len: 0, offset }).collect());
        }
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(self.chunks.len());
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let parts: Vec<Vec<(usize, Vec<Ranked>)>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads).map(|_| scope.spawn(|| self.match_chunks(&pattern, &next, &stop))).collect();
            // `cancelled` need not be Sync, so only this thread polls it and forwards a stop.
            while !workers.iter().all(|w| w.is_finished()) {
                if cancelled() { stop.store(true, Ordering::Relaxed); }
                std::thread::sleep(Duration::from_micros(200));
            }
            workers.into_iter().map(|w| w.join().expect("search worker panicked")).collect()
        });
        if stop.load(Ordering::Relaxed) || cancelled() { return Err(Cancelled); }
        let mut chunks: Vec<(usize, Vec<Ranked>)> = parts.into_iter().flatten().collect();
        chunks.sort_unstable_by_key(|(index, _)| *index);
        let mut out: Vec<Ranked> = chunks.into_iter().flat_map(|(_, rows)| rows).collect();
        if sorted {
            out.sort_unstable_by_key(|m| (std::cmp::Reverse(m.score), m.len, m.offset));
        }
        if cancelled() { return Err(Cancelled); }
        Ok(out)
    }

    fn match_chunks(&self, pattern: &Pattern, next: &AtomicUsize, stop: &AtomicBool) -> Vec<(usize, Vec<Ranked>)> {
        let mut matcher = Matcher::new(MATCHER_CONFIG);
        let mut out = Vec::new();
        while let Some(index) = self.claim(next, stop) {
            match self.match_chunk(index, pattern, &mut matcher, &|| stop.load(Ordering::Relaxed)) {
                Some(rows) => out.push((index, rows)),
                None => break,
            }
        }
        out
    }

    fn claim(&self, next: &AtomicUsize, stop: &AtomicBool) -> Option<usize> {
        let index = next.fetch_add(1, Ordering::Relaxed);
        (index < self.chunks.len() && !stop.load(Ordering::Relaxed)).then_some(index)
    }

    /// None when stopped part-way.
    fn match_chunk(&self, index: usize, pattern: &Pattern, matcher: &mut Matcher,
                   stopped: &dyn Fn() -> bool) -> Option<Vec<Ranked>> {
        let base = index * CHUNK;
        let mut out = Vec::new();
        for (local, text) in self.chunks[index].iter().enumerate() {
            if local % 256 == 0 && stopped() { return None; }
            if let Some(score) = pattern.score(text.slice(..), matcher) {
                out.push(Ranked { score, len: text.len() as u32, offset: (base + local) as u32 });
            }
        }
        Some(out)
    }

    #[cfg(test)]
    fn scored_matches(&self, query: &str) -> Vec<(u32, u32)> {
        self.ranked(query, true, &|| false).unwrap().into_iter().map(|m| (m.offset, m.score)).collect()
    }
}

struct Ranked {
    score: u32,
    len: u32,
    offset: u32,
}

#[cfg(test)]
mod tests;
