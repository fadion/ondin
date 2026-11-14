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

/// Run `name` with `args` and commit it, checking its undo and redo land where
/// they should; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Done {
    let done = answer(doc, ids, name, &args).unwrap_or_else(|e| panic!("{name}: {e}"));
    let before = doc.clone();
    let undo = doc
        .apply(&done.tx)
        .unwrap_or_else(|e| panic!("{name} commits: {e}"))
        .inverse;
    let after = doc.clone();
    let redo = doc.apply_restoring(&undo).expect("undo").inverse;
    assert!(*doc == before, "{name}: undo restores the document");
    doc.apply_restoring(&redo).expect("redo");
    assert!(*doc == after, "{name}: redo restores the edit");
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
/// and a geometry as path data alone are all taken, and stored as they mean.
///
/// flip: `loosen`'s two-number padding read as [horizontal, vertical] → fails at "padding".
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
    let stored = |doc: &Document, id: NodeId| json!({ "kind": serde_json::to_value(doc.get(id).unwrap().kind()).unwrap() });
    let n = json!([r.to_wire()]);
    let dark = ondin_core::peniko::Color::from_rgb8(0x12, 0x35, 0x2a);
    run(
        &mut doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": ["#12352a"] }),
    );
    let strokes = &doc.get(r).unwrap().paint().strokes;
    assert_eq!(
        strokes[0].brush,
        ondin_core::Brush::Solid(dark).into(),
        "strokes"
    );
    assert_eq!(strokes[0].width, 1.0);
    run(
        &mut doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": n, "strokes": [{ "color": "#12352a", "width": 2 }] }),
    );
    assert_eq!(doc.get(r).unwrap().paint().strokes[0].width, 2.0, "a width");
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#1a76ff", "opacity": 0.5 }] }),
    );
    let fill = serde_json::to_value(&doc.get(r).unwrap().paint().fills[0]).unwrap();
    let alpha = fill["brush"]["Solid"]["components"][3].as_f64().unwrap();
    assert!((alpha - 0.5).abs() < 0.01, "an opacity folded: {fill}");
    run(
        &mut doc,
        &mut ids,
        "set_display",
        json!({ "nodes": [f.to_wire()], "display": { "Flex": { "padding": [16, 24] } } }),
    );
    let flex = serde_json::to_value(doc.get(f).unwrap().display()).unwrap();
    assert_eq!(
        flex["Flex"]["padding"],
        json!([16.0, 24.0, 16.0, 24.0]),
        "padding"
    );
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": r.to_wire(), "geometry": { "CornerRadius": 4 } }),
    );
    assert_eq!(
        stored(&doc, r)["kind"]["Rect"]["corner_radii"]["top_left"],
        json!(4.0),
        "a corner radius"
    );
    let p = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Path": { "path": "M0,0 L10,0 L5,8 Z" } } }),
    )
    .created[0];
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": "M0,0 L20,0 L10,16 Z" }),
    );
    let NodeKind::Path { path, .. } = doc.get(p).unwrap().kind() else {
        panic!("a path");
    };
    assert_eq!(path.to_svg(), "M0,0 L20,0 L10,16 Z", "path data alone");
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": { "Path": { "path": "M0,0 L20,0 Z" } } }),
    );
    let tree = json!({ "kind": "Group", "children": [
        { "kind": { "Text": { "content": "Hi", "style": { "font_size": 14 } } } },
        { "kind": { "Star": { "size": { "width": 10, "height": 10 } } } },
    ] });
    let g = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": f.to_wire(), "tree": tree }),
    )
    .created[0];
    let kids = doc.get(g).unwrap().children().to_vec();
    assert!(matches!(
        doc.get(kids[0]).unwrap().kind(),
        NodeKind::Text {
            sizing: ondin_core::TextSizing::Auto,
            ..
        }
    ));
    assert_eq!(
        stored(&doc, kids[1])["kind"]["Star"]["inner_ratio"],
        json!(0.382)
    );
}

/// A rounded path's outline written short keeps its corners' radii while it has
/// as many points, and is refused naming `corner_radii` when it has not.
/// flip: `keep_radii` returning at once → fails at "the radii stay".
#[test]
fn a_rounded_paths_short_outline_keeps_its_radii() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let square = "M0,0 L100,0 L100,100 L0,100 Z";
    let p = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": { "Path": { "path": square, "corner_radii": [10, 10, 10, 10] } } }),
    )
    .created[0];
    let radii = |doc: &Document| match doc.get(p).unwrap().kind() {
        NodeKind::Path { corner_radii, .. } => corner_radii.clone(),
        _ => panic!("a path"),
    };
    assert_eq!(radii(&doc), vec![10.0; 4], "the fixture");
    for geometry in [json!(square), json!({ "Path": square })] {
        run(
            &mut doc,
            &mut ids,
            "set_geometry",
            json!({ "node": p.to_wire(), "geometry": geometry }),
        );
        assert_eq!(radii(&doc), vec![10.0; 4], "the radii stay: {geometry}");
    }
    let fewer = why(
        &doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": "M0,0 L100,0 L50,100 Z" }),
    );
    assert!(fewer.contains("corner_radii"), "{fewer}");
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        json!({ "node": p.to_wire(), "geometry": { "Path": { "path": square, "corner_radii": [] } } }),
    );
    assert!(radii(&doc).is_empty(), "radii given are written");
}

/// An opacity or alpha outside 0–1 is refused naming it, never folded into a
/// colour: a fill's `opacity`, a paint's `alpha`, a flat effect's `opacity`.
/// flip: `unit` taking any number → fails at "a fill's opacity".
#[test]
fn an_opacity_outside_zero_to_one_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let n = json!([f.to_wire()]);
    let fill = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#ff0000", "opacity": 50 }] }),
    );
    assert!(
        fill.contains("`opacity` is from 0 to 1") && fill.contains("0.5"),
        "a fill's opacity: {fill}"
    );
    let negative = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "color": "#ff0000", "opacity": -1 }] }),
    );
    assert!(negative.contains("opacity"), "{negative}");
    let alpha = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": n, "fills": [{ "paint": { "type": "Solid", "color": "#ff0000", "alpha": 50 } }] }),
    );
    assert!(alpha.contains("`alpha` is from 0 to 1"), "{alpha}");
    let shadow = |opacity: Value, color: Value| {
        json!({ "nodes": n, "effects": [{ "kind": "DropShadow", "color": color, "opacity": opacity,
                                          "blur": 4, "offset": [0, 2], "spread": 0 }] })
    };
    let effect = why(
        &doc,
        &mut ids,
        "set_effects",
        shadow(json!(50), json!("#000000")),
    );
    assert!(effect.contains("`opacity` is from 0 to 1"), "{effect}");

    // A colour in the model's own shape takes the fold too, not only hex.
    let model = json!({ "components": [0.0, 0.0, 0.0, 1.0], "cs": "srgb" });
    run(&mut doc, &mut ids, "set_effects", shadow(json!(0.5), model));
    let effects = serde_json::to_value(doc.get(f).unwrap().effects()).unwrap();
    let a = effects[0]["kind"]["color"]["components"][3].as_f64();
    assert_eq!(a, Some(0.5), "the opacity folded: {effects}");
}

/// A text sizing missing a dimension its type has is refused, not read as 0.
/// flip: `loosen_sizing` reading an absent dimension as 0 → fails at "Fixed".
#[test]
fn a_sizing_without_its_dimension_is_refused() {
    let (doc, mut ids, page) = blank();
    let text = |sizing: Value| json!({ "parent": page.to_wire(), "kind": { "Text": { "content": "Hi", "sizing": sizing } } });
    let fixed = why(
        &doc,
        &mut ids,
        "create_node",
        text(json!({ "type": "Fixed", "width": 100 })),
    );
    assert!(
        fixed.contains("`Fixed` sizing needs its `height`"),
        "Fixed: {fixed}"
    );
    let auto = why(
        &doc,
        &mut ids,
        "create_node",
        text(json!({ "type": "AutoHeight" })),
    );
    assert!(auto.contains("`width`"), "{auto}");
    let taken = answer(
        &doc,
        &mut ids,
        "create_node",
        &text(json!({ "type": "AutoHeight", "width": 80 })),
    );
    assert!(taken.is_ok(), "{taken:?}");
}

/// A field a gradient brush or a grid track has no place for is refused naming
/// it, as one on a solid fill is: their flattened and untagged shapes hide it
/// from serde.
/// flip: `attempt` skipping `unseen_field` → fails at "the gradient's".
#[test]
fn a_gradients_and_a_tracks_unknown_field_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let ramp = ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops(
        [
            ondin_core::peniko::Color::BLACK,
            ondin_core::peniko::Color::WHITE,
        ]
        .as_slice(),
    );
    let gradient = serde_json::to_value(ondin_core::Brush::Gradient(ondin_core::GradientBrush {
        gradient: ramp,
        transform: Default::default(),
        opacity: 1.0,
    }))
    .unwrap();
    let ok = json!({ "nodes": [f.to_wire()], "fills": [{ "brush": gradient.clone() }] });
    assert!(
        answer(&doc, &mut ids, "set_fills", &ok).is_ok(),
        "the control"
    );
    let mut bad = gradient;
    bad["Gradient"]["opactiy"] = json!(0.3);
    let refused = why(
        &doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [f.to_wire()], "fills": [{ "brush": bad }] }),
    );
    assert!(
        refused.contains("opactiy") && refused.contains("no such field"),
        "the gradient's: {refused}"
    );

    let grid = |track: Value| json!({ "nodes": [f.to_wire()], "display": { "Grid": { "columns": [track], "rows": ["Auto"] } } });
    let minmax = json!({ "min": { "Px": 100.0 }, "max": { "Fr": 1.0 } });
    run(&mut doc, &mut ids, "set_display", grid(minmax.clone()));
    let mut extra = minmax;
    extra["step"] = json!(2);
    let refused = why(&doc, &mut ids, "set_display", grid(extra));
    assert!(refused.contains("step"), "a track's: {refused}");
}

/// A field an effect's kind or a solid brush's colour has no place for is
/// refused naming it: the tagged and untagged shapes hide it from serde.
/// flip: `unseen_field` without its effect arm → fails at "the shadow's".
#[test]
fn an_effects_and_a_colours_unknown_field_is_refused() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
    let n = json!([f.to_wire()]);
    let effects = |effect: Value| json!({ "nodes": n, "effects": [effect] });
    let shadow = json!({ "kind": "DropShadow", "color": "#000000", "blur": 4,
                         "offset": [0, 2], "spread": 0 });
    run(&mut doc, &mut ids, "set_effects", effects(shadow.clone()));
    let mut blnd = shadow;
    blnd["blnd"] = json!("Multiply");
    let refused = why(&doc, &mut ids, "set_effects", effects(blnd));
    assert!(refused.contains("blnd"), "the shadow's: {refused}");
    let nested = json!({ "kind": { "type": "DropShadow", "color": "#000000", "blur": 4,
                                   "offset": [0, 2], "spread": 0, "blnd": "Multiply" } });
    let refused = why(&doc, &mut ids, "set_effects", effects(nested));
    assert!(refused.contains("blnd"), "the nested shadow's: {refused}");
    let blur = json!({ "kind": "LayerBlur", "radius": 4, "opacity": 0.5 });
    let refused = why(&doc, &mut ids, "set_effects", effects(blur));
    assert!(refused.contains("opacity"), "the blur's: {refused}");
    let solid = json!({ "Solid": { "components": [1, 0, 0, 1], "cs": "srgb", "alpha": 0.5 } });
    let fills = json!({ "nodes": n, "fills": [{ "brush": solid }] });
    let refused = why(&doc, &mut ids, "set_fills", fills);
    assert!(refused.contains("alpha"), "the colour's: {refused}");
}

/// What `get_node` writes reads back: a line height as `"120%"`, an effect's
/// blend by its label.
/// flip: `css_length` refusing a `%` → fails at "a percent line height".
#[test]
fn a_line_height_in_percent_and_a_blend_label_are_taken() {
    let (mut doc, mut ids, page) = blank();
    let f = frame_on(&mut doc, &mut ids, page);
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
        json!({ "nodes": [t.to_wire()], "style": { "line_height": "150%" } }),
    );
    let NodeKind::Text { style, .. } = doc.get(t).unwrap().kind() else {
        panic!("a text");
    };
    assert_eq!(
        style.line_height,
        Some(ondin_core::Length::Em(1.5)),
        "a percent line height"
    );
    run(
        &mut doc,
        &mut ids,
        "set_effects",
        json!({ "nodes": [f.to_wire()], "effects": [{ "kind": "DropShadow", "color": "#000000",
                 "blur": 4, "offset": [0, 2], "spread": 0, "blend": "Color burn" }] }),
    );
    let effects = serde_json::to_value(doc.get(f).unwrap().effects()).unwrap();
    assert_eq!(effects[0]["kind"]["blend"], json!("ColorBurn"), "{effects}");
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
