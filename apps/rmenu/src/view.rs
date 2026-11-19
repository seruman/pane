use std::borrow::Cow;

use rmenu_core::config::KeyChord;
use rmenu_core::model::{BindingIndex, Event, ListState, Nav, Target};
use rune::{
    Color, Cx, El, Family, GrowthSpec, Key, KeyEvent, ScrollSpec, Span, TextStyle, Tx, col, el, rich, row, text,
};

use pane_ui::{Palette, TextField, Theme, kbd, kbd_width};

use crate::cli::Appearance;
use crate::host::Menu;

pub const ROWS_KEY: &str = "rows";

const FALLBACK_WIDTH: f32 = 640.0;
const ROW_H: f32 = 28.0;
const ROW_SUB_H: f32 = 42.0;
const FOOTER_H: f32 = 30.0;
const FOOTER_PAD: f32 = 14.0;
const FOOTER_GAP: f32 = 14.0;
const PREVIEW_W: f32 = 380.0;

const TITLE: TextStyle = TextStyle::new(13.0, 400.0).lh(18.0);
const SMALL: TextStyle = TextStyle::new(11.0, 400.0).lh(15.0);
const CHIP: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
const MONO: TextStyle = TextStyle::new(11.0, 400.0).lh(15.0).family(Family::SystemMono);

fn theme(menu: &Menu, system_dark: bool) -> Theme {
    match menu.appearance {
        Appearance::System => Theme::current(system_dark),
        Appearance::Light => Theme::new(Palette::merino_light()),
        Appearance::Dark => Theme::new(Palette::merino_dark()),
    }
}

pub fn view(menu: &Menu, cx: &mut Cx<Menu>) -> El {
    cx.reduced_motion(true);
    let c = theme(menu, cx.system_dark());
    let model = &menu.model;
    let width = cx.layout_with("root", |r| r[2].round()).unwrap_or(FALLBACK_WIDTH);
    let list = col()
        .key("list")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .items_stretch()
        .child(query_row(menu, &c))
        .child(source_strip(menu, &c))
        .child(results(menu, &c));
    let body = match model.preview() {
        Some(preview) => row()
            .grow(1.0)
            .w_full()
            .min_h(0.0)
            .child(list)
            .child(el().w(1.0).h_full().fill(c.separator))
            .child(preview_pane(preview, &c, PREVIEW_W.min(width / 2.0))),
        None => row().grow(1.0).w_full().min_h(0.0).child(list),
    };
    col().key("root").w_full().h_full().fill(c.background).child(body).child(footer(menu, &c, width))
}

fn query_row(menu: &Menu, c: &Theme) -> El {
    let prompt = menu.model.prompt();
    // The prompt names the field for screen readers; the placeholder disappears once typing starts.
    let label = if prompt.is_empty() { "Search".to_owned() } else { prompt.clone() };
    let mut field = TextField::new(&menu.field, |m: &mut Menu| &mut m.field)
        .label(label)
        .placeholder("Search")
        .fill()
        .invalid(menu.refusal_until.is_some())
        .on_change(|m: &mut Menu, tx: &mut Tx| {
            let query = m.field.text.clone();
            m.handle(Event::SetQuery(query), tx);
        })
        .on_key(key);
    match prompt.as_str() {
        "" => {}
        crate::cli::DEFAULT_PROMPT => {
            field = field.prefix(text(crate::cli::DEFAULT_PROMPT).font(c.ty.body).color(c.text_secondary));
        }
        _ => {
            field = field.prefix(
                row()
                    .h(20.0)
                    .px(7.0)
                    .radius(6.0)
                    .center_y()
                    .fill(c.pressed)
                    .max_w(360.0)
                    .min_w(0.0)
                    .child(text(prompt).font(CHIP).color(c.text).truncate()),
            );
        }
    }
    el().w_full().padding(8.0).pb(5.0).child(field.build("query", c))
}

fn source_strip(menu: &Menu, c: &Theme) -> El {
    let model = &menu.model;
    let sources: Vec<_> = model.options().configuration.sources().iter().filter(|s| !s.hidden).collect();
    if sources.len() < 2 || model.depth() > 0 {
        return el();
    }
    let mut strip = row().h(32.0).px(8.0).gap(4.0).center_y();
    for source in sources {
        let active = model.source_id() == Some(&source.id);
        let id = source.id.clone();
        let mut button = row()
            .key(format!("source/{}", source.id))
            .h(24.0)
            .px(8.0)
            .gap(6.0)
            .radius(c.radius.field)
            .center_y()
            .fill(if active { c.field } else { Color::TRANSPARENT })
            .on_press_tx(move |m: &mut Menu, tx: &mut Tx| m.handle(Event::SwitchSource(id.clone()), tx))
            .child(text(source.label.clone()).font(CHIP).color(if active { c.text } else { c.text_secondary }));
        if let Some(chord) = source.key.as_ref() {
            button = button.child(text(chord.display()).font(CHIP).color(c.text_secondary.alpha(0.6)));
        }
        strip = strip.child(button);
    }
    strip
}

fn results(menu: &Menu, c: &Theme) -> El {
    let model = &menu.model;
    let empty = |message: &str, color| {
        col()
            .grow(1.0)
            .pt(28.0)
            .px(24.0)
            .items_center()
            .child(text(message.to_owned()).font(TITLE).color(color).wrap().max_lines(6))
    };
    let count = match model.list_state() {
        ListState::Rows(count) => count,
        ListState::Failed(message) => return empty(message, c.danger),
        ListState::Loading => return empty("Loading…", c.text_secondary),
        ListState::Searching => return empty("", c.text_secondary),
        ListState::Empty => return empty("No matches", c.text_secondary),
    };
    let c = c.clone();
    let rows = col()
        .key(ROWS_KEY)
        .w_full()
        .items_stretch()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            ROW_H,
            |m: &Menu, i| Cow::Borrowed(m.model.result(i).id.as_str()),
            move |m: &Menu, i| result_row(m, i, &c),
        )
        .virtual_rows_version((model.result_revision(), count));
    col().key("scroller").scroll(ScrollSpec { stick_bottom: false }).w_full().grow(1.0).min_h(0.0).pb(4.0).child(rows)
}

fn result_row(menu: &Menu, i: usize, c: &Theme) -> El {
    let model = &menu.model;
    let item = model.result(i);
    let selected = model.highlighted_row() == Some(i);
    let marked = model.is_marked(item);
    let subtitle = item.subtitle.as_deref().filter(|s| !s.is_empty());
    let (text_color, meta) = if selected { (c.on_accent, c.on_accent.alpha(0.75)) } else { (c.text, c.text_secondary) };
    let query = model.shown_query();
    let mark = if selected { c.on_accent } else { c.accent };
    let (title_marks, subtitle_marks) = rmenu_core::search::item_highlights(query, item);
    let mut labels =
        col().grow(1.0).min_w(0.0).gap(0.0).child(matched(item.title(), title_marks, TITLE, text_color, mark));
    if let Some(subtitle) = subtitle {
        labels = labels.child(matched(subtitle, subtitle_marks, SMALL, meta, mark));
    }
    let mut line = row().w_full().h_full().px(10.0).gap(8.0).center_y().radius(c.radius.row).fill(if selected {
        c.accent
    } else if marked {
        c.accent_soft
    } else {
        Color::TRANSPARENT
    });
    if selected && marked {
        line = line.border(1.0, c.on_accent.alpha(0.65));
    }
    if model.options().multi {
        line = line.child(text(if marked { "●" } else { "○" }).font(SMALL).color(if marked {
            text_color
        } else {
            meta.alpha(0.5)
        }));
    }
    line = line.child(labels);
    if let Some(detail) = item.detail.as_deref().filter(|d| !d.is_empty()) {
        line = line.child(text(detail.to_owned()).font(SMALL).color(meta).max_w(110.0).truncate());
    }
    let id = item.id.clone();
    // A virtual row's position can change before its handler runs.
    let row_for = move |m: &Menu| m.model.listed_row(&id);
    let line = if selected { line } else { line.hover(c.hover) };
    el().w_full()
        .h(if subtitle.is_some() { ROW_SUB_H } else { ROW_H })
        .px(6.0)
        .py(1.0)
        .on_press_tx(move |m: &mut Menu, tx: &mut Tx| {
            m.field.focused = true;
            let Some(r) = row_for(m) else { return };
            let event =
                if tx.mods().ctrl && m.model.options().multi { Event::ToggleRow(r) } else { Event::Highlight(r) };
            m.handle(event, tx);
        })
        .on_double_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Accept), tx))
        .child(line)
}

fn matched(content: &str, marks: Vec<std::ops::Range<usize>>, style: TextStyle, color: Color, mark: Color) -> El {
    let spans: Vec<Span> = marks.into_iter().map(|r| Span::new(r.start, r.end).color(mark).weight(650.0)).collect();
    rich(content.to_owned(), spans).font(style).color(color).truncate_middle()
}

fn preview_pane(preview: &str, c: &Theme, width: f32) -> El {
    let body = preview.to_owned();
    col()
        .key("preview")
        .w(width)
        .h_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .child(el().px(12.0).py(10.0).child(text(body).font(MONO).color(c.text).wrap().selectable(true)))
}

fn footer(menu: &Menu, c: &Theme, width: f32) -> El {
    let model = &menu.model;
    let status = if let Some(message) = model.message() {
        message.to_owned()
    } else {
        let marked = model.marked_count();
        let mut s =
            format!("{} {}", model.result_count(), if model.result_count() == 1 { "result" } else { "results" });
        if marked > 0 {
            s += &format!(" · {marked} selected");
        }
        if model.loading() {
            s += " · loading";
        }
        s
    };
    let failed = model.input_failed() || model.message_is_error();
    let can_act = model.can_navigate() && !model.running_operation() && model.has_choice();
    let back = if model.depth() > 0 { "Back" } else { "" };
    let mut bar = row()
        .h(FOOTER_H)
        .w_full()
        .px(FOOTER_PAD)
        .gap(FOOTER_GAP)
        .center_y()
        .child(
            keycap_button(back, "Esc", true, c)
                .on_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Back), tx)),
        )
        .child(
            text(status)
                .font(SMALL)
                .color(if failed { c.danger } else { c.text_secondary })
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
    let bindings: Vec<(usize, &str, String)> = model
        .options()
        .configuration
        .bindings()
        .iter()
        .enumerate()
        .filter(|(_, b)| b.applies(model.source_id()))
        .filter_map(|(i, b)| Some((i, b.label.as_deref()?, b.key.display())))
        .collect();
    let fixed =
        2.0 * FOOTER_PAD + keycap_width(back, "Esc") + keycap_width("Choose", "↩") + STATUS_MIN_W + 2.0 * FOOTER_GAP;
    let room = (width - fixed).max(0.0);
    let cost = |labelled: bool, n: usize| {
        bindings[..n]
            .iter()
            .map(|(_, label, key)| FOOTER_GAP + keycap_width(if labelled { label } else { "" }, key))
            .sum::<f32>()
    };
    let labelled = cost(true, bindings.len()) <= room;
    let shown = (0..=bindings.len()).rev().find(|&n| cost(labelled, n) <= room).expect("zero bindings always fit");
    for (index, label, key) in &bindings[..shown] {
        let index = *index;
        bar = bar.child(
            keycap_button(if labelled { label } else { "" }, key, true, c)
                .on_press_tx(move |m: &mut Menu, tx: &mut Tx| m.handle(Event::Perform(BindingIndex(index)), tx)),
        );
    }
    bar = bar.child(
        keycap_button("Choose", "↩", can_act, c)
            .on_press_tx(|m: &mut Menu, tx: &mut Tx| m.handle(Event::Nav(Nav::Accept), tx)),
    );
    col().w_full().child(el().h(1.0).w_full().fill(c.separator)).child(bar)
}

const STATUS_MIN_W: f32 = 120.0;

fn keycap_width(label: &str, key: &str) -> f32 {
    let label_w = if label.is_empty() { 0.0 } else { rune::text::measure(label, &[], SMALL, None).0 + 6.0 };
    label_w + kbd_width(key)
}

fn keycap_button(label: &str, key: &str, enabled: bool, c: &Theme) -> El {
    let mut b = row().h(20.0).gap(6.0).center_y();
    if !label.is_empty() {
        b = b.child(text(label.to_owned()).font(SMALL).color(c.text_secondary));
    }
    let b = b.child(kbd(key, c));
    if enabled { b } else { b.opacity(0.45) }
}

// Terminal chords use only Control: Rune reports it as `ctrl` (and as `cmd` off macOS).
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

/// Whether `event` is Control with `key`, as Rune's `Chord` matches it on this platform.
fn ctrl(key: &str, event: &KeyEvent) -> bool {
    rune::Chord::parse(&format!("ctrl+{key}")).is_ok_and(|c| c.matches(event))
}

pub fn key(menu: &mut Menu, event: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(chord) = chord(event)
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
    let nav = match &event.key {
        Key::Enter if plain => Nav::Accept,
        Key::Escape if plain => Nav::Back,
        Key::Up if plain => Nav::Up,
        Key::Down if plain => Nav::Down,
        Key::PageUp if plain => Nav::PageUp,
        Key::PageDown if plain => Nav::PageDown,
        Key::Tab if plain => Nav::Tab,
        Key::Tab if m.shift && !m.ctrl && !m.alt => Nav::BackTab,
        _ if ctrl("n", event) => Nav::Down,
        _ if ctrl("p", event) => Nav::Up,
        _ if ctrl("c", event) => Nav::Cancel,
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
    fn a_keycap_claims_the_room_its_text_takes() {
        for (label, key) in [("Choose", "↩"), ("", "Esc"), ("WWWWWW", "W"), ("打开文件夹", "⌃⇧O")] {
            let label_w = if label.is_empty() { 0.0 } else { rune::text::measure(label, &[], SMALL, None).0 };
            let key_w = rune::text::measure(key, &[], rune::TextStyle::new(11.0, 400.0), None).0;
            assert!(keycap_width(label, key) >= label_w + key_w, "{label:?} {key:?}");
        }
    }

    #[test]
    fn terminal_ctrl_becomes_a_control_chord_without_command() {
        let event = KeyEvent { key: Key::Command('w'), mods: Mods { ctrl: true, cmd: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("ctrl+w").unwrap()));
        // On macOS the terminal reports Control with a letter as typed text with `ctrl`.
        let event = KeyEvent { key: Key::Text("w".into()), mods: Mods { ctrl: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("ctrl+w").unwrap()));
        let event = KeyEvent { key: Key::Text("n".into()), mods: Mods { alt: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("alt+n").unwrap()));
        let event = KeyEvent { key: Key::Enter, mods: Mods { ctrl: true, cmd: true, ..Mods::default() } };
        assert_eq!(chord(&event), Some(KeyChord::parse("ctrl+enter").unwrap()));
    }
}
