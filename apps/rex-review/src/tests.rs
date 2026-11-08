use super::*;
use rune::{Env, Mods, Runtime};

const SRC: &str = "# Title\n\nSome **bold phrase** and more.\n\n- one\n- two\n\n```rust\nlet x = 1;\n```\n\nA [link][r] here.\n\n[r]: https://example.com\n";

fn app(src: &str) -> App_ {
    App_::new("t.md".into(), src.into(), None)
}

fn press(rt: &mut Runtime<App_>, keys: &str) {
    for c in keys.chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
}

fn key(rt: &mut Runtime<App_>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn runtime(src: &str) -> Runtime<App_> {
    let mut rt = Runtime::new(app(src), view, (900.0, 900.0), Env { system_dark: true, scale: 1.0 });
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
}

fn range(s: &App_, needle: &str) -> Range<usize> {
    let at = s.page.text().find(needle).unwrap();
    at..at + needle.len()
}

#[test]
fn the_page_text_is_the_whole_documents() {
    // Cut into blocks, the page reads as one RenderedText of the whole source reads.
    let page = Page::new(SRC);
    let whole = rune_kit::markdown::RenderedText::new(SRC);
    assert_eq!(page.text(), whole.text());
    let at = page.text().find("bold phrase").unwrap();
    assert_eq!(page.source_ranges(at..at + 11), whole.source_ranges(at..at + 11));
    assert_eq!(page.blocks.len(), 5, "the reference definition is no block");
    assert!(page.blocks[4].markdown.contains("[r]: https://example.com"), "a block keeps the links it uses");
}

#[test]
fn feedback_quotes_and_numbers_in_document_order() {
    let s = app(SRC);
    let mut review = Review::default();
    review.add(Comment { kind: Kind::Remove, range: Some(range(&s, "let x = 1;")), body: String::new() });
    review.add(Comment::on(range(&s, "phrase and"), "Why bold?\nSay it plainly."));
    review.add(Comment { kind: Kind::Label(1), range: Some(range(&s, "one\ntwo")), body: String::new() });
    review.add(Comment::on(range(&s, "Title"), "Too short."));
    review.add(Comment::general("Fine overall."));
    review.add(Comment::general("One more thing."));
    assert_eq!(review.comments[1].body, "One more thing.", "general comments first, in the order written");
    let out = review.feedback(&s.source, &s.page, Some("PLAN.md")).unwrap();
    assert_eq!(
        out,
        "# Review of `PLAN.md`\n\n## General\n\n> Fine overall.\n\n> One more thing.\n\n\
         I've reviewed `PLAN.md` and have 4 pieces of feedback:\n\n\
         ## 1. Comment (line 1)\n\nOn `Title`\n\n> Too short.\n\n\
         ## 2. Comment (line 3, under `Title`)\n\n\
         On `phrase and`, written as:\n\n```markdown\nphrase** and\n```\n\n\
         > Why bold?\n> Say it plainly.\n\n\
         ## 3. Verify this (lines 5–6, under `Title`)\n\n```markdown\n- one\n- two\n```\n\n\
         This seems like an assumption. Verify it by reading the actual code before going on.\n\n\
         ## 4. Remove this (line 9, under `Title`)\n\n```markdown\nlet x = 1;\n```\n\n\
         I don't want this in the document.\n"
    );
    assert_eq!(Review::default().feedback(&s.source, &s.page, None), None, "nothing to send");
}

#[test]
fn feedback_names_the_nearest_section_and_quotes_safely() {
    let src = "Intro line.\n\n## Setup\n\nRun `make \"all\"` first.\n\n### Detail\n\nA ``tick` `` here.\n\n## Next\n\nPlain words.\n";
    let s = app(src);
    let mut review = Review::default();
    review.add(Comment::on(range(&s, "Intro line."), "a"));
    review.add(Comment::on(range(&s, "Run make \"all\" first."), "b"));
    review.add(Comment::on(range(&s, "tick`"), "c"));
    review.add(Comment::on(range(&s, "Plain words."), "d"));
    let out = review.feedback(&s.source, &s.page, None).unwrap();
    assert!(out.starts_with("# Review\n\nI've reviewed the document and have 4"), "{out}");
    assert!(out.contains("## 1. Comment (line 1)\n\nOn `Intro line.`\n"), "no section before the first heading:\n{out}");
    assert!(
        out.contains("## 2. Comment (line 5, under `Setup`)\n\nOn `Run make \"all\" first.`, written as:\n\n```markdown\nRun `make \"all\"` first.\n```"),
        "quotes inside stay as they are, and the source shows its backticks:\n{out}"
    );
    assert!(out.contains("(line 9, under `Detail`)\n\nOn `` tick` ``\n"), "a backtick in the quote:\n{out}");
    assert!(out.contains("## 4. Comment (line 13, under `Next`)\n\nOn `Plain words.`\n"), "{out}");
    assert_eq!(review::inline_code("a ``b`` c"), "```a ``b`` c```");

    // Removing a whole list item quotes its source line, marker and all.
    let s = app("## Later\n\n- Sharing by link\n- Replies\n");
    let mut review = Review::default();
    review.add(Comment { kind: Kind::Remove, range: Some(range(&s, "Sharing by link")), body: String::new() });
    let out = review.feedback(&s.source, &s.page, None).unwrap();
    assert!(out.contains("```markdown\n- Sharing by link\n```"), "{out}");
}

#[test]
fn select_comment_save_edit_and_delete() {
    let mut rt = runtime(SRC);
    // Down to the paragraph, select "Some bold", comment.
    press(&mut rt, "jveec");
    assert!(rt.state.composer.is_some(), "c on a selection opens the composer");
    press(&mut rt, "Too loud");
    assert_eq!(rt.state.draft.text, "Too loud", "the field has the keys, not Vim");
    key(&mut rt, Key::Enter);
    assert!(rt.state.composer.is_none());
    let c = &rt.state.review.comments[0];
    assert_eq!((&rt.state.page.text()[c.range.clone().unwrap()], c.body.as_str()), ("Some bold", "Too loud"));
    assert!(rt.find(&thread_key(0)).is_some(), "the card shows under its block");

    // Enter on the comment edits it; Escape leaves it as it was.
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer, Some(Target::Edit(0)));
    press(&mut rt, "!");
    key(&mut rt, Key::Escape);
    assert!(rt.state.composer.is_none());
    assert_eq!(rt.state.review.comments[0].body, "Too loud");

    // `ce` too; Shift-Enter is a new line, Enter saves.
    press(&mut rt, "ce");
    rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    press(&mut rt, "really");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.review.comments[0].body, "Too loud\nreally");

    press(&mut rt, "dc");
    assert!(rt.state.review.comments.is_empty());
}

#[test]
fn several_general_comments_listed_and_stepped_through() {
    let mut rt = runtime(SRC);
    for body in ["First", "Second"] {
        press(&mut rt, "C");
        press(&mut rt, body);
        key(&mut rt, Key::Enter);
    }
    let general: Vec<&str> = rt.state.review.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(general, ["First", "Second"]);
    assert!(rt.find(&thread_key(0)).is_some() && rt.find(&thread_key(1)).is_some(), "both show above the page");

    key(&mut rt, Key::Tab);
    assert!(rt.find("list-0").is_some() && rt.find("list-1").is_some(), "the list has them");
    key(&mut rt, Key::Escape);

    // ]c reaches a general comment, and Enter edits it.
    press(&mut rt, "]c");
    let at = rt.state.selected.unwrap();
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer, Some(Target::Edit(at)));
}

#[test]
fn stepping_visits_comments_that_start_together() {
    // The case from the screenshot: three comments starting at the same place.
    let mut rt = runtime(SRC);
    press(&mut rt, "jVd");
    press(&mut rt, "v1");
    press(&mut rt, "vll2");
    assert_eq!(rt.state.review.comments.len(), 3);
    let mut seen = Vec::new();
    for _ in 0..3 {
        press(&mut rt, "]c");
        seen.push(rt.state.selected.unwrap());
    }
    seen.sort();
    assert_eq!(seen, [0, 1, 2], "]c visits each in turn");
    press(&mut rt, "]c");
    let first = rt.state.selected;
    press(&mut rt, "[c[c[c");
    assert_eq!(rt.state.selected, first, "[c comes back round");
}

#[test]
fn labels_removals_and_jumps() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jjV2"); // the list's first line, "Verify this"
    press(&mut rt, "gg");
    press(&mut rt, "dd"); // the heading, "Remove this"
    let kinds: Vec<Kind> = rt.state.review.comments.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [Kind::Remove, Kind::Label(1)], "kept in document order");

    press(&mut rt, "]c");
    assert_eq!(rt.state.current(), Some(1));
    press(&mut rt, "]c");
    assert_eq!(rt.state.current(), Some(0), "wraps round");
    press(&mut rt, "[c");
    assert_eq!(rt.state.current(), Some(1));

    // Moving drops the comment picked; the cursor's own picks the comment under it again.
    press(&mut rt, "gg");
    assert_eq!(rt.state.current(), Some(0));
}

#[test]
fn quitting() {
    let mut rt = runtime(SRC);
    // :q with nothing written quits.
    press(&mut rt, ":q");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));

    // With a comment, :q refuses; :q! quits without the review.
    let mut rt = runtime(SRC);
    press(&mut rt, "Vc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    press(&mut rt, ":q");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.shell_request, None);
    assert!(rt.state.note.as_deref().unwrap_or_default().contains(":wq"));
    press(&mut rt, ":q!");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
    assert_eq!(rt.state.feedback(), None);

    // :wq sends it; ZZ is the same.
    let mut rt = runtime(SRC);
    press(&mut rt, "Vc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    press(&mut rt, "ZZ");
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
    assert!(rt.state.feedback().is_some());
}

#[test]
fn which_key_shows_what_can_follow() {
    let keys = |rt: &Runtime<App_>, group: usize| -> Vec<&'static str> {
        pending_keys(&rt.state).unwrap().1[group].iter().map(|h| h.keys).collect()
    };
    let mut rt = runtime(SRC);
    assert!(rt.find("which-key").is_none());
    press(&mut rt, "v");
    assert!(rt.find("which-key").is_some(), "a selection lists its keys");
    assert_eq!(keys(&rt, 0), ["c", "d"], "the review's first");
    assert_eq!(keys(&rt, 1).len(), LABELS.len(), "then the labels");
    assert!(keys(&rt, 2).contains(&"gy"), "then Vim's, Markdown yank included");
    key(&mut rt, Key::Escape);
    assert!(rt.find("which-key").is_none());

    // Vim's own prefixes come from Vim.
    press(&mut rt, "gy");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "gy");
    press(&mut rt, "i");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "gyi");
    key(&mut rt, Key::Escape);

    press(&mut rt, "d");
    let (prefix, groups) = pending_keys(&rt.state).unwrap();
    assert_eq!((prefix.as_str(), groups[0].len()), ("d", 1), "dc only shows on a comment");
    key(&mut rt, Key::Escape);
    assert!(rt.state.prefix.is_none() && rt.find("which-key").is_none(), "Escape cancels a prefix");
    assert!(rt.state.review.comments.is_empty());

    press(&mut rt, "Vc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    press(&mut rt, "c");
    assert_eq!(keys(&rt, 0), ["c", "e"]);
    key(&mut rt, Key::Escape);

    // Space lists every key; the next key closes it.
    let line = |rt: &Runtime<App_>| rt.state.page.text()[..rt.state.cursor()].matches('\n').count();
    let before = line(&rt);
    press(&mut rt, " ");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "space");
    assert!(rt.find("which-key").is_some());
    press(&mut rt, "j");
    assert!(rt.find("which-key").is_none());
    assert!(line(&rt) > before, "and the key goes on to move the cursor");
}

#[test]
fn the_comment_list_floats_and_jumps() {
    let mut rt = runtime(SRC);
    // Three comments: on the heading, the paragraph and the list.
    press(&mut rt, "Vc");
    press(&mut rt, "one");
    key(&mut rt, Key::Enter);
    press(&mut rt, "jVc");
    press(&mut rt, "two");
    key(&mut rt, Key::Enter);
    press(&mut rt, "jV3");
    press(&mut rt, "gg");
    let heading = rt.find("block-0").unwrap();
    let cursor = rt.state.cursor();

    key(&mut rt, Key::Tab);
    assert!(rt.find("comment-list").is_some());
    assert_eq!(rt.state.list, Some(0), "opens on the comment under the cursor");
    assert_eq!(rt.find("block-0").unwrap(), heading, "the page doesn't move");

    // j and k move the list's cursor, not Vim's; the page stays put.
    press(&mut rt, "jj");
    assert_eq!(rt.state.list, Some(2));
    press(&mut rt, "j");
    assert_eq!(rt.state.list, Some(2), "stops at the end");
    press(&mut rt, "k");
    assert_eq!(rt.state.list, Some(1));
    assert_eq!(rt.state.cursor(), cursor);

    // Escape closes it where it was.
    key(&mut rt, Key::Escape);
    assert!(rt.find("comment-list").is_none());
    assert_eq!(rt.state.cursor(), cursor);

    // Enter goes to the comment in the document.
    key(&mut rt, Key::Tab);
    press(&mut rt, "G");
    key(&mut rt, Key::Enter);
    assert!(rt.state.list.is_none());
    assert_eq!(rt.state.current(), Some(2));
    assert_eq!(rt.state.cursor(), rt.state.review.comments[2].range.clone().unwrap().start);

    // A click on a row goes there too.
    key(&mut rt, Key::Tab);
    let (x, y) = rt.find("list-0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.list.is_none());
    assert_eq!(rt.state.current(), Some(0));
}
