use rune::Runtime;
use serde_json::{Value, json};

pub(super) fn snapshot(rt: &Runtime<crate::State>, kind: &str) -> Result<Value, String> {
    let elements = rt.elements();
    if elements.len() > 10_000 {
        return Err("more than 10000 elements".into());
    }
    let elements: Vec<_> = elements
        .into_iter()
        .map(|e| {
            json!({
                "key": e.key, "path": e.path, "parent": e.parent, "role": e.role,
                "label": e.label, "text": e.text, "rect": e.rect, "visible": e.visible,
                "opacity": e.opacity, "focusable": e.focusable, "focused": e.focused,
                "inert": e.inert, "exiting": e.exiting,
            })
        })
        .collect();
    Ok(json!({ "kind": kind, "elements": elements, "accessibility": rt.accessibility_json() }))
}
