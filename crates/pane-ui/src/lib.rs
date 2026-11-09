//! Rex-style components for programs that draw inside a Rex pane (or any terminal with the kitty
//! graphics protocol): buttons, fields, toggles, sliders, settings groups, menus, the command
//! palette, badges, toasts and layout, in the terminal's own colours.
//!
//! - **Themable.** Components read only the tokens in [`Theme`]: colours derived from the
//!   terminal's [`Palette`] (its cursor is the accent), radii and type. Change a field to restyle
//!   everything that uses it.
//! - **Composable.** Each component is a builder ending in `.build(..)` that returns an `El`, and
//!   takes other elements in its slots (`.control(..)`, `.action(..)`, `.child(..)`), so bigger
//!   pieces are put together from smaller ones. Handlers go through the builder
//!   (`.on_press(..)`, `.on_toggle(..)`, `.on_change(..)`) so a disabled control drops them.
//! - **Responsive.** Nothing needs a width from the app: controls prefer a width and shrink,
//!   rows wrap, [`Columns`] stack and long text truncates, so the same view works in a wide pane
//!   and a narrow one.
//!
//! ```no_run
//! # use pane_ui::*;
//! # struct App { notify: bool }
//! # impl App { fn run(&mut self) {} }
//! # fn view(s: &App, cx: &mut rune::Cx<App>) -> rune::El {
//! let theme = Theme::current(cx.system_dark());
//! Page::new()
//!     .child(SettingsGroup::new()
//!         .row(SettingsRow::new("Notify when done")
//!             .control(Toggle::new(s.notify, "Notify").on_toggle(|s: &mut App| s.notify = !s.notify).build(&theme)))
//!         .build(&theme))
//!     .child(Button::new("Run").icon(&icons::PLAY).on_press(|s: &mut App| s.run()).build("run", &theme))
//!     .build(&theme)
//! # }
//! ```
//!
//! Inside a terminal (`RUNE_TERMINAL=1`) [`Theme::current`] follows the terminal's colours and
//! re-runs the view when they change. Elsewhere it uses Rex's default theme, Merino.

pub mod icons;

mod button;
mod choice;
mod command_palette;
mod dialog;
mod display;
mod form;
mod layout;
mod list;
mod menu;
mod settings;
mod theme;

#[cfg(test)]
mod tests;

pub use button::*;
pub use choice::*;
pub use command_palette::*;
pub use dialog::*;
pub use display::*;
pub use form::*;
pub use layout::*;
pub use list::*;
pub use menu::*;
pub use settings::*;
pub use theme::*;

fn hairline(theme: &Theme) -> rune::El {
    rune::el().h(1.0).fill(theme.separator)
}

fn with_separator(row: rune::El, separator: Option<rune::El>) -> rune::El {
    match separator {
        Some(separator) => rune::col().child(row).child(separator),
        None => row,
    }
}

type Transform<T = rune::El> = Box<dyn FnOnce(T) -> T>;
type IndexedPress = std::rc::Rc<dyn Fn(usize) -> Transform>;

fn indexed_press<S: 'static>(f: impl Fn(&mut S, usize) + 'static) -> IndexedPress {
    let f = std::rc::Rc::new(f);
    std::rc::Rc::new(move |index| {
        let f = f.clone();
        Box::new(move |element: rune::El| element.on_press(move |state: &mut S| f(state, index)))
    })
}
