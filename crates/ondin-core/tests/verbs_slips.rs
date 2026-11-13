//! The slips an agent makes writing a verb's arguments: the ones put right on the
//! way in, and the refusals that show the form to write for the rest.

use ondin_core::verbs::{self, Ctx, Done, VerbError};
use ondin_core::{Document, IdSource, NodeId, NodeKind, Resolved};
use serde_json::{Value, json};

/// A document with one page and nothing on it, the id source that made it, and the page.
fn blank() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7E5B);
    let page = ids.mint();
    (Document::new(page), ids, page)
}

/// What `name` with `args` answers on `doc`, committing nothing.
fn answer(doc: &Document, ids: &mut IdSource, name: &str, args: &Value) -> Result<Done, VerbError> {
    let res = Resolved::rebuild(doc);
    verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        args,
    )
}

/// Run `name` with `args` and commit it; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Done {
    let done = answer(doc, ids, name, &args).unwrap_or_else(|e| panic!("{name}: {e}"));
    doc.apply(&done.tx)
        .unwrap_or_else(|e| panic!("{name} commits: {e}"));
    done
}

/// Why `name` with `args` is refused, panicking when it is taken.
fn why(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> String {
    match answer(doc, ids, name, &args) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("{name} took {args}"),
    }
}

/// A frame of 400 × 400 on the page, for layers to go in.
fn frame_on(doc: &mut Document, ids: &mut IdSource, page: NodeId) -> NodeId {
    run(
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "Artboard": { "size": { "width": 400, "height": 400 } } } }),
    )
    .created[0]
}

/// An enum word in another case or in kebab-case, and a field in camelCase, are
/// read as the model spells them.
///
/// flip: `repair` answering false for an unknown variant → fails at "align_items".
#[test]
fn an_enum_word_and_a_field_are_read_in_any_case() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let display = json!({ "Flex": { "direction": "column", "align_items": "center", "justify_content": "space-between" } });
    run(
        &mut doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": display }),
    );
    let flex = serde_json::to_value(doc.get(f).unwrap().display()).unwrap();
    assert_eq!(flex["Flex"]["align_items"], "Center", "align_items: {flex}");
    assert_eq!(flex["Flex"]["justify_content"], "SpaceBetween", "{flex}");
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": f.to_wire(), "text": "Hi" }),
    )
    .created[0];
    run(
        &mut doc,
        &mut ids,
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "fontSize": 40 } }),
    );
    let NodeKind::Text { style, .. } = doc.get(t).unwrap().kind() else {
        panic!("not a text");
    };
    assert_eq!(style.font_size, 40.0, "fontSize read as font_size");
}

/// `Frame` is taken for `Artboard`, the kind every tool's words call a frame.
#[test]
fn a_frame_is_an_artboard() {
    let (doc, mut ids, page) = blank();
    let done = answer(
        &doc,
        &mut ids,
        "create_node",
        &json!({ "parent": page.to_wire(), "kind": { "frame": { "size": { "width": 10, "height": 10 } } } }),
    );
    assert!(done.is_ok(), "{done:?}");
}

/// A kind written without its fields, or as an object of the wrong shape, is
/// refused with the form to write.
///
/// flip: `explain` answering `said` alone → fails at "the ellipse's form".
#[test]
fn a_kind_without_its_fields_is_shown_its_form() {
    let (doc, mut ids, page) = blank();
    let bare = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": "Ellipse" }),
    );
    assert!(
        bare.contains(r#"{"Ellipse":{"size""#),
        "the ellipse's form: {bare}"
    );
    // Quoted again by a client that sends the parameter as JSON text.
    let quoted = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": "\"Ellipse\"" }),
    );
    assert!(quoted.contains(r#"{"Ellipse":{"size""#), "{quoted}");
    let flat = why(
        &doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": { "type": "rect", "width": 10 } }),
    );
    assert!(flat.contains("one key naming the variant"), "{flat}");
}

/// A tree child written with `at` is refused with the `transform` that places it.
///
/// flip: `explain` without its tree-placement arm → fails at "the placing form".
#[test]
fn a_tree_child_placed_with_at_is_shown_transform() {
    let (doc, mut ids, page) = blank();
    let rect = json!({ "Rect": { "size": { "width": 1, "height": 1 } } });
    let why = why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": rect,
            "children": [{ "at": [4, 4], "kind": rect }] } }),
    );
    assert!(
        why.contains("[1, 0, 0, 1, x, y]"),
        "the placing form: {why}"
    );
}

/// A parameter a client sent as JSON text is read as the JSON.
#[test]
fn a_model_parameter_sent_as_json_text_is_read() {
    let (doc, mut ids, page) = blank();
    let kind = r#"{"Rect": {"size": {"width": 10, "height": 10}}}"#;
    let done = answer(
        &doc,
        &mut ids,
        "create_node",
        &json!({ "parent": page.to_wire(), "kind": kind }),
    );
    assert!(done.is_ok(), "{done:?}");
}

/// A stroke as a bare colour or with `color`, a fill with `color` and `opacity`,
/// padding as two numbers, a text without its sizing, a star without its ratio
/// and a geometry as path data alone are all taken.
///
/// flip: `loosen_paint` refusing a bare stroke colour again → fails at "strokes".
#[test]
fn the_short_forms_agents_write_are_taken() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } }),
    )
    .created[0];
    let ok = |doc: &Document, ids: &mut IdSource, name: &str, args: Value| {
        let done = answer(doc, ids, name, &args);
        assert!(done.is_ok(), "{name} {args}: {done:?}");
    };
    let n = json!([r.to_wire()]);
    ok(
        &doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": ["#12352a"] }),
    );
    ok(
        &doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": [{ "color": "#12352a", "width": 2 }] }),
    );
    ok(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#1a76ff", "opacity": 0.5 }] }),
    );
    ok(
        &doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": { "Flex": { "padding": [16, 24] } } }),
    );
    ok(
        &doc,
        &mut ids,
        "set_geometry",
        json!({ "node": r.to_wire(), "geometry": { "CornerRadius": 4 } }),
    );
    let p = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Path": { "path": "M0,0 L10,0 L5,8 Z" } } }),
    )
    .created[0];
    ok(
        &doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": "M0,0 L20,0 L10,16 Z" }),
    );
    ok(
        &doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": { "Path": { "path": "M0,0 L20,0 Z" } } }),
    );
    let tree = json!({ "kind": "Group", "children": [
        { "kind": { "Text": { "content": "Hi", "style": { "font_size": 14 } } } },
        { "kind": { "Star": { "size": { "width": 10, "height": 10 } } } },
    ] });
    ok(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree }),
    );
}

/// A fill given as a colour with an opacity draws that colour at that alpha.
#[test]
fn a_fill_opacity_folds_into_its_colour() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "color": "#ff0000", "opacity": 0.5 }] }),
    );
    let fills = serde_json::to_value(&doc.get(f).unwrap().paint().fills).unwrap();
    let alpha = fills[0]["brush"]["Solid"]["components"][3]
        .as_f64()
        .unwrap();
    assert!((alpha - 0.5).abs() < 0.01, "{fills}");
}

/// `create_node` places a layer's box by `x` and `y`; `insert_subtree` places its
/// root by `x` and `y`.
///
/// flip: `place_corner` returning early → fails at "the rect's left".
#[test]
fn create_node_and_insert_subtree_place_by_the_box() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "x": 30, "y": 40,
                "kind": { "Rect": { "size": { "width": 10, "height": 10 } } } }),
    )
    .created[0];
    let at = doc.get(r).unwrap().transform().translation();
    assert_eq!((at.x, at.y), (30.0, 40.0), "the rect's left and top");
    let tree = json!({ "kind": { "Artboard": { "size": { "width": 50, "height": 20 } } },
                       "transform": [1, 0, 0, 1, 5, 5] });
    let root = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree, "x": 100, "y": 7 }),
    )
    .created[0];
    let at = doc.get(root).unwrap().transform().translation();
    assert_eq!((at.x, at.y), (100.0, 7.0), "the root's left and top");
}

/// A variable is taken by its name where an id is wanted, and a value bare
/// when the kind says what it is; a taken name answers the variable's id.
///
/// flip: `by_names` leaving names as they are → fails at the first unwrap.
#[test]
fn a_variable_is_named_and_its_value_written_bare() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let made = run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "color", "name": "brand/orange", "value": "#ff7a1a" }),
    )
    .made
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "number", "name": "radius/card", "value": 16 }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": ["#000000"] }),
    );
    run(
        &mut doc,
        &mut ids,
        "bind_variable",
        json!({ "nodes": [f.to_wire()], "field": { "fill": 0 }, "variable": "brand/orange" }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_variable_value",
        json!({ "variable": "radius/card", "value": 24 }),
    );
    let taken = why(
        &doc,
        &mut ids,
        "add_variable",
        json!({ "kind": "color", "name": "brand/orange" }),
    );
    assert!(taken.contains(&made), "the taken name's id: {taken}");
    let missing = why(
        &doc,
        &mut ids,
        "bind_variable",
        json!({ "nodes": [f.to_wire()], "field": { "fill": 0 }, "variable": "brand/blue" }),
    );
    assert!(
        missing.contains("`brand/orange`"),
        "the names there are: {missing}"
    );
}

/// `get_tree` writes even corners as one number and a line's end as `[x, y]`,
/// and `insert_subtree` reads both back.
#[test]
fn get_tree_writes_corners_and_ends_short() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let tree = json!({ "kind": "Group", "children": [
        { "kind": { "Rect": { "size": { "width": 10, "height": 10 }, "corner_radii": 6 } } },
        { "kind": { "Line": { "end": [40, 0] } } },
    ] });
    let g = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree }),
    )
    .created[0];
    let written = verbs::tree(&doc, g).unwrap();
    let text = written.to_string();
    assert!(text.contains(r#""corner_radii":6"#), "{text}");
    assert!(text.contains(r#""end":[40.0,0.0]"#), "{text}");
    let back = answer(
        &doc,
        &mut ids,
        "insert_subtree",
        &json!({ "parent": f.to_wire(), "tree": written }),
    );
    assert!(back.is_ok(), "{back:?}");
}
