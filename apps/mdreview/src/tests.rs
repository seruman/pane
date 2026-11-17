use super::*;
use rune::{Env, Mods, Runtime};
use std::fmt::Write as _;

const SRC: &str = "# Title\n\nSome **bold phrase** and more.\n\n- one\n- two\n\n```rust\nlet x = 1;\n```\n\nA [link][r] here.\n\n[r]: https://example.com\n";

fn app(src: &str) -> Reviewer {
    Reviewer::new(Input {
        origin: Origin::Sample,
        source: src.into(),
        out: None,
    })
}

fn press(rt: &mut Runtime<Reviewer>, keys: &str) {
    for c in keys.chars() {
        rt.key(KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }
}

fn key(rt: &mut Runtime<Reviewer>, k: Key) {
    rt.key(KeyEvent {
        key: k,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
}

fn runtime(src: &str) -> Runtime<Reviewer> {
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

fn texts_under(rt: &Runtime<Reviewer>, key: &str) -> Vec<String> {
    let elements = rt.elements();
    let root = elements
        .iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no {key} on screen"));
    let prefix = format!("{}/", root.path);
    elements
        .iter()
        .filter(|element| {
            !element.exiting && (element.path == root.path || element.path.starts_with(&prefix))
        })
        .filter_map(|element| element.text.clone())
        .collect()
}

fn card_key(rt: &Runtime<Reviewer>, number: usize) -> String {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key)
        .find(|key| {
            key.starts_with("thread-") && texts_under(rt, key).contains(&format!("#{number}"))
        })
        .unwrap_or_else(|| panic!("no comment #{number} on screen"))
}

fn position(rt: &Runtime<Reviewer>) -> Option<usize> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key)
        .filter(|key| key.starts_with("thread-"))
        .find_map(|key| {
            let texts = texts_under(rt, &key);
            texts
                .iter()
                .any(|text| text == "Enter edit · \\x delete")
                .then(|| {
                    texts[0]
                        .strip_prefix('#')
                        .unwrap()
                        .parse::<usize>()
                        .unwrap()
                        - 1
                })
        })
}

fn feedback(rt: &Runtime<Reviewer>) -> Option<String> {
    rt.state
        .review
        .feedback(&rt.state.source, &rt.state.page, rt.state.origin.name())
}

fn cursor_line(rt: &Runtime<Reviewer>) -> String {
    texts_under(rt, "status").last().unwrap().clone()
}

fn range(s: &Reviewer, needle: &str) -> Range<usize> {
    let at = s.page.text().find(needle).unwrap();
    at..at + needle.len()
}

#[test]
fn an_image_beside_the_document_draws_its_picture() {
    let dir = std::env::temp_dir().join(format!("mdreview-image-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pic.png"),
        [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 3, 0, 0, 0, 2,
            8, 6, 0, 0, 0, 157, 116, 102, 26, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 56, 161,
            161, 241, 31, 134, 25, 144, 57, 0, 161, 45, 12, 139, 63, 134, 202, 182, 0, 0, 0, 0, 73,
            69, 78, 68, 174, 66, 96, 130,
        ],
    )
    .unwrap();
    let state = Reviewer::new(Input {
        origin: Origin::File(dir.join("doc.md").to_str().unwrap().into()),
        source: "Intro.\n\n![a chart](pic.png)\n".into(),
        out: None,
    });
    let mut rt = Runtime::new(state, view, (900.0, 900.0), Env::default());
    rt.poll = Some(|s: &mut Reviewer, _, _| s.images.poll());
    rt.state.images.start(std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    rt.state.images.get("pic.png").unwrap().wait().unwrap();
    for frame in 1..120 {
        rt.frame(f64::from(frame) / 60.0);
    }
    let picture = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.role == "image")
        .expect("the image");
    assert_eq!(picture.rect[2], 3.0, "at its own width");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_page_text_is_the_whole_documents() {
    let page = Page::new(SRC);
    let whole = rune_kit::markdown::RenderedText::new(SRC);
    assert_eq!(page.text(), whole.text());
    let at = page.text().find("bold phrase").unwrap();
    assert_eq!(
        page.source_ranges(at..at + 11),
        whole.source_ranges(at..at + 11)
    );
    let mut rt = runtime(SRC);
    press(&mut rt, "G");
    let drawn = texts_under(&rt, "page");
    assert!(
        drawn.contains(&"A link here.".to_owned()),
        "reference syntax is resolved: {drawn:?}"
    );
    assert!(!drawn.iter().any(|text| text.contains("[r]:")));
}

#[test]
fn parser_html_events_are_nested_and_page_ranges_stay_in_source() {
    use pulldown_cmark::{Event, Options, Parser};
    let fixtures = [
        "",
        "<div>hi</div>\n\n# Heading\n\nAfter.\n",
        "<!-- hidden -->\n\nVisible.\n",
        "<script>let x = 1;</script>\n",
        "A <span>word</span> here.\n",
        "<![CDATA[raw text]]>\n",
        "<?processing instruction?>\n",
    ];
    for source in fixtures {
        let mut depth = 0usize;
        let options =
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
        for event in Parser::new_ext(source, options) {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth = depth.checked_sub(1).expect("balanced parser events"),
                Event::Html(_) => assert!(depth > 0, "HTML is inside its block for {source:?}"),
                _ => {}
            }
        }
        assert_eq!(depth, 0, "{source:?}");
        let page = Page::new(source);
        assert_eq!(
            page.text(),
            rune_kit::markdown::RenderedText::new(source).text(),
            "{source:?}"
        );
        for range in page.source_ranges(0..page.text().len()) {
            assert!(source.get(range).is_some(), "{source:?}");
        }
        for index in 0..page.blocks.len() {
            let _ = page.section(index);
        }
    }
    let page = Page::new("<div>hi</div>\n\n# Heading\n\nAfter.\n");
    let at = page.text().find("After.").unwrap();
    assert_eq!(page.section(page.block_at(at)), Some("Heading"));
    assert_eq!(
        page::lines_of(
            "<div>hi</div>\n\n# Heading\n\nAfter.\n",
            &page.source_ranges(at..at + 6)
        ),
        Some((5, 5))
    );
}

#[test]
fn editing_comment_bodies_preserves_document_order() {
    let mut review = Review::default();
    let later = review.add(Comment::on(20..25, "later"));
    let general = review.add(Comment::general("general"));
    let earlier = review.add(Comment::on(2..5, "earlier"));
    assert!(review.set_body(later, "edited".into()));
    assert_eq!(
        review.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        [general, earlier, later]
    );
    let comment = review.get(later).unwrap();
    assert_eq!(comment.body, "edited");
    assert_eq!(comment.range().cloned(), Some(20..25));
    assert!(review.remove(earlier));
    assert!(!review.set_body(earlier, "deleted".into()));
    assert_eq!(
        review.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        [general, later]
    );
}

#[test]
fn feedback_quotes_and_numbers_in_document_order() {
    let s = app(SRC);
    let mut review = Review::default();
    review.add(Comment::remove(range(&s, "let x = 1;")));
    review.add(Comment::on(
        range(&s, "phrase and"),
        "Why bold?\nSay it plainly.",
    ));
    review.add(Comment::on(range(&s, "one\ntwo"), "Verify this."));
    review.add(Comment::on(range(&s, "Title"), "Too short."));
    review.add(Comment::general("Fine overall."));
    review.add(Comment::general("One more thing."));
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
         ## 3. Comment (lines 5 to 6, under `Title`)\n\n```markdown\n- one\n- two\n```\n\n> Verify this.\n\n\
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
    let ticks = app("A ```a ``b`` c``` here.");
    let mut review = Review::default();
    review.add(Comment::on(range(&ticks, "a ``b`` c"), "Keep the ticks."));
    assert_eq!(
        review.feedback(&ticks.source, &ticks.page, None).unwrap(),
        "# Review\n\nI've reviewed the document and have 1 piece of feedback:\n\n## 1. Comment (line 1)\n\nOn ```a ``b`` c```\n\n> Keep the ticks.\n"
    );

    let s = app("## Later\n\n- Sharing by link\n- Replies\n");
    let mut review = Review::default();
    review.add(Comment::remove(range(&s, "Sharing by link")));
    let out = review.feedback(&s.source, &s.page, None).unwrap();
    assert!(out.contains("```markdown\n- Sharing by link\n```"), "{out}");
}

#[test]
fn select_comment_save_edit_and_delete() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jveec");
    assert!(
        rt.find("composer-field").is_some(),
        "c on a selection opens the composer"
    );
    assert!(texts_under(&rt, "composer").contains(&"Some bold".to_owned()));
    press(&mut rt, "Too loud");
    assert!(
        texts_under(&rt, "composer-field").contains(&"Too loud".to_owned()),
        "the field has the keys, not Vim"
    );
    key(&mut rt, Key::Enter);
    assert!(rt.find("composer").is_none());
    let expected = "# Review\n\nI've reviewed the document and have 1 piece of feedback:\n\n## 1. Comment (line 3, under `Title`)\n\nOn `Some bold`, written as:\n\n```markdown\nSome **bold\n```\n\n> Too loud\n";
    assert_eq!(feedback(&rt).as_deref(), Some(expected));
    assert!(texts_under(&rt, &card_key(&rt, 1)).contains(&"Too loud".to_owned()));

    key(&mut rt, Key::Enter);
    assert!(texts_under(&rt, "composer").contains(&"Edit #1 · Comment".to_owned()));
    press(&mut rt, "!");
    key(&mut rt, Key::Escape);
    assert!(rt.find("composer").is_none());
    assert_eq!(feedback(&rt).as_deref(), Some(expected));

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
    assert_eq!(
        feedback(&rt).unwrap(),
        expected.replace("> Too loud\n", "> Too loud\n> really\n")
    );

    press(&mut rt, "\\x");
    assert_eq!(feedback(&rt), None);
    assert!(texts_under(&rt, "status").contains(&"0 comments".to_owned()));
}

#[test]
fn several_general_comments_listed_and_stepped_through() {
    let mut rt = runtime(SRC);
    for body in ["First", "Second"] {
        press(&mut rt, "C");
        press(&mut rt, body);
        key(&mut rt, Key::Enter);
    }
    assert_eq!(
        feedback(&rt).as_deref(),
        Some("# Review\n\n## General\n\n> First\n\n> Second\n")
    );
    assert!(texts_under(&rt, &card_key(&rt, 1)).contains(&"First".to_owned()));
    assert!(texts_under(&rt, &card_key(&rt, 2)).contains(&"Second".to_owned()));

    key(&mut rt, Key::Tab);
    assert!(
        rt.find("list-0").is_some() && rt.find("list-1").is_some(),
        "the list has them"
    );
    key(&mut rt, Key::Escape);

    press(&mut rt, "]c");
    key(&mut rt, Key::Enter);
    assert!(texts_under(&rt, "composer").contains(&"Edit #1 · General".to_owned()));
    assert!(texts_under(&rt, "composer-field").contains(&"First".to_owned()));
}

#[test]
fn stepping_visits_comments_that_start_together() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jVd");
    press(&mut rt, "vd");
    press(&mut rt, "vllc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    assert!(texts_under(&rt, "status").contains(&"3 comments".to_owned()));
    let mut seen = Vec::new();
    for _ in 0..3 {
        press(&mut rt, "]c");
        seen.push(position(&rt).unwrap());
    }
    assert_eq!(
        seen,
        [2, 0, 1],
        "]c follows displayed comment numbers and wraps"
    );
    press(&mut rt, "]c");
    let first = position(&rt);
    press(&mut rt, "[c[c[c");
    assert_eq!(position(&rt), first, "[c comes back round");
}

#[test]
fn removals_and_jumps() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jjVc");
    press(&mut rt, "check");
    key(&mut rt, Key::Enter);
    press(&mut rt, "gg");
    press(&mut rt, "dd");
    assert!(texts_under(&rt, &card_key(&rt, 1)).contains(&"Remove".to_owned()));
    assert!(texts_under(&rt, &card_key(&rt, 2)).contains(&"Comment".to_owned()));

    press(&mut rt, "]c");
    assert_eq!(position(&rt), Some(1));
    press(&mut rt, "]c");
    assert_eq!(position(&rt), Some(0), "wraps round");
    press(&mut rt, "[c");
    assert_eq!(position(&rt), Some(1));

    press(&mut rt, "gg");
    assert_eq!(position(&rt), Some(0));
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("mdreview-w4-{}-{id}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn run_command(rt: &mut Runtime<Reviewer>, command: &str) {
    press(rt, command);
    if command.starts_with(':') {
        key(rt, Key::Enter);
    }
}

const UNWRITTEN: &str =
    "The review isn't written. \\w or :w writes it, :wq sends it, :q! drops it.";

#[test]
fn quitting_runs_the_send_hook_only_for_send_commands() {
    let dir = TempDir::new();
    for command in [":q", ":quit", ":q!", ":quit!", "ZQ"] {
        let file = dir.0.join("empty-review.md");
        let mut rt = runtime(SRC);
        rt.state.out = Some(file.to_str().unwrap().into());
        run_command(&mut rt, command);
        assert_eq!(rt.shell_request, Some(rune::Shell::Quit), "{command}");
        rt.update(send);
        assert!(!file.exists(), "{command} wrote an empty review");
    }
    for command in [":q!", ":quit!", "ZQ", ":wq", ":x", ":wq!", ":x!", "ZZ"] {
        let file = dir.0.join("review.md");
        std::fs::write(&file, "untouched").unwrap();
        let mut rt = runtime(SRC);
        rt.state.out = Some(file.to_str().unwrap().into());
        press(&mut rt, "Ccared about this");
        key(&mut rt, Key::Enter);
        run_command(&mut rt, ":q");
        assert_eq!(rt.shell_request, None);
        assert!(texts_under(&rt, "status").contains(&UNWRITTEN.to_owned()));
        run_command(&mut rt, command);
        assert_eq!(rt.shell_request, Some(rune::Shell::Quit), "{command}");
        rt.update(send);
        let expected = if matches!(command, ":q!" | ":quit!" | "ZQ") {
            "untouched"
        } else {
            "# Review\n\n## General\n\n> cared about this\n"
        };
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            expected,
            "{command}"
        );
    }
}

#[test]
fn which_key_shows_what_can_follow() {
    let hint_shown = |rt: &Runtime<Reviewer>, key: &str, description: &str| {
        let texts = texts_under(rt, "which-key");
        assert!(
            texts.windows(2).any(|pair| pair == [description, key]),
            "missing {description} / {key} in {texts:?}"
        );
    };
    let mut rt = runtime(SRC);
    assert!(rt.find("which-key").is_none());
    press(&mut rt, "v");
    assert!(rt.find("which-key").is_some(), "a selection lists its keys");
    hint_shown(&rt, "c", "Comment");
    hint_shown(&rt, "d", "Remove this");
    hint_shown(&rt, "gy", "Yank as Markdown");
    key(&mut rt, Key::Escape);
    assert!(rt.find("which-key").is_none());

    press(&mut rt, "gy");
    assert_eq!(texts_under(&rt, "which-key")[0], "gy");
    press(&mut rt, "i");
    assert_eq!(texts_under(&rt, "which-key")[0], "gyi");
    key(&mut rt, Key::Escape);
    press(&mut rt, "]");
    assert_eq!(
        texts_under(&rt, "which-key"),
        ["]", "esc", "Cancel", "Next comment", "c"]
    );
    key(&mut rt, Key::Escape);
    press(&mut rt, "\\");
    hint_shown(&rt, "x", "Delete the comment");
    hint_shown(&rt, "?", "Every key");
    key(&mut rt, Key::Escape);

    press(&mut rt, "c");
    assert_eq!(texts_under(&rt, "which-key")[0], "c");
    hint_shown(&rt, "c", "The line");
    key(&mut rt, Key::Escape);
    assert!(rt.find("which-key").is_none(), "Escape cancels it");
    assert_eq!(feedback(&rt), None);

    let before = cursor_line(&rt);
    press(&mut rt, "\\?");
    assert_eq!(texts_under(&rt, "which-key")[0], "keys");
    assert!(rt.find("which-key").is_some());
    press(&mut rt, "j");
    assert!(rt.find("which-key").is_none());
    assert_ne!(
        cursor_line(&rt),
        before,
        "and the key goes on to move the cursor"
    );
}

#[test]
fn comment_and_remove_are_operators() {
    let mut rt = runtime(SRC);
    press(&mut rt, "jcw");
    assert!(texts_under(&rt, "composer").contains(&"Some".to_owned()));
    press(&mut rt, "a");
    key(&mut rt, Key::Enter);
    assert_eq!(
        feedback(&rt).as_deref(),
        Some(
            "# Review\n\nI've reviewed the document and have 1 piece of feedback:\n\n## 1. Comment (line 3, under `Title`)\n\nOn `Some`\n\n> a\n"
        )
    );
    press(&mut rt, "jdip");
    assert!(texts_under(&rt, &card_key(&rt, 2)).contains(&"Remove".to_owned()));
    assert!(feedback(&rt).unwrap().ends_with("## 2. Remove this (lines 5 to 6, under `Title`)\n\n```markdown\n- one\n- two\n```\n\nI don't want this in the document.\n"));
    press(&mut rt, "gg2cc");
    assert!(texts_under(&rt, "composer").contains(&"Title".to_owned()));
    press(&mut rt, "b");
    key(&mut rt, Key::Enter);
    assert!(texts_under(&rt, &card_key(&rt, 1)).contains(&"b".to_owned()));
    assert!(
        feedback(&rt)
            .unwrap()
            .contains("## 1. Comment (line 1)\n\nOn `Title`\n\n> b\n")
    );
}

#[test]
fn opening_an_editor_replaces_the_comment_list() {
    let mut state = Reviewer::default();
    let id = state.review.add(Comment::general("review"));
    let mut rt = Runtime::new(state, view, (900.0, 900.0), Env::default());
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    key(&mut rt, Key::Tab);
    assert!(rt.find("comment-list").is_some());
    rt.state.open(Target::Edit(id));
    key(&mut rt, Key::Left);
    assert!(rt.find("composer").is_some());
    assert!(rt.find("comment-list").is_none());
    key(&mut rt, Key::Escape);
    assert!(rt.find("composer").is_none());
    assert!(rt.find("comment-list").is_none());
}

#[test]
fn the_comment_list_floats_and_jumps() {
    let mut rt = runtime(SRC);
    press(&mut rt, "Vc");
    press(&mut rt, "one");
    key(&mut rt, Key::Enter);
    press(&mut rt, "jVc");
    press(&mut rt, "two");
    key(&mut rt, Key::Enter);
    press(&mut rt, "jVd");
    press(&mut rt, "gg");
    let heading = rt.find("block-0").unwrap();
    let cursor = cursor_line(&rt);

    key(&mut rt, Key::Tab);
    assert!(rt.find("comment-list").is_some());
    assert_eq!(
        position(&rt),
        Some(0),
        "opens on the comment under the cursor"
    );
    assert_eq!(
        rt.find("block-0").unwrap(),
        heading,
        "the page doesn't move"
    );

    press(&mut rt, "jj");
    assert_eq!(position(&rt), Some(2), "the page marks the list's comment");
    assert!(rt.find(CURRENT_MARK).is_some(), "keyed, to scroll to");
    press(&mut rt, "j");
    assert_eq!(position(&rt), Some(2), "stops at the end");
    press(&mut rt, "k");
    assert_eq!(position(&rt), Some(1));
    assert_eq!(cursor_line(&rt), cursor);

    key(&mut rt, Key::Escape);
    assert!(rt.find("comment-list").is_none());
    assert_eq!(cursor_line(&rt), cursor);
    assert_eq!(position(&rt), Some(0));

    key(&mut rt, Key::Tab);
    press(&mut rt, "G");
    key(&mut rt, Key::Enter);
    assert!(rt.find("comment-list").is_none());
    assert_eq!(position(&rt), Some(2));
    assert_eq!(cursor_line(&rt).split(':').next(), Some("5"));

    key(&mut rt, Key::Tab);
    let (x, y) = rt.find("list-0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.find("comment-list").is_none());
    assert_eq!(position(&rt), Some(0));
}

#[test]
fn moving_in_the_list_scrolls_the_page_to_the_comment() {
    let src = (0..80).fold(String::new(), |mut source, i| {
        write!(source, "Paragraph number {i} of filler.\n\n").unwrap();
        source
    });
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
    let on_screen = |rt: &Runtime<Reviewer>| {
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
    let mut rt = runtime(SRC);
    press(&mut rt, "jv2e");
    assert_eq!(feedback(&rt), None);
    press(&mut rt, "c");
    assert!(texts_under(&rt, "composer").contains(&"Some bold".to_owned()));
}

#[test]
fn a_review_that_cant_be_written_is_an_error_and_still_reaches_stdout() {
    let dir = std::env::temp_dir().join(format!("mdreview-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = dir.join("review.md");
    let mut stdout = Vec::new();
    assert_eq!(
        deliver("# Review\n", Some(file.to_str().unwrap()), &mut stdout),
        Ok(())
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Review\n");
    assert_eq!(stdout, b"", "a written file keeps stdout clear");

    let unwritable = dir.join("no-such-dir").join("review.md");
    let mut stdout = Vec::new();
    let result = deliver(
        "# Review\n",
        Some(unwritable.to_str().unwrap()),
        &mut stdout,
    );
    assert!(result.is_err(), "a failed write is an error: {result:?}");
    assert_eq!(
        String::from_utf8(stdout).unwrap(),
        "# Review\n",
        "and the review isn't lost"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn click(rt: &mut Runtime<Reviewer>, key_name: &str) {
    let (x, y) = rt
        .find(key_name)
        .unwrap_or_else(|| panic!("no {key_name} on screen"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn two_comments() -> Runtime<Reviewer> {
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
    assert!(
        rt.find("composer-field").is_some(),
        "editing the second comment"
    );
    let delete = card_key(&rt, 1).replacen("thread-", "delete-", 1);
    click(&mut rt, &delete);
    press(&mut rt, " edited");
    key(&mut rt, Key::Enter);
    assert_eq!(
        feedback(&rt).as_deref(),
        Some(
            "# Review\n\nI've reviewed the document and have 1 piece of feedback:\n\n## 1. Comment (line 3, under `Title`)\n\nOn `Some bold phrase and more.`, written as:\n\n```markdown\nSome **bold phrase** and more.\n```\n\n> second edited\n"
        )
    );
}

#[test]
fn deleting_another_comment_while_editing_keeps_the_editor_and_its_keys() {
    let mut rt = two_comments();
    press(&mut rt, "\\e");
    let delete = card_key(&rt, 1).replacen("thread-", "delete-", 1);
    click(&mut rt, &delete);
    assert!(
        rt.find("composer-field").is_some(),
        "the editor stays on screen"
    );
    key(&mut rt, Key::Escape);
    assert!(
        rt.find("composer-field").is_none(),
        "Escape still closes it"
    );
    let before = cursor_line(&rt);
    press(&mut rt, "gg");
    assert_ne!(cursor_line(&rt), before, "and keys reach Vim again");
}

#[test]
fn loading_input_reads_files_stdin_and_the_sample() {
    let dir = TempDir::new();
    let file = dir.0.join("source.md");
    let output = dir.0.join("feedback.md");
    std::fs::write(&file, "# From disk\n").unwrap();
    let args = vec![
        file.to_str().unwrap().into(),
        "-o".into(),
        output.to_str().unwrap().into(),
    ];
    let input = load_input(&args, &mut &b"not the file"[..], true).unwrap();
    assert_eq!(input.source, "# From disk\n");
    assert_eq!(input.origin.title(), file.to_str().unwrap());
    assert_eq!(input.origin.name(), file.to_str());
    assert_eq!(input.out.as_deref(), output.to_str());
    assert!(!output.exists());

    let input = load_input(&["-".into()], &mut &b"# From stdin\n"[..], false).unwrap();
    assert_eq!(input.source, "# From stdin\n");
    assert_eq!(input.origin.title(), "stdin");
    assert_eq!(input.origin.name(), None);
    assert_eq!(input.out, None);
    let input = load_input(&[], &mut &b"not the sample"[..], true).unwrap();
    assert_eq!(input.source, SAMPLE);
    assert_eq!(input.origin.title(), "sample.md");
    assert_eq!(input.origin.name(), None);
}

#[test]
fn loading_input_reports_bad_arguments_and_read_errors() {
    let dir = TempDir::new();
    let missing = dir.0.join("missing.md").to_str().unwrap().to_owned();
    let error = load_input(std::slice::from_ref(&missing), &mut &b""[..], false)
        .err()
        .unwrap();
    assert!(error.starts_with(&format!("could not read {missing}: ")));
    assert!(!std::path::Path::new(&missing).exists());
    assert_eq!(
        load_input(&["-o".into()], &mut &b""[..], false)
            .err()
            .as_deref(),
        Some("-o needs a file")
    );
    assert_eq!(
        load_input(&["-".into()], &mut &b""[..], true)
            .err()
            .as_deref(),
        Some("`-` reads the document from stdin, but stdin is the terminal")
    );
    let error = load_input(&["-".into()], &mut &[0xff][..], false)
        .err()
        .unwrap();
    assert!(error.starts_with("could not read stdin: "), "{error}");
}

#[test]
fn an_empty_document_accepts_general_feedback_and_sends_it() {
    let mut empty = runtime("");
    run_command(&mut empty, ":q");
    assert_eq!(empty.shell_request, Some(rune::Shell::Quit));

    let dir = TempDir::new();
    let file = dir.0.join("empty-feedback.md");
    let mut rt = runtime("");
    rt.state.out = Some(file.to_str().unwrap().into());
    assert_eq!(texts_under(&rt, "page"), Vec::<String>::new());
    assert_eq!(cursor_line(&rt), "1:1");
    press(&mut rt, "ggddccyy");
    assert!(rt.find("composer").is_none());
    assert_eq!(feedback(&rt), None);
    press(&mut rt, "C");
    assert!(
        rt.find("composer-field").is_some(),
        "C opens a general comment even when the document is empty"
    );
    press(&mut rt, "Please write the plan.");
    key(&mut rt, Key::Enter);
    assert!(texts_under(&rt, &card_key(&rt, 1)).contains(&"Please write the plan.".into()));
    run_command(&mut rt, ":x");
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
    rt.update(send);
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "# Review\n\n## General\n\n> Please write the plan.\n"
    );
}

#[test]
fn deleting_the_open_comment_closes_its_editor_and_restores_keys() {
    let mut rt = two_comments();
    press(&mut rt, "\\e");
    assert!(rt.find("composer").is_some());
    rt.update(|s, _| s.delete(s.review.at_position(1).unwrap()));
    rt.frame(0.0);
    assert!(rt.find("composer").is_none());
    press(&mut rt, "gg");
    assert_eq!(cursor_line(&rt).split(':').next(), Some("1"));
    assert_eq!(
        feedback(&rt).as_deref(),
        Some(
            "# Review\n\nI've reviewed the document and have 1 piece of feedback:\n\n## 1. Comment (line 1)\n\nOn `Title`\n\n> first\n"
        )
    );
}

#[test]
fn send_stdout_child() {
    if std::env::var_os("MDREVIEW_TEST_STDOUT_CHILD").is_none() {
        return;
    }
    let mut rt = runtime(SRC);
    press(&mut rt, "CFor the author.");
    key(&mut rt, Key::Enter);
    press(&mut rt, "ZZ");
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
    println!("FEEDBACK-BEGIN");
    rt.update(send);
    println!("FEEDBACK-END");
}

#[test]
fn the_send_hook_writes_feedback_to_stdout() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::send_stdout_child", "--nocapture"])
        .env("MDREVIEW_TEST_STDOUT_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let actual = stdout
        .split_once("FEEDBACK-BEGIN\n")
        .unwrap()
        .1
        .split_once("FEEDBACK-END\n")
        .unwrap()
        .0;
    assert_eq!(actual, "# Review\n\n## General\n\n> For the author.\n");
}

#[test]
fn the_command_line_takes_one_document_and_an_output() {
    let args = |a: &[&str]| parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        args(&["plan.md", "-o", "out.md"]),
        Ok(Args {
            path: Some("plan.md".into()),
            out: Some("out.md".into())
        })
    );
    assert_eq!(
        args(&["-"]),
        Ok(Args {
            path: Some("-".into()),
            out: None
        })
    );
    assert_eq!(args(&[]), Ok(Args::default()));
    assert!(args(&["-o"]).is_err(), "-o needs a file");
    assert!(
        args(&["--help"]).is_err(),
        "an unknown flag isn't a file name"
    );
    assert!(args(&["-x", "plan.md"]).is_err());
    assert!(
        args(&["a.md", "b.md"]).is_err(),
        "a second document isn't taken over the first"
    );
}

fn shown(rt: &Runtime<Reviewer>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.text.or(e.label))
        .collect()
}

#[test]
fn the_hints_name_the_keys_that_work() {
    let mut rt = runtime(SRC);
    press(&mut rt, "cc");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    press(&mut rt, "]c");
    let texts = shown(&rt).join("\n");
    assert!(
        !texts.contains("dc"),
        "dc doesn't delete a comment any more:\n{texts}"
    );
    assert!(
        texts.contains("\\x"),
        "the card names the delete key:\n{texts}"
    );
    assert!(
        !texts.contains('—') && !texts.contains('–'),
        "no long dashes in what the app shows"
    );
}

/// Measures gaps between blocks in a single rune-kit document.
fn rune_kit_gaps(src: &str) -> Vec<f32> {
    struct Doc(String);
    fn view(d: &Doc, cx: &mut rune::Cx<Doc>) -> El {
        let theme = Theme::current(cx.system_dark());
        rune_kit::Markdown::new(d.0.as_str())
            .style(style(&theme))
            .build(&theme.kit)
    }
    let mut rt = Runtime::new(
        Doc(src.to_owned()),
        view,
        (800.0, 4000.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    let rects: Vec<[f32; 4]> = (0..)
        .map_while(|i| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(&format!("b{i}")))
                .map(|e| e.rect)
        })
        .collect();
    std::iter::once(0.0)
        .chain(rects.windows(2).map(|w| w[1][1] - (w[0][1] + w[0][3])))
        .collect()
}

#[test]
fn block_gaps_match_what_rune_kit_draws() {
    let src = "# Title\n\nA paragraph.\n\n![logo](logo.png)\n\n## Section\n\n![logo](logo.png)\n\n- one\n- two\n\n> quoted\n\n```\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\nThe end.\n";
    let ours: Vec<f32> = Page::new(src).blocks.iter().map(|b| b.top).collect();
    assert_eq!(ours, rune_kit_gaps(src));
    // rune-kit splits paragraphs at images, so compare total height rather than individual gaps.
    let src =
        "Intro.\n\nSee ![logo](logo.png) here.\n\n![a](a.png) ![b](b.png)\n\n## Then\n\nEnd.\n";
    assert_eq!(page_height(src, true), page_height(src, false), "{src:?}");
}

fn page_height(src: &str, by_block: bool) -> f32 {
    struct Doc(String, bool);
    fn view(d: &Doc, cx: &mut rune::Cx<Doc>) -> El {
        let theme = Theme::current(cx.system_dark());
        let md = |m: &str| {
            rune_kit::Markdown::new(m)
                .style(style(&theme))
                .build(&theme.kit)
        };
        let mut page = rune::col().key("doc").w_full();
        if d.1 {
            for block in Page::new(&d.0).blocks {
                page = page.child(el().w_full().mt(block.top).child(md(&block.markdown)));
            }
        } else {
            page = page.child(md(&d.0));
        }
        page
    }
    let mut rt = Runtime::new(
        Doc(src.to_owned(), by_block),
        view,
        (800.0, 4000.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("doc"))
        .map(|e| e.rect[3])
        .unwrap()
}

fn text_rect(rt: &Runtime<Reviewer>, shown: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(shown))
        .unwrap_or_else(|| panic!("no text {shown:?}"))
        .rect
}

fn pointer_click(rt: &mut Runtime<Reviewer>, x: f32, y: f32) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

const MOUSE: &str = "# Title\n\nalpha beta gamma\n\nSee [the docs](https://example.com/docs) and www.rust-lang.org.\n\n- one\n- two\n";

#[test]
fn a_click_moves_the_cursor_where_it_lands() {
    let mut rt = runtime(MOUSE);
    let r = text_rect(&rt, "alpha beta gamma");
    // Past the end of the line: the cursor goes on its last character, as in Vim.
    pointer_click(&mut rt, r[0] + r[2] - 1.0, r[1] + r[3] / 2.0);
    assert_eq!(rt.state.vim.mode(), VimMode::Normal);
    let at = rt.state.page.text().find("gamma").unwrap() + 4;
    assert_eq!(rt.state.cursor(), at);
    // The start of a line in another block.
    let r = text_rect(&rt, "two");
    pointer_click(&mut rt, r[0] + 0.5, r[1] + r[3] / 2.0);
    assert_eq!(rt.state.cursor(), rt.state.page.text().find("two").unwrap());
    assert_eq!(
        rt.selection(),
        None,
        "the page shows Vim's cursor, not the pointer's caret"
    );
}

#[test]
fn a_drag_across_blocks_selects_in_visual_mode_and_comments_on_it() {
    let mut rt = runtime(MOUSE);
    let from = text_rect(&rt, "alpha beta gamma");
    let to = text_rect(&rt, "one");
    rt.pointer_down(from[0] + 0.5, from[1] + from[3] / 2.0);
    rt.pointer_move(from[0] + 40.0, from[1] + from[3] / 2.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(to[0] + to[2] - 0.5, to[1] + to[3] / 2.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(to[0] + to[2] - 0.5, to[1] + to[3] / 2.0);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.vim.mode(), VimMode::Visual);
    let text = rt.state.page.text();
    let selected = rt.state.vim.selection(text).map(|r| text[r].to_owned());
    assert_eq!(
        selected.as_deref(),
        Some(&text[text.find("alpha").unwrap()..text.find("one").unwrap() + 3])
    );
    press(&mut rt, "c");
    press(&mut rt, "x");
    key(&mut rt, Key::Enter);
    let out = feedback(&rt).unwrap();
    assert!(
        out.contains("alpha beta gamma") && out.contains("- one"),
        "{out}"
    );
}

#[test]
fn a_double_click_selects_a_word() {
    let mut rt = runtime(MOUSE);
    let r = text_rect(&rt, "alpha beta gamma");
    // The box spans the page; its text starts at its left edge.
    let (x, y) = (r[0] + 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let text = rt.state.page.text();
    assert_eq!(rt.state.vim.mode(), VimMode::Visual);
    assert_eq!(
        rt.state.vim.selection(text).map(|r| &text[r]),
        Some("alpha")
    );
}

#[test]
fn gx_and_a_click_open_links() {
    let mut rt = runtime(MOUSE);
    let text = rt.state.page.text().to_owned();
    rt.update(|s, _| s.vim.set_cursor(text.find("docs").unwrap(), &text));
    rt.frame(1.0 / 60.0);
    press(&mut rt, "gx");
    rt.update(|s, _| s.vim.set_cursor(text.find("rust-lang").unwrap(), &text));
    rt.frame(1.0 / 60.0);
    press(&mut rt, "gx");
    rt.update(|s, _| s.vim.set_cursor(text.find("alpha").unwrap(), &text));
    rt.frame(1.0 / 60.0);
    press(&mut rt, "gx");
    assert_eq!(rt.state.note.as_deref(), Some("No link under the cursor"));
    assert_eq!(
        rt.take_url_requests(),
        ["https://example.com/docs", "https://www.rust-lang.org"]
    );
    let r = rt
        .elements()
        .into_iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("See ")))
        .unwrap()
        .rect;
    // "See " is about four characters in: the link starts just after it.
    pointer_click(&mut rt, r[0] + 40.0, r[1] + r[3] / 2.0);
    assert_eq!(rt.take_url_requests(), ["https://example.com/docs"]);
    assert_eq!(rt.state.vim.mode(), VimMode::Normal);
}

#[test]
fn reference_links_and_bare_addresses_resolve() {
    let page = Page::new(
        "A [link][r] and <https://auto.example>, then (https://x.y/z).\n\n[r]: https://example.com/ref\n",
    );
    let text = page.text();
    let at = |needle: &str| {
        let i = text.find(needle).unwrap();
        page.link_at(i..i + 1)
    };
    assert_eq!(at("link").as_deref(), Some("https://example.com/ref"));
    assert_eq!(at("auto").as_deref(), Some("https://auto.example"));
    assert_eq!(at("x.y").as_deref(), Some("https://x.y/z"));
    assert_eq!(at("then"), None);
}

fn leader(rt: &mut Runtime<Reviewer>, c: char) {
    press(rt, &format!("\\{c}"));
}

#[test]
fn leader_w_writes_and_keeps_reviewing_and_leader_q_quits_once_written() {
    let dir = TempDir::new();
    let file = dir.0.join("review.md");
    let mut rt = runtime(SRC);
    rt.state.out = Some(file.to_str().unwrap().into());

    leader(&mut rt, 'w');
    assert_eq!(rt.state.note.as_deref(), Some("Nothing to write yet"));
    assert!(!file.exists());

    press(&mut rt, "Cfirst");
    key(&mut rt, Key::Enter);
    leader(&mut rt, 'q');
    assert_eq!(rt.shell_request, None, "an unwritten comment keeps it open");
    assert_eq!(rt.state.note.as_deref(), Some(UNWRITTEN));

    leader(&mut rt, 'w');
    assert_eq!(rt.shell_request, None, "writing doesn't quit");
    let first = "# Review\n\n## General\n\n> first\n";
    assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
    assert_eq!(
        rt.state.note.as_deref(),
        Some(&*format!("Wrote 1 comment to {}", file.display()))
    );

    press(&mut rt, "Csecond");
    key(&mut rt, Key::Enter);
    leader(&mut rt, 'q');
    assert_eq!(
        rt.shell_request, None,
        "a comment after the write keeps it open"
    );
    run_command(&mut rt, ":w");
    let both = std::fs::read_to_string(&file).unwrap();
    assert!(
        both.contains("> first") && both.contains("> second"),
        "{both}"
    );

    leader(&mut rt, 'q');
    assert_eq!(rt.shell_request, Some(rune::Shell::Quit));
    rt.update(send);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        both,
        "quitting leaves the written review"
    );
}

#[test]
fn write_without_an_output_file_says_so_and_keeps_quit_guarded() {
    let mut rt = runtime(SRC);
    press(&mut rt, "Cnote");
    key(&mut rt, Key::Enter);
    leader(&mut rt, 'w');
    assert_eq!(
        rt.state.note.as_deref(),
        Some("No file to write to: start with -o FILE, or :wq sends to stdout")
    );
    leader(&mut rt, 'q');
    assert_eq!(rt.shell_request, None);
}

#[test]
fn send_and_quit_after_a_write_does_not_write_again() {
    let dir = TempDir::new();
    let file = dir.0.join("review.md");
    let mut rt = runtime(SRC);
    rt.state.out = Some(file.to_str().unwrap().into());
    press(&mut rt, "Conce");
    key(&mut rt, Key::Enter);
    leader(&mut rt, 'w');
    std::fs::write(&file, "edited by hand").unwrap();
    run_command(&mut rt, ":wq");
    rt.update(send);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited by hand");
}

#[test]
fn the_title_bar_keeps_clear_of_the_window_controls() {
    let mut rt = runtime(SRC);
    let title = |rt: &Runtime<Reviewer>| text_rect(rt, "sample.md")[0];
    let flush = title(&rt);
    let mut insets = rune::ChromeInsets::default();
    insets.leading = 78.0;
    insets.height = markdown::TITLE_BAR_HEIGHT;
    rt.set_chrome_insets(insets);
    rt.frame(1.0 / 60.0);
    // Layout moves glide; let it settle.
    rt.frame(2.0);
    assert_eq!(
        title(&rt),
        flush + 78.0,
        "the title moves past the traffic lights"
    );
}

fn rect_of(rt: &Runtime<Reviewer>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no {key} on screen"))
        .visible
}

#[test]
fn the_composer_floats_where_it_can_be_seen_inside_a_long_block() {
    let code: String = (0..60).map(|i| format!("let x{i} = {i};\n")).collect();
    let src = format!("# Long\n\nIntro line.\n\n```rust\n{code}```\n\nAfter.\n");
    for needle in ["let x0", "let x30", "let x59", "Intro"] {
        let mut rt = Runtime::new(
            app(&src),
            view,
            (900.0, 600.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        let text = rt.state.page.text().to_owned();
        rt.update(|s, tx| {
            s.vim.set_cursor(text.find(needle).unwrap(), &text);
            tx.scroll_into_view(VIM_CURSOR);
        });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        press(&mut rt, "cc");
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        let body = rect_of(&rt, "body");
        let card = rect_of(&rt, "composer");
        let field = rect_of(&rt, "composer-field");
        assert!(field[3] > 0.0, "{needle}: the field is drawn, at {field:?}");
        assert!(
            card[1] >= body[1] && card[1] + card[3] <= body[1] + body[3],
            "{needle}: the composer {card:?} fits in the body {body:?}"
        );
    }
}
