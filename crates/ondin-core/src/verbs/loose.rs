//! The short forms an agent may write and `get_tree` answers in, and their
//! expansion into the model's own serde shapes.
//!
//! A colour under `Solid`, `color` or `Color`, or a gradient stop's `color`, may
//! be `"#rgb"`, `"#rgba"`, `"#rrggbb"` or `"#rrggbbaa"`. A fill may be a bare
//! colour; a fill's or stroke's `brush` may be one; and a fill or stroke leaves out
//! what a new one has (shown, mitre join, butt cap, no dashes). A text kind's
//! `style` leaves out what [`TextStyle::default`](crate::TextStyle) has.
//!
//! What `get_node` and CSS write is taken too: a length as `"22px"`, `"1.2em"` or
//! `"auto"`; a sizing as `{"type": "AutoHeight", "width": w}`; a fill as
//! `{"paint": {"type": "Solid", "color", "alpha"}}`; an effect flat, its kind
//! `DropShadow` or `drop_shadow`, its `opacity` folded into its colour, shown
//! unless it says not; a rectangle's corners left out (square) or as one number;
//! padding as one number and `gap` for both gaps; a path as SVG path data; a
//! bound corner by its name.
//!
//! [`tighten`] writes the short forms only where [`loosen`] gives back the same
//! value, so a tree read and written back is the tree it was, with one exception:
//! an sRGB colour is written as hex rounded to the nearest byte, a change no eye
//! sees, where its floats cost four times the text.

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

/// A colour's components as `#rrggbb`, or `#rrggbbaa` when not opaque, each
/// rounded to the nearest byte: at most half of a 1/255 step, which no eye sees,
/// for a colour a quarter of the size of its components. `None` for one outside
/// sRGB's 0–1, which a byte cannot hold.
fn nearest_hex(components: &[Value]) -> Option<String> {
    let [r, g, b, a] = components else {
        return None;
    };
    let mut bytes = [0u8; 4];
    for (byte, c) in bytes.iter_mut().zip([r, g, b, a]) {
        let f = c.as_f64()?;
        if !(0.0..=1.0).contains(&f) {
            return None;
        }
        *byte = (f * 255.0).round() as u8;
    }
    let [r, g, b, a] = bytes;
    Some(match a {
        255 => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
    })
}

/// The keys whose value is a [`Length`](crate::Length), as fields and as
/// character and paragraph attributes.
const LENGTH_KEYS: [&str; 18] = [
    "line_height",
    "letter_spacing",
    "word_spacing",
    "baseline_shift",
    "thickness",
    "offset",
    "spacing",
    "indent",
    "indent_start",
    "indent_end",
    "LineHeight",
    "LetterSpacing",
    "WordSpacing",
    "BaselineShift",
    "Spacing",
    "Indent",
    "IndentStart",
    "IndentEnd",
];

/// A length written as CSS writes it: `"22px"`, `"1.2em"`, or `"auto"` for no
/// line height of its own.
fn css_length(s: &str) -> Option<Value> {
    let s = s.trim();
    if s == "auto" {
        return Some(Value::Null);
    }
    let (n, unit) = match (s.strip_suffix("px"), s.strip_suffix("em")) {
        (Some(n), _) => (n, "Px"),
        (_, Some(n)) => (n, "Em"),
        _ => return None,
    };
    let n: f64 = n.trim().parse().ok().filter(|n: &f64| n.is_finite())?;
    Some(serde_json::json!({ unit: n }))
}

/// The four corners a radius written as one number, or left out of a rectangle,
/// stands for.
fn corners(r: f64) -> Value {
    serde_json::json!({ "top_left": r, "top_right": r, "bottom_right": r, "bottom_left": r })
}

/// A corner of [`BoundField::Corner`](crate::build::BoundField) by its name.
fn corner_index(name: &str) -> Option<usize> {
    ["top_left", "top_right", "bottom_right", "bottom_left"]
        .iter()
        .position(|c| *c == name)
}

/// `CamelCase` as `snake_case`, how an effect's `type` is spelled.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A text's sizing as `get_node` writes it, `{"type": "AutoHeight", "width": w}`,
/// in the model's shape.
fn loosen_sizing(v: &mut Value) {
    let Value::Object(map) = v else {
        return;
    };
    let Some(kind) = map.get("type").and_then(Value::as_str) else {
        return;
    };
    let n = |k: &str| map.get(k).cloned().unwrap_or(Value::from(0.0));
    *v = match kind {
        "Auto" => Value::from("Auto"),
        "AutoHeight" => serde_json::json!({ "AutoHeight": n("width") }),
        "Fixed" => serde_json::json!({ "Fixed": [n("width"), n("height")] }),
        _ => return,
    };
}

/// An effect written flat, as `get_node` does (`{"kind": "DropShadow", "blur": …,
/// "opacity": …}`), or as its kind alone, or without `visible`: in the model's
/// shape, the opacity folded into the colour's alpha.
fn loosen_effect(item: &mut Value) {
    let Value::Object(map) = item else {
        return;
    };
    map.remove("id");
    map.remove("index");
    let flat_kind = match map.get("kind") {
        Some(Value::String(s)) => Some(snake(s)),
        None => map.get("type").and_then(Value::as_str).map(snake),
        _ => None,
    };
    if let Some(kind) = flat_kind {
        let visible = map.remove("visible").unwrap_or(Value::Bool(true));
        map.remove("kind");
        let opacity = map.remove("opacity").and_then(|o| o.as_f64());
        let mut inner = std::mem::take(map);
        inner.insert("type".into(), Value::String(kind));
        if let (Some(o), Some(Value::String(s))) = (opacity, inner.get("color"))
            && let Some(c) = hex_colour(s)
        {
            let c = c.multiply_alpha(o as f32);
            inner.insert("color".into(), serde_json::to_value(c).unwrap_or_default());
        }
        map.insert("kind".into(), Value::Object(inner));
        map.insert("visible".into(), visible);
    } else if let Some(Value::Object(kind)) = map.get_mut("kind") {
        if let Some(Value::String(t)) = kind.get("type") {
            let t = snake(t);
            kind.insert("type".into(), Value::String(t));
        }
        map.entry("visible").or_insert(Value::Bool(true));
    }
}

/// A path's geometry as SVG path data, under `svg` or `path`.
fn loosen_path(map: &mut Map<String, Value>) {
    let data = match (map.get("path"), map.get("svg")) {
        (Some(Value::String(s)), _) | (None, Some(Value::String(s))) => s.clone(),
        _ => return,
    };
    if let Ok(path) = kurbo::BezPath::from_svg(&data) {
        map.remove("svg");
        map.insert(
            "path".into(),
            serde_json::to_value(path).unwrap_or_default(),
        );
    }
}

/// Expand the short forms in `v`, the value of a parameter or field called `key`,
/// into the model's shapes. What is not a short form is left as it is.
pub(crate) fn loosen(key: &str, v: &mut Value) {
    if let Value::String(s) = v
        && LENGTH_KEYS.contains(&key)
        && let Some(length) = css_length(s)
    {
        *v = length;
    }
    match (key, &mut *v) {
        ("sizing", _) => loosen_sizing(v),
        ("padding", Value::Number(n)) => {
            let n = n.clone();
            *v = Value::Array(vec![Value::Number(n); 4]);
        }
        ("corner_radii", Value::Number(n)) => *v = corners(n.as_f64().unwrap_or(0.0)),
        ("Path", Value::String(s)) => *v = serde_json::json!({ "path": s.clone() }),
        _ => {}
    }
    if let Value::Object(map) = v {
        match key {
            "Rect" => {
                map.entry("corner_radii").or_insert_with(|| corners(0.0));
            }
            "Path" => loosen_path(map),
            "style" => complete_style(map),
            "Flex" | "Grid" => {
                if let Some(gap) = map.remove("gap") {
                    map.entry("row_gap").or_insert_with(|| gap.clone());
                    map.entry("column_gap").or_insert(gap);
                }
            }
            _ => {}
        }
        if let Some(Value::String(name)) = map.get("corner")
            && let Some(k) = corner_index(name)
        {
            map.insert("corner".into(), Value::from(k));
        }
    }
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
                    "effects" => loosen_effect(item),
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
    // As `get_node` writes one: `{"id", "index", "paint": {"type": "Solid", …}}`.
    map.remove("id");
    map.remove("index");
    if let Some(Value::Object(paint)) = map.get("paint")
        && paint.get("type").and_then(Value::as_str) == Some("Solid")
        && let Some(c) = paint
            .get("color")
            .and_then(Value::as_str)
            .and_then(hex_colour)
    {
        let alpha = paint.get("alpha").and_then(Value::as_f64).unwrap_or(1.0);
        let c = c.multiply_alpha(alpha as f32);
        map.remove("paint");
        let mut solid = Map::new();
        solid.insert("Solid".into(), serde_json::to_value(c).unwrap_or_default());
        map.insert("brush".into(), Value::Object(solid));
    }
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
    let style = text
        .entry("style")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(style) = style {
        complete_style(style);
    }
}

/// A text style given in part, completed with the default style's fields that
/// every style must have.
fn complete_style(style: &mut Map<String, Value>) {
    let Value::Object(default) =
        serde_json::to_value(crate::typography::TextStyle::default()).unwrap_or_default()
    else {
        return;
    };
    for k in ["font_family", "font_size", "weight", "italic"] {
        if let Some(d) = default.get(k) {
            style.entry(k).or_insert_with(|| d.clone());
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
            if key == "Path"
                && let Some(path) = map.get("path")
                && let Ok(bez) = serde_json::from_value::<kurbo::BezPath>(path.clone())
                && kurbo::BezPath::from_svg(&bez.to_svg()).is_ok_and(|back| back == bez)
            {
                map.insert("path".into(), Value::String(bez.to_svg()));
            }
            if COLOUR_KEYS.contains(&key)
                && map.len() == 1
                && let Some(Value::Array(components)) = map.get("components")
                && let Some(hex) = nearest_hex(components)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An sRGB colour between two bytes is written as the nearest hex, and one
    /// outside 0–1 keeps its floats.
    ///
    /// flip: `nearest_hex` refusing a colour that does not read back exactly →
    /// fails at "the nearest byte".
    #[test]
    fn an_srgb_colour_is_written_as_its_nearest_hex() {
        let between =
            serde_json::json!({ "components": [0.101_960_78_f64, 0.461_176_34_f64, 1.0, 1.0] });
        let mut v = between;
        tighten("Solid", &mut v);
        assert_eq!(v, serde_json::json!("#1a76ff"), "the nearest byte");
        let wide = serde_json::json!({ "components": [1.2, 0.5, 0.0, 1.0] });
        let mut v = wide.clone();
        tighten("Solid", &mut v);
        assert_eq!(v, wide, "past sRGB stays as it is");
    }
}
