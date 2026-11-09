//! An interactive gallery of `pane-ui` components on one scrolling page.
//! It includes tabs, buttons, progress, settings, lists, menus, checkboxes, status indicators,
//! badges, a colour slider, toasts, a dialog, and a command palette.
//!
//! `RUNE_TERMINAL=1 cargo run --release -p gallery` runs it in a terminal with
//! the kitty graphics protocol. Ctrl-P opens the command palette.
//! Colours follow the terminal's theme. Make the pane narrow to see the
//! layout stack.

use std::time::Duration;

use pane_ui::{
    Badge, Button, Checkbox, Chip, Columns, CommandPalette, Dialog, EmptyState, List, ListRow,
    Menu, MenuItem, Page, Palette, PaletteEntry, PaletteKey, SegmentedControl, Select,
    SettingsGroup, SettingsRow, Slider, Status, Swatch, TextField, Theme, Toast, Toggle, Tone,
    heading, icons, kbd, modal_layer, palette_matches, progress_bar, spinner, toast_region, token,
};
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, TextEdit, Tx, col, el, icon, row, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeChoice {
    Terminal,
    Light,
    Dark,
}

impl ThemeChoice {
    const ALL: [Self; 3] = [Self::Terminal, Self::Light, Self::Dark];

    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Follow terminal",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn palette(self) -> Option<Palette> {
        match self {
            Self::Terminal => None,
            Self::Light => Some(Palette::merino_light()),
            Self::Dark => Some(Palette::merino_dark()),
        }
    }
}

#[derive(Clone, Copy)]
enum CommandAction {
    Toast,
    Theme(ThemeChoice),
}

#[derive(Clone, Copy)]
struct Command {
    entry: PaletteEntry,
    action: CommandAction,
}

const COMMANDS: [Command; 8] = [
    Command {
        entry: PaletteEntry::new("Run tests").shortcut("⌃R"),
        action: CommandAction::Toast,
    },
    Command {
        entry: PaletteEntry::new("Deploy to staging").submenu(),
        action: CommandAction::Toast,
    },
    Command {
        entry: PaletteEntry::new("Open logs"),
        action: CommandAction::Toast,
    },
    Command {
        entry: PaletteEntry::new("Restart server").shortcut("⌃⇧R"),
        action: CommandAction::Toast,
    },
    Command {
        entry: PaletteEntry::new("Clear cache"),
        action: CommandAction::Toast,
    },
    Command {
        entry: PaletteEntry::new("Follow terminal theme").section("Appearance"),
        action: CommandAction::Theme(ThemeChoice::Terminal),
    },
    Command {
        entry: PaletteEntry::new("Light theme").section("Appearance"),
        action: CommandAction::Theme(ThemeChoice::Light),
    },
    Command {
        entry: PaletteEntry::new("Dark theme").section("Appearance"),
        action: CommandAction::Theme(ThemeChoice::Dark),
    },
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

struct File {
    name: &'static str,
    detail: &'static str,
    changed: bool,
}

const FILES: [File; 6] = [
    File {
        name: "Cargo.toml",
        detail: "1.2 KB · 2 min ago",
        changed: true,
    },
    File {
        name: "README.md",
        detail: "4.8 KB · yesterday",
        changed: false,
    },
    File {
        name: "src/main.rs",
        detail: "12 KB · 5 min ago",
        changed: true,
    },
    File {
        name: "src/server.rs",
        detail: "7.1 KB · last week",
        changed: false,
    },
    File {
        name: "tests/api.rs",
        detail: "3.3 KB · 3 days ago",
        changed: false,
    },
    File {
        name: "deploy/staging.yaml",
        detail: "880 B · just now",
        changed: true,
    },
];

/// ANSI slots offered as label colours.
const LABEL_COLORS: [usize; 6] = [1, 3, 2, 6, 4, 5];

const RUN_SECONDS: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    None,
    ThemeSelect,
    Palette,
    ConfirmDelete,
}

enum Run {
    Idle,
    Starting,
    Working { started: f64, now: f64 },
    Done,
}

impl Run {
    fn status(&self) -> Status {
        match self {
            Self::Idle => Status::Idle,
            Self::Starting | Self::Working { .. } => Status::Working,
            Self::Done => Status::Done,
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "progress is clamped to 0..=1 for the f32 renderer"
    )]
    fn progress(&self) -> f32 {
        match self {
            Self::Idle | Self::Starting => 0.0,
            Self::Working { started, now } => {
                ((now - started) / RUN_SECONDS).clamp(0.0, 1.0) as f32
            }
            Self::Done => 1.0,
        }
    }

    fn advance(&mut self, time: f64) -> bool {
        match self {
            Self::Idle | Self::Done => return false,
            Self::Starting => {
                *self = Self::Working {
                    started: time,
                    now: time,
                }
            }
            Self::Working { started, now } => {
                if time - *started >= RUN_SECONDS {
                    *self = Self::Done;
                } else {
                    *now = time;
                }
            }
        }
        true
    }
}

struct State {
    filter: TextEdit,
    name: TextEdit,
    query: TextEdit,
    overlay: Overlay,
    selected: usize,
    tab: usize,
    theme: ThemeChoice,
    file: Option<usize>,
    run: Run,
    menu_selected: usize,
    sort_by_name: bool,
    notify: bool,
    autosave: bool,
    tasks: [bool; TASKS.len()],
    hue: f32,
    volume: f32,
    swatch: usize,
    toast: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            filter: TextEdit::default(),
            name: TextEdit::new("my-service"),
            query: TextEdit::default(),
            overlay: Overlay::None,
            selected: 0,
            tab: 0,
            theme: ThemeChoice::Terminal,
            file: Some(2),
            run: Run::Idle,
            menu_selected: 0,
            sort_by_name: true,
            notify: true,
            autosave: false,
            tasks: std::array::from_fn(|i| i == 0),
            hue: 0.7,
            volume: 0.4,
            swatch: 4,
            toast: None,
        }
    }
}

impl State {
    fn running(&self) -> bool {
        self.run.status() == Status::Working
    }

    fn show_overlay(&mut self, overlay: Overlay) {
        self.overlay = overlay;
        self.query.focused = overlay == Overlay::Palette;
        if overlay == Overlay::Palette {
            self.selected = 0;
            self.query.clear();
        }
    }

    fn open_palette(&mut self) {
        self.show_overlay(Overlay::Palette);
    }

    fn close_palette(&mut self) {
        self.show_overlay(Overlay::None);
    }

    fn run_command(&mut self, index: usize) {
        self.execute(COMMANDS[index]);
    }

    fn execute(&mut self, command: Command) {
        match command.action {
            CommandAction::Theme(choice) => self.theme = choice,
            CommandAction::Toast => self.toast = Some(command.entry.title.to_owned()),
        }
        self.close_palette();
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let theme = match s.theme.palette() {
        Some(palette) => Theme::new(palette),
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
                    Badge::new(format!(
                        "{} changed",
                        FILES.iter().filter(|f| f.changed).count()
                    ))
                    .tone(Tone::Warning)
                    .build(&theme),
                ),
            files(s, &theme),
        ))
        .child(
            Columns::new(220.0)
                .child(section(heading("Menu", &theme), menu(s, &theme)))
                .child(
                    col()
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
    if s.overlay == Overlay::ConfirmDelete {
        page = page.overlay(delete_confirmation(&theme));
    }
    if s.overlay == Overlay::Palette {
        page = page.overlay(modal_layer(
            "palette-layer",
            palette(s, &theme),
            |s: &mut State| s.close_palette(),
            &theme,
        ));
    }
    page.build(&theme)
}

fn delete_confirmation(theme: &Theme) -> El {
    let dialog = Dialog::new("Delete my-service?")
        .icon(&icons::TRIANGLE_ALERT, Tone::Danger)
        .message("Its settings and run history go too. This can't be undone.")
        .action(
            Button::new("Cancel")
                .gray()
                .on_press(|s: &mut State| s.show_overlay(Overlay::None))
                .build("cancel", theme),
        )
        .action(
            Button::new("Delete")
                .destructive()
                .on_press(|s: &mut State| {
                    s.show_overlay(Overlay::None);
                    s.toast = Some("Deleted my-service".to_owned());
                })
                .build("confirm", theme),
        )
        .build("confirm-delete", theme);
    modal_layer(
        "confirm-layer",
        dialog,
        |s: &mut State| s.show_overlay(Overlay::None),
        theme,
    )
}

fn buttons(s: &State, theme: &Theme) -> El {
    let running = s.running();
    let buttons = cluster(8.0)
        .child(
            Button::new(if running { "Running…" } else { "Run" })
                .icon(&icons::PLAY)
                .disabled(running)
                .on_press(|s: &mut State| {
                    s.run = Run::Starting;
                    s.toast = None;
                })
                .build("run", theme),
        )
        .child(
            Button::new("Stop")
                .tinted()
                .disabled(!running)
                .on_press(|s: &mut State| {
                    s.run = Run::Idle;
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
                .on_press(|s: &mut State| s.show_overlay(Overlay::ConfirmDelete))
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
    col()
        .gap(10.0)
        .child(buttons)
        .child(progress_bar(s.run.progress(), theme))
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
    let theme_picker = Select::new(ThemeChoice::ALL.map(ThemeChoice::label))
        .current(s.theme as usize)
        .open(s.overlay == Overlay::ThemeSelect)
        .width(170.0)
        .on_toggle(|s: &mut State| {
            s.show_overlay(if s.overlay == Overlay::ThemeSelect {
                Overlay::None
            } else {
                Overlay::ThemeSelect
            });
        })
        .on_choose(|s: &mut State, i| {
            s.theme = ThemeChoice::ALL[i];
            s.show_overlay(Overlay::None);
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
        .filter(|(_, file)| file.name.to_lowercase().contains(&filter))
        .map(|(i, file)| {
            let mut row = ListRow::new(file.name)
                .detail(file.detail)
                .selected(s.file == Some(i))
                .leading(icon(file_icon(file.name), 16.0).color(theme.text_secondary))
                .on_press(move |s: &mut State| s.file = Some(i));
            if file.changed {
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
                    Button::new("Clear filter")
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
                .on_press(move |s: &mut State| s.toast = Some(label.to_owned()));
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
    let mut list = col().gap(6.0);
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
    let chip = |status: Status, label: String| {
        row()
            .gap(6.0)
            .center_y()
            .child(status.mark(theme))
            .child(text(label).font(theme.ty.body).color(theme.text))
    };
    let mut r = cluster(14.0).child(chip(
        s.run.status(),
        format!("This program: {}", s.run.status().label()),
    ));
    for status in [Status::Done, Status::Blocked, Status::Error] {
        r = r.child(chip(status, status.label().to_owned()));
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
    let stops: Vec<Color> = (0_u8..7)
        .map(|i| rune::oklch(0.7, 0.16, f32::from(i) * 60.0))
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
    CommandPalette::new(
        &s.query,
        |s: &mut State| &mut s.query,
        &COMMANDS.map(|c| c.entry),
    )
    .selected(s.selected)
    .hint("Run", "↩")
    .hint("Close", "esc")
    .on_key(|s: &mut State, key| {
        let matches = palette_matches(&COMMANDS.map(|c| c.entry), &s.query.text);
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
    if !s.run.advance(now) {
        return false;
    }
    if s.running() {
        tx.wake_after(Duration::from_millis(50));
    } else {
        s.toast = Some("Run finished".to_owned());
    }
    true
}

fn on_key(s: &mut State, k: &KeyEvent, _: &mut Tx) -> bool {
    match k.key {
        Key::Command('p') if k.mods.ctrl => {
            if s.overlay == Overlay::Palette {
                s.close_palette();
            } else {
                s.open_palette();
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
                s.query.text = "te".to_owned();
            }),
            ("running", |s| s.run = Run::Starting),
            ("toast", |s| s.toast = Some("Run finished".to_owned())),
            ("select", |s| s.show_overlay(Overlay::ThemeSelect)),
            ("dialog", |s| s.show_overlay(Overlay::ConfirmDelete)),
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
    fn finishing_a_run_fills_the_progress_bar() {
        let mut state = State {
            run: Run::Starting,
            ..State::default()
        };
        let mut tx = Tx::default();
        assert!(poll(&mut state, 10.0, &mut tx));
        assert!(poll(&mut state, 11.0, &mut tx));
        assert_eq!(state.run.progress(), 0.5);
        assert!(poll(&mut state, 12.0, &mut tx));
        assert_eq!(state.run.status(), Status::Done);
        assert_eq!(state.run.progress(), 1.0);
        assert!(!poll(&mut state, 13.0, &mut tx));
    }

    #[test]
    fn stopping_and_restarting_a_run_resets_its_clock() {
        let mut state = State {
            run: Run::Starting,
            ..State::default()
        };
        let mut tx = Tx::default();
        poll(&mut state, 10.0, &mut tx);
        poll(&mut state, 11.0, &mut tx);
        state.run = Run::Idle;
        assert!(!poll(&mut state, 12.0, &mut tx));
        assert_eq!(state.run.progress(), 0.0);
        assert!(state.toast.is_none());
        state.run = Run::Starting;
        assert_eq!(state.run.progress(), 0.0);
        poll(&mut state, 20.0, &mut tx);
        poll(&mut state, 21.0, &mut tx);
        assert_eq!(state.run.status(), Status::Working);
        assert_eq!(state.run.progress(), 0.5);
    }

    #[test]
    fn changing_overlays_updates_palette_focus() {
        let mut state = State::default();
        let overlays = [
            Overlay::None,
            Overlay::ThemeSelect,
            Overlay::Palette,
            Overlay::ConfirmDelete,
        ];
        for before in overlays {
            for after in overlays {
                state.show_overlay(before);
                state.show_overlay(after);
                assert_eq!(state.overlay, after);
                assert_eq!(state.query.focused, after == Overlay::Palette);
            }
        }
        state.query.text = "old search".to_owned();
        state.selected = 2;
        state.open_palette();
        assert!(state.query.text.is_empty());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn theme_commands_do_not_depend_on_display_titles() {
        let mut state = State::default();
        for choice in ThemeChoice::ALL {
            state.open_palette();
            state.execute(Command {
                entry: PaletteEntry::new("Renamed command"),
                action: CommandAction::Theme(choice),
            });
            assert_eq!(state.theme, choice);
            assert_eq!(
                state.theme.palette().is_none(),
                choice == ThemeChoice::Terminal
            );
            assert!(state.toast.is_none());
            assert_eq!(state.overlay, Overlay::None);
        }
    }

    #[test]
    fn every_palette_command_has_its_expected_effect() {
        for (index, command) in COMMANDS.iter().enumerate() {
            let mut state = State::default();
            state.open_palette();
            state.run_command(index);
            assert_eq!(state.overlay, Overlay::None);
            match index {
                0..=4 => assert_eq!(state.toast.as_deref(), Some(command.entry.title)),
                5 => assert_eq!(state.theme, ThemeChoice::Terminal),
                6 => assert_eq!(state.theme, ThemeChoice::Light),
                7 => assert_eq!(state.theme, ThemeChoice::Dark),
                _ => panic!("a new command needs an expected effect"),
            }
        }
    }

    #[test]
    fn file_filter_shows_matching_rows() {
        fn view_files(s: &State, _: &mut Cx<State>) -> El {
            files(s, &Theme::new(Palette::merino_dark()))
        }
        for (query, expected) in [
            ("  README  ", vec!["README.md"]),
            ("SRC/", vec!["src/main.rs", "src/server.rs"]),
            ("nothing-matches", vec![]),
            ("", FILES.iter().map(|file| file.name).collect()),
        ] {
            let state = State {
                filter: TextEdit::new(query),
                ..State::default()
            };
            let mut rt = Runtime::new(state, view_files, (400.0, 500.0), Env::default());
            rt.frame(0.0);
            let shown: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
            let names: Vec<&str> = shown
                .iter()
                .map(String::as_str)
                .filter(|text| FILES.iter().any(|file| file.name == *text))
                .collect();
            assert_eq!(names, expected, "filter {query:?}");
            assert_eq!(
                shown.iter().any(|text| text == "No files match"),
                expected.is_empty()
            );
        }
    }

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
