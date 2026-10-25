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
/// The range check is made on the header's own `u64` and the value is never
/// narrowed (§15 D638): `as u32` first would truncate 2³³ to 0 and migrate it as
/// the oldest file, and name a version the file does not claim in the refusal.
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
            4 => migrate_4_to_5(&mut value),
            5 => migrate_5_to_6(&mut value),
            6 => migrate_6_to_7(&mut value),
            7 => migrate_7_to_8(&mut value),
            8 => migrate_8_to_9(&mut value),
            9 => migrate_9_to_10(&mut value),
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
/// (§5.3); an old file's one number is spread over all four corners.
///
/// A node that is not a `Rect`, or already carries `corner_radii`, is left alone:
/// rejecting here would hide the `DocumentDto` parse error that says why.
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

/// v2 → v3. The text attribute model (§5.4, `typography.rs`) split `TextStyle`
/// into three scopes; the rest of the growth is additive under `#[serde(default)]`.
///
/// 1. `line_height` was a bare multiplier and is now `Option<Length>`: a v2
///    number `m` loads as `{"Em": m}`; serde alone would default it to `None`.
/// 2. `align` moved from the character defaults to the paragraph scope.
///    `Left`/`Right` are preserved verbatim, not mapped to `Start`/`End`, which
///    would flip an old file's alignment in an RTL document.
///
/// A node that is not `Text`, or already carries the new shape, is left alone:
/// rejecting here would hide the `DocumentDto` parse error that says why.
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
            // A default paragraph block is not written, so only a non-default
            // alignment gains a key here.
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
/// Prepended, not appended: the list is z-order, bottom first, and the background
/// was painted behind the frame's own paint. A v3 frame could not have a fill, so
/// the list is normally empty and the two agree; prepending keeps the ground
/// from landing on top of artwork in any other file.
///
/// A `null` background (how "no ground" was written) contributes no fill. A node
/// that is not an `Artboard`, or has no `background` key, is left alone.
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
            // A background was always drawn, so `visible: true`.
            let fill = serde_json::json!({ "brush": bg, "visible": true });
            // Every written node has `paint` with both lists, but a hand-edited
            // file may not, and losing the ground is worse than filling the field
            // in. Built a level at a time so existing strokes are kept.
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

/// v4 → v5: every item of a node's five item lists gains an id (§15 D980).
///
/// `paint.fills`, `paint.strokes`, `effects`, `exports` and `grids` — each item is
/// given `"id": "0:<index>"`, `ItemId::positional`'s wire form. Deterministic, so a
/// migrated document saves to the same bytes twice (invariant 9), and unique
/// within its list. A v4 file holds no instances, so no cross-node match can go
/// wrong; `reserve_existing_ids` starts a later session past every item id in use.
///
/// An item that already carries an `id` keeps it, and a list that is not an array
/// or an item that is not an object is left for the loader to refuse.
fn migrate_4_to_5(value: &mut Value) {
    fn number(list: Option<&mut Value>) {
        let Some(items) = list.and_then(Value::as_array_mut) else {
            return;
        };
        for (i, item) in items.iter_mut().enumerate() {
            if let Some(obj) = item.as_object_mut() {
                obj.entry("id")
                    .or_insert_with(|| Value::from(format!("0:{i}")));
            }
        }
    }
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            if let Some(paint) = node.get_mut("paint") {
                number(paint.get_mut("fills"));
                number(paint.get_mut("strokes"));
            }
            number(node.get_mut("effects"));
            number(node.get_mut("exports"));
            number(node.get_mut("grids"));
        }
    }
    value["schema_version"] = Value::from(5u32);
}

/// v5 → v6: the vault (§15 D1012). A number and nothing else: a v5 file has no
/// vault and no origins, which v6 reads from absent keys.
///
/// The bump is for the older build's sake (§5.11): a v5 reader would drop the
/// `vault` key and refuse the file with *"non-root node … has no parent"*
/// (`tests/kits.rs` pins that message) instead of saying it is newer.
fn migrate_5_to_6(value: &mut Value) {
    value["schema_version"] = Value::from(6u32);
}

/// v6 → v7: variables and modes (§5.3f, §15 D1039). A number and nothing else, for
/// `migrate_5_to_6`'s reason: a v6 file has no variables, modes or bindings, and a
/// v6 reader would drop `variables` and refuse a bound fill as a malformed brush.
///
/// The published document is nested where the ladder does not look, so its number
/// moves with the outer one's (§5.11, D1037).
fn migrate_6_to_7(value: &mut Value) {
    value["schema_version"] = Value::from(7u32);
    if let Some(inner) = value
        .get_mut("published")
        .and_then(|p| p.get_mut("document"))
        .filter(|d| d.is_object())
    {
        inner["schema_version"] = Value::from(7u32);
    }
}

/// v7 → v8: a container's gaps and padding bound to number variables (§15
/// D1058). A number and nothing else: a v7 file binds none. The bump is for the
/// older build, which would silently drop `spacing_vars` and lay bound gaps and
/// padding out at their literal 0. The published document's number moves too, as
/// in `migrate_6_to_7`.
fn migrate_7_to_8(value: &mut Value) {
    value["schema_version"] = Value::from(8u32);
    if let Some(inner) = value
        .get_mut("published")
        .and_then(|p| p.get_mut("document"))
        .filter(|d| d.is_object())
    {
        inner["schema_version"] = Value::from(8u32);
    }
}

/// v8 → v9: bound size limits and type numbers, descriptions and preferred mains.
/// A number and nothing else: a v8 file holds none of them. The bump is for the
/// older build, which would lay a bound limit out at its literal 0, refuse a
/// widened `text_vars` as a parse error, and drop descriptions and preferred mains.
/// The published document's number moves too, as in `migrate_6_to_7`.
fn migrate_8_to_9(value: &mut Value) {
    value["schema_version"] = Value::from(9u32);
    if let Some(inner) = value
        .get_mut("published")
        .and_then(|p| p.get_mut("document"))
        .filter(|d| d.is_object())
    {
        inner["schema_version"] = Value::from(9u32);
    }
}

/// v9 → v10: pages (`crate::page`). The one `root` becomes the only page, its
/// ground the document's `canvas_background` (or the default a file without one
/// was drawn on); a guide with no owner ran the whole canvas, so it names that
/// page; a root still called `Root` becomes `Page 1`. The published document is
/// reshaped the same way, since the ladder does not descend into it.
///
/// A file whose `root` is missing is left for the `DocumentDto` parse to refuse
/// with its own message.
fn migrate_9_to_10(value: &mut Value) {
    one_page(value);
    if let Some(inner) = value
        .get_mut("published")
        .and_then(|p| p.get_mut("document"))
        .filter(|d| d.is_object())
    {
        one_page(inner);
    }
}

/// [`migrate_9_to_10`] for one document value.
fn one_page(doc: &mut Value) {
    doc["schema_version"] = Value::from(10u32);
    let Some(obj) = doc.as_object_mut() else {
        return;
    };
    let Some(root) = obj.remove("root") else {
        return;
    };
    let ground = obj.remove("canvas_background").unwrap_or_else(|| {
        serde_json::to_value(crate::document::DEFAULT_CANVAS_BACKGROUND)
            .expect("a colour serializes")
    });
    if let Some(guides) = obj.get_mut("guides").and_then(Value::as_array_mut) {
        for guide in guides {
            if guide.get("owner").is_none_or(Value::is_null) {
                guide["owner"] = root.clone();
            }
        }
    }
    if let Some(nodes) = obj.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            if node.get("id") == Some(&root) && node.get("name") == Some(&Value::from("Root")) {
                node["name"] = Value::from(format!("{} 1", crate::page::PAGE_STEM));
            }
        }
    }
    obj.insert(
        "pages".into(),
        serde_json::json!([{ "id": root, "ground": ground }]),
    );
}
