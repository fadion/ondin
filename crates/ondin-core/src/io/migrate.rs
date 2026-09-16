//! Schema migrations old→current (§5.11).
//!
//! Migrations run on the parsed JSON `Value` before it is deserialized into the
//! current `DocumentDto`, so each step is a self-contained transform between two
//! on-disk shapes. The loader repeatedly applies the step for the file's version
//! until it reaches `CURRENT_SCHEMA_VERSION`.

use crate::io::{CURRENT_SCHEMA_VERSION, IoError};
use serde_json::Value;

/// Migrate a raw document value up to the current schema version.
///
/// ⚠️ **The range check is done in the header's own denomination and the value is
/// never narrowed** (§15 D638). The header is whatever `serde_json` parsed, so it
/// can be any `u64`, and `as u32` before the comparison makes `version > CURRENT`
/// mean something other than what it reads as: `8589934592` (2³³) truncates to
/// **0** and is migrated through the whole v0→v4 chain as though it were the
/// oldest file this loader knows, and `u64::MAX` was refused with a message
/// naming `4294967295` — a version the file does not claim. Comparing first is
/// the whole fix; below `CURRENT` the value provably fits, and the `match` runs
/// on the same `u64` rather than on a second copy that could disagree with the
/// one the guard read.
pub(crate) fn migrate(mut value: Value) -> Result<Value, IoError> {
    loop {
        let version = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| IoError::Integrity("missing or non-integer schema_version".into()))?;

        if version == u64::from(CURRENT_SCHEMA_VERSION) {
            return Ok(value);
        }
        if version > u64::from(CURRENT_SCHEMA_VERSION) {
            return Err(IoError::UnsupportedVersion(version));
        }

        match version {
            0 => migrate_0_to_1(&mut value),
            1 => migrate_1_to_2(&mut value),
            2 => migrate_2_to_3(&mut value),
            3 => migrate_3_to_4(&mut value),
            other => return Err(IoError::UnsupportedVersion(other)),
        }
    }
}

/// v0 → v1. v0 is structurally identical to v1; this stub exists to prove the
/// migration chain runs end-to-end before any real migration is needed.
fn migrate_0_to_1(value: &mut Value) {
    value["schema_version"] = Value::from(1u32);
}

/// v1 → v2. A rect's single `corner_radius` became per-corner `corner_radii`
/// (§5.3), so an old file's one number is spread over all four corners — which
/// is exactly the shape it described.
///
/// A node whose `kind` is not a `Rect`, or a `Rect` already carrying
/// `corner_radii`, is left alone: the loop has to tolerate anything the JSON
/// can hold, because rejecting here would turn a slightly odd file into a
/// failure to open rather than a `DocumentDto` parse error that says why.
fn migrate_1_to_2(value: &mut Value) {
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            let Some(rect) = node.get_mut("kind").and_then(|k| k.get_mut("Rect")) else {
                continue;
            };
            let Some(r) = rect
                .get("corner_radius")
                .and_then(Value::as_f64)
                .filter(|r| r.is_finite())
            else {
                continue;
            };
            if let Some(obj) = rect.as_object_mut() {
                obj.remove("corner_radius");
                obj.insert(
                    "corner_radii".into(),
                    serde_json::json!({
                        "top_left": r, "top_right": r,
                        "bottom_right": r, "bottom_left": r,
                    }),
                );
            }
        }
    }
    value["schema_version"] = Value::from(2u32);
}

/// v2 → v3. The text attribute model (§5.4, `typography.rs`) split one flat
/// `TextStyle` into three scopes, and two of its fields changed shape rather than
/// merely gaining neighbours — which is exactly what a version bump is for. The
/// rest of the growth is additive and rides `#[serde(default)]`.
///
/// 1. **`line_height` was a bare multiplier**, and is now
///    `Option<Length>` — `None` for Auto (the font's own), `{"Em": m}` for a
///    multiple of the font size. A v2 file's number is `Em(m)`: exactly what it
///    meant. Leaving serde to do it would default the field to `None` and
///    silently retype every document ever saved.
/// 2. **`align` moved out of the character defaults** and into the paragraph
///    scope, where it belongs and where a per-paragraph version can later find
///    it. `Left`/`Right` are preserved verbatim rather than mapped to
///    `Start`/`End`: an old file said *left*, and turning that into "the start
///    edge, whichever that is" would flip the alignment of the first RTL
///    document anyone opened.
///
/// Anything that is not a `Text` node, or a `Text` node already carrying the new
/// shape, is left alone — the loop has to tolerate whatever the JSON holds,
/// because rejecting here turns a slightly odd file into a failure to open
/// rather than a `DocumentDto` parse error that says why.
fn migrate_2_to_3(value: &mut Value) {
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            let Some(text) = node.get_mut("kind").and_then(|k| k.get_mut("Text")) else {
                continue;
            };
            let align = text
                .get_mut("style")
                .and_then(Value::as_object_mut)
                .and_then(|style| {
                    if let Some(lh) = style.get("line_height").and_then(Value::as_f64)
                        && lh.is_finite()
                    {
                        style.insert("line_height".into(), serde_json::json!({ "Em": lh }));
                    }
                    style.remove("align")
                });
            let Some(obj) = text.as_object_mut() else {
                continue;
            };
            // `skip_serializing_if` keeps a default paragraph block out of the
            // file, so only a document that had a non-default alignment gains a
            // key here — and `Start` is the new default, which is what a v2
            // `Left` file re-saves as only if it *was* left-aligned anyway.
            if let Some(align) = align.filter(|a| a.as_str() != Some("Start")) {
                obj.insert("paragraph".into(), serde_json::json!({ "align": align }));
            }
        }
    }
    value["schema_version"] = Value::from(3u32);
}

/// v3 → v4. A frame's fill left `NodeKind::Artboard` and joined `Node::paint`
/// (§15 D400), so `{"Artboard": {"size": …, "background": <brush>}}` becomes
/// `{"Artboard": {"size": …}}` with the brush prepended to that node's `fills` as
/// an ordinary visible entry.
///
/// **Prepended, not appended, and it matters on exactly one shape of file.** The
/// list is the z-order, bottom first, and a frame's background was painted behind
/// everything the frame's own paint drew. A v3 frame could not have a fill —
/// `SetFills` refused an `Artboard` — so today that list is always empty and the
/// two are the same operation; the rule is written the way it is so a file some
/// future reader hands this function does not have its ground painted on top of
/// its own artwork.
///
/// A `null` background — which is how "no ground" was written — contributes no
/// fill, which is exactly what it meant. A node that is not an `Artboard`, or an
/// `Artboard` already carrying no `background` key, is left alone: the loop has to
/// tolerate anything the JSON can hold, because rejecting here turns a slightly
/// odd file into a failure to open rather than a `DocumentDto` parse error that
/// says why.
fn migrate_3_to_4(value: &mut Value) {
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            let Some(bg) = node
                .get_mut("kind")
                .and_then(|k| k.get_mut("Artboard"))
                .and_then(Value::as_object_mut)
                .and_then(|ab| ab.remove("background"))
                .filter(|bg| !bg.is_null())
            else {
                continue;
            };
            // The `Fill` shape: a brush and a flag. A background was always drawn
            // when it was there, so `true` is what every one of them meant.
            let fill = serde_json::json!({ "brush": bg, "visible": true });
            // `paint` is not `skip_serializing_if`, so every node this writer ever
            // emitted has one with both lists in it — but a hand-edited file may
            // not, and losing the ground is a worse answer than filling in the
            // field the loader is about to read. Built up a level at a time so a
            // frame carrying strokes and no `fills` key keeps its strokes.
            let paint = node.as_object_mut().map(|n| {
                n.entry("paint")
                    .or_insert_with(|| serde_json::json!({ "fills": [], "strokes": [] }))
            });
            let Some(fills) = paint
                .and_then(Value::as_object_mut)
                .map(|p| p.entry("fills").or_insert_with(|| Value::Array(vec![])))
                .and_then(Value::as_array_mut)
            else {
                continue;
            };
            fills.insert(0, fill);
        }
    }
    value["schema_version"] = Value::from(4u32);
}
