//! Every `pane-ui` component on one scrolling page, as a program running in a Rex pane
//! would draw them: tabs, buttons, a progress bar, a settings group with a select, a list, a
//! menu, checkboxes, program status, badges, a colour slider, toasts, a dialog and the command
//! palette.
//!
//! `RUNE_TERMINAL=1 cargo run --release -p gallery` runs it in a terminal with
//! the kitty graphics protocol. Ctrl-P opens the command palette; Tab, Space, Enter and the arrow
//! keys work as usual. Colours follow the terminal's theme. Make the pane narrow to see the
//! layout stack.

use std::time::Duration;

use pane_ui::{
    Badge, Button, Checkbox, Chip, Columns, CommandPalette, Dialog, EmptyState, List, ListRow,
    Menu, MenuItem, Page, Palette, PaletteEntry, PaletteKey, SegmentedControl, Select,
    SettingsGroup, SettingsRow, Slider, Status, Swatch, TextField, Theme, Toast, Toggle, Tone,
    heading, icons, kbd, modal_layer, palette_matches, progress_bar, spinner, toast_region, token,
};
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, TextEdit, Tx, col, el, icon, row, text};

const COMMANDS: &[PaletteEntry] = &[
    PaletteEntry::new("Run Tests").shortcut("⌃R"),
    PaletteEntry::new("Deploy to Staging").submenu(),
    PaletteEntry::new("Open Logs"),
    PaletteEntry::new("Restart Server").shortcut("⌃⇧R"),
    PaletteEntry::new("Clear Cache"),
    PaletteEntry::new("Follow Terminal Theme").section("Appearance"),
    PaletteEntry::new("Light Theme").section("Appearance"),
    PaletteEntry::new("Dark Theme").section("Appearance"),
];

const MENU: [(&str, &rune::Icon, Option<&str>); 5] = [
    ("Open", &icons::FOLDER_OPEN, Some("⌃O")),
    ("Duplicate", &icons::COPY, Some("⌃D")),
    ("Rename…", &icons::PENCIL, None),
    ("Share", &icons::SHARE_2, None),
    ("Delete", &icons::TRASH, Some("⌫")),
];

const TASKS: [&str; 3] = ["Build", "Run tests", "Publish"];

const TABS: [&str; 3] = ["Overview", "Activity", "Settings"];

const THEMES: [&str; 3] = ["Follow terminal", "Light", "Dark"];

/// (name, detail, changed)
const FILES: [(&str, &str, bool); 6] = [
    ("Cargo.toml", "1.2 KB · 2 min ago", true),
    ("README.md", "4.8 KB · yesterday", false),
    ("src/main.rs", "12 KB · 5 min ago", true),
    ("src/server.rs", "7.1 KB · last week", false),
    ("tests/api.rs", "3.3 KB · 3 days ago", false),
    ("deploy/staging.yaml", "880 B · just now", true),
];

/// ANSI slots offered as label colours.
const LABEL_COLORS: [usize; 6] = [1, 3, 2, 6, 4, 5];

const RUN_SECONDS: f64 = 2.0;

struct State {
    filter: TextEdit,
    name: TextEdit,
    query: TextEdit,
    palette_open: bool,
    selected: usize,
    tab: usize,
    theme: usize,
    theme_open: bool,
    confirm_delete: bool,
    file: Option<usize>,
    /// How far the pretend run has got, 0 to 1.
    progress: f32,
    menu_selected: usize,
    sort_by_name: bool,
    notify: bool,
    autosave: bool,
    tasks: [bool; 3],
    status: Status,
    /// When the pretend run finishes, on the app clock; set by `poll` once a run starts.
    run_ends: Option<f64>,
    hue: f32,
    volume: f32,
    swatch: usize,
    toast: Option<String>,
    /// A theme picked from the palette; None follows the terminal.
    fixed: Option<Palette>,
}

impl Default for State {
    fn default() -> Self {
        State {
            filter: TextEdit::default(),
            name: TextEdit::new("my-service"),
            query: TextEdit::default(),
            palette_open: false,
            selected: 0,
            tab: 0,
            theme: 0,
            theme_open: false,
            confirm_delete: false,
            file: Some(2),
            progress: 0.0,
            menu_selected: 0,
            sort_by_name: true,
            notify: true,
            autosave: false,
            tasks: [true, false, false],
            status: Status::Idle,
            run_ends: None,
            hue: 0.7,
            volume: 0.4,
            swatch: 4,
            toast: None,
            fixed: None,
        }
    }
}

impl State {
    fn running(&self) -> bool {
        self.status == Status::Working
    }

    fn open_palette(&mut self) {
        self.palette_open = true;
        self.selected = 0;
        self.query.clear();
        self.query.focused = true;
    }

    fn close_palette(&mut self) {
        self.palette_open = false;
        self.query.focused = false;
    }

    fn set_theme(&mut self, theme: usize) {
        self.theme = theme;
        self.fixed = match theme {
            1 => Some(Palette::merino_light()),
            2 => Some(Palette::merino_dark()),
            _ => None,
        };
    }

    fn run_command(&mut self, index: usize) {
        let title = COMMANDS[index].title;
        match title {
            "Follow Terminal Theme" => self.set_theme(0),
            "Light Theme" => self.set_theme(1),
            "Dark Theme" => self.set_theme(2),
            _ => self.toast = Some(title.to_owned()),
        }
        self.close_palette();
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let theme = match &s.fixed {
        Some(palette) => Theme::new(palette.clone()),
        None => Theme::current(cx.system_dark()),
    };
    let top = row()
        .gap(10.0)
        .center_y()
        .child(
            TextField::search(&s.filter, |s: &mut State| &mut s.filter)
                .placeholder("Filter files")
                .fill()
                .build("filter", &theme)
                .grow(1.0),
        )
        .child(
            Button::new("Commands")
                .plain()
                .small()
                .on_press(|s: &mut State| s.open_palette())
                .build("palette-hint", &theme),
        )
        .child(kbd("⌃P", &theme));
    let mut layer = Vec::new();
    let mut page = Page::new()
        .child(top)
        .child(section(
            heading("Tabs", &theme),
            SegmentedControl::new(TABS)
                .selected(s.tab)
                .on_select(|s: &mut State, i| s.tab = i)
                .build("tabs", &theme),
        ))
        .child(section(heading("Buttons", &theme), buttons(s, &theme)))
        .child(section(
            heading("Settings", &theme),
            settings(s, &theme, &mut layer),
        ))
        .child(section(
            row()
                .gap(8.0)
                .center_y()
                .justify_between()
                .child(heading("Files", &theme))
                .child(
                    Badge::new(format!("{} changed", FILES.iter().filter(|f| f.2).count()))
                        .tone(Tone::Warning)
                        .build(&theme),
                ),
            files(s, &theme),
        ))
        .child(
            Columns::new(220.0)
                .child(section(heading("Menu", &theme), menu(s, &theme)))
                .child(
                    rune::col()
                        .w_full()
                        .gap(12.0)
                        .child(section(heading("Checklist", &theme), checklist(s, &theme)))
                        .child(section(heading("Status", &theme), statuses(s, &theme)))
                        .child(section(heading("Tags", &theme), tags(&theme)))
                        .child(section(heading("Color", &theme), color_picker(s, &theme))),
                )
                .build(),
        )
        .overlays(layer);
    if let Some(message) = &s.toast {
        let dismiss = Button::new("Dismiss")
            .gray()
            .small()
            .on_press(|s: &mut State| s.toast = None)
            .build("dismiss", &theme);
        page = page.overlay(toast_region(
            Toast::new(message.as_str())
                .leading(icon(&icons::CIRCLE_CHECK, 18.0).color(theme.done))
                .action(dismiss)
                .build("toast", &theme),
        ));
    } else if s.running() {
        page = page.overlay(toast_region(
            Toast::new("Running…")
                .leading(spinner(&theme))
                .build("running", &theme),
        ));
    }
    if s.confirm_delete {
        let dialog = Dialog::new("Delete my-service?")
            .icon(&icons::TRIANGLE_ALERT, Tone::Danger)
            .message("Its settings and run history go too. This can't be undone.")
            .action(
                Button::new("Cancel")
                    .gray()
                    .on_press(|s: &mut State| s.confirm_delete = false)
                    .build("cancel", &theme),
            )
            .action(
                Button::new("Delete")
                    .destructive()
                    .on_press(|s: &mut State| {
                        s.confirm_delete = false;
                        s.toast = Some("Deleted my-service".to_owned());
                    })
                    .build("confirm", &theme),
            )
            .build("confirm-delete", &theme);
        page = page.overlay(modal_layer(
            "confirm-layer",
            dialog,
            |s: &mut State| s.confirm_delete = false,
            &theme,
        ));
    }
    if s.palette_open {
        page = page.overlay(modal_layer(
            "palette-layer",
            palette(s, &theme),
            |s: &mut State| s.close_palette(),
            &theme,
        ));
    }
    page.build(&theme)
}

fn buttons(s: &State, theme: &Theme) -> El {
    let running = s.running();
    let buttons = cluster(8.0)
        .child(
            Button::new(if running { "Running…" } else { "Run" })
                .icon(&icons::PLAY)
                .disabled(running)
                .on_press(|s: &mut State| {
                    s.status = Status::Working;
                    s.run_ends = None;
                    s.toast = None;
                })
                .build("run", theme),
        )
        .child(
            Button::new("Stop")
                .tinted()
                .disabled(!running)
                .on_press(|s: &mut State| {
                    s.status = Status::Idle;
                    s.run_ends = None;
                })
                .build("stop", theme),
        )
        .child(
            Button::new("Logs")
                .gray()
                .on_press(|s: &mut State| s.toast = Some("Logs".into()))
                .build("logs", theme),
        )
        .child(
            Button::new("Help")
                .plain()
                .on_press(|s: &mut State| s.toast = Some("Help".into()))
                .build("help", theme),
        )
        .child(
            Button::new("Delete")
                .destructive()
                .on_press(|s: &mut State| s.confirm_delete = true)
                .build("delete", theme),
        )
        .child(
            row()
                .gap(2.0)
                .child(
                    Button::icon_only(&icons::REFRESH_CW, "Refresh")
                        .plain()
                        .on_press(|s: &mut State| s.toast = Some("Refresh".into()))
                        .build("refresh", theme),
                )
                .child(
                    Button::icon_only(&icons::COLUMNS_2, "Split")
                        .plain()
                        .on_press(|s: &mut State| s.toast = Some("Split".into()))
                        .build("split", theme),
                )
                .child(
                    Button::icon_only(&icons::ELLIPSIS, "More")
                        .plain()
                        .small()
                        .on_press(|s: &mut State| s.toast = Some("More".into()))
                        .build("more", theme),
                ),
        );
    if !running {
        return buttons;
    }
    rune::col()
        .gap(10.0)
        .child(buttons)
        .child(progress_bar(s.progress, theme))
}

fn settings(s: &State, theme: &Theme, layer: &mut Vec<El>) -> El {
    let mut swatches = cluster(6.0);
    for (i, slot) in LABEL_COLORS.into_iter().enumerate() {
        swatches = swatches.child(
            Swatch::new(theme.ansi(slot))
                .selected(s.swatch == i)
                .on_press(move |s: &mut State| s.swatch = i)
                .build(&format!("label-{i}"), theme),
        );
    }
    let theme_picker = Select::new(THEMES)
        .current(s.theme)
        .open(s.theme_open)
        .width(170.0)
        .on_toggle(|s: &mut State| s.theme_open = !s.theme_open)
        .on_choose(|s: &mut State, i| {
            s.set_theme(i);
            s.theme_open = false;
        })
        .build("theme", theme, layer);
    SettingsGroup::new()
        .row(
            SettingsRow::new("Theme")
                .description("Rex's colours, or the terminal's.")
                .control(theme_picker),
        )
        .row(
            SettingsRow::new("Name")
                .description("Shown in the pane title.")
                .control(
                    TextField::new(&s.name, |s: &mut State| &mut s.name)
                        .width(180.0)
                        .build("name", theme),
                ),
        )
        .row(
            SettingsRow::new("Notify when done")
                .description("Ring the bell when a run finishes.")
                .control(
                    Toggle::new(s.notify, "Notify when done")
                        .on_toggle(|s: &mut State| s.notify = !s.notify)
                        .build(theme),
                ),
        )
        .row(
            SettingsRow::new("Save automatically").control(
                Toggle::new(s.autosave, "Save automatically")
                    .on_toggle(|s: &mut State| s.autosave = !s.autosave)
                    .build(theme),
            ),
        )
        .row(SettingsRow::new("Label color").control(swatches))
        .row(
            SettingsRow::new("Volume").control(
                Slider::new(s.volume)
                    .label("Volume")
                    .value_label(format!("{:.0}%", s.volume * 100.0))
                    .width(220.0)
                    .on_change(|s: &mut State, v| s.volume = v)
                    .build("volume", theme),
            ),
        )
        .build(theme)
}

fn files(s: &State, theme: &Theme) -> El {
    let filter = s.filter.text.trim().to_lowercase();
    let rows = FILES
        .iter()
        .enumerate()
        .filter(|(_, (name, _, _))| name.to_lowercase().contains(&filter))
        .map(|(i, (name, detail, changed))| {
            let mut row = ListRow::new(*name)
                .detail(*detail)
                .selected(s.file == Some(i))
                .leading(icon(file_icon(name), 16.0).color(theme.text_secondary))
                .on_press(move |s: &mut State| s.file = Some(i));
            if *changed {
                row = row.trailing(Badge::new("M").tone(Tone::Warning).build(theme));
            }
            row
        });
    List::new()
        .rows(rows)
        .empty(
            EmptyState::new(&icons::INBOX, "No files match")
                .message("Try a shorter filter.")
                .action(
                    Button::new("Clear Filter")
                        .tinted()
                        .on_press(|s: &mut State| s.filter.clear())
                        .build("clear-filter", theme),
                )
                .build(theme),
        )
        .build("files", theme)
}

fn menu(s: &State, theme: &Theme) -> El {
    let items = MENU
        .into_iter()
        .enumerate()
        .map(|(i, (label, glyph, keys))| {
            let mut item = MenuItem::new(label)
                .icon(glyph)
                .selected(s.menu_selected == i)
                .on_hover(move |s: &mut State| s.menu_selected = i)
                .on_press(move |s: &mut State| s.toast = Some(MENU[i].0.to_owned()));
            if let Some(keys) = keys {
                item = item.shortcut(keys);
            }
            item
        });
    Menu::new()
        .items(items)
        .section("Sort by")
        .item(
            MenuItem::new("Name")
                .checked(s.sort_by_name)
                .on_press(|s: &mut State| s.sort_by_name = true),
        )
        .item(
            MenuItem::new("Date modified")
                .checked(!s.sort_by_name)
                .on_press(|s: &mut State| s.sort_by_name = false),
        )
        .build("menu", theme)
}

fn checklist(s: &State, theme: &Theme) -> El {
    let mut list = rune::col().gap(6.0);
    for (i, task) in TASKS.into_iter().enumerate() {
        list = list.child(
            Checkbox::new(s.tasks[i])
                .label(task)
                .on_toggle(move |s: &mut State| s.tasks[i] = !s.tasks[i])
                .build(&format!("task-{i}"), theme),
        );
    }
    list
}

fn statuses(s: &State, theme: &Theme) -> El {
    let mut r = cluster(14.0);
    for (i, status) in [s.status, Status::Done, Status::Blocked, Status::Error]
        .into_iter()
        .enumerate()
    {
        let label = if i == 0 {
            format!("This program: {}", status.label())
        } else {
            status.label().to_owned()
        };
        r = r.child(
            row()
                .gap(6.0)
                .center_y()
                .child(status.mark(theme))
                .child(text(label).font(theme.ty.body).color(theme.text)),
        );
    }
    r
}

fn tags(theme: &Theme) -> El {
    cluster(6.0)
        .child(Chip::new("rust").icon(&icons::CODE).build(theme))
        .child(Badge::new("beta").build(theme))
        .child(Badge::new("3 errors").tone(Tone::Danger).build(theme))
        .child(Badge::new("passing").tone(Tone::Success).build(theme))
        .child(token("main", theme))
}

fn color_picker(s: &State, theme: &Theme) -> El {
    let current = rune::oklch(0.68, 0.16, s.hue * 360.0);
    let stops: Vec<Color> = (0..7)
        .map(|i| rune::oklch(0.7, 0.16, i as f32 * 60.0))
        .collect();
    row()
        .gap(12.0)
        .items_end()
        .w_full()
        .child(el().size(36.0, 36.0).radius(10.0).fill(current))
        .child(
            Slider::new(s.hue)
                .label("Hue")
                .value_label(format!("{:.0}°", s.hue * 360.0))
                .gradient(stops)
                .width(260.0)
                .on_change(|s: &mut State, v| s.hue = v)
                .build("hue", theme),
        )
}

fn palette(s: &State, theme: &Theme) -> El {
    CommandPalette::new(&s.query, |s: &mut State| &mut s.query, COMMANDS)
        .selected(s.selected)
        .hint("Run", "↩")
        .hint("Close", "esc")
        .on_key(|s: &mut State, key| {
            let matches = palette_matches(COMMANDS, &s.query.text);
            match key {
                PaletteKey::Confirm => match matches.get(s.selected) {
                    Some((index, _)) => s.run_command(*index),
                    None => s.close_palette(),
                },
                PaletteKey::Dismiss => s.close_palette(),
                step => s.selected = step.step(s.selected, matches.len()),
            }
        })
        .on_choose(|s: &mut State, index| s.run_command(index))
        .build("palette", theme)
}

/// Finishes the pretend run `RUN_SECONDS` after it starts.
fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    if !s.running() {
        return false;
    }
    let ends = *s.run_ends.get_or_insert(now + RUN_SECONDS);
    if now < ends {
        s.progress = (1.0 - (ends - now) / RUN_SECONDS) as f32;
        tx.wake_after(Duration::from_millis(50));
        return true;
    }
    s.status = Status::Done;
    s.run_ends = None;
    s.toast = Some("Run finished".to_owned());
    true
}

fn on_key(s: &mut State, k: &KeyEvent, _: &mut Tx) -> bool {
    match k.key {
        Key::Command('p') if k.mods.ctrl => {
            if s.palette_open {
                s.close_palette()
            } else {
                s.open_palette()
            }
            true
        }
        _ => false,
    }
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("palette", |s| s.open_palette()),
            ("palette-search", |s| {
                s.open_palette();
                s.query = TextEdit::new("te");
            }),
            ("running", |s| s.status = Status::Working),
            ("toast", |s| s.toast = Some("Run finished".to_owned())),
            ("select", |s| s.theme_open = true),
            ("dialog", |s| s.confirm_delete = true),
            ("no-files", |s| s.filter = TextEdit::new("zzz")),
        ],
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Gallery", (900.0, 900.0), State::default, view)
    });
}

/// A heading and what's under it, as each block of the page is.
fn section(head: El, content: El) -> El {
    col().w_full().gap(10.0).child(head.pt(6.0)).child(content)
}

/// A row of controls that wraps onto more lines when narrow.
fn cluster(gap: f32) -> El {
    row().flex_wrap().center_y().gaps(gap, gap)
}

/// The icon for a file: text for Markdown, a plain page for the rest.
fn file_icon(name: &str) -> &'static rune::Icon {
    let markdown = std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    if markdown {
        &icons::FILE_TEXT
    } else {
        &icons::FILE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn every_button_answers_a_press() {
        for key in ["logs", "help", "refresh", "split", "more"] {
            let mut rt = Runtime::new(State::default(), view, (900.0, 900.0), Env::default());
            rt.frame(0.0);
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(1.0 / 60.0);
            assert!(rt.state.toast.is_some(), "{key} shows a toast");
        }
    }

    #[test]
    fn markdown_files_get_the_text_icon_whatever_their_case() {
        assert!(std::ptr::eq(file_icon("README.md"), &icons::FILE_TEXT));
        assert!(std::ptr::eq(file_icon("NOTES.MD"), &icons::FILE_TEXT));
        assert!(std::ptr::eq(file_icon("main.rs"), &icons::FILE));
    }
}
