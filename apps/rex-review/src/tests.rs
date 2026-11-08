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

fn key(rt: &mut Runtime<App_>, k: Key, mods: Mods) {
    rt.key(KeyEvent { key: k, mods });
    rt.frame(1.0 / 60.0);
}

fn runtime(src: &str) -> Runtime<App_> {
    let mut rt = Runtime::new(app(src), view, (900.0, 900.0), Env { system_dark: true, scale: 1.0 });
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
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
    let text = s.page.text().to_owned();
    let range = |needle: &str| {
        let at = text.find(needle).unwrap();
        at..at + needle.len()
    };
    let mut review = Review::default();
    review.add(Comment { kind: Kind::Remove, range: range("let x = 1;"), body: String::new() });
    review.add(Comment { kind: Kind::Comment, range: range("bold phrase"), body: "Why bold?\nSay it plainly.".into() });
    review.add(Comment { kind: Kind::Label(1), range: range("one\ntwo"), body: String::new() });
    review.general = "Fine overall.".into();
    let out = review.feedback(&s.source, &s.page).unwrap();
    assert_eq!(
        out,
        "# Review feedback\n\n## General\n\n> Fine overall.\n\n\
         I've reviewed this document and have 3 pieces of feedback:\n\n\
         ## 1. (line 3) Comment\n\nOn: \"bold phrase\"\n\n> Why bold?\n> Say it plainly.\n\n\
         ## 2. (lines 5–6) Verify this\n\n```markdown\n- one\n- two\n```\n\n\
         This seems like an assumption. Verify it by reading the actual code before going on.\n\n\
         ## 3. (line 9) Remove this\n\nOn: \"let x = 1;\"\n\nI don't want this in the document.\n"
    );
    assert_eq!(Review::default().feedback(&s.source, &s.page), None, "nothing to send");
}

#[test]
fn select_comment_save_edit_and_delete() {
    let mut rt = runtime(SRC);
    // Down to the paragraph, select "Some bold", comment.
    press(&mut rt, "jveec");
    assert!(rt.state.composer.is_some(), "c on a selection opens the composer");
    press(&mut rt, "Too loud");
    assert_eq!(rt.state.draft.text, "Too loud", "the field has the keys, not Vim");
    key(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.composer.is_none());
    let c = &rt.state.review.comments[0];
    assert_eq!((&rt.state.page.text()[c.range.clone()], c.body.as_str()), ("Some bold", "Too loud"));
    assert!(rt.find(&thread_key(0)).is_some(), "the thread shows under its block");

    // Vim has the keys back: `ce` on the comment edits it, Escape leaves it as it was.
    press(&mut rt, "ce");
    assert_eq!(rt.state.composer.as_ref().map(|c| c.target.clone()), Some(Target::Edit(0)));
    press(&mut rt, "!");
    key(&mut rt, Key::Escape, Mods::default());
    assert!(rt.state.composer.is_none());
    assert_eq!(rt.state.review.comments[0].body, "Too loud");

    // Shift-Enter is a new line; Enter saves.
    press(&mut rt, "ce");
    key(&mut rt, Key::Enter, Mods { shift: true, ..Mods::default() });
    press(&mut rt, "really");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.review.comments[0].body, "Too loud\nreally");

    press(&mut rt, "dc");
    assert!(rt.state.review.comments.is_empty());
}

#[test]
fn labels_removals_general_and_jumps() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jjV2"); // the list's first line, "Verify this"
    press(&mut rt, "gg");
    press(&mut rt, "dd"); // the heading, "Remove this"
    let kinds: Vec<Kind> = rt.state.review.comments.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [Kind::Remove, Kind::Label(1)], "kept in document order");
    assert_eq!(&rt.state.page.text()[rt.state.review.comments[0].range.clone()], "Title");

    press(&mut rt, "]c");
    assert_eq!(rt.state.here(), Some(1));
    press(&mut rt, "]c");
    assert_eq!(rt.state.here(), Some(0), "wraps round");
    press(&mut rt, "[c");
    assert_eq!(rt.state.here(), Some(1));

    press(&mut rt, "C");
    press(&mut rt, "Ship it");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.review.general, "Ship it");
    assert!(rt.find("general").is_some());

    key(&mut rt, Key::Tab, Mods::default());
    assert!(rt.state.panel && rt.find("panel-1").is_some());
}

#[test]
fn zq_quits_without_the_review() {
    let mut rt = runtime(SRC);
    press(&mut rt, "Vc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter, Mods::default());
    assert!(rt.state.feedback().is_some());
    press(&mut rt, "ZQ");
    assert!(rt.state.discard && rt.state.feedback().is_none());
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
}
