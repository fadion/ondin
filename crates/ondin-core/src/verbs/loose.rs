//! The short forms an agent may write and `get_tree` answers in, and their
//! expansion into the model's own serde shapes.
//!
//! A colour under `Solid`, `color` or `Color`, or a gradient stop's `color`, may
//! be `"#rgb"`, `"#rgba"`, `"#rrggbb"` or `"#rrggbbaa"`. A fill may be a bare
//! colour; a fill's or stroke's `brush` may be one; and a fill or stroke leaves out
//! what a new one has (shown, mitre join, butt cap, no dashes). A text kind's
//! `style` leaves out what [`TextStyle::default`](crate::TextStyle) has.
//! [`tighten`] writes those forms only where [`loosen`] gives back the same value,
//! so a tree read and written back is the tree it was.

use serde_json::{Map, Value};

/// The keys whose value is one colour in the model's own shape.
const COLOUR_KEYS: [&str; 3] = ["Solid", "color", "Color"];

/// What a stroke has unless it says otherwise, as its serde shape spells it.
fn stroke_defaults() -> [(&'static str, Value); 5] {
    [
        ("visible", Value::Bool(true)),
        ("join", Value::from("Miter")),
        ("cap", Value::from("Butt")),
        ("dashes", Value::Array(Vec::new())),
        ("dash_offset", Value::from(0.0)),
    ]
}

/// A colour written `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
pub(crate) fn hex_colour(s: &str) -> Option<peniko::Color> {
    let digits = s.strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&digits[i..=i], 16).ok().map(|n| n * 17);
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    let [r, g, b, a] = match digits.len() {
        3 => [nibble(0)?, nibble(1)?, nibble(2)?, 255],
        4 => [nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?],
        6 => [byte(0)?, byte(2)?, byte(4)?, 255],
        8 => [byte(0)?, byte(2)?, byte(4)?, byte(6)?],
        _ => return None,
    };
    Some(peniko::Color::from_rgba8(r, g, b, a))
}

/// A colour's components as `#rrggbb`, or `#rrggbbaa` when not opaque, when every
/// component reads back exactly from that; `None` for one that would not.
fn exact_hex(components: &[Value]) -> Option<String> {
    let [r, g, b, a] = components else {
        return None;
    };
    let mut given = [0f32; 4];
    let mut bytes = [0u8; 4];
    for ((f, byte), c) in given.iter_mut().zip(&mut bytes).zip([r, g, b, a]) {
        *f = c.as_f64()? as f32;
        let q = (*f * 255.0).round();
        if !(0.0..=255.0).contains(&q) {
            return None;
        }
        *byte = q as u8;
    }
    let [r, g, b, a] = bytes;
    // Exact as `hex_colour` will read it back, which is peniko's own conversion.
    if peniko::Color::from_rgba8(r, g, b, a).components != given {
        return None;
    }
    Some(match a {
        255 => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
    })
}

/// Expand the short forms in `v`, the value of a parameter or field called `key`,
/// into the model's shapes. What is not a short form is left as it is.
pub(crate) fn loosen(key: &str, v: &mut Value) {
    match v {
        Value::String(s) if COLOUR_KEYS.contains(&key) => {
            if let Some(c) = hex_colour(s) {
                *v = serde_json::to_value(c).unwrap_or_default();
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                match key {
                    "fills" => loosen_paint(item, false),
                    "strokes" => loosen_paint(item, true),
                    _ => {}
                }
                loosen("", item);
            }
        }
        Value::Object(map) => {
            // A gradient stop's colour is the dynamic kind, not a plain one.
            if map.contains_key("offset")
                && let Some(c) = map
                    .get("color")
                    .and_then(Value::as_str)
                    .and_then(hex_colour)
            {
                let c = peniko::color::DynamicColor::from_alpha_color(c);
                map.insert("color".into(), serde_json::to_value(c).unwrap_or_default());
            }
            for (k, child) in map.iter_mut() {
                loosen(k, child);
            }
            if key == "Text" {
                loosen_text_style(map);
            }
        }
        _ => {}
    }
}

/// A fill or stroke written short: a bare colour, a colour as its `brush`, or
/// without the fields a new one has.
fn loosen_paint(item: &mut Value, stroke: bool) {
    if let Value::String(s) = item
        && !stroke
    {
        let brush = std::mem::take(s);
        *item = serde_json::json!({ "brush": brush });
    }
    let Value::Object(map) = item else {
        return;
    };
    if let Some(Value::String(s)) = map.get("brush") {
        let mut solid = Map::new();
        solid.insert("Solid".into(), Value::String(s.clone()));
        map.insert("brush".into(), Value::Object(solid));
    }
    if stroke {
        for (k, d) in stroke_defaults() {
            map.entry(k).or_insert(d);
        }
    } else {
        map.entry("visible").or_insert(Value::Bool(true));
    }
}

/// A text kind's `style`, given or not, completed from the default style.
fn loosen_text_style(text: &mut Map<String, Value>) {
    let Value::Object(default) =
        serde_json::to_value(crate::typography::TextStyle::default()).unwrap_or_default()
    else {
        return;
    };
    let style = text
        .entry("style")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(style) = style {
        for k in ["font_family", "font_size", "weight", "italic"] {
            if let Some(d) = default.get(k) {
                style.entry(k).or_insert_with(|| d.clone());
            }
        }
    }
}

/// Write `v`, the value of a field called `key`, in the short forms wherever
/// [`loosen`] reads them back to the same value.
pub(crate) fn tighten(key: &str, v: &mut Value) {
    match v {
        Value::Array(items) => {
            for item in items.iter_mut() {
                tighten("", item);
                match key {
                    "fills" => tighten_paint(item, false),
                    "strokes" => tighten_paint(item, true),
                    _ => {}
                }
            }
        }
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                tighten(k, child);
            }
            if COLOUR_KEYS.contains(&key)
                && map.len() == 1
                && let Some(Value::Array(components)) = map.get("components")
                && let Some(hex) = exact_hex(components)
            {
                *v = Value::String(hex);
                return;
            }
            if key == "Text"
                && let Some(Value::Object(style)) = map.get_mut("style")
                && style.get("italic") == Some(&Value::Bool(false))
            {
                style.remove("italic");
            }
        }
        _ => {}
    }
}

/// A fill or stroke in its short form: the brush as a bare colour, the fields a
/// new one has left out, and a shown solid fill as the colour alone.
fn tighten_paint(item: &mut Value, stroke: bool) {
    let Value::Object(map) = item else {
        return;
    };
    if let Some(Value::Object(brush)) = map.get("brush")
        && brush.len() == 1
        && let Some(Value::String(hex)) = brush.get("Solid")
    {
        let hex = hex.clone();
        map.insert("brush".into(), Value::String(hex));
    }
    if stroke {
        for (k, d) in stroke_defaults() {
            if map.get(k) == Some(&d) {
                map.remove(k);
            }
        }
    } else if map.get("visible") == Some(&Value::Bool(true)) {
        map.remove("visible");
    }
    if !stroke
        && map.len() == 1
        && let Some(Value::String(hex)) = map.get("brush")
    {
        *item = Value::String(hex.clone());
    }
}
