# Build terminal interfaces with pane-ui

pane-ui provides components in the style of Rex, Superlogical's terminal multiplexer.
Programs draw inside a Rex pane or another terminal with the kitty graphics protocol,
through Rune's terminal shell. The theme uses the terminal's colours and takes its
cursor colour as the accent when the terminal reports one.

## Compose controls and resize the page

Compound components use builders that return a Rune `El` from `.build(..)`.
Small components, such as `heading`, `kbd`, and `progress_bar`, are functions that
return an `El` directly. Element slots include `SettingsRow::control`,
`EmptyState::action`, `Toast::leading`, and `SettingsGroup::child`.
`SettingsGroup::row` takes a `SettingsRow` instead.

Attach handlers through the builder. A disabled control omits its handlers and
cannot take keyboard focus. Controls have preferred widths and shrink to fit.
Use `.fill()` on text fields, selects, sliders, or segmented controls to use the
available width. Settings rows move controls under their titles in narrow panes,
and `Columns` wraps columns below their minimum widths.

## Construct a theme before building elements

`Theme` contains shared colour, radius, and text-style tokens. Components also
use fixed sizes, type styles, and opacity values, so not every visual property
has a theme token.

`Theme::new(palette)` derives the tokens and `theme.kit` once. Construct a new
`Theme` when you change the palette. Changing a field on an existing theme only
affects subsequently built elements that read that field. It does not update
other derived fields, the kit, or elements you already built. For example,
changing `theme.accent` does not change `theme.accent_hover` or
`theme.kit.c.bg_accent`.

Inside a terminal with `RUNE_TERMINAL=1` and Rune's `terminal` feature,
`Theme::current` reads `rune::terminal_colors()`. Call it in the view so Rune can
rebuild the view when the terminal reports new colours. Until the terminal
reports its background and foreground, `Theme::current` uses Merino in the
requested appearance. It also uses Merino outside a terminal.

For a fixed theme, pass `Palette::merino_dark()`, `Palette::merino_light()`, or
`Palette::charcoal_atelier()` to `Theme::new`. Use `Palette::new` to supply your
own background, foreground, cursor, and ANSI colours.

`Toggle` and `spinner` use rune-kit's switch and spinner. `theme.kit` contains the
kit derived from the same palette, with white switch knobs and pane-ui radii.
Use that kit for other rune-kit components. Tooltips use Rune's `.help("...")`
method.

## Build a settings view

```rust,no_run
use pane_ui::{Button, Page, SettingsGroup, SettingsRow, Slider, Theme, Toggle, heading, icons};
use rune::{Cx, El, row};

struct App {
	notify: bool,
	volume: f32,
	running: bool,
}

impl App {
	fn run(&mut self) {
		self.running = true;
	}
}

fn view(s: &App, cx: &mut Cx<App>) -> El {
	let theme = Theme::current(cx.system_dark());
	Page::new()
		.child(heading("Settings", &theme))
		.child(SettingsGroup::new()
			.row(SettingsRow::new("Notify when done")
				.description("Ring the bell when a run finishes.")
				.control(Toggle::new(s.notify, "Notify")
					.on_toggle(|s: &mut App| s.notify = !s.notify).build(&theme)))
			.row(SettingsRow::new("Volume")
				.control(Slider::new(s.volume)
					.on_change(|s: &mut App, v| s.volume = v).build("volume", &theme)))
			.build(&theme))
		.child(row().flex_wrap().gaps(8.0, 8.0)
			.child(Button::new("Run").icon(&icons::PLAY)
				.on_press(|s: &mut App| s.run()).build("run", &theme)))
		.build(&theme)
}
```

`cargo test -p pane-ui --doc` compiles this example as part of the crate documentation.

## Choose a component

| Group | Components |
| --- | --- |
| Buttons | `Button`, with five variants, three sizes, and `icon_only` |
| Form | `TextField`, `Toggle`, `Checkbox`, `Slider`, `Swatch` |
| Choice | `SegmentedControl`, also used as tabs, and `Select` |
| Display | `Badge`, `Chip`, `token`, `kbd`, `Status`, `spinner`, `Toast`, `EmptyState` |
| Settings | `SettingsGroup`, `SettingsRow` |
| Lists | `List`, `ListRow`, `progress_bar` |
| Dialogs | `Dialog`, `modal_layer` |
| Menus | `Menu`, `MenuItem`, `menu_section`, `menu_separator`, `popover` |
| Command palette | `CommandPalette`, `PaletteEntry`, `palette_matches`, `PaletteKey` |
| Layout | `Page`, `heading`, `Columns`, `toast_region`, `modal_layer` |

Rex reserves Command-key combinations and its configured shortcuts. Bind program
shortcuts to Ctrl or plain keys.

## Run the component gallery

From the workspace root, run:

```sh
RUNE_TERMINAL=1 cargo run --release -p gallery
```

`apps/gallery` displays pane-ui components. Ctrl-P opens the command palette.
Make the pane narrow to see the layout wrap.

Icons are a subset of [Lucide](https://lucide.dev) under the ISC licence.
