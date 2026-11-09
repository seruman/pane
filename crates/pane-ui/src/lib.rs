#![doc = include_str!("../README.md")]

pub mod icons;

mod button;
mod choice;
mod command_palette;
mod dialog;
mod display;
mod form;
mod layout;
mod list;
pub mod markdown;
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

fn split_space(value: f32) -> [rune::El; 2] {
    [
        rune::el().grow(value.max(1e-4)),
        rune::el().grow((1.0 - value).max(1e-4)),
    ]
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
