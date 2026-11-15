use rune::{Key, KeyEvent, Mods};
use serde_json::Value;

#[derive(Debug)]
pub(super) enum Command {
    Info,
    Elements,
    Tree,
    Screenshot,
    Logs,
    Focus(String),
    Activate(String),
    Text { key: String, text: String },
    Key(KeyEvent),
    Scroll { key: String, x: f32, y: f32 },
    Pick { x: f32, y: f32 },
}

pub(super) fn parse(value: Value) -> Result<Command, String> {
    let object = value.as_object().ok_or("request must be an object")?;
    let op = string(&value, "op")?;
    let allowed: &[&str] = match op {
        "info" | "elements" | "tree" | "screenshot" | "logs" => &["op"],
        "focus" | "activate" => &["op", "key"],
        "text" => &["op", "key", "text"],
        "key" => &["op", "key", "mods"],
        "scroll" => &["op", "key", "x", "y"],
        "pick" => &["op", "x", "y"],
        _ => return Err(format!("unsupported operation: {op}")),
    };
    if let Some(name) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unexpected field: {name}"));
    }
    Ok(match op {
        "info" => Command::Info,
        "elements" => Command::Elements,
        "tree" => Command::Tree,
        "screenshot" => Command::Screenshot,
        "logs" => Command::Logs,
        "focus" => Command::Focus(target(&value)?),
        "activate" => Command::Activate(target(&value)?),
        "text" => {
            let text = string(&value, "text")?;
            if text.len() > 16 * 1024 {
                return Err("text exceeds 16 KiB".into());
            }
            Command::Text {
                key: target(&value)?,
                text: text.into(),
            }
        }
        "key" => Command::Key(key_event(&value)?),
        "scroll" => Command::Scroll {
            key: target(&value)?,
            x: coordinate(&value, "x")?,
            y: coordinate(&value, "y")?,
        },
        "pick" => Command::Pick {
            x: coordinate(&value, "x")?,
            y: coordinate(&value, "y")?,
        },
        _ => unreachable!(),
    })
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))
}

fn target(value: &Value) -> Result<String, String> {
    let key = string(value, "key")?;
    if key.is_empty() || key.len() > 512 {
        return Err("key must contain 1..512 bytes".into());
    }
    Ok(key.into())
}

fn coordinate(value: &Value, name: &str) -> Result<f32, String> {
    let n = value
        .get(name)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{name} must be a number"))?;
    if !n.is_finite() || n.abs() > 1_000_000.0 {
        return Err(format!("{name} is outside ±1000000"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(n as f32)
}

fn key_event(value: &Value) -> Result<KeyEvent, String> {
    let mut mods = Mods::default();
    if let Some(list) = value.get("mods") {
        for modifier in list.as_array().ok_or("mods must be an array")? {
            match modifier.as_str() {
                Some("shift") => mods.shift = true,
                Some("cmd") => mods.cmd = true,
                Some("ctrl") => mods.ctrl = true,
                Some("alt") => mods.alt = true,
                Some("logo") => mods.logo = true,
                _ => return Err("modifiers are shift, cmd, ctrl, alt, logo".into()),
            }
        }
    }
    let name = string(value, "key")?;
    let key = match name {
        "Tab" => Key::Tab,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Space" => Key::Text(" ".into()),
        _ if name.chars().count() == 1 => {
            let ch = name.chars().next().ok_or("empty key")?;
            if ch.is_control() {
                return Err("use a named control key".into());
            }
            if mods.cmd {
                Key::Command(ch.to_ascii_lowercase())
            } else {
                Key::Text(name.into())
            }
        }
        _ => return Err("unknown key; use a named key or one character".into()),
    };
    Ok(KeyEvent { key, mods })
}
