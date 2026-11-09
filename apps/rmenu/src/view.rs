//! The palette: query row, source strip, result rows, footer and preview, filling the
//! terminal. Modelled on mmenu's PaletteView.

use std::borrow::Cow;

use rmenu_core::config::KeyChord;
use rmenu_core::model::{Event, Nav, Target};
use rune::tok;
use rune::{Color, Cx, El, Family, GrowthSpec, Key, KeyEvent, ScrollSpec, Span, TextStyle, Tx, col, el, rich, row, text, text_input};

use crate::cli::Appearance;
use crate::host::Menu;

pub const ROWS_KEY: &str = "rows";

const QUERY_H: f32 = 30.0;
const ROW_H: f32 = 28.0;
const ROW_SUB_H: f32 = 42.0;
const FOOTER_H: f32 = 30.0;
const PREVIEW_W: f32 = 380.0;
/// Rows the list lays out. Rune keys every row of a list on each change, so a million matches
/// would cost a million strings per keystroke; nobody scrolls past the first few thousand of
/// a ranked list, and the footer still counts them all.
pub const LIST_CAP: usize = 5_000;

const TITLE: TextStyle = TextStyle::new(13.0, 400.0).lh(18.0);
const SMALL: TextStyle = TextStyle::new(11.0, 400.0).lh(15.0);
const CHIP: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
const KEYCAP: TextStyle = TextStyle::new(10.0, 500.0).lh(14.0);
const MONO: TextStyle = TextStyle::new(11.0, 400.0).lh(15.0).family(Family::SystemMono);

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    foreground: Color,
    secondary: Color,
    input: Color,
    accent: Color,
    selected_text: Color,
    separator: Color,
    error: Color,
}

fn palette(menu: &Menu, system_dark: bool) -> Palette {
    let terminal = rune::terminal_colors().unwrap_or_default();
    let dark = match menu.appearance {
        Appearance::Light => false,
        Appearance::Dark => true,
        Appearance::System => terminal.dark.or(terminal.background.map(|b| b.luminance() < 0.18)).unwrap_or(system_dark),
    };
    let theme = tok::theme(dark);
    let background = terminal.background.filter(|_| menu.appearance == Appearance::System).unwrap_or(theme.card);
    let foreground = terminal.foreground.filter(|_| menu.appearance == Appearance::System).unwrap_or(theme.text);
    let accent = terminal.cursor.or(terminal.palette[4]).unwrap_or_else(|| tok::accent(0, dark));
    // Text on the accent reads best as whichever of black and white contrasts more.
    let selected_text = if accent.luminance() > 0.36 { Color { r: 0.02, g: 0.02, b: 0.02, a: 1.0 } } else { Color::WHITE };
    let error = terminal.palette[if dark { 9 } else { 1 }].unwrap_or(Color { r: 0.85, g: 0.25, b: 0.2, a: 1.0 });
    Palette {
        background,
        foreground,
        secondary: foreground.alpha(if dark { 0.5 } else { 0.55 }),
        input: foreground.alpha(if dark { 0.07 } else { 0.06 }),
        accent,
        selected_text,
        separator: foreground.alpha(if dark { 0.1 } else { 0.12 }),
        error,
    }
}

pub fn view(menu: &Menu, cx: &mut Cx<Menu>) -> El {
    // A picker is a tool: every change lands in the frame it happens in.
    cx.reduced_motion(true);
    let c = palette(menu, cx.system_dark());
    let model = &menu.model;
    // The terminal's size in points; before the first layout, a typical floating layer.
    let width = cx.layout_with("root", |r| r[2].round()).unwrap_or(640.0);
    let list = col().key("list").grow(1.0).min_w(0.0).h_full().items_stretch()
        .child(query_row(menu, &c))
        .child(source_strip(menu, &c))
        .child(results(menu, &c));
    let body = match model.preview() {
        Some(preview) => row().grow(1.0).w_full().min_h(0.0)
            .child(list)
            .child(el().w(1.0).h_full().fill(c.separator))
            // The list keeps at least half the width, as in mmenu.
            .child(preview_pane(preview, &c, PREVIEW_W.min(width / 2.0))),
        None => row().grow(1.0).w_full().min_h(0.0).child(list),
    };
    col().key("root").w_full().h_full().fill(c.background)
        .child(body)
        .child(footer(menu, &c, width))
}

fn query_row(menu: &Menu, c: &Palette) -> El {
    let prompt = menu.model.prompt();
    let field = text_input(&menu.field, TITLE).key("query").single_line().grow(1.0).min_w(0.0)
        .color(c.foreground).caret(c.accent, c.accent.alpha(0.3))
        .placeholder("Search", c.secondary)
        .on_edit(|m: &mut Menu| &mut m.field)
        .on_input_change(|m: &mut Menu, tx: &mut Tx| {
            let query = m.field.text.clone();
            m.handle(Event::SetQuery(query), tx);
        })
        .on_input_key(key);
    let chip = match prompt.as_str() {
        "" => None,
        // The default prompt is a quiet marker, any other is a chip, as in mmenu.
        crate::cli::DEFAULT_PROMPT => Some(text(crate::cli::DEFAULT_PROMPT).font(TITLE).color(c.secondary)),
        _ => Some(row().h(20.0).px(7.0).radius(6.0).center_y().fill(c.secondary.alpha(0.16)).max_w(360.0).min_w(0.0)
            .child(text(prompt).font(CHIP).color(c.foreground).truncate())),
    };
    let mut line = row().h(QUERY_H).pl(5.0).pr(9.0).gap(6.0).center_y().radius(9.0).fill(c.input);
    if let Some(chip) = chip { line = line.child(chip); } else { line = line.pl(9.0); }
    el().w_full().padding(8.0).pb(5.0).child(line.child(field).w_full())
}

fn source_strip(menu: &Menu, c: &Palette) -> El {
    let model = &menu.model;
    let sources: Vec<_> = model.options().configuration.sources().iter().filter(|s| !s.hidden).collect();
    if sources.len() < 2 || model.depth() > 0 { return el(); }
    let mut strip = row().h(32.0).px(8.0).gap(4.0).center_y();
    for source in sources {
        let active = model.source_id() == Some(&source.id);
        let id = source.id.clone();
        let mut button = row().key(format!("source/{}", source.id)).h(24.0).px(8.0).gap(6.0).radius(9.0).center_y()
            .fill(if active { c.input } else { Color::TRANSPARENT })
            .on_press_tx(move |m: &mut Menu, tx: &mut Tx| m.handle(Event::SwitchSource(id.clone()), tx))
            .child(text(source.label.clone()).font(CHIP).color(if active { c.foreground } else { c.secondary }));
        if let Some(chord) = source.key.as_ref() {
            button = button.child(text(chord.display()).font(CHIP).color(c.secondary.alpha(0.6)));
        }
        strip = strip.child(button);
    }
    strip
}

fn results(menu: &Menu, c: &Palette) -> El {
    let model = &menu.model;
    let count = model.result_count().min(LIST_CAP);
    if count == 0 {
        let (message, color) = if model.input_failed() {
            // The whole error, wrapped, where there's room for it; the footer only has a line.
            (model.message().unwrap_or("Input failed"), c.error)
        } else if model.loading() {
            ("Loading…", c.secondary)
        } else if model.is_searching() {
            ("", c.secondary)
        } else {
            ("No matches", c.secondary)
        };
        return col().grow(1.0).pt(28.0).px(24.0).items_center()
            .child(text(message.to_owned()).font(TITLE).color(color).wrap().max_lines(6));
    }
    let c = *c;
    let rows = col().key(ROWS_KEY).w_full().items_stretch().growth(GrowthSpec::default())
        .virtual_rows(count, ROW_H,
            |m: &Menu, i| Cow::Owned(m.model.result(i).id.clone()),
            move |m: &Menu, i| result_row(m, i, &c))
        .virtual_rows_version((model.result_revision(), count));
    col().key("scroller").scroll(ScrollSpec { stick_bottom: false }).w_full().grow(1.0).min_h(0.0).pb(4.0)
        .child(rows)
}

fn result_row(menu: &Menu, i: usize, c: &Palette) -> El {
    let model = &menu.model;
    let item = model.result(i);
    let selected = model.highlighted_row() == Some(i);
    let marked = model.is_marked(item);
    let subtitle = item.subtitle.as_deref().filter(|s| !s.is_empty());
    let (text_color, meta) = if selected { (c.selected_text, c.selected_text.alpha(0.75)) } else { (c.foreground, c.secondary) };
    let query = model.shown_query();
    let mark = if selected { c.selected_text } else { c.accent };
    let (title_marks, subtitle_marks) = rmenu_core::search::item_highlights(query, item);
    let mut labels = col().grow(1.0).min_w(0.0).gap(0.0)
        .child(matched(item.title(), title_marks, TITLE, text_color, mark));
    if let Some(subtitle) = subtitle {
        labels = labels.child(matched(subtitle, subtitle_marks, SMALL, meta, mark));
    }
    let mut line = row().w_full().h_full().px(10.0).gap(8.0).center_y().radius(8.0)
        .fill(if selected { c.accent } else if marked { c.accent.alpha(0.2) } else { Color::TRANSPARENT });
    if selected && marked { line = line.border(1.0, c.selected_text.alpha(0.65)); }
    if model.options().multi {
        line = line.child(text(if marked { "●" } else { "○" }).font(SMALL).color(if marked { text_color } else { meta.alpha(0.5) }));
    }
    line = line.child(labels);
    if let Some(detail) = item.detail.as_deref().filter(|d| !d.is_empty()) {
        line = line.child(text(detail.to_owned()).font(SMALL).color(meta).max_w(110.0).truncate());
    }
    let id = item.id.clone();
    // Handlers find their row by item ID: the index a virtual row was built at can go stale.
    let row_for = move |m: &Menu| (0..m.model.result_count()).find(|&r| m.model.result(r).id == id);
    let line = if selected { line } else { line.hover(c.foreground.alpha(0.05)) };
    el().w_full().h(if subtitle.is_some() { ROW_SUB_H } else { ROW_H }).px(6.0).py(1.0)
        .on_press_tx(move |m: &mut Menu, tx: &mut Tx| {
            // Typing after a click goes to the query, as it does in mmenu.
            m.field.focused = true;
            let Some(r) = row_for(m) else { return };
            m.handle(Event::Highlight(r), tx);
            if tx.mods().ctrl && m.model.options().multi { m.handle(Event::ToggleRow(r), tx); }
        })
        .on_double_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Accept), tx))
        .child(line)
}

/// One line of row text: the characters the query matched in bold accent, the middle elided
/// when it doesn't fit, so both ends of a path stay readable.
fn matched(content: &str, marks: Vec<std::ops::Range<usize>>, style: TextStyle, color: Color, mark: Color) -> El {
    let spans: Vec<Span> = marks.into_iter()
        .map(|r| Span::new(r.start, r.end).color(mark).weight(650.0)).collect();
    let line = if spans.is_empty() { text(content.to_owned()) } else { rich(content.to_owned(), spans).single_line() };
    line.font(style).color(color).truncate_middle()
}

fn preview_pane(preview: &str, c: &Palette, width: f32) -> El {
    let body = if preview.is_empty() { String::new() } else { plain_text(preview) };
    col().key("preview").w(width).h_full().scroll(ScrollSpec { stick_bottom: false })
        .child(el().px(12.0).py(10.0).child(text(body).font(MONO).color(c.foreground).wrap().selectable(true)))
}

fn footer(menu: &Menu, c: &Palette, width: f32) -> El {
    let model = &menu.model;
    let status = match model.message() {
        Some(message) => message.to_owned(),
        None => {
            let marked = model.marked_count();
            let mut s = format!("{} {}", model.result_count(), if model.result_count() == 1 { "result" } else { "results" });
            if marked > 0 { s += &format!(" · {marked} selected"); }
            if model.loading() { s += " · loading"; }
            s
        }
    };
    let failed = model.input_failed() || model.message_is_error();
    // Enter during a search is queued by the model, so Choose stays lit while one runs.
    let can_act = model.can_navigate() && !model.running_operation() && model.has_choice();
    let back = if model.depth() > 0 { "Back" } else { "" };
    let mut bar = row().h(FOOTER_H).w_full().px(14.0).gap(14.0).center_y()
        .child(keycap_button(back, "Esc", true, c)
            .on_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Back), tx)))
        .child(text(status).font(SMALL).color(if failed { c.error } else { c.secondary }).truncate().grow(1.0).min_w(0.0));
    let bindings: Vec<(usize, &str, String)> = model.options().configuration.bindings().iter().enumerate()
        .filter(|(_, b)| b.applies(model.source_id()))
        .filter_map(|(i, b)| Some((i, b.label.as_deref()?, b.key.display())))
        .collect();
    // Esc, Choose and a readable status always fit: what's left goes to bindings, labelled
    // when they all fit that way, keycaps only when that's what fits, the rightmost dropped
    // after that.
    let fixed = 28.0 + keycap_width(back, "Esc") + keycap_width("Choose", "↩") + STATUS_MIN_W + 2.0 * 14.0;
    let room = (width - fixed).max(0.0);
    let cost = |labelled: bool, n: usize| bindings[..n].iter()
        .map(|(_, label, key)| 14.0 + keycap_width(if labelled { label } else { "" }, key)).sum::<f32>();
    let labelled = cost(true, bindings.len()) <= room;
    let shown = (0..=bindings.len()).rev().find(|&n| cost(labelled, n) <= room).unwrap_or(0);
    for (index, label, key) in &bindings[..shown] {
        let index = *index;
        bar = bar.child(keycap_button(if labelled { label } else { "" }, key, true, c)
            .on_press_tx(move |m: &mut Menu, tx: &mut Tx| m.handle(Event::Perform(index), tx)));
    }
    bar = bar.child(keycap_button("Choose", "↩", can_act, c)
        .on_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Accept), tx)));
    col().w_full().child(el().h(1.0).w_full().fill(c.separator)).child(bar)
}

/// The status line's narrowest useful width in the footer.
const STATUS_MIN_W: f32 = 120.0;

/// The width `keycap_button` lays out for `label` and `key`.
fn keycap_width(label: &str, key: &str) -> f32 {
    let label_w = if label.is_empty() { 0.0 } else { rune::text::measure(label, &[], SMALL, None).0 + 6.0 };
    label_w + (rune::text::measure(key, &[], KEYCAP, None).0 + 10.0).max(18.0)
}

fn keycap_button(label: &str, key: &str, enabled: bool, c: &Palette) -> El {
    let ink = if enabled { c.secondary } else { c.secondary.alpha(0.45) };
    let mut b = row().h(20.0).gap(6.0).center_y();
    if !label.is_empty() { b = b.child(text(label.to_owned()).font(SMALL).color(ink)); }
    b.child(row().h(18.0).min_w(18.0).px(5.0).radius(4.5).center().border(1.0, c.foreground.alpha(0.16))
        .child(text(key.to_owned()).font(KEYCAP).color(ink)))
}

/// The text a terminal would show for `text`: escape sequences and C1 controls removed, and a
/// carriage return writing the rest of its line over the line's start. Tabs and newlines stay.
pub fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut line: Vec<char> = Vec::new();
    let mut column = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        // A C1 control is the 7-bit `ESC` plus the character 0x40 below it.
        let escape = match ch {
            '\u{1b}' => match chars.peek() {
                Some(&next) if ('@'..='_').contains(&next) => { chars.next(); Some(next) }
                _ => None,
            },
            '\u{80}'..='\u{9f}' => Some(char::from(ch as u8 - 0x40)),
            _ => {
                match ch {
                    '\r' => column = 0,
                    '\n' => {
                        out.extend(line.drain(..));
                        out.push('\n');
                        column = 0;
                    }
                    _ => {
                        if column < line.len() { line[column] = ch; } else { line.push(ch); }
                        column += 1;
                    }
                }
                continue;
            }
        };
        match escape {
            Some('[') => {
                while chars.next_if(|c| ('0'..='?').contains(c)).is_some() {}
                while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                chars.next_if(|c| ('@'..='~').contains(c));
            }
            // OSC, DCS, SOS, PM and APC run to a string terminator. Some programs end OSC with BEL.
            Some(']' | 'P' | 'X' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    match c {
                        '\u{7}' | '\u{9c}' => break,
                        '\u{1b}' => { chars.next_if_eq(&'\\'); break; }
                        _ => {}
                    }
                }
            }
            Some(_) => {}
            // `ESC ( B` picks a character set. Others like it take intermediates, then a final.
            None => {
                while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                chars.next_if(|c| ('0'..='~').contains(c));
            }
        }
    }
    out.extend(line);
    out
}

/// The terminal reports Ctrl as both `ctrl` and `cmd`; a chord sees only Control.
pub fn chord(event: &KeyEvent) -> Option<KeyChord> {
    let key = match &event.key {
        Key::Enter => "enter".to_owned(),
        Key::Escape => "escape".to_owned(),
        Key::Tab => "tab".to_owned(),
        Key::Up => "up".to_owned(),
        Key::Down => "down".to_owned(),
        Key::Command(' ') => "space".to_owned(),
        Key::Text(t) if t == " " => "space".to_owned(),
        Key::Command(c) => c.to_lowercase().to_string(),
        Key::Text(t) if t.chars().count() == 1 => t.to_lowercase(),
        _ => return None,
    };
    let m = event.mods;
    Some(KeyChord { key, command: false, control: m.ctrl, option: m.alt, shift: m.shift })
}

/// Configured chords first, then the palette's own keys; true when the key was taken.
pub fn key(menu: &mut Menu, event: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(chord) = chord(event)
        && (chord.control || chord.option || chord.key.chars().count() > 1)
        && let Some(target) = menu.model.target(&chord)
    {
        let event = match target {
            Target::Perform(i) => Event::Perform(i),
            Target::SwitchSource(id) => Event::SwitchSource(id),
        };
        menu.handle(event, tx);
        return true;
    }
    let m = event.mods;
    let plain = !m.ctrl && !m.alt && !m.shift;
    let ctrl_only = m.ctrl && !m.alt && !m.shift;
    let nav = match &event.key {
        Key::Enter if plain => Nav::Accept,
        Key::Escape if plain => Nav::Back,
        Key::Up if plain => Nav::Up,
        Key::Down if plain => Nav::Down,
        Key::PageUp if plain => Nav::PageUp,
        Key::PageDown if plain => Nav::PageDown,
        Key::Tab if plain => Nav::Tab,
        Key::Tab if m.shift && !m.ctrl && !m.alt => Nav::BackTab,
        Key::Command('n') if ctrl_only => Nav::Down,
        Key::Command('p') if ctrl_only => Nav::Up,
        Key::Command('c') if ctrl_only => Nav::Cancel,
        Key::Backspace if plain && menu.field.text.is_empty() => Nav::PopIfEmpty,
        _ => return false,
    };
    menu.handle(Event::Nav(nav), tx);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    #[test]
    fn control_sequences_are_stripped_and_text_is_kept() {
        for (input, expected) in [
            ("\u{1b}[1;31mred\u{1b}[0m plain", "red plain"),
            ("\u{1b}[38;2;10;20;30mtrue\u{1b}[m", "true"),
            ("\u{1b}]8;;https://example.com\u{7}link\u{1b}]8;;\u{7}", "link"),
            ("\u{1b}]0;title\u{1b}\\body", "body"),
            ("\u{1b}[2K\u{1b}[1Gline", "line"),
            ("unicode 日本語 é stays", "unicode 日本語 é stays"),
            ("tabs\tand\nnewlines stay", "tabs\tand\nnewlines stay"),
        ] {
            assert_eq!(plain_text(input), expected);
        }
    }

    #[test]
    fn escapes_beyond_csi_and_osc_are_stripped() {
        for (input, expected) in [
            // `tput -T xterm-256color bold` and `sgr0`.
            ("\u{1b}[1mbold\u{1b}(B\u{1b}[m plain", "bold plain"),
            ("\u{1b}(0lqk\u{1b}(B", "lqk"),
            ("\u{1b})0\u{1b}#8x", "x"),
            ("\u{1b}7a\u{1b}8\u{1b}=\u{1b}>\u{1b}c", "a"),
            ("\u{9b}31mc1\u{9b}0m", "c1"),
            ("\u{1b}Pq#0;2;0;0;0\u{1b}\\after", "after"),
            ("\u{1b}_Gf=24;AAAA\u{1b}\\img", "img"),
            ("\u{1b}^pm\u{7}\u{1b}Xsos\u{1b}\\x", "x"),
            ("\u{9d}0;title\u{9c}x\u{90}dcs\u{9c}", "x"),
            ("x\u{1b}", "x"),
            ("a\u{1b}\nb", "a\nb"),
        ] {
            assert_eq!(plain_text(input), expected, "{input:?}");
        }
    }

    #[test]
    fn carriage_return_writes_over_the_start_of_its_line() {
        for (input, expected) in [
            ("line\r\nnext", "line\nnext"),
            ("10%\r100%\ndone", "100%\ndone"),
            ("abc\rX", "Xbc"),
        ] {
            assert_eq!(plain_text(input), expected, "{input:?}");
        }
    }

    #[test]
    fn a_keycap_claims_the_room_its_text_takes() {
        for (label, key) in [("Choose", "↩"), ("", "Esc"), ("WWWWWW", "W"), ("打开文件夹", "⌃⇧O")] {
            let label_w = if label.is_empty() { 0.0 } else { rune::text::measure(label, &[], SMALL, None).0 };
            let key_w = rune::text::measure(key, &[], KEYCAP, None).0;
            assert!(keycap_width(label, key) >= label_w + key_w, "{label:?} {key:?}");
        }
    }

    #[test]
    fn terminal_ctrl_becomes_a_control_chord_without_command() {
        let event = KeyEvent { key: Key::Command('w'), mods: Mods { ctrl: true, cmd: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("ctrl+w").unwrap()));
        let event = KeyEvent { key: Key::Text("n".into()), mods: Mods { alt: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("alt+n").unwrap()));
        let event = KeyEvent { key: Key::Enter, mods: Mods { ctrl: true, cmd: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("ctrl+enter").unwrap()));
    }
}
