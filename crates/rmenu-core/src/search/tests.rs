use std::cell::Cell;

use super::*;

fn index(titles: &[&str]) -> SearchIndex {
    let items: Vec<MenuItem> = titles.iter().enumerate().map(|(i, t)| MenuItem::new(i.to_string(), *t)).collect();
    SearchIndex::new(&items)
}

fn search(index: &SearchIndex, query: &str) -> Vec<u32> { index.search(query, true, &|| false).unwrap() }

fn unsorted(index: &SearchIndex, query: &str) -> Vec<u32> { index.search(query, false, &|| false).unwrap() }

#[test]
fn better_matches_rank_first_and_ties_keep_input_order() {
    let rows = index(&["xfoobar", "src/foo", "foo", "f---o---o", "foo"]);
    assert_eq!(search(&rows, "foo"), [2, 4, 1, 3, 0], "a boundary-started gapped match beats a mid-word run");
    assert_eq!(search(&index(&["a--b--c", "abc"]), "abc"), [1, 0]);
    assert_eq!(search(&index(&["xfoo", "f----/foo"]), "foo"), [1, 0], "a word-boundary match beats a mid-word one");
}

#[test]
fn equal_scores_prefer_the_shorter_text() {
    let rows = index(&["foo/bar-long", "foo/bar"]);
    let scores = rows.scored_matches("foo");
    assert_eq!(scores[0].1, scores[1].1, "fixture: both rows score the same");
    assert_eq!(search(&rows, "foo"), [1, 0]);
}

#[test]
fn token_scores_are_summed() {
    let rows = index(&["foo bar baz"]);
    let one = |q| rows.scored_matches(q)[0].1;
    assert_eq!(one("fbb baz"), one("fbb") + one("baz"));
}

#[test]
fn tokens_are_anded_in_any_order() {
    let rows = index(&[
        "Sources/MenuCore/SearchIndex.swift",
        "Sources/MenuCore/RecordDecoder.swift",
        "Tests/MenuCoreTests/SearchTests.swift",
        "README.md",
    ]);
    assert_eq!(unsorted(&rows, "swift mcore srch"), [0, 2]);
    assert!(search(&rows, "swift impossible").is_empty());
    assert!(search(&rows, "ssssssssssssssss").is_empty());
}

#[test]
fn smart_case_and_diacritics() {
    let rows = index(&["CAFÉ", "Cafe\u{301}", "École/東京/🧑‍💻.swift", "àá", "cafe"]);
    assert_eq!(unsorted(&rows, "cafe"), [0, 1, 4]);
    assert_eq!(unsorted(&rows, "CAFE"), [0], "smart case: an uppercase query matches case");
    assert_eq!(unsorted(&rows, "ecole 京 🧑‍💻"), [2]);
    assert_eq!(unsorted(&rows, "aa"), [3]);
    assert!(search(&index(&["a"]), "aa").is_empty());
}

#[test]
fn a_query_with_diacritics_matches_them_exactly() {
    let rows = index(&["CAFÉ", "cafe"]);
    assert_eq!(unsorted(&rows, "café"), [0]);
    assert_eq!(unsorted(&rows, "CAFÉ"), [0]);
}

#[test]
fn scalars_match_whole() {
    // These CJK characters contain bytes that could falsely match 丁 if a matcher
    // allowed gaps inside a multibyte scalar.
    let cjk = index(&["一䁁", "丁"]);
    assert_eq!(unsorted(&cjk, "丁"), [1]);
    assert_eq!(search(&cjk, "丁"), [1]);
}

#[test]
fn empty_queries_append_and_snapshots() {
    let mut rows = index(&["zfoo", "foo", "foo"]);
    let snapshot = rows.clone();
    rows.append(&[MenuItem::new("3", "foo")]);
    assert_eq!(rows.len(), 4);
    assert_eq!(snapshot.len(), 3);
    assert_eq!(search(&rows, ""), [0, 1, 2, 3]);
    assert_eq!(search(&rows, "   "), [0, 1, 2, 3]);
    assert_eq!(unsorted(&rows, "foo"), [0, 1, 2, 3]);
    assert_eq!(search(&snapshot, "foo"), [1, 2, 0]);
    assert!(search(&SearchIndex::default(), "anything").is_empty());
    assert!(search(&SearchIndex::default(), "").is_empty());
    assert!(SearchIndex::default().is_empty());
}

#[test]
fn appends_past_a_chunk_leave_snapshots_alone() {
    let titles: Vec<String> = (0..CHUNK + 10).map(|i| format!("row-{i}")).collect();
    let items: Vec<MenuItem> = titles.iter().map(|t| MenuItem::new(t.clone(), t.clone())).collect();
    let mut rows = SearchIndex::new(&items[..CHUNK - 5]);
    let snapshot = rows.clone();
    rows.append(&items[CHUNK - 5..]);
    assert_eq!(rows.len(), CHUNK + 10);
    assert_eq!(search(&rows, "").len(), CHUNK + 10);
    assert_eq!(unsorted(&rows, &format!("row-{}", CHUNK + 9)), [(CHUNK + 9) as u32]);
    assert_eq!(snapshot.len(), CHUNK - 5);
    assert_eq!(search(&snapshot, "").len(), CHUNK - 5);
    assert!(unsorted(&snapshot, &format!("row-{}", CHUNK + 9)).is_empty());
}

#[test]
fn uses_menu_item_search_text() {
    let mut a = MenuItem::new("a", "Visible");
    a.subtitle = Some("Searchable".into());
    let mut b = MenuItem::new("b", "Visible");
    b.subtitle = Some("Hidden".into());
    b.search = Some("Override".into());
    let mut c = MenuItem::new("c", "Other").with_value("Secret");
    c.detail = Some("Private".into());
    let rows = SearchIndex::new(&[a, b, c]);
    assert_eq!(search(&rows, "visible"), [0]);
    assert_eq!(search(&rows, "searchable"), [0]);
    assert_eq!(search(&rows, "override"), [1]);
    assert!(search(&rows, "secret").is_empty());
    assert!(search(&rows, "private").is_empty());
}

#[test]
fn cancellation_including_the_empty_fast_path() {
    let rows = index(&["Sources/MenuCore/SearchIndex.swift"; 2_000]);
    for query in ["", "srch", "not-present"] {
        for sorted in [true, false] {
            assert_eq!(rows.search(query, sorted, &|| true), Err(Cancelled), "{query:?} sorted {sorted}");
        }
    }
}

#[test]
fn cancellation_mid_scan_stops_early() {
    let rows = index(&["Sources/MenuCore/SearchIndex.swift"; 50_000]);
    let polls = Cell::new(0);
    let result = rows.search("swift", true, &|| { polls.set(polls.get() + 1); polls.get() > 3 });
    assert_eq!(result, Err(Cancelled));
    assert!(polls.get() >= 4);
}

#[test]
fn large_indexes_keep_input_order_for_ties() {
    let titles: Vec<String> = (0..50_000).map(|i| format!("src/module{}/file.swift", i % 13)).collect();
    let items: Vec<MenuItem> = titles.iter().enumerate().map(|(i, t)| MenuItem::new(i.to_string(), t.clone())).collect();
    let rows = SearchIndex::new(&items);
    let ranked = search(&rows, "module1 swift");
    let module1: Vec<u32> = (0..50_000).filter(|i| i % 13 == 1).collect();
    assert_eq!(ranked.len(), (0..50_000).filter(|i| i % 13 == 1 || i % 13 >= 10).count());
    assert_eq!(&ranked[..module1.len()], &module1[..], "module1 ranks first; its ties keep input order");
    let mut sorted = ranked.clone();
    sorted.sort_unstable();
    assert_eq!(unsorted(&rows, "module1 swift"), sorted);
}

#[test]
fn sorted_and_unsorted_have_the_same_matches() {
    let titles: Vec<String> = (0..600).map(|i| format!("src/module{}/file{i}.swift", i % 13)).collect();
    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let rows = index(&refs);
    for query in ["sf", "module1 swift", "file55", "src", "zz", "111", " / . "] {
        let mut ranked = search(&rows, query);
        ranked.sort_unstable();
        assert_eq!(ranked, unsorted(&rows, query), "{query:?}");
    }
}

#[test]
fn highlights_mark_the_matched_characters_as_byte_ranges() {
    assert_eq!(super::highlights("rune", "github.com/seruman/rune"), vec![19..23]);
    assert_eq!(super::highlights("", "anything"), vec![]);
    assert_eq!(super::highlights("zzz", "abc"), vec![]);
    let ranges = super::highlights("cafe", "Café/x");
    assert_eq!(ranges, vec![0..5], "a folded multi-byte character is one whole range");
    let both = super::highlights("ser rune", "github.com/seruman/rune");
    assert!(both.iter().any(|r| r.start >= 19), "every token is highlighted: {both:?}");
    assert!(both.len() >= 2);
}

#[test]
fn highlights_cover_whole_graphemes() {
    let marked = |query, text: &'static str| -> Vec<&str> {
        super::highlights(query, text).into_iter().map(|r| &text[r]).collect()
    };
    assert_eq!(marked("b", "a\u{301}bc"), ["b"]);
    assert_eq!(marked("x", "e\u{301}e\u{301}x"), ["x"]);
    assert_eq!(marked("rune", "🧑\u{200d}💻 rune"), ["rune"]);
    assert_eq!(marked("e", "e\u{301}x"), ["e\u{301}"], "a combining mark is marked with its base");
    assert_eq!(marked("b", "a\r\nb"), ["b"]);
}

#[test]
fn item_highlights_follow_the_text_the_row_matched() {
    let marked = |query, item: &MenuItem| {
        let (title, subtitle) = super::item_highlights(query, item);
        let sub = item.subtitle.as_deref().unwrap_or("");
        (title.into_iter().map(|r| item.title[r].to_owned()).collect::<Vec<_>>(),
         subtitle.into_iter().map(|r| sub[r].to_owned()).collect::<Vec<_>>())
    };
    let mut item = MenuItem::new("1", "a");
    item.subtitle = Some("b".into());
    assert_eq!(search(&SearchIndex::new(std::slice::from_ref(&item)), "ab"), [0], "fixture: the row matches");
    assert_eq!(marked("ab", &item), (vec!["a".into()], vec!["b".into()]), "a token spanning both fields marks both");
    let mut item = MenuItem::new("1", "src/rune");
    item.subtitle = Some("seruman".into());
    assert_eq!(marked("rune ser", &item), (vec!["rune".into()], vec!["ser".into()]));
    item.search = Some("Override".into());
    assert_eq!(marked("rune", &item), (vec!["rune".into()], vec![]),
               "a search override can't map to shown text, so each field is marked on its own");
}
