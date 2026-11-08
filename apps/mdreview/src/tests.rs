use super::*;
use rune::{Env, Mods, Runtime};

const SRC: &str = "# Title\n\nSome **bold phrase** and more.\n\n- one\n- two\n\n```rust\nlet x = 1;\n```\n\nA [link][r] here.\n\n[r]: https://example.com\n";

fn app(src: &str) -> App_ {
    App_::new("t.md".into(), src.into(), None)
}

fn press(rt: &mut Runtime<App_>, keys: &str) {
    for c in keys.chars() {
        rt.key(KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }
}

fn key(rt: &mut Runtime<App_>, k: Key) {
    rt.key(KeyEvent {
        key: k,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
}

fn runtime(src: &str) -> Runtime<App_> {
    let mut rt = Runtime::new(
        app(src),
        view,
        (900.0, 900.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
}

/// The id of the comment at `position` in the review's order.
fn id_at(rt: &Runtime<App_>, position: usize) -> CommentId {
    rt.state.review.at_position(position).unwrap()
}

/// Where the current comment is in the review's order.
fn position(rt: &Runtime<App_>) -> Option<usize> {
    rt.state.current().and_then(|id| rt.state.review.position(id))
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
    assert_eq!(
        page.source_ranges(at..at + 11),
        whole.source_ranges(at..at + 11)
    );
    assert_eq!(page.blocks.len(), 5, "the reference definition is no block");
    assert!(
        page.blocks[4].markdown.contains("[r]: https://example.com"),
        "a block keeps the links it uses"
    );
}

#[test]
fn feedback_quotes_and_numbers_in_document_order() {
    let s = app(SRC);
    let mut review = Review::default();
    review.add(Comment {
        kind: Kind::Remove,
        range: Some(range(&s, "let x = 1;")),
        body: String::new(),
    });
    review.add(Comment::on(
        range(&s, "phrase and"),
        "Why bold?\nSay it plainly.",
    ));
    review.add(Comment::on(range(&s, "one\ntwo"), "Verify this."));
    review.add(Comment::on(range(&s, "Title"), "Too short."));
    review.add(Comment::general("Fine overall."));
    review.add(Comment::general("One more thing."));
    assert_eq!(
        review.comments()[1].body, "One more thing.",
        "general comments first, in the order written"
    );
    let out = review
        .feedback(&s.source, &s.page, Some("PLAN.md"))
        .unwrap();
    assert_eq!(
        out,
        "# Review of `PLAN.md`\n\n## General\n\n> Fine overall.\n\n> One more thing.\n\n\
         I've reviewed `PLAN.md` and have 4 pieces of feedback:\n\n\
         ## 1. Comment (line 1)\n\nOn `Title`\n\n> Too short.\n\n\
         ## 2. Comment (line 3, under `Title`)\n\n\
         On `phrase and`, written as:\n\n```markdown\nphrase** and\n```\n\n\
         > Why bold?\n> Say it plainly.\n\n\
         ## 3. Comment (lines 5–6, under `Title`)\n\n```markdown\n- one\n- two\n```\n\n> Verify this.\n\n\
         ## 4. Remove this (line 9, under `Title`)\n\n```markdown\nlet x = 1;\n```\n\n\
         I don't want this in the document.\n"
    );
    assert_eq!(
        Review::default().feedback(&s.source, &s.page, None),
        None,
        "nothing to send"
    );
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
    assert!(
        out.starts_with("# Review\n\nI've reviewed the document and have 4"),
        "{out}"
    );
    assert!(
        out.contains("## 1. Comment (line 1)\n\nOn `Intro line.`\n"),
        "no section before the first heading:\n{out}"
    );
    assert!(
        out.contains("## 2. Comment (line 5, under `Setup`)\n\nOn `Run make \"all\" first.`, written as:\n\n```markdown\nRun `make \"all\"` first.\n```"),
        "quotes inside stay as they are, and the source shows its backticks:\n{out}"
    );
    assert!(
        out.contains("(line 9, under `Detail`)\n\nOn `` tick` ``\n"),
        "a backtick in the quote:\n{out}"
    );
    assert!(
        out.contains("## 4. Comment (line 13, under `Next`)\n\nOn `Plain words.`\n"),
        "{out}"
    );
    assert_eq!(review::inline_code("a ``b`` c"), "```a ``b`` c```");

    // Removing a whole list item quotes its source line, marker and all.
    let s = app("## Later\n\n- Sharing by link\n- Replies\n");
    let mut review = Review::default();
    review.add(Comment {
        kind: Kind::Remove,
        range: Some(range(&s, "Sharing by link")),
        body: String::new(),
    });
    let out = review.feedback(&s.source, &s.page, None).unwrap();
    assert!(out.contains("```markdown\n- Sharing by link\n```"), "{out}");
}

#[test]
fn select_comment_save_edit_and_delete() {
    let mut rt = runtime(SRC);
    // Down to the paragraph, select "Some bold", comment.
    press(&mut rt, "jveec");
    assert!(
        rt.state.composer.is_some(),
        "c on a selection opens the composer"
    );
    press(&mut rt, "Too loud");
    assert_eq!(
        rt.state.draft.text, "Too loud",
        "the field has the keys, not Vim"
    );
    key(&mut rt, Key::Enter);
    assert!(rt.state.composer.is_none());
    let c = &rt.state.review.comments()[0];
    assert_eq!(
        (
            &rt.state.page.text()[c.range.clone().unwrap()],
            c.body.as_str()
        ),
        ("Some bold", "Too loud")
    );
    assert!(
        rt.find(&thread_key(id_at(&rt, 0))).is_some(),
        "the card shows under its block"
    );

    // Enter on the comment edits it; Escape leaves it as it was.
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer, Some(Target::Edit(id_at(&rt, 0))));
    press(&mut rt, "!");
    key(&mut rt, Key::Escape);
    assert!(rt.state.composer.is_none());
    assert_eq!(rt.state.review.comments()[0].body, "Too loud");

    // `ce` too; Shift-Enter is a new line, Enter saves.
    press(&mut rt, "\\e");
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    press(&mut rt, "really");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.review.comments()[0].body, "Too loud\nreally");

    press(&mut rt, "\\x");
    assert!(rt.state.review.comments().is_empty());
}

#[test]
fn several_general_comments_listed_and_stepped_through() {
    let mut rt = runtime(SRC);
    for body in ["First", "Second"] {
        press(&mut rt, "C");
        press(&mut rt, body);
        key(&mut rt, Key::Enter);
    }
    let general: Vec<&str> = rt
        .state
        .review
        .comments()
        .iter()
        .map(|c| c.body.as_str())
        .collect();
    assert_eq!(general, ["First", "Second"]);
    assert!(
        rt.find(&thread_key(id_at(&rt, 0))).is_some() && rt.find(&thread_key(id_at(&rt, 1))).is_some(),
        "both show above the page"
    );

    key(&mut rt, Key::Tab);
    assert!(
        rt.find("list-0").is_some() && rt.find("list-1").is_some(),
        "the list has them"
    );
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
    press(&mut rt, "vd");
    press(&mut rt, "vllc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.review.comments().len(), 3);
    let mut seen = Vec::new();
    for _ in 0..3 {
        press(&mut rt, "]c");
        seen.push(position(&rt).unwrap());
    }
    seen.sort();
    assert_eq!(seen, [0, 1, 2], "]c visits each in turn");
    press(&mut rt, "]c");
    let first = position(&rt);
    press(&mut rt, "[c[c[c");
    assert_eq!(position(&rt), first, "[c comes back round");
}

#[test]
fn removals_and_jumps() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jjVc"); // the list's first line
    press(&mut rt, "check");
    key(&mut rt, Key::Enter);
    press(&mut rt, "gg");
    press(&mut rt, "dd"); // the heading, "Remove this"
    let kinds: Vec<Kind> = rt.state.review.comments().iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [Kind::Remove, Kind::Comment],
        "kept in document order"
    );

    press(&mut rt, "]c");
    assert_eq!(position(&rt), Some(1));
    press(&mut rt, "]c");
    assert_eq!(position(&rt), Some(0), "wraps round");
    press(&mut rt, "[c");
    assert_eq!(position(&rt), Some(1));

    // Moving drops the comment picked; the cursor's own picks the comment under it again.
    press(&mut rt, "gg");
    assert_eq!(position(&rt), Some(0));
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
    let keys = |rt: &Runtime<App_>, group: usize| -> Vec<String> {
        pending_keys(&rt.state).unwrap().1[group]
            .iter()
            .map(|h| h.keys.clone())
            .collect()
    };
    let mut rt = runtime(SRC);
    assert!(rt.find("which-key").is_none());
    press(&mut rt, "v");
    assert!(rt.find("which-key").is_some(), "a selection lists its keys");
    assert_eq!(keys(&rt, 0), ["c", "d"], "the review's operators first");
    assert!(
        keys(&rt, 1).contains(&"gy".to_owned()),
        "then Vim's, Markdown yank included"
    );
    key(&mut rt, Key::Escape);
    assert!(rt.find("which-key").is_none());

    // Prefixes, the review's and Vim's, come from the keymap.
    press(&mut rt, "gy");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "gy");
    press(&mut rt, "i");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "gyi");
    key(&mut rt, Key::Escape);
    press(&mut rt, "]");
    assert_eq!(
        pending_keys(&rt.state).unwrap(),
        ("]".to_owned(), vec![vec![hint("c", "Next comment")]])
    );
    key(&mut rt, Key::Escape);
    press(&mut rt, "\\");
    assert_eq!(keys(&rt, 0), ["x", "?"], "the leader's bindings");
    key(&mut rt, Key::Escape);

    // `c` is an operator: it waits for a motion, `c` takes the line.
    press(&mut rt, "c");
    let (typed, groups) = pending_keys(&rt.state).unwrap();
    assert_eq!(
        (typed.as_str(), groups[0][0].clone()),
        ("c", hint("c", "The line"))
    );
    key(&mut rt, Key::Escape);
    assert!(
        !rt.state.vim.is_pending() && rt.find("which-key").is_none(),
        "Escape cancels it"
    );
    assert!(rt.state.review.comments().is_empty());

    // `\?` lists every key; the next key closes it and does what it does.
    let line = |rt: &Runtime<App_>| {
        rt.state.page.text()[..rt.state.cursor()]
            .matches('\n')
            .count()
    };
    let before = line(&rt);
    press(&mut rt, "\\?");
    assert_eq!(pending_keys(&rt.state).unwrap().0, "keys");
    assert!(rt.find("which-key").is_some());
    press(&mut rt, "j");
    assert!(rt.find("which-key").is_none());
    assert!(line(&rt) > before, "and the key goes on to move the cursor");
}

#[test]
fn comment_and_remove_are_operators() {
    let mut rt = runtime(SRC);
    let quote = |rt: &Runtime<App_>, n: usize| {
        rt.state.page.text()[rt.state.review.comments()[n].range.clone().unwrap()].to_owned()
    };
    // `cw` on the paragraph's first word.
    press(&mut rt, "jcw");
    press(&mut rt, "a");
    key(&mut rt, Key::Enter);
    assert_eq!(quote(&rt, 0), "Some ");
    // `dip`: the list, as a paragraph.
    press(&mut rt, "jdip");
    assert_eq!(quote(&rt, 1), "one\ntwo");
    assert_eq!(rt.state.review.comments()[1].kind, Kind::Remove);
    // `2cc`: two lines.
    press(&mut rt, "gg2cc");
    press(&mut rt, "b");
    key(&mut rt, Key::Enter);
    let comments = rt.state.review.comments();
    let c = comments.iter().find(|c| c.body == "b").unwrap();
    assert!(rt.state.page.text()[c.range.clone().unwrap()].starts_with("Title"));
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
    press(&mut rt, "jVd");
    press(&mut rt, "gg");
    let heading = rt.find("block-0").unwrap();
    let cursor = rt.state.cursor();

    key(&mut rt, Key::Tab);
    assert!(rt.find("comment-list").is_some());
    assert_eq!(
        rt.state.list,
        Some(0),
        "opens on the comment under the cursor"
    );
    assert_eq!(
        rt.find("block-0").unwrap(),
        heading,
        "the page doesn't move"
    );

    // j and k move the list's cursor, not Vim's, and the page marks that comment.
    press(&mut rt, "jj");
    assert_eq!(rt.state.list, Some(2));
    assert_eq!(
        position(&rt),
        Some(2),
        "the page marks the list's comment"
    );
    assert!(rt.find(CURRENT_MARK).is_some(), "keyed, to scroll to");
    press(&mut rt, "j");
    assert_eq!(rt.state.list, Some(2), "stops at the end");
    press(&mut rt, "k");
    assert_eq!(rt.state.list, Some(1));
    assert_eq!(rt.state.cursor(), cursor);

    // Escape closes it where it was; the mark goes back to the cursor's comment.
    key(&mut rt, Key::Escape);
    assert!(rt.find("comment-list").is_none());
    assert_eq!(rt.state.cursor(), cursor);
    assert_eq!(position(&rt), Some(0));

    // Enter goes to the comment in the document.
    key(&mut rt, Key::Tab);
    press(&mut rt, "G");
    key(&mut rt, Key::Enter);
    assert!(rt.state.list.is_none());
    assert_eq!(position(&rt), Some(2));
    assert_eq!(
        rt.state.cursor(),
        rt.state.review.comments()[2].range.clone().unwrap().start
    );

    // A click on a row goes there too.
    key(&mut rt, Key::Tab);
    let (x, y) = rt.find("list-0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.list.is_none());
    assert_eq!(position(&rt), Some(0));
}

#[test]
fn moving_in_the_list_scrolls_the_page_to_the_comment() {
    // A page far taller than the window, with a comment near its end.
    let src: String = (0..80)
        .map(|i| format!("Paragraph number {i} of filler.\n\n"))
        .collect();
    let mut rt = Runtime::new(
        app(&src),
        view,
        (900.0, 400.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    press(&mut rt, "Vc");
    press(&mut rt, "top");
    key(&mut rt, Key::Enter);
    press(&mut rt, "GVc");
    press(&mut rt, "bottom");
    key(&mut rt, Key::Enter);
    press(&mut rt, "gg");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let on_screen = |rt: &Runtime<App_>| {
        rt.find(CURRENT_MARK)
            .is_some_and(|(_, y)| (0.0..400.0).contains(&y))
    };

    key(&mut rt, Key::Tab);
    press(&mut rt, "j");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(position(&rt), Some(1));
    assert!(
        on_screen(&rt),
        "the page scrolled to the last comment: {:?}",
        rt.find(CURRENT_MARK)
    );

    // Escape scrolls back to the cursor, which stayed at the top.
    key(&mut rt, Key::Escape);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(
        rt.find("vim-cursor")
            .is_some_and(|(_, y)| (0.0..400.0).contains(&y))
    );
}

#[test]
fn counts_work_in_visual_mode() {
    // With no labels on the number keys, a count grows the selection as in Vim.
    let mut rt = runtime(SRC);
    press(&mut rt, "jv2e");
    assert!(rt.state.review.comments().is_empty());
    let sel = rt.state.vim.selection(rt.state.page.text()).unwrap();
    assert_eq!(&rt.state.page.text()[sel], "Some bold");
}

#[test]
fn a_review_that_cant_be_written_is_an_error_and_still_reaches_stdout() {
    let dir = std::env::temp_dir().join(format!("mdreview-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = dir.join("review.md");
    let mut stdout = Vec::new();
    assert_eq!(deliver("# Review\n", Some(file.to_str().unwrap()), &mut stdout), Ok(()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Review\n");
    assert!(stdout.is_empty(), "a written file keeps stdout clear");

    let unwritable = dir.join("no-such-dir").join("review.md");
    let mut stdout = Vec::new();
    let result = deliver("# Review\n", Some(unwritable.to_str().unwrap()), &mut stdout);
    assert!(result.is_err(), "a failed write is an error: {result:?}");
    assert_eq!(String::from_utf8(stdout).unwrap(), "# Review\n", "and the review isn't lost");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn click(rt: &mut Runtime<App_>, key_name: &str) {
    let (x, y) = rt.find(key_name).unwrap_or_else(|| panic!("no {key_name} on screen"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

/// Two comments, `first` on the heading and `second` on the paragraph.
fn two_comments() -> Runtime<App_> {
    let mut rt = runtime(SRC);
    press(&mut rt, "cc");
    press(&mut rt, "first");
    key(&mut rt, Key::Enter);
    press(&mut rt, "jcc");
    press(&mut rt, "second");
    key(&mut rt, Key::Enter);
    rt
}

#[test]
fn deleting_another_comment_while_editing_keeps_the_edit_on_its_comment() {
    let mut rt = two_comments();
    press(&mut rt, "\\e");
    assert!(rt.find("composer-field").is_some(), "editing the second comment");
    let delete = format!("delete-{}", id_at(&rt, 0));
    click(&mut rt, &delete);
    press(&mut rt, " edited");
    key(&mut rt, Key::Enter);
    let bodies: Vec<&str> = rt.state.review.comments().iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, ["second edited"]);
}

#[test]
fn deleting_another_comment_while_editing_keeps_the_editor_and_its_keys() {
    let mut rt = two_comments();
    press(&mut rt, "\\e");
    let delete = format!("delete-{}", id_at(&rt, 0));
    click(&mut rt, &delete);
    assert!(rt.find("composer-field").is_some(), "the editor stays on screen");
    key(&mut rt, Key::Escape);
    assert!(rt.find("composer-field").is_none(), "Escape still closes it");
    let before = rt.state.cursor();
    press(&mut rt, "gg");
    assert_ne!(rt.state.cursor(), before, "and keys reach Vim again");
}


