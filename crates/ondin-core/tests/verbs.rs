//! The verb registry (`verbs`): every verb is well formed, arguments are checked
//! against its parameters before it runs, and a run of verbs edits a document as
//! the canvas would.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::svg_in::ImageSink;
use ondin_core::verbs::{self, Ctx, Done, Lent, ParamKind, VerbError};
use ondin_core::{
    Brush, Document, Fill, Guide, GuideAxis, GuideId, IdSource, ImageEntry, ImageFormat, ImageId,
    ImageSource, NodeId, NodeKind, Operation, Resolved, Transaction, image_brush,
};
use serde_json::{Value, json};

/// A document with one page and nothing on it, the id source that made it, and the page.
fn blank() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7E5B);
    let page = ids.mint();
    (Document::new(page), ids, page)
}

/// Run `name` with `args` on `doc` and commit its edit, with what the components owe
/// it, as a script does; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Vec<NodeId> {
    let res = Resolved::rebuild(doc);
    let Done {
        mut tx, created, ..
    } = verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        &args,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    ondin_core::propagate::owed(doc, &mut tx, ids);
    doc.apply(&tx)
        .unwrap_or_else(|e| panic!("{name} commits: {e}"));
    created
}

/// What `name` with `args` answers on `doc`, committing nothing.
fn answer(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> Result<Done, VerbError> {
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
        &args,
    )
}

/// The parameter `name` with `args` is refused under, panicking when it is not
/// refused as an invalid argument.
fn refused_param(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> &'static str {
    match answer(doc, ids, name, args) {
        Err(VerbError::Invalid { param, .. }) => param,
        other => panic!("{name}: {other:?}"),
    }
}

/// The JSON a `kind` argument takes for a frame of `w` × `h`.
fn frame(w: f64, h: f64) -> Value {
    serde_json::to_value(NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap()
}

/// A frame of `w` × `h` named `name` holding one rectangle, made a main component
/// through `make_component`; answers the main and its rectangle.
fn main_component(
    doc: &mut Document,
    ids: &mut IdSource,
    page: NodeId,
    name: &str,
    w: f64,
) -> (NodeId, NodeId) {
    let f = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(w, 40.0), "name": name }),
    )[0];
    let r = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    let made = run(
        doc,
        ids,
        "make_component",
        json!({ "nodes": [f.to_wire()] }),
    );
    assert_eq!(made, [f], "a frame is made a main where it stands");
    (f, r)
}

/// The JSON a `kind` argument takes for a rectangle of `w` × `h`.
fn rect(w: f64, h: f64) -> Value {
    serde_json::to_value(NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap()
}

/// Every verb has a snake_case name of its own, a doc line, and parameters with
/// distinct names, each documented, each choice offering words.
#[test]
fn every_verb_is_well_formed() {
    let mut seen = std::collections::HashSet::new();
    for v in verbs::VERBS {
        assert!(seen.insert(v.name), "{} twice", v.name);
        assert!(
            v.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{} is not snake_case",
            v.name
        );
        assert!(!v.doc.is_empty(), "{} has no doc", v.name);
        let mut names = std::collections::HashSet::new();
        for p in v.params {
            assert!(names.insert(p.name), "{}: `{}` twice", v.name, p.name);
            assert!(!p.doc.is_empty(), "{}: `{}` has no doc", v.name, p.name);
            if let ParamKind::Choice(words) = p.kind {
                assert!(!words.is_empty(), "{}: `{}` offers nothing", v.name, p.name);
            }
        }
        assert_eq!(verbs::find(v.name).map(|f| f.name), Some(v.name));
    }
}

/// Every model type a parameter takes has an example, and the example reads as
/// that type, so a caller shown it can send it back as it is.
///
/// flip: `check_model` reading `NodeKind` as another type → fails at "reads as `NodeKind`".
#[test]
fn every_model_parameter_has_an_example_that_reads() {
    let mut models: Vec<&str> = verbs::VERBS
        .iter()
        .flat_map(|v| v.params)
        .filter_map(|p| match p.kind {
            ParamKind::Model(m) => Some(m),
            _ => None,
        })
        .collect();
    models.sort_unstable();
    models.dedup();
    assert!(models.len() > 10, "{models:?}");
    for m in models {
        let example = verbs::example(m).unwrap_or_else(|| panic!("`{m}` has no example"));
        verbs::check_model(m, &example)
            .unwrap_or_else(|e| panic!("`{m}`'s example reads as `{m}`: {e}\n{example}"));
    }
    assert!(verbs::check_model("Colour", &json!("#fff")).is_err());

    let (mut doc, mut ids, page) = blank();
    let tree = verbs::example("Tree").unwrap();
    let made = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": tree }),
    );
    assert_eq!(made.len(), 3, "the tree example commits whole");
}

/// Create two rectangles, fill one, group them, align their left edges, then
/// delete the group: each verb's edit commits, and the document reads as the
/// canvas would leave it.
#[test]
fn a_run_of_verbs_edits_the_document() {
    let (mut doc, mut ids, page) = blank();
    let page_wire = page.to_wire();
    let a = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page_wire, "kind": rect(40.0, 20.0), "name": "A" }),
    )[0];
    let b = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({
            "parent": page_wire,
            "kind": rect(10.0, 10.0),
            "transform": serde_json::to_value(Affine::translate((100.0, 50.0))).unwrap(),
        }),
    )[0];
    assert_eq!(doc.get(page).unwrap().children(), [a, b], "b on top");
    assert_eq!(doc.get(a).unwrap().name(), "A");

    let red = Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 30, 40)).into(),
        visible: true,
    };
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [a.to_wire()], "fills": [serde_json::to_value(&red).unwrap()] }),
    );
    assert_eq!(doc.get(a).unwrap().paint().fills.len(), 1);

    run(
        &mut doc,
        &mut ids,
        "align",
        json!({ "nodes": [a.to_wire(), b.to_wire()], "edge": "left" }),
    );
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(b).unwrap().x0,
        res.world_bounds(a).unwrap().x0,
        "left edges lined up"
    );

    let group = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    )[0];
    assert_eq!(doc.get(group).unwrap().children(), [a, b]);
    run(
        &mut doc,
        &mut ids,
        "delete_node",
        json!({ "nodes": [group.to_wire(), a.to_wire()] }),
    );
    assert!(doc.get(group).is_none() && doc.get(a).is_none());
    assert!(doc.get(page).unwrap().children().is_empty());
}

/// Arguments are refused before a verb runs, each refusal naming the parameter:
/// missing, unknown, of the wrong kind, a word not offered, a malformed id.
#[test]
fn arguments_are_checked_against_the_parameters() {
    let (doc, mut ids, page) = blank();
    let res = Resolved::rebuild(&doc);
    let mut ctx = Ctx {
        doc: &doc,
        res: &res,
        ids: &mut ids,
        images: None,
        contrast: None,
    };
    let p = page.to_wire();
    let mut refused = |name: &str, args: Value| match verbs::call(&mut ctx, name, &args) {
        Err(e) => e,
        Ok(_) => panic!("{name} {args} was not refused"),
    };
    assert!(matches!(
        refused("set_name", json!({ "node": p })),
        VerbError::Missing { param: "name", .. }
    ));
    assert!(matches!(
        refused("set_name", json!({ "node": p, "name": "x", "colour": 1 })),
        VerbError::Unexpected { .. }
    ));
    assert!(matches!(
        refused("set_visible", json!({ "nodes": [p], "visible": "yes" })),
        VerbError::Invalid {
            param: "visible",
            ..
        }
    ));
    assert!(matches!(
        refused("boolean", json!({ "nodes": [p], "op": "merge" })),
        VerbError::Invalid { param: "op", .. }
    ));
    assert!(matches!(
        refused("set_name", json!({ "node": "page one", "name": "x" })),
        VerbError::Invalid { param: "node", .. }
    ));
    assert!(matches!(
        refused("set_visible", json!({ "nodes": [], "visible": true })),
        VerbError::Invalid { param: "nodes", .. }
    ));
    assert!(matches!(
        refused("set_name", json!(["not", "an", "object"])),
        VerbError::NotAnObject("set_name")
    ));
    assert!(matches!(
        refused("make_it_pop", json!({})),
        VerbError::Unknown(_)
    ));
}

/// A fill list written whole keeps the item ids the layer's list holds at each
/// position, so an instance still reads the edit as overrides of its main's items.
///
/// flip: `set_fills` keying by position from scratch (`keyed_by_position`) → fails at
/// "the first fill keeps its id".
#[test]
fn a_fill_list_written_whole_keeps_its_ids_by_position() {
    let (mut doc, mut ids, page) = blank();
    let a = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    let fill = |r| Fill {
        brush: Brush::Solid(Color::from_rgb8(r, 0, 0)).into(),
        visible: true,
    };
    // Ids as an edit in the inspector mints them, not positional ones.
    let own: Vec<_> = (0..2)
        .map(|i| ondin_core::Keyed::new(ids.mint_item(), fill(10 + i)))
        .collect();
    let first: Vec<_> = own.iter().map(|k| k.id).collect();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: a,
        fills: own,
    }]))
    .unwrap();
    let fills: Vec<Value> = [30, 40, 50]
        .map(|r| serde_json::to_value(fill(r)).unwrap())
        .into();
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [a.to_wire()], "fills": fills }),
    );
    let again: Vec<_> = doc
        .get(a)
        .unwrap()
        .paint()
        .fills
        .iter()
        .map(|k| k.id)
        .collect();
    assert_eq!(again[0], first[0], "the first fill keeps its id");
    assert_eq!(again[1], first[1]);
    assert!(!first.contains(&again[2]), "a new position gets a new id");
}

/// Deleting a frame takes the guides scoped to it, ahead of the frame, so the
/// inverse puts the frame back before its guide and the undo applies.
///
/// flip: `build::delete` emitting the `DeleteNode`s before `guides_of` → fails at
/// "the undo applies".
#[test]
fn deleting_a_frame_takes_its_guides_and_undoes() {
    let (mut doc, mut ids, page) = blank();
    let frame = ids.mint();
    let guide = GuideId(ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::AddGuide {
            guide: Guide {
                id: guide,
                axis: GuideAxis::Vertical,
                position: 20.0,
                color: None,
                owner: frame,
            },
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let Done { tx, .. } = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "delete_node",
        &json!({ "nodes": [frame.to_wire()] }),
    )
    .unwrap();
    let undo = doc.apply(&tx).expect("the delete commits").inverse;
    assert!(doc.guide(guide).is_none(), "the guide went with its frame");
    doc.apply(&undo).expect("the undo applies");
    assert!(doc.get(frame).is_some() && doc.guide(guide).is_some());
}

/// `clean_up` with no layers named cleans every page with the box's defaults, and
/// a rule switched off by name is left.
#[test]
fn clean_up_takes_what_draws_nothing_and_obeys_its_switches() {
    let (mut doc, mut ids, page) = blank();
    let f = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(100.0, 100.0) }),
    )[0];
    let empty = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": f.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "clean_up",
        json!({ "empty_layers": false }),
    );
    assert!(doc.get(empty).is_some(), "switched off by name");
    run(&mut doc, &mut ids, "clean_up", json!({}));
    assert!(
        doc.get(empty).is_none() && doc.get(f).is_none(),
        "both drew nothing"
    );
}

/// `duplicate` copies the outermost layers given, each copy one step above its
/// original with a copy's name, moved by the offset; a child named beside its group
/// is copied once, inside the group's copy.
///
/// flip: `duplicate_placements` skipping `outermost` → fails at "one copy per outermost layer".
#[test]
fn duplicate_copies_each_outermost_layer_above_it() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let a = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let b = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let c = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let group = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [b.to_wire(), c.to_wire()] }),
    )[0];
    let copies = run(
        &mut doc,
        &mut ids,
        "duplicate",
        json!({ "nodes": [a.to_wire(), group.to_wire(), b.to_wire()], "dx": 30, "dy": -5 }),
    );
    assert_eq!(copies.len(), 2, "one copy per outermost layer");
    assert_eq!(
        doc.get(page).unwrap().children(),
        [a, copies[0], group, copies[1]],
        "each copy one step above its original"
    );
    assert_eq!(doc.get(copies[1]).unwrap().children().len(), 2);
    assert_ne!(
        doc.get(copies[0]).unwrap().name(),
        doc.get(a).unwrap().name()
    );
    let res = Resolved::rebuild(&doc);
    let (from, to) = (
        res.world_bounds(a).unwrap(),
        res.world_bounds(copies[0]).unwrap(),
    );
    assert_eq!((to.x0 - from.x0, to.y0 - from.y0), (30.0, -5.0));
}

/// `insert_subtree` creates a nested tree in one edit, each layer with the
/// properties given and its children bottom first, answers its layers root first,
/// and undoes as one step; a field no layer takes is refused before anything runs.
///
/// flip: `tree_ops` creating each child at index 0 → fails at "children bottom first".
#[test]
fn insert_subtree_creates_a_tree_in_one_edit() {
    let (mut doc, mut ids, page) = blank();
    let fill = serde_json::to_value(Fill {
        brush: Brush::Solid(Color::from_rgb8(10, 20, 30)).into(),
        visible: true,
    })
    .unwrap();
    let frame = serde_json::to_value(NodeKind::Artboard {
        size: Size::new(200.0, 100.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    })
    .unwrap();
    let tree = json!({
        "kind": frame,
        "name": "Card",
        "clip": true,
        "children": [
            { "kind": rect(10.0, 10.0), "name": "Back", "fills": [fill], "opacity": 0.5 },
            { "kind": "Group", "name": "Front", "children": [
                { "kind": rect(5.0, 5.0), "blend": "Multiply", "visible": false },
            ] },
        ],
    });
    let res = Resolved::rebuild(&doc);
    let Done { tx, created, .. } = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "insert_subtree",
        &json!({ "parent": page.to_wire(), "tree": tree }),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let undo = doc.apply(&tx).expect("the tree commits").inverse;
    let [card, back, front, inner] = created[..] else {
        panic!("four layers, root first: {created:?}");
    };
    assert_eq!(doc.get(page).unwrap().children(), [card]);
    assert_eq!(
        doc.get(card).unwrap().children(),
        [back, front],
        "children bottom first"
    );
    assert_eq!(doc.get(front).unwrap().children(), [inner]);
    assert_eq!(doc.get(card).unwrap().name(), "Card");
    let b = doc.get(back).unwrap();
    assert_eq!((b.paint().fills.len(), b.opacity()), (1, 0.5));
    let i = doc.get(inner).unwrap();
    assert!(!i.visible());
    assert_eq!(i.blend(), ondin_core::BlendMode::Multiply);
    doc.apply(&undo).expect("the undo applies");
    assert!(doc.get(page).unwrap().children().is_empty());

    let res = Resolved::rebuild(&doc);
    let refused = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "insert_subtree",
        &json!({ "parent": page.to_wire(), "tree": { "kind": rect(1.0, 1.0), "colour": "red" } }),
    );
    assert!(
        matches!(refused, Err(VerbError::Invalid { param: "tree", .. })),
        "{refused:?}"
    );
}

/// Why a model argument is refused, panicking when it is not refused so.
fn refused_why(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> String {
    match answer(doc, ids, name, args) {
        Err(VerbError::Invalid { why, .. }) => why,
        other => panic!("{name}: {other:?}"),
    }
}

/// A model argument is refused naming where it went wrong, and a field the model
/// has no place for is refused rather than dropped.
///
/// flip: `read_model` leaving out the `serde_ignored` check → the misspelt tree
/// is taken, and `refused_why` panics.
#[test]
fn a_model_argument_is_refused_with_its_path() {
    let (doc, mut ids, page) = blank();
    let typo = json!({ "kind": frame(10.0, 10.0), "display": { "Flex": { "gapp": 4 } } });
    let why = refused_why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": typo }),
    );
    assert!(
        why.starts_with("at `display.Flex.gapp`: no such field; the fields there are")
            && why.contains("`gap`")
            && why.contains("`wrap`"),
        "a field it does not have, with every one there is: {why}"
    );
    let text = json!({ "Text": { "content": "Hi", "sizing": { "AutoHeight": { "w": 1 } } } });
    let bad =
        json!({ "kind": "Group", "children": [{ "kind": rect(1.0, 1.0) }, { "kind": text }] });
    let why = refused_why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": bad }),
    );
    assert!(
        why.starts_with("at `children[1].kind.Text.sizing"),
        "the path of the refused value: {why}"
    );
}

/// A field a stroke or a paragraph has no place for is refused with every
/// field it has, which no example shows whole, and a tree's corners point to
/// its `kind`.
///
/// flip: `model_fields` answering `None` for a stroke → fails at "a stroke's
/// `dashes`", whose example leaves them out.
#[test]
fn an_unknown_field_is_refused_with_every_field_there_is() {
    let (doc, mut ids, page) = blank();
    let dashed = json!({ "kind": rect(10.0, 10.0),
        "strokes": [{ "brush": "#000000", "width": 2, "dash": [4, 4] }] });
    let why = refused_why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": dashed }),
    );
    assert!(
        why.contains("the fields there are") && why.contains("`dashes`"),
        "a stroke's `dashes`: {why}"
    );
    let spaced = json!({ "kind": { "Text": { "content": "Hi", "sizing": "Auto",
        "paragraph": { "paragraph_spacing": "8px" } } } });
    let why = refused_why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": spaced }),
    );
    assert!(why.contains("`spacing`"), "a paragraph's `spacing`: {why}");
    let rounded = json!({ "kind": frame(10.0, 10.0), "corner_radius": 8 });
    let why = refused_why(
        &doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": rounded }),
    );
    assert!(why.contains("part of its `kind`"), "{why}");
}

/// `insert_subtree` takes the forms `get_node` and CSS write: lengths as `"22px"`,
/// a sizing as `{type, width}`, an effect and a fill flat with their opacity, a
/// rectangle with no corners, one-number padding and `gap`, a path as SVG data;
/// and `verbs::tree` answers a length and the path in those forms, which read back.
///
/// flip: `loosen_effect` keeping a flat effect's `opacity` out of its colour →
/// fails at "the opacity is the shadow's alpha".
#[test]
fn insert_subtree_takes_what_get_node_and_css_write() {
    use ondin_core::container::Display;
    use ondin_core::{EffectKind, Length, TextSizing};
    let (mut doc, mut ids, page) = blank();
    let tree = json!({
        "kind": frame(300.0, 200.0),
        "display": { "Flex": { "gap": 12, "padding": 8 } },
        "children": [
            { "kind": { "Rect": { "size": { "width": 40, "height": 40 } } },
              "fills": [{ "id": "0:0", "index": 0, "paint": { "type": "Solid", "color": "#ff7a1a", "alpha": 0.5 } }],
              "effects": [{ "kind": "DropShadow", "blur": 14, "color": "#12352a", "opacity": 0.3, "offset": [0, 6], "spread": 0 }] },
            { "kind": { "Text": { "content": "Hi", "sizing": { "type": "AutoHeight", "width": 120 },
                                   "style": { "line_height": "22px", "letter_spacing": "-0.5px" } } } },
            { "kind": { "Path": { "svg": "M0,0 L10,10 L20,0" } } },
        ],
    });
    let created = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": tree }),
    );
    let [card, rect, text, path] = created[..] else {
        panic!("four layers: {created:?}");
    };
    let Some(Display::Flex(flex)) = doc.get(card).unwrap().display() else {
        panic!("a flex frame");
    };
    assert_eq!(
        (flex.row_gap, flex.column_gap, flex.padding),
        (12.0, 12.0, [8.0; 4])
    );
    let r = doc.get(rect).unwrap();
    let NodeKind::Rect { corner_radii, .. } = r.kind() else {
        panic!("a rect");
    };
    assert_eq!(corner_radii.top_left, 0.0, "square unless given");
    let fill = &r.paint().fills[0];
    assert_eq!(
        fill.brush,
        Brush::Solid(Color::from_rgb8(255, 122, 26).multiply_alpha(0.5)).into(),
        "the paint's alpha is the fill's"
    );
    let effect = &r.effects()[0];
    let EffectKind::DropShadow(shadow) = &effect.kind else {
        panic!("a drop shadow");
    };
    assert!(effect.visible);
    assert!(
        (shadow.color.components[3] - 0.3).abs() < 1e-6,
        "the opacity is the shadow's alpha"
    );
    let NodeKind::Text { style, sizing, .. } = doc.get(text).unwrap().kind() else {
        panic!("a text");
    };
    assert_eq!(style.line_height, Some(Length::Px(22.0)));
    assert_eq!(style.letter_spacing, Length::Px(-0.5));
    assert_eq!(*sizing, TextSizing::AutoHeight(120.0));
    let read_text = verbs::tree(&doc, text).expect("the text's tree");
    assert_eq!(
        read_text["kind"]["Text"]["style"]["line_height"],
        json!("22px"),
        "a length reads back as written"
    );
    let read = verbs::tree(&doc, path).expect("the path's tree");
    assert_eq!(read["kind"]["Path"]["path"], json!("M0,0 L10,10 L20,0"));
    let again = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": read }),
    );
    assert_eq!(
        doc.get(again[0]).unwrap().kind(),
        doc.get(path).unwrap().kind(),
        "the tree read back writes the same path"
    );
}

/// `style_text` sets a range of characters as runs, counted in characters, not
/// bytes, and the whole text as its defaults with that attribute's runs dropped;
/// `style_paragraphs` reaches the whole paragraph a range touches; a range past the
/// text is refused.
///
/// flip: `char_range` taking the arguments as byte offsets → fails at "the run
/// covers `world`".
#[test]
fn style_text_sets_runs_over_a_range_and_defaults_over_all() {
    use ondin_core::{CharAttr, Length, ParaAttr};
    let (mut doc, mut ids, page) = blank();
    let content = "Héllo world\nSecond";
    let kind = serde_json::to_value(NodeKind::Text {
        content: content.into(),
        style: Box::default(),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    })
    .unwrap();
    let t = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": kind }),
    )[0];
    let w = t.to_wire();
    let bold = serde_json::to_value(vec![CharAttr::Weight(700)]).unwrap();
    run(
        &mut doc,
        &mut ids,
        "style_text",
        json!({ "node": w, "attrs": bold, "start": 6, "end": 11 }),
    );
    let text = |doc: &Document| match doc.get(t).unwrap().kind().clone() {
        NodeKind::Text {
            style,
            spans,
            para_spans,
            ..
        } => (style, spans, para_spans),
        _ => unreachable!(),
    };
    let (style, spans, _) = text(&doc);
    let world = content.find("world").unwrap();
    assert_eq!(
        spans.values_in(&style, ondin_core::CharAttrKind::Weight, world..world + 5),
        [CharAttr::Weight(700)],
        "the run covers `world`"
    );
    assert_eq!(
        style.get(ondin_core::CharAttrKind::Weight),
        CharAttr::Weight(400)
    );

    run(
        &mut doc,
        &mut ids,
        "style_text",
        json!({ "node": w, "attrs": serde_json::to_value(vec![CharAttr::Weight(300)]).unwrap() }),
    );
    let (style, spans, _) = text(&doc);
    assert_eq!(
        style.get(ondin_core::CharAttrKind::Weight),
        CharAttr::Weight(300)
    );
    assert!(
        spans.is_empty(),
        "the whole-text write drops the weight's runs"
    );

    let spacing = serde_json::to_value(vec![ParaAttr::Spacing(Length::Px(12.0))]).unwrap();
    run(
        &mut doc,
        &mut ids,
        "style_paragraphs",
        json!({ "node": w, "attrs": spacing, "start": 14, "end": 15 }),
    );
    let (_, _, para) = text(&doc);
    let second = content.find("Second").unwrap();
    assert_eq!(para.as_slice().len(), 1);
    assert_eq!(
        (para.as_slice()[0].start, para.as_slice()[0].end),
        (second, content.len())
    );

    let res = Resolved::rebuild(&doc);
    let past = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "style_text",
        &json!({ "node": w, "attrs": [], "end": 99 }),
    );
    assert!(
        matches!(past, Err(VerbError::Invalid { param: "end", .. })),
        "{past:?}"
    );
}

/// `set_text_style` changes only the fields it is given: each text keeps its own
/// family and the rest, and a length may be written `"28px"`.
///
/// flip: `set_text_style` completing the style from the default rather than from
/// the text's own → fails at "the family is the text's own".
#[test]
fn set_text_style_changes_only_what_it_is_given() {
    use ondin_core::{Length, TextStyle};
    let (mut doc, mut ids, page) = blank();
    let style = TextStyle {
        font_family: "Inter Tight".into(),
        italic: true,
        ..TextStyle::default()
    };
    let kind = json!({ "Text": { "content": "Hi", "sizing": "Auto", "style": style } });
    let t = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": kind }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "font_size": 20, "line_height": "28px" } }),
    );
    let NodeKind::Text { style, .. } = doc.get(t).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(
        style.font_family, "Inter Tight",
        "the family is the text's own"
    );
    assert!(style.italic);
    assert_eq!(
        (style.font_size, style.line_height),
        (20.0, Some(Length::Px(28.0)))
    );
}

/// The JSON a `kind` argument takes for an open path from `a` to `b`.
fn segment(a: (f64, f64), b: (f64, f64)) -> Value {
    let mut path = ondin_core::kurbo::BezPath::new();
    path.move_to(a);
    path.line_to(b);
    serde_json::to_value(NodeKind::Path {
        path,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    })
    .unwrap()
}

/// `join_paths` splices two open paths at their nearest ends into the first, and
/// the second goes; a path with no open end is refused.
///
/// flip: `edit::path_join_ends` keeping the farthest pair → fails at "joined at
/// the near ends".
#[test]
fn join_paths_splices_the_nearest_ends() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let a = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": segment((0.0, 0.0), (10.0, 0.0)) }),
    )[0];
    let b = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": segment((12.0, 0.0), (30.0, 5.0)) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "join_paths",
        json!({ "node": a.to_wire(), "other": b.to_wire() }),
    );
    assert!(doc.get(b).is_none(), "the second path went");
    let res = Resolved::rebuild(&doc);
    let (_, subs) = ondin_core::edit::path_subpaths(&doc, &res, a).unwrap();
    let at: Vec<(f64, f64)> = subs[0].anchors.iter().map(|x| (x.at.x, x.at.y)).collect();
    assert_eq!(subs.len(), 1);
    assert_eq!(
        at,
        [(0.0, 0.0), (10.0, 0.0), (12.0, 0.0), (30.0, 5.0)],
        "joined at the near ends"
    );
}

/// A path layer of `path` at `at`, made through `create_node`.
fn path_at(doc: &mut Document, ids: &mut IdSource, page: NodeId, path: &str, at: Affine) -> NodeId {
    let kind = serde_json::to_value(NodeKind::Path {
        path: ondin_core::kurbo::BezPath::from_svg(path).unwrap(),
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    })
    .unwrap();
    let transform = serde_json::to_value(at).unwrap();
    run(
        doc,
        ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": kind, "transform": transform }),
    )[0]
}

/// The anchors of path `id` in its own space: each point and its arriving and
/// leaving control points.
fn local_anchors(doc: &Document, id: NodeId) -> Vec<[(f64, f64); 3]> {
    let NodeKind::Path {
        path, corner_radii, ..
    } = doc.get(id).unwrap().kind()
    else {
        panic!("not a path");
    };
    let xy = |p: ondin_core::kurbo::Point| ((p.x * 1e6).round() / 1e6, (p.y * 1e6).round() / 1e6);
    ondin_core::edit::pen_anchors(path, corner_radii)
        .iter()
        .flat_map(|s| s.anchors.clone())
        .map(|a| [xy(a.at), xy(a.arriving()), xy(a.leaving())])
        .collect()
}

/// `set_point` reads and writes a point in the layer's own space, keeps a
/// coordinate it was not given, and leaves the other points where they were.
///
/// flip: `set_point` placing `x` and `y` in world space → fails at "moved in
/// the layer's space".
#[test]
fn set_point_moves_a_point_in_the_layers_space() {
    let (mut doc, mut ids, page) = blank();
    let at = Affine::translate((100.0, 50.0));
    let p = path_at(&mut doc, &mut ids, page, "M0,0 L10,0 L10,10", at);
    run(
        &mut doc,
        &mut ids,
        "set_point",
        json!({ "node": p.to_wire(), "point": 1, "x": 20 }),
    );
    let points: Vec<(f64, f64)> = local_anchors(&doc, p).iter().map(|a| a[0]).collect();
    assert_eq!(
        points,
        [(0.0, 0.0), (20.0, 0.0), (10.0, 10.0)],
        "moved in the layer's space"
    );
}

/// One handle given on a smooth point mirrors the other; both given set both,
/// breaking the pair; then one given on the broken point leaves the other.
///
/// flip: `set_point` always breaking the pair → fails at "a smooth point stays
/// smooth".
#[test]
fn set_point_mirrors_a_handle_only_on_a_smooth_point() {
    let (mut doc, mut ids, page) = blank();
    let p = path_at(
        &mut doc,
        &mut ids,
        page,
        "M0,0 C0,0 5,0 10,0 C15,0 20,0 20,0",
        Affine::IDENTITY,
    );
    let set = |doc: &mut Document, ids: &mut IdSource, args: Value| {
        let mut call = json!({ "node": p.to_wire(), "point": 1 });
        call.as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        run(doc, ids, "set_point", call);
    };
    set(&mut doc, &mut ids, json!({ "out_x": 10, "out_y": 5 }));
    assert_eq!(
        local_anchors(&doc, p)[1],
        [(10.0, 0.0), (10.0, -5.0), (10.0, 5.0)],
        "a smooth point stays smooth"
    );
    set(
        &mut doc,
        &mut ids,
        json!({ "in_x": 5, "in_y": 0, "out_x": 10, "out_y": 5 }),
    );
    assert_eq!(
        local_anchors(&doc, p)[1],
        [(10.0, 0.0), (5.0, 0.0), (10.0, 5.0)],
        "both handles set as given"
    );
    set(&mut doc, &mut ids, json!({ "out_y": 8 }));
    assert_eq!(
        local_anchors(&doc, p)[1],
        [(10.0, 0.0), (5.0, 0.0), (10.0, 8.0)],
        "a broken point stays broken"
    );
}

/// `set_point` rounds the corner at a point, and refuses a layer that is not a
/// path, a point or subpath past the end, a negative radius and a call that
/// changes nothing.
#[test]
fn set_point_rounds_a_corner_and_refuses_what_is_not_there() {
    let (mut doc, mut ids, page) = blank();
    let p = path_at(
        &mut doc,
        &mut ids,
        page,
        "M0,0 L10,0 L10,10",
        Affine::IDENTITY,
    );
    run(
        &mut doc,
        &mut ids,
        "set_point",
        json!({ "node": p.to_wire(), "point": 1, "radius": 4 }),
    );
    let NodeKind::Path { corner_radii, .. } = doc.get(p).unwrap().kind() else {
        panic!("not a path");
    };
    assert_eq!(corner_radii, &[0.0, 4.0]);
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    let wire = p.to_wire();
    for (args, param) in [
        (json!({ "node": r.to_wire(), "point": 0, "x": 1 }), "node"),
        (json!({ "node": wire, "point": 3, "x": 1 }), "point"),
        (
            json!({ "node": wire, "point": 0, "subpath": 1, "x": 1 }),
            "subpath",
        ),
        (json!({ "node": wire, "point": 0, "radius": -1 }), "radius"),
    ] {
        assert_eq!(refused_param(&doc, &mut ids, "set_point", args), param);
    }
    let idle = answer(
        &doc,
        &mut ids,
        "set_point",
        json!({ "node": wire, "point": 0 }),
    );
    assert!(matches!(idle, Err(VerbError::Nothing(_))), "{idle:?}");
}

/// `set_geometry` refuses a rounded path's drawn outline written back as its
/// points, whole or with a point moved, and takes its anchors moved.
///
/// flip: `bakes_rounding` answering `false` → the drawn outline commits, and
/// `refused_param` panics on it.
#[test]
fn set_geometry_refuses_a_rounded_paths_drawn_outline() {
    use ondin_core::kurbo::BezPath;
    let (mut doc, mut ids, page) = blank();
    let p = path_at(
        &mut doc,
        &mut ids,
        page,
        "M0,0 L40,0 L40,40",
        Affine::IDENTITY,
    );
    run(
        &mut doc,
        &mut ids,
        "set_point",
        json!({ "node": p.to_wire(), "point": 1, "radius": 8 }),
    );
    let drawn = ondin_core::geometry::local_path(doc.get(p).unwrap().kind()).unwrap();
    let mut nudged = drawn.clone();
    if let Some(ondin_core::kurbo::PathEl::LineTo(end)) = nudged.elements_mut().last_mut() {
        end.y += 5.0;
    }
    let patch = |path: BezPath, radii: Vec<f64>| {
        json!({
            "node": p.to_wire(),
            "geometry": serde_json::to_value(ondin_core::GeometryPatch::Path {
                path,
                corner_radii: radii,
                mirror: None,
            })
            .unwrap(),
        })
    };
    assert_eq!(
        refused_param(&doc, &mut ids, "set_geometry", patch(drawn, vec![])),
        "geometry",
        "the outline as drawn"
    );
    assert_eq!(
        refused_param(&doc, &mut ids, "set_geometry", patch(nudged, vec![])),
        "geometry",
        "the outline with its last point moved"
    );
    let moved = BezPath::from_svg("M0,0 L50,0 L40,40").unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_geometry",
        patch(moved, vec![0.0, 8.0]),
    );
    let points: Vec<(f64, f64)> = local_anchors(&doc, p).iter().map(|a| a[0]).collect();
    assert_eq!(points, [(0.0, 0.0), (50.0, 0.0), (40.0, 40.0)]);
}

/// `repeat` makes a group of the original and its copies round the point, named
/// for the count.
#[test]
fn repeat_makes_a_ring_in_a_group() {
    let (mut doc, mut ids, page) = blank();
    let at = serde_json::to_value(Affine::translate((100.0, 0.0))).unwrap();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0), "name": "Tick", "transform": at }),
    )[0];
    let g = run(
        &mut doc,
        &mut ids,
        "repeat",
        json!({ "nodes": [r.to_wire()], "count": 4, "x": 0, "y": 0 }),
    )[0];
    let group = doc.get(g).unwrap();
    assert_eq!(group.children().len(), 4);
    assert_eq!(group.name(), "Tick × 4");
    let res = Resolved::rebuild(&doc);
    let b = res.world_bounds(g).unwrap();
    assert!(
        (b.x0 + 110.0).abs() < 1e-6 && (b.x1 - 110.0).abs() < 1e-6,
        "{b:?}"
    );
}

/// A linked `repeat` makes its rectangle a component inside a group named for it,
/// its copies instances of that main; a main is refused, its copies being linked
/// already.
///
/// flip: `repeat::linked` repeating the layer rather than the main it made →
/// fails at "one main among": the wrapper made the main is left out of the ring.
#[test]
fn a_linked_repeat_makes_instances() {
    let (mut doc, mut ids, page) = blank();
    let at = serde_json::to_value(Affine::translate((100.0, 0.0))).unwrap();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0), "name": "Tick", "transform": at }),
    )[0];
    let ring = run(
        &mut doc,
        &mut ids,
        "repeat",
        json!({ "nodes": [r.to_wire()], "count": 3, "x": 0, "y": 0, "linked": true }),
    )[0];
    let kids = doc.get(ring).unwrap().children().to_vec();
    assert_eq!(kids.len(), 3);
    let mains: Vec<NodeId> = kids
        .iter()
        .copied()
        .filter(|k| doc.get(*k).unwrap().component())
        .collect();
    let [main] = mains[..] else {
        panic!("one main among {kids:?}");
    };
    assert_eq!(doc.get(main).unwrap().name(), "Tick");
    assert!(
        kids.iter()
            .filter(|k| **k != main)
            .all(|k| ondin_core::component::main_of(&doc, *k) == Some(main)),
        "the copies are instances"
    );

    let res = Resolved::rebuild(&doc);
    let again = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "repeat",
        &json!({ "nodes": [main.to_wire()], "count": 2, "linked": true }),
    );
    assert!(
        matches!(
            again,
            Err(VerbError::Invalid {
                param: "linked",
                ..
            })
        ),
        "{again:?}"
    );

    let loose: Vec<String> = (0..2)
        .map(|_| {
            run(
                &mut doc,
                &mut ids,
                "create_node",
                json!({ "parent": page.to_wire(), "kind": rect(5.0, 5.0) }),
            )[0]
            .to_wire()
        })
        .collect();
    let inside = doc.get(main).unwrap().children()[0];
    for (nodes, says) in [
        (json!(loose), "one layer at a time"),
        (json!([inside.to_wire()]), "inside"),
    ] {
        match answer(
            &doc,
            &mut ids,
            "repeat",
            json!({ "nodes": nodes, "count": 2, "linked": true }),
        ) {
            Err(VerbError::Invalid {
                param: "linked",
                why,
                ..
            }) => assert!(why.contains(says), "{why}"),
            other => panic!("{says}: {other:?}"),
        }
    }
}

/// `set_size` sizes any kind: a rectangle's size, a group by its box (its child
/// scaled with it).
///
/// flip: `edit::resize_to` writing `GeometryPatch::Size` for every kind → fails at
/// the group's commit, which refuses a size on a group.
#[test]
fn set_size_sizes_any_kind() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_size",
        json!({ "node": r.to_wire(), "width": 40, "height": 20 }),
    );
    let res = Resolved::rebuild(&doc);
    assert_eq!(res.world_bounds(r).unwrap().size(), Size::new(40.0, 20.0));
    let g = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [r.to_wire()] }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_size",
        json!({ "node": g.to_wire(), "width": 80, "height": 20 }),
    );
    let res = Resolved::rebuild(&doc);
    assert_eq!(res.world_bounds(r).unwrap().size(), Size::new(80.0, 20.0));
}

/// `pixel_fit` puts a rectangle's box on whole units, through the group it is in,
/// and leaves a layer that is already there as it is.
///
/// flip: `pixel_fit::fit_layer` not descending into a group → fails at "the box
/// lands on whole units".
#[test]
fn pixel_fit_puts_a_box_on_whole_units() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let at = serde_json::to_value(Affine::translate((10.3, 4.6))).unwrap();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(20.4, 9.7), "transform": at }),
    )[0];
    let g = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [r.to_wire()] }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "pixel_fit",
        json!({ "nodes": [g.to_wire()] }),
    );
    let res = Resolved::rebuild(&doc);
    let b = res.world_bounds(r).unwrap();
    assert_eq!(
        (b.x0, b.y0, b.x1, b.y1),
        (10.0, 5.0, 31.0, 14.0),
        "the box lands on whole units"
    );
    let again = Resolved::rebuild(&doc);
    let Done { tx, .. } = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &again,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "pixel_fit",
        &json!({ "nodes": [g.to_wire()] }),
    )
    .unwrap();
    assert!(tx.0.is_empty(), "already on the grid: {tx:?}");
}

/// `set_blend` takes the modes as the snapshot labels them and writes as the panel
/// does: the outermost layers only, and Pass through as Normal on a layer that is
/// not a frame or group, in a tree's layers too.
///
/// flip: `set_blend` writing a `SetBlend` per node given → fails at "the child is left
/// to its group".
#[test]
fn set_blend_writes_the_outermost_layers_by_label() {
    let labels: Vec<&str> = ondin_core::BlendMode::ALL
        .iter()
        .map(|m| m.label())
        .collect();
    assert_eq!(verbs::BLENDS, labels, "the choice is the snapshot's labels");

    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let a = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let b = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let group = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [a.to_wire()] }),
    )[0];
    let wire = [group.to_wire(), a.to_wire()];
    run(
        &mut doc,
        &mut ids,
        "set_blend",
        json!({ "nodes": wire, "blend": "Color burn" }),
    );
    assert_eq!(
        doc.get(group).unwrap().blend(),
        ondin_core::BlendMode::ColorBurn
    );
    assert_eq!(
        doc.get(a).unwrap().blend(),
        ondin_core::BlendMode::Normal,
        "the child is left to its group"
    );

    run(
        &mut doc,
        &mut ids,
        "set_blend",
        json!({ "nodes": [group.to_wire(), b.to_wire()], "blend": "Pass through" }),
    );
    assert_eq!(
        doc.get(group).unwrap().blend(),
        ondin_core::BlendMode::PassThrough
    );
    assert_eq!(doc.get(b).unwrap().blend(), ondin_core::BlendMode::Normal);

    let leaf = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": p, "tree": { "kind": rect(1.0, 1.0), "blend": "Pass through" } }),
    )[0];
    assert_eq!(
        doc.get(leaf).unwrap().blend(),
        ondin_core::BlendMode::Normal
    );
}

/// The page verbs: a page added where asked, duplicated with its layers below it as
/// `… copy`, moved, marked a draft, and deleted with its layers in an edit that
/// undoes; the last page is never deleted.
///
/// flip: `page::duplicate` placing the copy at the original's own index → fails at
/// "the copy sits below its page".
#[test]
fn pages_are_added_copied_moved_and_deleted() {
    let (mut doc, mut ids, first) = blank();
    let landing = run(&mut doc, &mut ids, "add_page", json!({ "name": "Landing" }))[0];
    let p = landing.to_wire();
    run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    );
    let copy = run(&mut doc, &mut ids, "duplicate_page", json!({ "page": p }))[0];
    let order = |doc: &Document| doc.page_ids().collect::<Vec<_>>();
    assert_eq!(
        order(&doc),
        [first, landing, copy],
        "the copy sits below its page"
    );
    assert_eq!(doc.get(copy).unwrap().name(), "Landing copy");
    assert_eq!(doc.get(copy).unwrap().children().len(), 1);

    run(
        &mut doc,
        &mut ids,
        "move_page",
        json!({ "page": copy.to_wire(), "index": 0 }),
    );
    assert_eq!(order(&doc), [copy, first, landing]);
    run(
        &mut doc,
        &mut ids,
        "set_page_draft",
        json!({ "page": p, "draft": true }),
    );
    assert!(doc.page(landing).unwrap().draft);

    let res = Resolved::rebuild(&doc);
    let Done { tx, .. } = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "delete_page",
        &json!({ "page": p }),
    )
    .unwrap();
    let undo = doc.apply(&tx).expect("the delete commits").inverse;
    assert_eq!(order(&doc), [copy, first]);
    doc.apply(&undo).expect("the undo applies");
    assert_eq!(doc.get(landing).unwrap().children().len(), 1);

    run(&mut doc, &mut ids, "delete_page", json!({ "page": p }));
    run(
        &mut doc,
        &mut ids,
        "delete_page",
        json!({ "page": copy.to_wire() }),
    );
    let res = Resolved::rebuild(&doc);
    let last = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "delete_page",
        &json!({ "page": first.to_wire() }),
    );
    assert!(matches!(last, Err(VerbError::Nothing(_))), "{last:?}");
}

/// The component verbs: a rectangle wrapped and made a main, an instance placed
/// with its box's corner on a point that follows its main's edit, swapped to another main, and
/// detached; a frame already a main is refused.
///
/// flip: the test's commit without `propagate::owed` → fails at "the instance
/// follows its main".
#[test]
fn components_are_made_placed_swapped_and_detached() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let (card, inner) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    let (other, _) = main_component(&mut doc, &mut ids, page, "Other", 100.0);

    let loose = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(5.0, 5.0) }),
    )[0];
    let wrapped = run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [loose.to_wire()] }),
    )[0];
    assert_ne!(wrapped, loose, "a rectangle is wrapped in a frame first");
    assert!(doc.get(wrapped).unwrap().component());

    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": p, "x": 500, "y": 300 }),
    )[0];
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(inst).unwrap().origin(),
        ondin_core::kurbo::Point::new(500.0, 300.0)
    );
    assert_eq!(ondin_core::component::main_of(&doc, inst), Some(card));

    let red = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [inner.to_wire()], "fills": red }),
    );
    let copy = doc.get(inst).unwrap().children()[0];
    assert_eq!(
        doc.get(copy).unwrap().paint().fills.len(),
        1,
        "the instance follows its main"
    );

    run(
        &mut doc,
        &mut ids,
        "swap_instance",
        json!({ "node": inst.to_wire(), "main": other.to_wire() }),
    );
    assert_eq!(ondin_core::component::main_of(&doc, inst), Some(other));

    run(
        &mut doc,
        &mut ids,
        "detach_instance",
        json!({ "nodes": [inst.to_wire()] }),
    );
    assert_eq!(ondin_core::component::main_of(&doc, inst), None);

    let res = Resolved::rebuild(&doc);
    let again = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "make_component",
        &json!({ "nodes": [card.to_wire()] }),
    );
    assert!(
        matches!(again, Err(VerbError::Nothing(why)) if why.contains("already")),
        "{again:?}"
    );
}

/// `create_instance` reads `x`/`y` as its box's left and top in its parent's space,
/// an edge not given left at the main's place, and `swap_instance` to the main an
/// instance shows already answers an empty edit.
///
/// flip: `create_instance` taking `x`/`y` as world coordinates → fails at "its
/// box's corner in the frame's space"; `cornered_at` moving an edge not given to
/// 0 → fails at "an edge not given".
#[test]
fn an_instance_is_placed_in_its_parents_space_and_swapped_to_its_own_main() {
    let (mut doc, mut ids, page) = blank();
    let (card, _) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    let holder = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(400.0, 400.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_transform",
        json!({ "node": holder.to_wire(), "transform": Affine::translate((200.0, 100.0)) }),
    );
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": holder.to_wire(), "x": 50, "y": 20 }),
    )[0];
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(inst).unwrap().origin(),
        ondin_core::kurbo::Point::new(250.0, 120.0),
        "its box's corner in the frame's space"
    );
    run(
        &mut doc,
        &mut ids,
        "set_transform",
        json!({ "node": card.to_wire(), "transform": Affine::translate((0.0, 30.0)) }),
    );
    let only_x = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": holder.to_wire(), "x": 50 }),
    )[0];
    let res = Resolved::rebuild(&doc);
    assert_eq!(res.world_bounds(card).unwrap().y0, 30.0);
    assert_eq!(
        res.world_bounds(only_x).unwrap().origin(),
        ondin_core::kurbo::Point::new(250.0, 130.0),
        "an edge not given stays at the main's own place, in the frame"
    );
    let same = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "swap_instance",
        &json!({ "node": inst.to_wire(), "main": card.to_wire() }),
    )
    .unwrap_or_else(|e| panic!("the main it shows: {e}"));
    assert!(same.tx.0.is_empty());
}

/// A script commits each step with what the components owe it, so an instance
/// follows an edit its main took in a later step.
///
/// flip: `run_script` committing without `propagate::owed` → fails at "the
/// instance follows its main".
#[test]
fn a_script_keeps_instances_in_step_with_their_mains() {
    let (mut doc, mut ids, page) = blank();
    let red = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    let script = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": frame(50.0, 50.0) }, "as": "f" },
        { "verb": "create_node", "args": { "parent": "@f", "kind": rect(10.0, 10.0) }, "as": "r" },
        { "verb": "make_component", "args": { "nodes": ["@f"] } },
        { "verb": "create_instance", "args": { "main": "@f", "parent": "@page" }, "as": "i" },
        { "verb": "set_fills", "args": { "nodes": ["@r"], "fills": red } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let inst = NodeId::from_wire(&made[3][0]).unwrap();
    let copy = doc.get(inst).unwrap().children()[0];
    assert_eq!(
        doc.get(copy).unwrap().paint().fills.len(),
        1,
        "the instance follows its main"
    );
    assert!(doc.get(page).is_some());
}

/// The variable and mode verbs, run as a script whose labels carry their ids: a
/// colour variable added and renamed, a mode added and named, its value set there,
/// a frame made to show it, then the mode and the variable removed. A taken name
/// and a value of another kind are refused.
///
/// flip: `variable::set_cell_ops` taking a value of any kind → fails at "a number
/// is refused".
#[test]
fn variables_and_modes_are_added_set_and_removed() {
    let (mut doc, mut ids, page) = blank();
    let blue = serde_json::to_value(ondin_core::Cell::Color(Color::from_rgb8(0, 0, 255))).unwrap();
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "color", "name": "brand/primary" }, "as": "v" },
        { "verb": "add_mode", "args": { "name": "Dark" }, "as": "dark" },
        { "verb": "set_variable_value", "args": { "variable": "@v", "mode": "@dark", "value": blue } },
        { "verb": "rename_variable", "args": { "variable": "@v", "name": "brand/accent" } },
        { "verb": "rename_mode", "args": { "mode": "@dark", "name": "Night" } },
        { "verb": "create_node", "args": { "parent": "@page", "kind": frame(10.0, 10.0) }, "as": "f" },
        { "verb": "set_frame_mode", "args": { "node": "@f", "mode": "@dark" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let table = doc.variables();
    let var: ondin_core::VarId = serde_json::from_value(json!(made[0][0])).unwrap();
    let dark: ondin_core::ModeId = serde_json::from_value(json!(made[1][0])).unwrap();
    let v = table.get(var).unwrap();
    assert_eq!(v.name, "brand/accent");
    assert_eq!(
        v.cells.get(&dark),
        Some(&ondin_core::Cell::Color(Color::from_rgb8(0, 0, 255)))
    );
    assert_eq!(table.mode(dark).unwrap().name, "Night");
    let f = NodeId::from_wire(&made[5][0]).unwrap();
    assert_eq!(doc.get(f).unwrap().modes(), [dark]);

    let refused = |doc: &mut Document, ids: &mut IdSource, step: Value| {
        verbs::run_script(doc, ids, Lent::default(), &json!([step]))
            .expect_err("refused")
            .why
    };
    let taken = refused(
        &mut doc,
        &mut ids,
        json!({ "verb": "add_variable", "args": { "kind": "color", "name": "brand/accent" } }),
    );
    assert!(taken.contains("already exists"), "{taken}");
    let number = serde_json::to_value(ondin_core::Cell::Number(3.0)).unwrap();
    let wrong = refused(
        &mut doc,
        &mut ids,
        json!({ "verb": "set_variable_value", "args": { "variable": made[0][0], "mode": made[1][0], "value": number } }),
    );
    assert!(
        wrong.contains("another kind"),
        "a number is refused: {wrong}"
    );

    let tidy = json!([
        { "verb": "remove_mode", "args": { "mode": made[1][0] } },
        { "verb": "remove_variable", "args": { "variable": made[0][0] } },
    ]);
    verbs::run_script(&mut doc, &mut ids, Lent::default(), &tidy).unwrap_or_else(|e| panic!("{e}"));
    assert!(doc.variables().get(var).is_none());
    assert!(doc.variables().mode(dark).is_none());
    assert!(
        doc.get(f).unwrap().modes().is_empty(),
        "the frame inherits again"
    );
    assert!(doc.get(page).is_some());
}

/// `set_variable_value` without a `mode` sets the only mode of the variable's
/// collection, and is refused once the collection has two.
///
/// flip: `set_variable_value` taking the first mode whatever the count → fails at
/// "two modes need a name".
#[test]
fn a_variable_value_takes_the_only_mode_unnamed() {
    let (mut doc, mut ids, _) = blank();
    let blue = serde_json::to_value(ondin_core::Cell::Color(Color::from_rgb8(0, 0, 255))).unwrap();
    let made = script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "add_variable", "args": { "kind": "color", "name": "brand" }, "as": "v" },
            { "verb": "set_variable_value", "args": { "variable": "@v", "value": blue } },
        ]),
    );
    let var: ondin_core::VarId = serde_json::from_value(json!(made[0][0])).unwrap();
    let table = doc.variables();
    let modes = &table.collection_of(var).unwrap().modes;
    assert_eq!(modes.len(), 1, "the fixture has one mode");
    assert_eq!(
        table.get(var).unwrap().cells.get(&modes[0].id),
        Some(&ondin_core::Cell::Color(Color::from_rgb8(0, 0, 255)))
    );
    script(
        &mut doc,
        &mut ids,
        json!([{ "verb": "add_mode", "args": {} }]),
    );
    let two = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent::default(),
        &json!([{ "verb": "set_variable_value", "args": { "variable": made[0][0], "value": blue } }]),
    )
    .expect_err("two modes need a name")
    .why;
    assert!(two.contains("more than one mode"), "{two}");
}

/// `script` run on `doc`, panicking on a refusal; answers what each step made.
fn script(doc: &mut Document, ids: &mut IdSource, steps: Value) -> Vec<Vec<String>> {
    verbs::run_script(doc, ids, Lent::default(), &steps).unwrap_or_else(|e| panic!("{e}"))
}

/// Why the one step `step` is refused on `doc`.
fn refused_step(doc: &mut Document, ids: &mut IdSource, step: Value) -> String {
    verbs::run_script(doc, ids, Lent::default(), &json!([step]))
        .expect_err("refused")
        .why
}

/// Collections are added, renamed, copied with their variables and deleted; a
/// taken name and the last collection are refused.
///
/// flip: `duplicate_collection` answering no id → fails at "answers the copy".
/// `delete_collection` without its emptiness check is no flip: the core answers
/// no ops for the last collection, which the verb then says.
#[test]
fn collections_are_added_renamed_copied_and_deleted() {
    let (mut doc, mut ids, _) = blank();
    let made = script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "add_collection", "args": { "name": "Theme" }, "as": "c" },
            { "verb": "rename_collection", "args": { "collection": "@c", "name": "Brand" } },
            { "verb": "add_variable", "args": { "kind": "number", "name": "gap", "collection": "@c" } },
            { "verb": "duplicate_collection", "args": { "collection": "@c" } },
        ]),
    );
    let names = |doc: &Document| -> Vec<String> {
        doc.variables()
            .collections()
            .iter()
            .map(|c| c.name.clone())
            .collect()
    };
    let first = names(&doc)[0].clone();
    assert_eq!(names(&doc)[1..], ["Brand", "Brand copy"]);
    let copy = made[3].first().expect("answers the copy").clone();
    let copy: ondin_core::variable::CollectionId = serde_json::from_value(json!(copy)).unwrap();
    assert_eq!(
        doc.variables().of_collection(copy).count(),
        1,
        "the copy holds a copy of the variable"
    );
    let taken = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "add_collection", "args": { "name": first } }),
    );
    assert!(taken.contains("already exists"), "{taken}");
    let all: Vec<String> = doc
        .variables()
        .collections()
        .iter()
        .map(|c| c.id.0.to_wire())
        .collect();
    for c in &all[1..] {
        script(
            &mut doc,
            &mut ids,
            json!([{ "verb": "delete_collection", "args": { "collection": c } }]),
        );
    }
    assert_eq!(names(&doc), [first]);
    let last = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "delete_collection", "args": { "collection": all[0] } }),
    );
    assert!(last.contains("last collection"), "{last}");
}

/// A mode moves in its collection; picked by width, the modes take widths, a
/// mode's breakpoint is set, and a move is refused, since widths order them.
///
/// flip: `move_mode` skipping its by-width refusal → fails at the by-width move,
/// which the core answers with no ops rather than refusing; `set_breakpoint`
/// skipping its by-width check → fails at "not picked by width yet" the same way.
#[test]
fn modes_move_and_take_breakpoints_by_width() {
    let (mut doc, mut ids, _) = blank();
    let made = script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "add_collection", "args": { "name": "Screens" }, "as": "c" },
            { "verb": "add_mode", "args": { "name": "Tablet", "collection": "@c" }, "as": "t" },
            { "verb": "add_mode", "args": { "name": "Desktop", "collection": "@c" }, "as": "d" },
            { "verb": "move_mode", "args": { "mode": "@d", "index": 1 } },
        ]),
    );
    let c: ondin_core::variable::CollectionId = serde_json::from_value(json!(made[0][0])).unwrap();
    let modes = |doc: &Document| -> Vec<String> {
        doc.variables()
            .collection(c)
            .unwrap()
            .modes
            .iter()
            .map(|m| m.name.clone())
            .collect()
    };
    assert_eq!(modes(&doc)[1..], ["Desktop", "Tablet"]);
    let (tablet, desktop) = (made[1][0].clone(), made[2][0].clone());
    let index = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "move_mode", "args": { "mode": tablet, "index": 3 } }),
    );
    assert!(index.contains("3 modes"), "{index}");
    let early = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "set_breakpoint", "args": { "mode": desktop, "width": 900 } }),
    );
    assert!(
        early.contains("set_by_width"),
        "not picked by width yet: {early}"
    );
    script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "set_by_width", "args": { "collection": made[0][0], "on": true } },
            { "verb": "set_breakpoint", "args": { "mode": desktop, "width": 900 } },
        ]),
    );
    let col = doc.variables().collection(c).unwrap();
    assert!(col.by_width());
    let d: ondin_core::ModeId = serde_json::from_value(json!(desktop)).unwrap();
    assert_eq!(col.breakpoints.as_ref().unwrap()[&d], 900.0);
    let first = col.modes[0].id.0.to_wire();
    let moved = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "move_mode", "args": { "mode": tablet, "index": 0 } }),
    );
    assert!(moved.contains("by width"), "refused by width: {moved}");
    let fixed = refused_step(
        &mut doc,
        &mut ids,
        json!({ "verb": "set_breakpoint", "args": { "mode": first, "width": 10 } }),
    );
    assert!(fixed.contains("width"), "{fixed}");
}

/// A main picks a collection's mode by its own width and stops; a plain frame
/// and a rectangle are refused, as the mode rows offer *By width* on a main only.
///
/// flip: `set_width_query` passing `true` to `query_ops` always → fails at
/// "stops"; without its main check → fails at "a plain frame is refused".
#[test]
fn set_width_query_lets_a_main_pick_by_its_width() {
    let (mut doc, mut ids, page) = blank();
    let (f, _) = main_component(&mut doc, &mut ids, page, "Card", 300.0);
    let plain = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(300.0, 200.0) }),
    )[0];
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    let args = json!({ "node": f.to_wire(), "on": true });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_width_query", args),
        "collection",
        "no modes are nothing to pick between"
    );
    run(&mut doc, &mut ids, "add_mode", json!({ "name": "Narrow" }));
    run(&mut doc, &mut ids, "add_mode", json!({ "name": "Wide" }));
    assert_eq!(doc.variables().first_collection().modes.len(), 2);
    run(
        &mut doc,
        &mut ids,
        "set_width_query",
        json!({ "node": f.to_wire(), "on": true }),
    );
    assert_eq!(doc.get(f).unwrap().queries().len(), 1);
    run(
        &mut doc,
        &mut ids,
        "set_width_query",
        json!({ "node": f.to_wire(), "on": false }),
    );
    assert!(doc.get(f).unwrap().queries().is_empty(), "stops");
    let args = json!({ "node": plain.to_wire(), "on": true });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_width_query", args),
        "node",
        "a plain frame is refused"
    );
    let args = json!({ "node": r.to_wire(), "on": true });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_width_query", args),
        "node"
    );
}

/// A variable's scope narrows to fields of its kind, detaching a user it leaves
/// and saying so, and widens again; a field of another kind is refused.
///
/// flip: `set_scope` ignoring `scope` (always every field) → fails at the scope.
#[test]
fn set_scope_narrows_a_variable_and_detaches_its_users() {
    let (mut doc, mut ids, _) = blank();
    let eight = serde_json::to_value(ondin_core::Cell::Number(8.0)).unwrap();
    let made = script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "add_variable", "args": { "kind": "number", "name": "r", "value": eight }, "as": "n" },
            { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "x" },
            { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": "radius", "variable": "@n" } },
        ]),
    );
    let var: ondin_core::VarId = serde_json::from_value(json!(made[0][0])).unwrap();
    let set = answer(
        &doc,
        &mut ids,
        "set_scope",
        json!({ "variable": made[0][0], "scope": ["spacing"] }),
    )
    .unwrap();
    assert_eq!(set.said.as_deref(), Some("1 layer was detached"));
    run(
        &mut doc,
        &mut ids,
        "set_scope",
        json!({ "variable": made[0][0], "scope": ["spacing"] }),
    );
    let scope = |doc: &Document| doc.variables().get(var).unwrap().scope;
    assert_eq!(
        scope(&doc),
        Some(ondin_core::variable::Scope::of([
            ondin_core::variable::Field::Spacing
        ]))
    );
    let x = NodeId::from_wire(&made[1][0]).unwrap();
    assert!(
        doc.get(x)
            .unwrap()
            .corner_vars()
            .iter()
            .all(Option::is_none)
    );
    let args = json!({ "variable": made[0][0], "scope": ["fill"] });
    assert_eq!(refused_param(&doc, &mut ids, "set_scope", args), "scope");
    run(
        &mut doc,
        &mut ids,
        "set_scope",
        json!({ "variable": made[0][0] }),
    );
    assert_eq!(scope(&doc), None);
}

/// A document holding a kit's number variable `kit/radius` in the kit's mode
/// `Light`; answers the variable and the mode.
fn kit_variable(doc: &mut Document, ids: &mut IdSource) -> (ondin_core::VarId, ondin_core::ModeId) {
    use ondin_core::variable::{KitMode, KitModes, VarKind, VarOrigin, Variable};
    const KIT: &str = "5eed0000000000000000000000000011";
    let (light, var) = (ids.mint_mode(), ids.mint_var());
    doc.apply(&Transaction(vec![
        Operation::SetKitModes {
            kit: KIT.into(),
            modes: Some(KitModes {
                name: None,
                modes: vec![KitMode {
                    id: light,
                    name: "Light".into(),
                    from: light,
                    collection: String::new(),
                }],
                map: Default::default(),
            }),
        },
        Operation::AddVariable {
            var: Variable {
                id: var,
                name: "kit/radius".into(),
                kind: VarKind::Number,
                cells: [(light, ondin_core::Cell::Number(3.0))].into(),
                scope: None,
                origin: Some(VarOrigin {
                    kit: KIT.into(),
                    var,
                    version: 1,
                }),
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .unwrap();
    (var, light)
}

/// A kit's variable and mode are read-only to the verbs: renaming, rewriting or
/// removing the variable and renaming the mode are refused under the parameter
/// that names them, and a variable of the document's own may still alias it.
///
/// flip: `own_variable` not refusing a kit's variable → fails at "rename is
/// refused".
#[test]
fn a_kits_variable_is_read_only_to_the_verbs() {
    let (mut doc, mut ids, _) = blank();
    let (kit, light) = kit_variable(&mut doc, &mut ids);
    let res = Resolved::rebuild(&doc);
    let mut call = |name: &str, args: Value| {
        verbs::call(
            &mut Ctx {
                doc: &doc,
                res: &res,
                ids: &mut ids,
                images: None,
                contrast: None,
            },
            name,
            &args,
        )
    };
    let read_only = |r: Result<Done, VerbError>, param: &str| match r {
        Err(VerbError::Invalid { param: p, why, .. }) => p == param && why.contains("read-only"),
        _ => false,
    };
    let number = serde_json::to_value(ondin_core::Cell::Number(42.0)).unwrap();
    assert!(
        read_only(
            call(
                "rename_variable",
                json!({ "variable": kit.0.to_wire(), "name": "hacked" })
            ),
            "variable"
        ),
        "rename is refused"
    );
    assert!(
        read_only(
            call(
                "set_variable_value",
                json!({ "variable": kit.0.to_wire(), "mode": light.0.to_wire(), "value": number })
            ),
            "variable"
        ),
        "a new value is refused"
    );
    assert!(
        read_only(
            call("remove_variable", json!({ "variable": kit.0.to_wire() })),
            "variable"
        ),
        "removal is refused"
    );
    assert!(
        read_only(
            call(
                "rename_mode",
                json!({ "mode": light.0.to_wire(), "name": "Day" })
            ),
            "mode"
        ),
        "the kit's mode keeps its name"
    );

    let script = json!([
        { "verb": "add_variable", "args": { "kind": "number", "name": "radius" }, "as": "v" },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let first = doc.variables().default_mode().expect("a mode");
    let alias = serde_json::to_value(ondin_core::Cell::Alias(kit)).unwrap();
    let step = json!([{ "verb": "set_variable_value", "args": { "variable": made[0][0], "mode": first.0.to_wire(), "value": alias } }]);
    verbs::run_script(&mut doc, &mut ids, Lent::default(), &step)
        .unwrap_or_else(|e| panic!("an alias of the kit's variable: {e}"));
    let v = doc.variables().get(kit).unwrap();
    assert_eq!(
        (v.name.as_str(), v.cells.get(&light)),
        ("kit/radius", Some(&ondin_core::Cell::Number(3.0))),
        "the kit's variable is as it was"
    );
}

/// A mode or collection id that names nothing is refused under its own
/// parameter, never answered as done.
///
/// flip: `rename_mode` reading its mode without `own_mode` → fails at "an
/// unknown mode".
#[test]
fn an_unknown_mode_or_collection_is_refused_by_name() {
    let (mut doc, mut ids, page) = blank();
    let nothing = ids.mint().to_wire();
    let frame_id = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(10.0, 10.0) }),
    )[0]
    .to_wire();
    let res = Resolved::rebuild(&doc);
    let mut refused_at = |name: &str, args: Value| match verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        name,
        &args,
    ) {
        Err(VerbError::Invalid { param, .. }) => param,
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(
        refused_at("rename_mode", json!({ "mode": nothing, "name": "X" })),
        "mode",
        "an unknown mode"
    );
    assert_eq!(
        refused_at(
            "set_frame_mode",
            json!({ "node": frame_id, "collection": nothing })
        ),
        "collection"
    );
    assert_eq!(
        refused_at(
            "add_variable",
            json!({ "kind": "color", "collection": nothing })
        ),
        "collection"
    );
    assert_eq!(
        refused_at("add_mode", json!({ "collection": nothing })),
        "collection"
    );
    assert!(doc.get(page).is_some());
}

/// `bind_variable` binds a fill's colour and the corners to variables, the layer
/// drawing their values, then detaches the corners to the radius they drew; a
/// corner past the fourth is refused.
///
/// flip: `bind_field_ops`' `Radius` arm detaching instead of binding → fails at
/// "the corners are bound".
#[test]
fn bind_variable_binds_and_detaches_fields() {
    let (mut doc, mut ids, _) = blank();
    let red = serde_json::to_value(ondin_core::Cell::Color(Color::from_rgb8(200, 0, 0))).unwrap();
    let eight = serde_json::to_value(ondin_core::Cell::Number(8.0)).unwrap();
    let fill = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(1, 1, 1)).into(),
        visible: true,
    }])
    .unwrap();
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "color", "name": "red", "value": red }, "as": "c" },
        { "verb": "add_variable", "args": { "kind": "number", "name": "r", "value": eight }, "as": "n" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "x" },
        { "verb": "set_fills", "args": { "nodes": ["@x"], "fills": fill } },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": { "fill": 0 }, "variable": "@c" } },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": "radius", "variable": "@n" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let x = NodeId::from_wire(&made[2][0]).unwrap();
    let node = doc.get(x).unwrap();
    assert!(
        node.paint().fills[0].brush.var().is_some(),
        "the fill is bound"
    );
    assert!(
        node.corner_vars().iter().all(Option::is_some),
        "the corners are bound"
    );

    let detach =
        json!([{ "verb": "bind_variable", "args": { "nodes": [x.to_wire()], "field": "radius" } }]);
    verbs::run_script(&mut doc, &mut ids, Lent::default(), &detach)
        .unwrap_or_else(|e| panic!("{e}"));
    let node = doc.get(x).unwrap();
    assert!(node.corner_vars().iter().all(Option::is_none));
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        unreachable!()
    };
    assert_eq!(corner_radii.top_left, 8.0, "detached to what it drew");

    let past = json!([{ "verb": "bind_variable", "args": { "nodes": [x.to_wire()], "field": { "corner": 4 } } }]);
    let err =
        verbs::run_script(&mut doc, &mut ids, Lent::default(), &past).expect_err("no fifth corner");
    assert!(err.why.contains("no such place"), "{err}");
}

/// A corner is named as well as numbered: `bottom_right` is corner 2.
///
/// flip: `corner_index` listing the corners clockwise from bottom-left → fails at
/// "bottom-right is bound".
#[test]
fn bind_variable_names_a_corner() {
    let (mut doc, mut ids, _) = blank();
    let eight = serde_json::to_value(ondin_core::Cell::Number(8.0)).unwrap();
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "number", "name": "r", "value": eight }, "as": "n" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "x" },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": { "corner": "bottom_right" }, "variable": "@n" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let x = NodeId::from_wire(&made[1][0]).unwrap();
    let node = doc.get(x).unwrap();
    let bound: Vec<bool> = node.corner_vars().iter().map(Option::is_some).collect();
    assert_eq!(bound, [false, false, true, false], "bottom-right is bound");
}

/// `bind_variable` binds a polygon's one radius whichever corner it is given, so
/// the shape draws the variable; a paint index no layer has and a layer that does
/// not exist are refused, not answered as done.
///
/// flip: `bind_field_ops` binding a polygon's `Corner(k)` slot alone → fails at
/// "the polygon draws the variable".
#[test]
fn bind_variable_binds_a_polygons_radius_and_refuses_what_is_not_there() {
    let (mut doc, mut ids, _) = blank();
    let twelve = serde_json::to_value(ondin_core::Cell::Number(12.0)).unwrap();
    let pentagon = serde_json::to_value(NodeKind::Polygon {
        size: Size::new(40.0, 40.0),
        sides: 5,
        corner_radius: 4.0,
        corner_smoothing: 0.0,
    })
    .unwrap();
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "number", "name": "r", "value": twelve }, "as": "n" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": pentagon }, "as": "p" },
        { "verb": "bind_variable", "args": { "nodes": ["@p"], "field": { "corner": 2 }, "variable": "@n" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let p = NodeId::from_wire(&made[1][0]).unwrap();
    let res = Resolved::rebuild(&doc);
    let drawn = match res.used_kind(&doc, p) {
        Some(NodeKind::Polygon { corner_radius, .. }) => *corner_radius,
        other => panic!("{other:?}"),
    };
    assert_eq!(drawn, 12.0, "the polygon draws the variable");

    let refused = |doc: &mut Document, ids: &mut IdSource, args: Value| {
        verbs::run_script(
            doc,
            ids,
            Lent::default(),
            &json!([{ "verb": "bind_variable", "args": args }]),
        )
        .expect_err("refused")
        .why
    };
    let past = refused(
        &mut doc,
        &mut ids,
        json!({ "nodes": [p.to_wire()], "field": { "fill": 7 }, "variable": made[0][0] }),
    );
    assert!(past.contains("fill there"), "{past}");
    let nothing = ids.mint().to_wire();
    let gone = refused(
        &mut doc,
        &mut ids,
        json!({ "nodes": [nothing], "field": "visible" }),
    );
    assert!(!gone.is_empty());
}

/// The style verbs, as a script: a fill style added from a value and applied, its
/// value changed and its users following, renamed, detached from a layer that
/// keeps its look, one made from a layer, and removed. A taken name is refused.
///
/// flip: `style_value` keying a changed list from scratch → fails at "the fill
/// keeps its item id".
#[test]
fn styles_are_added_changed_applied_and_removed() {
    let (mut doc, mut ids, page) = blank();
    let fills = |r: u8| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(Color::from_rgb8(r, 0, 0)).into(),
            visible: true,
        }])
        .unwrap()
    };
    let colour =
        |doc: &Document, id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
            Some(Brush::Solid(c)) => c.to_rgba8().r,
            other => panic!("{other:?}"),
        };
    let script = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "a" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "b" },
        { "verb": "add_style", "args": { "value": { "fills": fills(100) }, "name": "Brand", "nodes": ["@a", "@b"] }, "as": "s" },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let (a, b) = (
        NodeId::from_wire(&made[0][0]).unwrap(),
        NodeId::from_wire(&made[1][0]).unwrap(),
    );
    let s = made[2][0].clone();
    assert_eq!((colour(&doc, a), colour(&doc, b)), (100, 100), "applied");
    let style: ondin_core::style::StyleId = serde_json::from_value(json!(s)).unwrap();
    let item = match &doc.styles().get(style).unwrap().value {
        ondin_core::style::StyleValue::Fills(f) => f[0].id,
        _ => unreachable!(),
    };

    let more = json!([
        { "verb": "set_style_value", "args": { "style": s, "value": { "fills": fills(200) } } },
        { "verb": "rename_style", "args": { "style": s, "name": "Accent" } },
        { "verb": "apply_style", "args": { "nodes": [b.to_wire()], "slot": "fills" } },
        { "verb": "add_style", "args": { "from": a.to_wire(), "slot": "fills", "name": "Copy" }, "as": "c" },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &more)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (colour(&doc, a), colour(&doc, b)),
        (200, 200),
        "users follow"
    );
    let st = doc.styles().get(style).unwrap();
    assert_eq!(st.name, "Accent");
    match &st.value {
        ondin_core::style::StyleValue::Fills(f) => {
            assert_eq!(f[0].id, item, "the fill keeps its item id")
        }
        _ => unreachable!(),
    }
    assert!(
        doc.get(b)
            .unwrap()
            .styles()
            .get(ondin_core::style::StyleSlot::Fills)
            .is_none(),
        "b detached"
    );
    assert!(!made[3].is_empty());

    let taken = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent::default(),
        &json!([{ "verb": "add_style", "args": { "value": { "fills": fills(1) }, "name": "Accent" } }]),
    )
    .expect_err("a taken name");
    assert!(taken.why.contains("already exists"), "{taken}");

    verbs::run_script(
        &mut doc,
        &mut ids,
        Lent::default(),
        &json!([{ "verb": "remove_style", "args": { "style": s } }]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(doc.styles().get(style).is_none());
    assert_eq!(colour(&doc, a), 200, "a keeps the look it drew");
    assert!(doc.get(page).is_some());
}

/// A fill style given a group styles the group's shapes, as the Fill card's
/// picker does, by `add_style` and by `apply_style`; detaching the group
/// detaches them.
///
/// flip: `style::targets` answering the ids as given → fails at the script's
/// `add_style` step, `op_set_fills` refusing the group.
#[test]
fn a_paint_style_given_a_group_styles_its_shapes() {
    let (mut doc, mut ids, _) = blank();
    let fills = |r: u8| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(Color::from_rgb8(r, 0, 0)).into(),
            visible: true,
        }])
        .unwrap()
    };
    let script = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "a" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "b" },
        { "verb": "group", "args": { "nodes": ["@a", "@b"] }, "as": "g" },
        { "verb": "add_style", "args": { "value": { "fills": fills(100) }, "nodes": ["@g"] }, "as": "s" },
        { "verb": "add_style", "args": { "value": { "fills": fills(50) } }, "as": "t" },
        { "verb": "apply_style", "args": { "nodes": ["@g"], "style": "@t" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let shapes = [made[0][0].clone(), made[1][0].clone()].map(|w| NodeId::from_wire(&w).unwrap());
    let t: ondin_core::style::StyleId = serde_json::from_value(json!(made[4][0])).unwrap();
    for s in shapes {
        let node = doc.get(s).unwrap();
        assert_eq!(
            node.styles().get(ondin_core::style::StyleSlot::Fills),
            Some(t),
            "each shape applies the last style"
        );
        assert_eq!(
            node.paint().fills[0].brush.literal(),
            Some(&Brush::Solid(Color::from_rgb8(50, 0, 0)))
        );
    }

    let detach =
        json!([{ "verb": "apply_style", "args": { "nodes": [made[2][0]], "slot": "fills" } }]);
    verbs::run_script(&mut doc, &mut ids, Lent::default(), &detach)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(shapes.iter().all(|s| {
        doc.get(*s)
            .unwrap()
            .styles()
            .get(ondin_core::style::StyleSlot::Fills)
            .is_none()
    }));
}

/// A fill style given a layer that does not exist is refused by `apply_style`
/// and `add_style`, as an effects style is, rather than answered as done.
/// flip: `style_nodes` not checking each layer exists → fails at "a missing
/// layer is refused".
#[test]
fn a_paint_style_given_a_missing_layer_is_refused() {
    let (mut doc, mut ids, _) = blank();
    let fills = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(9, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    let script = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "a" },
        { "verb": "add_style", "args": { "value": { "fills": fills.clone() } }, "as": "s" },
        { "verb": "delete_node", "args": { "nodes": ["@a"] } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let (gone, style) = (made[0][0].clone(), made[1][0].clone());
    assert!(
        doc.get(NodeId::from_wire(&gone).unwrap()).is_none(),
        "the fixture: a is gone"
    );
    let styles = doc.styles().iter().count();
    let steps = [
        json!({ "verb": "apply_style", "args": { "nodes": [gone.clone()], "style": style } }),
        json!({ "verb": "add_style", "args": { "value": { "fills": fills }, "nodes": [gone] } }),
    ];
    for step in steps {
        let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &json!([step]))
            .expect_err(&format!("{step}: a missing layer is refused"));
        assert!(err.why.contains("no such node"), "{err}");
    }
    assert_eq!(
        doc.styles().iter().count(),
        styles,
        "the refused add_style adds no style"
    );
}

/// `define_property` binds a main's text and rectangle to a text and a boolean
/// property, a named one and one named for its layer, and `set_instance_property`
/// sets each on an instance by name; a value of the wrong kind is refused.
///
/// flip: `rebind` not taking a layer off the kind's other properties → fails at
/// "the badge is hidden": `variant::settle` prunes the double binding, keeping the
/// first, so "bound once" holds either way and the new property loses its layer.
#[test]
fn properties_are_defined_and_set_on_instances() {
    let (mut doc, mut ids, page) = blank();
    let (card, rect_in) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    let title = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": card.to_wire(), "text": "Title" }),
    )[0];
    let res = Resolved::rebuild(&doc);
    let call = |doc: &Document, ids: &mut IdSource, name: &str, args: Value| {
        verbs::call(
            &mut Ctx {
                doc,
                res: &res,
                ids,
                images: None,
                contrast: None,
            },
            name,
            &args,
        )
    };
    let defined = call(
        &doc,
        &mut ids,
        "define_property",
        json!({ "node": title.to_wire(), "kind": "text", "name": "Heading" }),
    )
    .unwrap();
    assert!(defined.made.is_some(), "a new property answers its id");
    let mut tx = defined.tx;
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).unwrap();
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": rect_in.to_wire(), "kind": "boolean" }),
    );
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": rect_in.to_wire(), "kind": "boolean", "name": "Badge" }),
    );
    let props = doc.get(card).unwrap().props().to_vec();
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"Heading") && names.contains(&"Badge"),
        "{names:?}"
    );
    let binding = props.iter().filter(|p| p.bound.contains(&rect_in)).count();
    assert_eq!(binding, 1, "bound once");

    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": page.to_wire() }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_instance_property",
        json!({ "node": inst.to_wire(), "property": "Heading", "value": "Hello" }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_instance_property",
        json!({ "node": inst.to_wire(), "property": "Badge", "value": false }),
    );
    let kids = doc.get(inst).unwrap().children().to_vec();
    let text = kids.iter().find_map(|k| match doc.get(*k).unwrap().kind() {
        NodeKind::Text { content, .. } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("Hello"));
    assert!(
        kids.iter().any(|k| !doc.get(*k).unwrap().visible()),
        "the badge is hidden"
    );

    let res = Resolved::rebuild(&doc);
    let wrong = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "set_instance_property",
        &json!({ "node": inst.to_wire(), "property": "Badge", "value": "yes" }),
    );
    assert!(
        matches!(wrong, Err(VerbError::Invalid { param: "value", .. })),
        "{wrong:?}"
    );
}

/// A swap property: `define_property` makes one on a nested instance and binds a
/// second to it by `property`, refusing a swap on a plain layer and a name that
/// is empty, taken or a set's variant property; `set_instance_property` swaps the
/// slot, refusing an id that is no main and a main of another kind.
///
/// flip: `set_instance_property` not checking `swap::can_swap_to` → fails at "a
/// group main is refused".
#[test]
fn swap_properties_are_defined_bound_and_set() {
    let (mut doc, mut ids, page) = blank();
    let (icon_a, _) = main_component(&mut doc, &mut ids, page, "Icon A", 20.0);
    let (icon_b, _) = main_component(&mut doc, &mut ids, page, "Icon B", 20.0);
    let button = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(120.0, 40.0), "name": "Button" }),
    )[0];
    let slots: Vec<NodeId> = (0..2)
        .map(|_| {
            run(
                &mut doc,
                &mut ids,
                "create_instance",
                json!({ "main": icon_a.to_wire(), "parent": button.to_wire() }),
            )[0]
        })
        .collect();
    let label = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": button.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [button.to_wire()] }),
    );

    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "define_property",
            json!({ "node": label.to_wire(), "kind": "swap" })
        ),
        "kind",
        "a swap needs a nested instance"
    );
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": slots[0].to_wire(), "kind": "swap", "name": "Icon" }),
    );
    for name in ["", "Icon"] {
        assert_eq!(
            refused_param(
                &doc,
                &mut ids,
                "define_property",
                json!({ "node": label.to_wire(), "kind": "boolean", "name": name })
            ),
            "name",
            "{name:?} is refused"
        );
    }
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": slots[1].to_wire(), "kind": "swap", "property": "Icon" }),
    );
    let props = doc.get(button).unwrap().props().to_vec();
    assert_eq!(props.len(), 1, "bound to the one property");
    assert_eq!(props[0].bound.len(), 2);

    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": button.to_wire(), "parent": page.to_wire() }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_instance_property",
        json!({ "node": inst.to_wire(), "property": "Icon", "value": icon_b.to_wire() }),
    );
    let copies: Vec<NodeId> = doc.get(inst).unwrap().children()[..2].to_vec();
    assert!(
        copies
            .iter()
            .all(|c| ondin_core::component::main_of(&doc, *c) == Some(icon_b)),
        "both slots show Icon B"
    );

    let loose = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(5.0, 5.0) }),
    )[0];
    let grouped = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(5.0, 5.0) }),
    )[0];
    let group = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [grouped.to_wire()] }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [group.to_wire()] }),
    );
    assert!(doc.get(group).unwrap().component(), "fixture: a group main");
    let set = |value: Value| json!({ "node": inst.to_wire(), "property": "Icon", "value": value });
    for (value, why) in [
        (json!(loose.to_wire()), "a layer that is no main"),
        (json!("not an id"), "a word"),
        (json!(group.to_wire()), "a group main is refused"),
    ] {
        assert_eq!(
            refused_param(&doc, &mut ids, "set_instance_property", set(value)),
            "value",
            "{why}"
        );
    }

    let (small, small_rect) = main_component(&mut doc, &mut ids, page, "Size=Small", 60.0);
    let (large, _) = main_component(&mut doc, &mut ids, page, "Size=Large", 120.0);
    run(
        &mut doc,
        &mut ids,
        "combine_as_variants",
        json!({ "nodes": [small.to_wire(), large.to_wire()] }),
    );
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "define_property",
            json!({ "node": small_rect.to_wire(), "kind": "boolean", "name": "Size" })
        ),
        "name",
        "a set's variant property reserves its name"
    );
}

/// `reset_overrides` puts an instance's overridden fill back as its main has it,
/// and refuses layers in no instance.
///
/// flip: `reset_overrides` keeping the layers that sit in no instance and
/// resetting those → fails at "a loose layer is refused".
#[test]
fn reset_overrides_puts_an_instance_back() {
    let (mut doc, mut ids, page) = blank();
    let (card, _) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": page.to_wire() }),
    )[0];
    let copy = doc.get(inst).unwrap().children()[0];
    let red = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [copy.to_wire()], "fills": red }),
    );
    assert_eq!(
        doc.get(copy).unwrap().paint().fills.len(),
        1,
        "fixture: overridden"
    );
    run(
        &mut doc,
        &mut ids,
        "reset_overrides",
        json!({ "nodes": [inst.to_wire()] }),
    );
    assert!(
        doc.get(copy).unwrap().paint().fills.is_empty(),
        "the main has no fill"
    );

    let res = Resolved::rebuild(&doc);
    let loose = verbs::call(
        &mut Ctx {
            doc: &doc,
            res: &res,
            ids: &mut ids,
            images: None,
            contrast: None,
        },
        "reset_overrides",
        &json!({ "nodes": [card.to_wire()] }),
    );
    assert!(
        matches!(loose, Err(VerbError::Nothing(_))),
        "a loose layer is refused: {loose:?}"
    );
}

/// `reset_overrides`' scope: `fields` puts an overridden fill back and leaves a
/// removed child gone, and `children` then restores it.
///
/// flip: `reset_overrides` reading every scope as `all` → fails at "fields
/// restore no child".
#[test]
fn reset_overrides_resets_only_its_scope() {
    let (mut doc, mut ids, page) = blank();
    let (card, _) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": card.to_wire(), "kind": rect(5.0, 5.0) }),
    );
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": page.to_wire() }),
    )[0];
    let copies = doc.get(inst).unwrap().children().to_vec();
    assert_eq!(copies.len(), 2, "fixture: two copies");
    let red = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [copies[0].to_wire()], "fills": red }),
    );
    run(
        &mut doc,
        &mut ids,
        "delete_node",
        json!({ "nodes": [copies[1].to_wire()] }),
    );
    let reset = |doc: &mut Document, ids: &mut IdSource, scope: &str| {
        run(
            doc,
            ids,
            "reset_overrides",
            json!({ "nodes": [inst.to_wire()], "scope": scope }),
        );
    };
    reset(&mut doc, &mut ids, "fields");
    assert!(doc.get(copies[0]).unwrap().paint().fills.is_empty());
    assert_eq!(
        doc.get(inst).unwrap().children().len(),
        1,
        "fields restore no child"
    );
    reset(&mut doc, &mut ids, "children");
    assert_eq!(doc.get(inst).unwrap().children().len(), 2);
}

/// `combine_as_variants` makes a set of two mains, and `switch_variant` shows the
/// other variant in an instance by its property's value.
#[test]
fn variants_are_combined_and_switched() {
    let (mut doc, mut ids, page) = blank();
    let (a, _) = main_component(&mut doc, &mut ids, page, "Size=Small", 60.0);
    let (b, _) = main_component(&mut doc, &mut ids, page, "Size=Large", 120.0);
    let set = run(
        &mut doc,
        &mut ids,
        "combine_as_variants",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    )[0];
    let vs = doc.get(set).unwrap().set().cloned().expect("a set");
    let prop = vs.props[0].clone();
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": set.to_wire(), "parent": page.to_wire() }),
    )[0];
    let shown = ondin_core::component::main_of(&doc, inst).unwrap();
    let want = if shown == a { b } else { a };
    let value = doc.get(want).unwrap().variant()[0].clone();
    run(
        &mut doc,
        &mut ids,
        "switch_variant",
        json!({ "node": inst.to_wire(), "property": prop.name, "value": value }),
    );
    assert_eq!(ondin_core::component::main_of(&doc, inst), Some(want));
}

/// A set combined from `Size=Small` and `Size=Large`: the set, then the two variants.
fn size_set(doc: &mut Document, ids: &mut IdSource, page: NodeId) -> (NodeId, NodeId, NodeId) {
    let (small, _) = main_component(doc, ids, page, "Size=Small", 60.0);
    let (large, _) = main_component(doc, ids, page, "Size=Large", 120.0);
    let set = run(
        doc,
        ids,
        "combine_as_variants",
        json!({ "nodes": [small.to_wire(), large.to_wire()] }),
    )[0];
    assert_eq!(
        variant_props(doc, set),
        [("Size".into(), vec!["Small".into(), "Large".into()])]
    );
    (set, small, large)
}

/// `set`'s variant properties, each its name and values.
fn variant_props(doc: &Document, set: NodeId) -> Vec<(String, Vec<String>)> {
    doc.get(set)
        .and_then(|s| s.set())
        .map(|s| {
            s.props
                .iter()
                .map(|p| (p.name.clone(), p.values.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// A set's variant properties are added, renamed and deleted, a variant naming its
/// set as the set does, and a taken name or the last property refused.
///
/// flip: `add_variant_property` passing its name as the value → fails at the set's
/// new property and its one value.
#[test]
fn variant_properties_are_added_renamed_and_deleted() {
    let (mut doc, mut ids, page) = blank();
    let (set, small, large) = size_set(&mut doc, &mut ids, page);
    run(
        &mut doc,
        &mut ids,
        "add_variant_property",
        json!({ "node": small.to_wire(), "name": "State" }),
    );
    assert_eq!(
        variant_props(&doc, set)[1],
        ("State".into(), vec!["Default".into()])
    );
    for v in [small, large] {
        assert_eq!(
            doc.get(v).unwrap().variant()[1],
            "Default",
            "every variant takes the new property's value"
        );
    }
    run(
        &mut doc,
        &mut ids,
        "rename_variant_property",
        json!({ "node": set.to_wire(), "property": "Size", "name": "Scale" }),
    );
    assert_eq!(variant_props(&doc, set)[0].0, "Scale");
    for (name, param) in [("State", "name"), ("", "name")] {
        let args = json!({ "node": set.to_wire(), "property": "Scale", "name": name });
        assert_eq!(
            refused_param(&doc, &mut ids, "rename_variant_property", args),
            param
        );
    }
    let args = json!({ "node": set.to_wire(), "property": "Size", "name": "X" });
    assert_eq!(
        refused_param(&doc, &mut ids, "rename_variant_property", args),
        "property",
        "a property is named as it is now"
    );
    run(
        &mut doc,
        &mut ids,
        "delete_variant_property",
        json!({ "node": set.to_wire(), "property": "State" }),
    );
    assert_eq!(variant_props(&doc, set).len(), 1);
    assert_eq!(
        doc.get(small).unwrap().variant(),
        ["Small"],
        "variants drop it too"
    );
    let last = answer(
        &doc,
        &mut ids,
        "delete_variant_property",
        json!({ "node": set.to_wire(), "property": "Scale" }),
    );
    assert!(
        matches!(last, Err(VerbError::Nothing(_))),
        "the last stays: {last:?}"
    );
    let plain = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(10.0, 10.0) }),
    )[0];
    let args = json!({ "node": plain.to_wire(), "name": "X" });
    assert_eq!(
        refused_param(&doc, &mut ids, "add_variant_property", args),
        "node"
    );
}

/// A property's values are added, moved, renamed in every variant holding them,
/// set on a variant, and deleted with the variants holding them, which it says.
///
/// flip: `delete_variant_value` dropping its `saying` → fails at "says what went";
/// `move_variant_value` ignoring `index` (moving to 0) → fails at "Value 2 moves
/// to the middle".
#[test]
fn variant_values_are_added_moved_renamed_set_and_deleted() {
    let (mut doc, mut ids, page) = blank();
    let (set, small, large) = size_set(&mut doc, &mut ids, page);
    let on = |extra: Value| {
        let mut args = json!({ "node": set.to_wire(), "property": "Size" });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    run(&mut doc, &mut ids, "add_variant_value", on(json!({})));
    let values = |doc: &Document| variant_props(doc, set)[0].1.clone();
    assert_eq!(values(&doc), ["Small", "Large", "Value 2"]);
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "add_variant_value",
            on(json!({ "value": "Small" }))
        ),
        "value"
    );
    run(
        &mut doc,
        &mut ids,
        "move_variant_value",
        on(json!({ "value": "Value 2", "index": 1 })),
    );
    assert_eq!(
        values(&doc),
        ["Small", "Value 2", "Large"],
        "Value 2 moves to the middle"
    );
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "move_variant_value",
            on(json!({ "value": "Small", "index": 3 }))
        ),
        "index"
    );
    run(
        &mut doc,
        &mut ids,
        "rename_variant_value",
        on(json!({ "value": "Small", "name": "Tiny" })),
    );
    assert_eq!(values(&doc)[0], "Tiny");
    assert_eq!(
        doc.get(small).unwrap().variant(),
        ["Tiny"],
        "its variant follows"
    );
    run(
        &mut doc,
        &mut ids,
        "set_variant_value",
        json!({ "node": small.to_wire(), "property": "Size", "value": "Value 2" }),
    );
    assert_eq!(doc.get(small).unwrap().variant(), ["Value 2"]);
    let args = json!({ "node": set.to_wire(), "property": "Size", "value": "Tiny" });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_variant_value", args),
        "node",
        "a set has no values of its own"
    );
    let unused = answer(
        &doc,
        &mut ids,
        "delete_variant_value",
        on(json!({ "value": "Tiny" })),
    )
    .unwrap();
    assert_eq!(
        unused.said, None,
        "a value no variant holds takes none with it"
    );
    let used = answer(
        &doc,
        &mut ids,
        "delete_variant_value",
        on(json!({ "value": "Large" })),
    )
    .unwrap();
    assert_eq!(
        used.said.as_deref(),
        Some("deleted 1 variant"),
        "says what went"
    );
    run(
        &mut doc,
        &mut ids,
        "delete_variant_value",
        on(json!({ "value": "Large" })),
    );
    assert!(
        doc.get(large).is_none(),
        "the variant holding it goes with it"
    );
    assert_eq!(values(&doc), ["Tiny", "Value 2"]);
}

/// A value tied to a state makes its property the interaction state; rest's value
/// is refused with why, and untying the last makes the property plain again.
///
/// flip: `tie_state` skipping `why_not_tie` → fails at "rest is refused", the core
/// refusing it under `state` without the reason.
#[test]
fn tie_state_ties_and_unties_a_value() {
    let (mut doc, mut ids, page) = blank();
    let (set, ..) = size_set(&mut doc, &mut ids, page);
    let rest = ondin_core::variant::rest_value(&doc, set, 0).expect("a default variant");
    let other = if rest == "Small" { "Large" } else { "Small" };
    let tie = |value: &str, state: Value| json!({ "node": set.to_wire(), "property": "Size", "value": value, "state": state });
    assert_eq!(
        refused_param(&doc, &mut ids, "tie_state", tie(&rest, json!("hover"))),
        "value",
        "rest is refused"
    );
    run(&mut doc, &mut ids, "tie_state", tie(other, json!("hover")));
    let states = |doc: &Document| doc.get(set).unwrap().set().unwrap().props[0].states.clone();
    assert_eq!(
        states(&doc).get(other),
        Some(&ondin_core::variant::Interaction::Hover)
    );
    run(&mut doc, &mut ids, "tie_state", tie(other, Value::Null));
    assert!(
        states(&doc).is_empty(),
        "untied, the property is plain again"
    );
}

/// A component property is renamed and deleted by name on its main; a filter is
/// refused on one that is not a swap.
///
/// flip: `edit_property` keeping the old name → fails at "renamed".
#[test]
fn component_properties_are_renamed_and_deleted() {
    let (mut doc, mut ids, page) = blank();
    let (card, rect_in) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": rect_in.to_wire(), "kind": "boolean", "name": "Show box" }),
    );
    let names = |doc: &Document| -> Vec<String> {
        doc.get(card)
            .unwrap()
            .props()
            .iter()
            .map(|p| p.name.clone())
            .collect()
    };
    run(
        &mut doc,
        &mut ids,
        "edit_property",
        json!({ "node": card.to_wire(), "property": "Show box", "name": "Box" }),
    );
    assert_eq!(names(&doc), ["Box"], "renamed");
    let args = json!({ "node": card.to_wire(), "property": "Box", "filter": "Icon" });
    assert_eq!(
        refused_param(&doc, &mut ids, "edit_property", args),
        "filter"
    );
    let args = json!({ "node": rect_in.to_wire(), "property": "Box" });
    assert_eq!(
        refused_param(&doc, &mut ids, "delete_property", args),
        "node",
        "a layer inside the main owns none"
    );
    run(
        &mut doc,
        &mut ids,
        "delete_property",
        json!({ "node": card.to_wire(), "property": "Box" }),
    );
    assert!(names(&doc).is_empty());
}

/// A swap property's preferred mains are those its slot can show, each once: the
/// main holding the slot is refused, as the card's picker never offers it.
///
/// flip: `edit_property` taking any main → fails at "the owner is refused";
/// without dropping repeats → fails at "each once".
#[test]
fn preferred_mains_are_what_the_slot_can_show() {
    let (mut doc, mut ids, page) = blank();
    let (icon_a, _) = main_component(&mut doc, &mut ids, page, "Icon A", 20.0);
    let (icon_b, _) = main_component(&mut doc, &mut ids, page, "Icon B", 20.0);
    let button = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(120.0, 40.0), "name": "Button" }),
    )[0];
    let slot = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": icon_a.to_wire(), "parent": button.to_wire() }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "make_component",
        json!({ "nodes": [button.to_wire()] }),
    );
    run(
        &mut doc,
        &mut ids,
        "define_property",
        json!({ "node": slot.to_wire(), "kind": "swap", "name": "Icon" }),
    );
    let prefer =
        |list: Value| json!({ "node": button.to_wire(), "property": "Icon", "preferred": list });
    run(
        &mut doc,
        &mut ids,
        "edit_property",
        prefer(json!([
            icon_b.to_wire(),
            icon_b.to_wire(),
            icon_a.to_wire()
        ])),
    );
    assert_eq!(
        doc.get(button).unwrap().props()[0].preferred,
        [icon_b, icon_a],
        "each once"
    );
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "edit_property",
            prefer(json!([button.to_wire()]))
        ),
        "preferred",
        "the owner is refused"
    );
}

/// A nested instance's properties are shown on the main holding it, and stop;
/// a layer that is no nested instance is refused.
///
/// flip: `show_nested_properties` always passing `true` → fails at "shown no more".
#[test]
fn show_nested_properties_shows_and_hides_a_slot() {
    let (mut doc, mut ids, page) = blank();
    let (icon, _) = main_component(&mut doc, &mut ids, page, "Icon", 20.0);
    let (button, rect_in) = main_component(&mut doc, &mut ids, page, "Button", 120.0);
    let slot = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": icon.to_wire(), "parent": button.to_wire() }),
    )[0];
    let shown = |doc: &Document| ondin_core::variant::slot_is_shown(doc, slot);
    run(
        &mut doc,
        &mut ids,
        "show_nested_properties",
        json!({ "node": slot.to_wire(), "shown": true }),
    );
    assert!(shown(&doc));
    run(
        &mut doc,
        &mut ids,
        "show_nested_properties",
        json!({ "node": slot.to_wire(), "shown": false }),
    );
    assert!(!shown(&doc), "shown no more");
    let args = json!({ "node": rect_in.to_wire(), "shown": true });
    assert_eq!(
        refused_param(&doc, &mut ids, "show_nested_properties", args),
        "node"
    );
}

/// A probe that answers one finding for every layer asked with a solid first
/// fill: its colour on white, held to `needs`. Any other literal paint is skipped
/// as a gradient; a layer with no fill or a bound one is not considered.
struct OnWhite {
    needs: f64,
}

impl ondin_core::contrast::Probe for OnWhite {
    fn measure(
        &mut self,
        doc: &Document,
        ids: &[NodeId],
        level: ondin_core::contrast::Level,
    ) -> ondin_core::contrast::Measured {
        use ondin_core::contrast::{Finding, Measured, Sample};
        let mut skipped = Vec::new();
        let findings = ids
            .iter()
            .filter_map(|id| {
                let ink = match doc.get(*id)?.paint().fills.first()?.brush.literal()? {
                    Brush::Solid(c) => *c,
                    _ => {
                        skipped.push((*id, "gradient fill"));
                        return None;
                    }
                };
                let sample = Sample {
                    mode: None,
                    name: None,
                    score: 0.0,
                    needs: self.needs,
                    ink,
                    opacity: 1.0,
                    under: Color::WHITE,
                    backdrop: Color::WHITE,
                };
                let mut f = Finding {
                    id: *id,
                    trail: Vec::new(),
                    ratio: 0.0,
                    needs: self.needs,
                    large: false,
                    non_text: true,
                    mode: None,
                    samples: vec![sample.clone()],
                };
                f.samples[0].score = sample.score_of(level, ink);
                f.settle(level);
                Some(f)
            })
            .collect();
        Measured { findings, skipped }
    }
}

/// `fix_contrast` gives a failing layer the nearest colour that passes, as the
/// probe it borrows measured it, and leaves a passing one; a caller with no probe
/// is refused.
///
/// flip: `fix_contrast` writing the ink back rather than `contrast::nearest`'s
/// colour → fails at "the pale layer passes now". Fixing passing findings too
/// does not bite: `fix_transaction` leaves a paint that passes as it is.
#[test]
fn fix_contrast_gives_a_failing_layer_the_nearest_passing_colour() {
    let (mut doc, mut ids, page) = blank();
    let fill = |c: Color| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }])
        .unwrap()
    };
    let p = page.to_wire();
    let pale = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    let dark = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": p, "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [pale.to_wire()], "fills": fill(Color::from_rgb8(0xcc, 0xcc, 0xcc)) }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [dark.to_wire()], "fills": fill(Color::BLACK) }),
    );

    let mut probe = OnWhite { needs: 3.0 };
    let script =
        json!([{ "verb": "fix_contrast", "args": { "nodes": [pale.to_wire(), dark.to_wire()] } }]);
    let lent = Lent {
        contrast: Some(&mut probe),
        ..Lent::default()
    };
    verbs::run_script(&mut doc, &mut ids, lent, &script).unwrap_or_else(|e| panic!("{e}"));
    let colour = |id: NodeId| match doc.get(id).unwrap().paint().fills[0].brush.literal() {
        Some(Brush::Solid(c)) => *c,
        other => panic!("{other:?}"),
    };
    let fixed = ondin_core::contrast::Probe::measure(
        &mut OnWhite { needs: 3.0 },
        &doc,
        &[pale],
        ondin_core::contrast::Level::Aa,
    );
    assert!(
        fixed.findings[0].passes(),
        "the pale layer passes now: {:?}",
        colour(pale)
    );
    assert_eq!(colour(dark), Color::BLACK, "the passing layer is left");

    let err =
        verbs::run_script(&mut doc, &mut ids, Lent::default(), &script).expect_err("no probe");
    assert!(err.why.contains("cannot measure"), "{err}");
}

/// `fix_contrast` binds a failing layer to a colour variable it is given, and
/// refuses a colour and a variable both, a variable that is not a colour, and
/// layers it could not measure, naming why, rather than answering as though they
/// passed.
///
/// flip: `fix_contrast` answering an empty edit when nothing was measured → fails
/// at `expect_err("a gradient")`.
#[test]
fn fix_contrast_binds_a_variable_and_refuses_what_it_cannot_fix() {
    let (mut doc, mut ids, page) = blank();
    let black = serde_json::to_value(ondin_core::Cell::Color(Color::BLACK)).unwrap();
    let two = serde_json::to_value(ondin_core::Cell::Number(2.0)).unwrap();
    let pale = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(0xcc, 0xcc, 0xcc)).into(),
        visible: true,
    }])
    .unwrap();
    let gradient = serde_json::to_value(vec![Fill {
        brush: Brush::Gradient(
            ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0)).into(),
        )
        .into(),
        visible: true,
    }])
    .unwrap();
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "color", "name": "ink", "value": black }, "as": "ink" },
        { "verb": "add_variable", "args": { "kind": "number", "name": "two", "value": two }, "as": "two" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0), "name": "Pale" }, "as": "p" },
        { "verb": "set_fills", "args": { "nodes": ["@p"], "fills": pale } },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0), "name": "Sky" }, "as": "g" },
        { "verb": "set_fills", "args": { "nodes": ["@g"], "fills": gradient } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let (ink, two, p, g) = (
        made[0][0].clone(),
        made[1][0].clone(),
        made[2][0].clone(),
        made[4][0].clone(),
    );
    let mut probe = OnWhite { needs: 3.0 };
    let mut fix = |doc: &mut Document, ids: &mut IdSource, args: Value| {
        let lent = Lent {
            contrast: Some(&mut probe),
            ..Lent::default()
        };
        verbs::run_script(
            doc,
            ids,
            lent,
            &json!([{ "verb": "fix_contrast", "args": args }]),
        )
    };
    let colour = serde_json::to_value(Color::BLACK).unwrap();
    let both = fix(
        &mut doc,
        &mut ids,
        json!({ "nodes": [p], "colour": colour, "variable": ink }),
    )
    .expect_err("a colour and a variable");
    assert!(both.why.contains("not both"), "{both}");
    let number = fix(&mut doc, &mut ids, json!({ "nodes": [p], "variable": two }))
        .expect_err("a number variable");
    assert!(number.why.contains("not a colour"), "{number}");
    let skipped = fix(&mut doc, &mut ids, json!({ "nodes": [g] })).expect_err("a gradient");
    assert!(
        skipped.why.contains("Sky (gradient fill)"),
        "a gradient is named: {skipped}"
    );

    fix(&mut doc, &mut ids, json!({ "nodes": [p], "variable": ink }))
        .unwrap_or_else(|e| panic!("{e}"));
    let var: ondin_core::VarId = serde_json::from_value(json!(ink)).unwrap();
    let node = doc.get(NodeId::from_wire(&p).unwrap()).unwrap();
    assert_eq!(
        node.paint().fills[0].brush.var(),
        Some(var),
        "the fill is bound to the variable"
    );
    assert!(doc.get(page).is_some());
}

/// A sink that takes any picture; its id is the byte length.
struct Pictures;

impl ImageSink for Pictures {
    fn place(&mut self, bytes: Vec<u8>, format: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        Some((
            ImageId(format!("test:{}", bytes.len())),
            ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format,
                width: 4,
                height: 4,
            },
        ))
    }
}

/// `add_image` puts a picture in the table and answers its id, which a script's
/// label carries into a fill; the same bytes again add nothing. A caller with no
/// sink, or bytes no picture format starts with, is refused.
///
/// flip: `add_image` emitting `AddImage` when the table already holds the id →
/// fails at the second script's commit, with `DuplicateImage`.
#[test]
fn add_image_answers_an_id_a_fill_can_show() {
    use base64::Engine as _;
    let (mut doc, mut ids, page) = blank();
    let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n-pixels-");
    let fill = serde_json::to_value(Fill {
        brush: image_brush(ImageId("@logo".into())).into(),
        visible: true,
    })
    .unwrap();
    let script = json!([
        { "verb": "add_image", "args": { "bytes": png }, "as": "logo" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "r" },
        { "verb": "set_fills", "args": { "nodes": ["@r"], "fills": [fill] } },
    ]);
    let made = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent {
            images: Some(&mut Pictures),
            ..Lent::default()
        },
        &script,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let id = ImageId(made[0][0].clone());
    assert!(doc.has_image(&id), "{made:?}");
    let r = doc.get(page).unwrap().children()[0];
    assert!(
        matches!(doc.get(r).unwrap().paint().fills[0].brush.literal(),
            Some(Brush::Image(b)) if b.image.id == id),
        "the fill shows the picture"
    );

    let again = json!([{ "verb": "add_image", "args": { "bytes": png } }]);
    let made = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent {
            images: Some(&mut Pictures),
            ..Lent::default()
        },
        &again,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(made[0], std::slice::from_ref(&id.0));
    assert_eq!(doc.images().count(), 1);

    let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &again).expect_err("no sink");
    assert!(err.why.contains("cannot read pictures"), "{err}");
    let text = base64::engine::general_purpose::STANDARD.encode(b"hello");
    let err = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent {
            images: Some(&mut Pictures),
            ..Lent::default()
        },
        &json!([{ "verb": "add_image", "args": { "bytes": text } }]),
    )
    .expect_err("not a picture");
    assert!(err.why.contains("not a PNG"), "{err}");
}

/// `create_text` makes a text layer whose box's top-left is where it was asked
/// for, wrapping a line longer than the width given.
///
/// The verb's default style has no box trim, so its box's offset is zero and
/// dropping the offset from the placement does not bite here; the offset matters
/// only once the verb takes a trimmed style.
#[test]
fn create_text_places_its_box() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": p, "text": "Hello there", "x": 20, "y": 30 }),
    )[0];
    let res = Resolved::rebuild(&doc);
    let b = res.world_bounds(t).unwrap();
    assert!(
        (b.x0 - 20.0).abs() < 1e-6 && (b.y0 - 30.0).abs() < 1e-6,
        "the box's corner is at x, y: {b:?}"
    );
    let narrow = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": p, "text": "a much longer line of words", "width": 40 }),
    )[0];
    let NodeKind::Text { sizing, .. } = doc.get(narrow).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(*sizing, ondin_core::TextSizing::AutoHeight(40.0));
}

/// `replace_image` swaps the picture a fill shows and keeps its framing; a picture
/// the document does not hold is refused.
///
/// flip: `build::replace_image_fill` rebuilding every brush from `image_brush` →
/// fails at "the framing stays".
#[test]
fn replace_image_keeps_the_framing() {
    use base64::Engine as _;
    let (mut doc, mut ids, page) = blank();
    let enc = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let first = enc(b"\x89PNG\r\n\x1a\n-one-");
    let second = enc(b"\x89PNG\r\n\x1a\n-second-");
    let mut brush = image_brush(ImageId("@a".into()));
    if let Brush::Image(b) = &mut brush {
        b.image.fit = ondin_core::ImageFit::Tile;
    }
    let fill = serde_json::to_value(Fill {
        brush: brush.into(),
        visible: true,
    })
    .unwrap();
    let script = json!([
        { "verb": "add_image", "args": { "bytes": first }, "as": "a" },
        { "verb": "add_image", "args": { "bytes": second }, "as": "b" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "r" },
        { "verb": "set_fills", "args": { "nodes": ["@r"], "fills": [fill] } },
        { "verb": "replace_image", "args": { "node": "@r", "image": "@b" } },
    ]);
    let made = verbs::run_script(
        &mut doc,
        &mut ids,
        Lent {
            images: Some(&mut Pictures),
            ..Lent::default()
        },
        &script,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let r = doc.get(page).unwrap().children()[0];
    let Some(Brush::Image(b)) = doc.get(r).unwrap().paint().fills[0]
        .brush
        .literal()
        .cloned()
    else {
        panic!("a picture fill");
    };
    assert_eq!(b.image.id.0, made[1][0], "the second picture shows");
    assert_eq!(b.image.fit, ondin_core::ImageFit::Tile, "the framing stays");

    let missing = json!([{ "verb": "replace_image", "args": { "node": r.to_wire(), "image": "sha256:none" } }]);
    let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &missing)
        .expect_err("no such picture");
    assert!(err.why.contains("no such picture"), "{err}");
}

/// A script's later steps name an earlier step's node by its label, and `@page` is
/// the page; a refusal names its step, counted from 1, and keeps the steps before it.
///
/// flip: `run_script` skipping `substitute` → fails at the first `run_script`, step 1:
/// "`@page` is not a node id".
#[test]
fn a_script_refers_to_what_its_earlier_steps_made() {
    let (mut doc, mut ids, page) = blank();
    let script = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(10.0, 10.0) }, "as": "a" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(30.0, 5.0) }, "as": "b" },
        { "verb": "group", "args": { "nodes": ["@a", "@b"] }, "as": "pair" },
        { "verb": "set_name", "args": { "node": "@pair", "name": "Pair" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let pair = NodeId::from_wire(&made[2][0]).expect("a node's wire form");
    assert_eq!(doc.get(pair).unwrap().name(), "Pair");
    assert_eq!(doc.get(page).unwrap().children(), [pair]);

    // Labels belong to one script: this one knows `page` and its own, not `pair`.
    let broken = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(1.0, 1.0) }, "as": "c" },
        { "verb": "set_name", "args": { "node": "@pair", "name": "x" } },
    ]);
    let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &broken)
        .expect_err("an unknown label");
    assert_eq!(err.step, 2, "{err}");
    assert!(err.why.contains("pair"), "{err}");
    assert_eq!(
        doc.get(page).unwrap().children().len(),
        2,
        "the step before the refusal stands"
    );
}

/// *Duplicate page* copies a page holding several top-level layers, in order.
/// flip: `page::duplicate` giving each template its own index → fails at the
/// commit ("index out of range").
#[test]
fn a_page_of_several_layers_duplicates_in_order() {
    let (mut doc, mut ids, page) = blank();
    for name in ["a", "b", "c"] {
        run(
            &mut doc,
            &mut ids,
            "create_node",
            json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0), "name": name }),
        );
    }
    let copy = run(
        &mut doc,
        &mut ids,
        "duplicate_page",
        json!({ "page": page.to_wire() }),
    )[0];
    let names: Vec<&str> = doc
        .get(copy)
        .unwrap()
        .children()
        .iter()
        .map(|c| doc.get(*c).unwrap().name())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
}

/// A text made from a kind whose run ends inside a character, or past the
/// content, is stored with its runs on character boundaries, so it shapes.
/// flip: drop the clamp in `Document::op_create` → panics in the rebuild.
#[test]
fn a_created_text_keeps_its_runs_on_character_boundaries() {
    let (mut doc, mut ids, page) = blank();
    let mut kind = serde_json::to_value(NodeKind::Text {
        content: "héllo".into(),
        style: Box::new(ondin_core::TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    })
    .unwrap();
    kind["Text"]["spans"] = json!([
        { "start": 0, "end": 2, "attr": { "Weight": 700 } },
        { "start": 4, "end": 999, "attr": { "Weight": 300 } },
    ]);
    let t = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": kind }),
    )[0];
    Resolved::rebuild(&doc);
    let NodeKind::Text { content, spans, .. } = doc.get(t).unwrap().kind() else {
        unreachable!()
    };
    let json = serde_json::to_value(spans).unwrap();
    let runs = json.as_array().expect("runs are a list");
    assert!(!runs.is_empty(), "the fixture: runs are stored");
    for run in runs {
        for end in ["start", "end"] {
            let at = run[end].as_u64().unwrap() as usize;
            assert!(content.is_char_boundary(at), "{run}");
        }
    }
}

/// The layer verbs refuse a page, which has no control for what they write;
/// `set_name` still renames one, trimmed, and an empty page name is refused.
/// flip: `Args::layer` or `Args::layers` not asking `is_page` → fails at that
/// verb's refusal.
#[test]
fn layer_verbs_refuse_a_page_and_a_page_name_is_trimmed() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let steps = [
        json!({ "verb": "set_transform", "args": { "node": p, "transform": [2, 0, 0, 2, 5, 5] } }),
        json!({ "verb": "set_pivot", "args": { "node": p } }),
        json!({ "verb": "set_opacity", "args": { "nodes": [p], "opacity": 0.5 } }),
        json!({ "verb": "set_visible", "args": { "nodes": [p], "visible": false } }),
        json!({ "verb": "set_locked", "args": { "nodes": [p], "locked": true } }),
        json!({ "verb": "set_blend", "args": { "nodes": [p], "blend": "Multiply" } }),
        json!({ "verb": "set_exports", "args": { "nodes": [p], "exports": [] } }),
        json!({ "verb": "set_name", "args": { "node": p, "name": "  " } }),
        json!({ "verb": "add_page", "args": { "name": "" } }),
    ];
    for step in steps {
        let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &json!([step]))
            .expect_err(&format!("{step} is refused"));
        assert!(
            err.why.contains("a page is not a layer") || err.why.contains("not empty"),
            "{err}"
        );
    }
    run(
        &mut doc,
        &mut ids,
        "set_name",
        json!({ "node": p, "name": "  Pricing " }),
    );
    assert_eq!(doc.get(page).unwrap().name(), "Pricing");
}

/// `bind_variable` refuses a page for the eye and the opacity, as `set_visible`
/// and `set_opacity` do, and the page stays shown and unbound.
/// flip: `bind_variable` reading `nodes` with `Args::nodes` for every field →
/// fails at "is refused".
#[test]
fn bind_variable_refuses_a_page_for_its_eye_and_opacity() {
    let (mut doc, mut ids, page) = blank();
    let no = serde_json::to_value(ondin_core::Cell::Boolean(false)).unwrap();
    let half = serde_json::to_value(ondin_core::Cell::Number(0.5)).unwrap();
    let vars = json!([
        { "verb": "add_variable", "args": { "kind": "boolean", "name": "shown", "value": no } },
        { "verb": "add_variable", "args": { "kind": "number", "name": "fade", "value": half } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &vars)
        .unwrap_or_else(|e| panic!("{e}"));
    let (b, n) = (made[0][0].clone(), made[1][0].clone());
    let p = page.to_wire();
    for (field, var) in [("visible", b), ("opacity", n)] {
        let step = json!([{ "verb": "bind_variable",
            "args": { "nodes": [p.clone()], "field": field, "variable": var } }]);
        let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &step)
            .expect_err(&format!("{field} on a page is refused"));
        assert!(err.why.contains("a page is not a layer"), "{err}");
    }
    let root = doc.get(page).unwrap();
    assert!(doc.shown(page), "the page stays shown");
    assert!(root.visible_var().is_none() && root.opacity_var().is_none());
}

/// A step with a key other than `verb`, `args` and `as` is refused before it
/// runs, and `@@` writes a literal `@`.
/// flip: `run_script` not checking a step's keys → fails at "a misspelled
/// `args` is refused"; `substitute` without its `@@` arm → fails at the text.
#[test]
fn a_script_refuses_a_stray_key_and_escapes_an_at_sign() {
    let (mut doc, mut ids, page) = blank();
    let stray = json!([{ "verb": "add_page", "arg": { "name": "Other" } }]);
    let err = verbs::run_script(&mut doc, &mut ids, Lent::default(), &stray)
        .expect_err("a misspelled `args` is refused");
    assert!(err.why.contains("`arg`"), "{err}");
    assert_eq!(doc.pages().len(), 1, "nothing ran");

    let script = json!([
        { "verb": "create_text", "args": { "parent": "@page", "text": "@@home is here" }, "as": "t" },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let t = NodeId::from_wire(&made[0][0]).unwrap();
    let NodeKind::Text { content, .. } = doc.get(t).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(content, "@home is here");
    assert_eq!(doc.get(t).unwrap().parent(), Some(page));
}

/// The description of chart `id` in `doc`.
fn chart_of(doc: &Document, id: NodeId) -> ondin_core::chart::Chart {
    doc.get(id).unwrap().chart().expect("a chart").clone()
}

/// `insert_chart` makes a 480 × 280 frame named *Chart* at its place on the
/// kind's sample, or on a table given with its mapping read off the column
/// types, and its parts follow once settled; a mapping naming no column of the
/// table is refused as the mapping's fault.
#[test]
fn insert_chart_makes_a_chart_on_a_sample_or_a_table() {
    use ondin_core::chart::{Chart, ChartKind, ColumnId};
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "line", "x": 40, "y": 24 }),
    )[0];
    let n = doc.get(c).unwrap();
    assert_eq!(n.name(), "Chart");
    assert!(
        matches!(n.kind(), NodeKind::Artboard { size, .. } if *size == Size::new(480.0, 280.0))
    );
    assert_eq!(n.transform(), Affine::translate((40.0, 24.0)));
    let sample = Chart::sample(ChartKind::Line);
    assert!(chart_of(&doc, c).same_description(&sample));
    Resolved::settled(&mut doc);
    assert!(chart_of(&doc, c).parts.len() > 10, "its parts follow");

    let table = json!({
        "columns": [{ "id": 0, "name": "City" }, { "id": 1, "name": "Sales" }],
        "rows": [["Oslo", "4"], ["Rome", "7"]],
    });
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "pie", "table": table,
                "width": 300, "height": 300, "name": "Cities" }),
    )[0];
    let got = chart_of(&doc, c);
    assert_eq!(got.kind, ChartKind::Pie);
    assert_eq!(got.table.rows.len(), 2);
    assert_eq!(
        (got.mapping.category, got.mapping.series.as_slice()),
        (Some(ColumnId(0)), &[ColumnId(1)][..]),
        "the mapping is read off the column types"
    );

    let bad = json!({ "parent": page.to_wire(), "kind": "column", "mapping": { "category": 9 } });
    assert_eq!(
        refused_param(&doc, &mut ids, "insert_chart", bad),
        "mapping"
    );
    let flat = json!({ "parent": page.to_wire(), "kind": "column", "width": 0 });
    assert_eq!(refused_param(&doc, &mut ids, "insert_chart", flat), "width");
}

/// `set_chart` changes the kind and patches the mapping and the options, leaving
/// what the patch does not name; a null clears or resets what it names. It asks
/// for something to change, and a chart.
///
/// flip: `patched` replacing the whole value with the patch → fails at "the
/// category is kept".
#[test]
fn set_chart_patches_the_kind_mapping_and_options() {
    use ondin_core::chart::{ChartKind, ColumnId, Stacking};
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    let gap = chart_of(&doc, c).options.bars.bar_gap;
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({
            "node": c.to_wire(),
            "kind": "bar",
            "mapping": { "series": [1, 3] },
            "options": { "bars": { "stacking": "Stacked" }, "legend": null },
        }),
    );
    let got = chart_of(&doc, c);
    assert_eq!(got.kind, ChartKind::Bar);
    assert_eq!(got.mapping.series, [ColumnId(1), ColumnId(3)]);
    assert_eq!(
        got.mapping.category,
        Some(ColumnId(0)),
        "the category is kept"
    );
    assert_eq!(got.options.bars.stacking, Stacking::Stacked);
    assert_eq!(got.options.bars.bar_gap, gap, "the bar gap is kept");
    assert_eq!(got.options.legend, Default::default());

    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "mapping": { "category": null } }),
    );
    assert_eq!(chart_of(&doc, c).mapping.category, None, "a null clears");

    let none = json!({ "node": c.to_wire() });
    assert!(matches!(
        answer(&doc, &mut ids, "set_chart", none),
        Err(VerbError::Nothing(_))
    ));
    let range = json!({ "node": c.to_wire(), "options": { "bars": { "group_gap": 500 } } });
    assert_eq!(refused_param(&doc, &mut ids, "set_chart", range), "options");
    let unknown = json!({ "node": c.to_wire(), "mapping": { "series": [42] } });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_chart", unknown),
        "mapping"
    );
    let plain = json!({ "node": page.to_wire(), "kind": "pie" });
    assert_eq!(refused_param(&doc, &mut ids, "set_chart", plain), "node");
}

/// `set_chart` turning a chart dark moves its starting ink with it, as the
/// card's *Dark* does, and a palette patch recolours a stored series colour
/// as the card's pick does.
/// flip: `set_chart` writing the patched `dark` as it is → fails at "the ink
/// follows"; skipping `with_palette` → fails at "the palette pick recolours".
#[test]
fn set_chart_dark_and_palette_move_the_colours() {
    use ondin_core::Brush;
    use ondin_core::chart::{Palette, chrome, palette, series_fill};
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "options": { "dark": true } }),
    );
    let got = chart_of(&doc, c);
    assert!(got.options.dark);
    assert_eq!(
        got.options.text.ink,
        Brush::Solid(chrome(true).ink).into(),
        "the ink follows"
    );
    let first = got.mapping.series[0];
    let painted = ondin_core::chart::with_series_fill(
        &got,
        first,
        Brush::Solid(Color::from_rgb8(1, 2, 3)).into(),
    );
    assert!(
        doc.apply(&ondin_core::Transaction(vec![
            ondin_core::Operation::SetChart {
                id: c,
                chart: Some(Box::new(painted)),
            }
        ]))
        .is_ok()
    );
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "options": { "palette": "Warm" } }),
    );
    let [r, g, b] = palette(Palette::Warm, true)[0];
    assert_eq!(
        series_fill(&chart_of(&doc, c), first),
        Some(Brush::Solid(Color::from_rgb8(r, g, b)).into()),
        "the palette pick recolours, in the dark set"
    );
}

/// `set_chart` switching to or from a Sankey re-reads the mapping off the
/// table's columns, as the Chart card does (`Chart::with_kind`).
/// flip: the verb assigning `chart.kind` alone → fails at "a column chart maps
/// a category and series".
#[test]
fn set_chart_rereads_the_mapping_across_a_sankey() {
    use ondin_core::chart::{ChartKind, ColumnId};
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "sankey" }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "kind": "column" }),
    );
    let got = chart_of(&doc, c);
    assert_eq!(got.kind, ChartKind::Column);
    assert_eq!(
        (got.mapping.category, got.mapping.series.as_slice()),
        (Some(ColumnId(0)), &[ColumnId(2)][..]),
        "a column chart maps a category and series"
    );
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "kind": "sankey" }),
    );
    let got = chart_of(&doc, c);
    assert_eq!(
        (got.mapping.from, got.mapping.to, got.mapping.value),
        (Some(ColumnId(0)), Some(ColumnId(1)), Some(ColumnId(2))),
        "a Sankey maps its flows"
    );
}

/// A null in `set_chart`'s options resets the key to the chart's default, not
/// to the default of the struct holding it: the value axis's line and the
/// category axis's gridlines start off.
/// flip: `patched` removing a nulled key → fails at "the value axis's line".
#[test]
fn set_chart_null_resets_to_the_charts_default() {
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "options": {
            "value_axis": { "line": true }, "category_axis": { "grid": { "show": true } } } }),
    );
    let set = chart_of(&doc, c).options;
    assert!(
        set.value_axis.line && set.category_axis.grid.show,
        "the fixture set both"
    );
    run(
        &mut doc,
        &mut ids,
        "set_chart",
        json!({ "node": c.to_wire(), "options": {
            "value_axis": { "line": null }, "category_axis": { "grid": { "show": null } } } }),
    );
    let got = chart_of(&doc, c).options;
    assert!(!got.value_axis.line, "the value axis's line");
    assert!(
        !got.category_axis.grid.show,
        "the category axis's gridlines"
    );
}

/// `set_chart_data` replaces the table, keeping the mapping and series styles of
/// each column whose name survives, and drops the linked file.
#[test]
fn set_chart_data_keeps_the_columns_whose_names_survive() {
    use ondin_core::chart::{ColumnId, DataLink, SeriesStyle};
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    let mut linked = chart_of(&doc, c);
    linked.source = Some(DataLink {
        path: "C:/sales.csv".into(),
        size: 1,
        modified: 1,
        header: None,
    });
    linked.series.push(SeriesStyle {
        column: ColumnId(1),
        fill: ondin_core::Brush::Solid(Color::BLACK).into(),
        visible: true,
        dash: Default::default(),
        point: Default::default(),
    });
    doc.apply(&Transaction(vec![Operation::SetChart {
        id: c,
        chart: Some(Box::new(linked)),
    }]))
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_chart_data",
        json!({
            "node": c.to_wire(),
            "table": {
                "columns": [{ "id": 7, "name": "Revenue" }, { "id": 8, "name": "Category" }],
                "rows": [["5", "Q1"], ["9", "Q2"]],
            },
        }),
    );
    let got = chart_of(&doc, c);
    assert_eq!(got.table.columns[0].id, ColumnId(1), "Revenue keeps its id");
    assert_eq!(got.mapping.category, Some(ColumnId(0)));
    assert_eq!(got.mapping.series, [ColumnId(1)]);
    assert_eq!(got.series.len(), 1, "its colour is kept");
    assert_eq!(got.source, None, "the file no longer is its table");

    let ragged = json!({
        "node": c.to_wire(),
        "table": { "columns": [{ "id": 0, "name": "A" }], "rows": [["1", "2"]] },
    });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_chart_data", ragged),
        "table"
    );
}

/// `flatten_chart` drops the description and leaves the parts where they are;
/// a layer that is not a chart is refused.
#[test]
fn flatten_chart_leaves_the_parts() {
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "area" }),
    )[0];
    Resolved::settled(&mut doc);
    let children = doc.get(c).unwrap().children().to_vec();
    assert!(!children.is_empty(), "the fixture settled");
    run(
        &mut doc,
        &mut ids,
        "flatten_chart",
        json!({ "nodes": [c.to_wire()] }),
    );
    let n = doc.get(c).unwrap();
    assert!(n.chart().is_none());
    assert_eq!(n.children(), children.as_slice());
    let again = json!({ "nodes": [c.to_wire()] });
    assert_eq!(
        refused_param(&doc, &mut ids, "flatten_chart", again),
        "nodes"
    );
}

/// `flatten_chart` detaches a line's stroke drawing a colour scoped to fills to
/// the colour it drew, whether the scope came before the binding or was
/// narrowed after it, so the commit takes it.
/// flip: `flatten_ops` without its `out_of_scope_ops` → fails at "flatten_chart commits".
#[test]
fn flatten_chart_detaches_a_line_drawing_a_fill_scoped_colour() {
    use ondin_core::chart::PartRole;
    use ondin_core::variable::{Cell, CollectionId, Field, Scope, VarKind, Variable};
    let red = Color::from_rgb8(255, 0, 0);
    let fills = Some(Scope::of([Field::Fill]));
    for narrowed in [false, true] {
        let (mut doc, mut ids, page) = blank();
        let c = run(
            &mut doc,
            &mut ids,
            "insert_chart",
            json!({ "parent": page.to_wire(), "kind": "line" }),
        )[0];
        let (light, var) = (ids.mint_mode(), ids.mint_var());
        let bound = ondin_core::chart::with_variables(&chart_of(&doc, c), &[var]);
        doc.apply(&Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: ondin_core::Mode {
                    id: light,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(light, Cell::Color(red))].into(),
                    scope: if narrowed { None } else { fills },
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetChart {
                id: c,
                chart: Some(Box::new(bound)),
            },
        ]))
        .expect("a colour binds the chart");
        Resolved::settled(&mut doc);
        if narrowed {
            let ops = ondin_core::variable::scope_ops(&doc, var, fills);
            doc.apply(&Transaction(ops)).expect("the scope narrows");
        }
        let line = chart_of(&doc, c)
            .parts
            .iter()
            .find(|(k, _)| k.role == PartRole::Line)
            .map(|(_, id)| *id)
            .expect("a line part");
        let stroke = |doc: &Document| doc.get(line).unwrap().paint().strokes[0].brush.clone();
        assert_eq!(stroke(&doc).var(), Some(var), "the line draws the binding");
        run(
            &mut doc,
            &mut ids,
            "flatten_chart",
            json!({ "nodes": [c.to_wire()] }),
        );
        assert_eq!(
            stroke(&doc).literal().cloned(),
            Some(Brush::Solid(red)),
            "narrowed {narrowed}: the line holds the colour it drew"
        );
    }
}

/// A verb writing a chart's part answers its edit, and the commit refuses it with
/// the chart's sentence; the chart's own verbs reach the part instead.
#[test]
fn a_part_edited_by_a_verb_is_refused_at_the_commit() {
    let (mut doc, mut ids, page) = blank();
    let c = run(
        &mut doc,
        &mut ids,
        "insert_chart",
        json!({ "parent": page.to_wire(), "kind": "column" }),
    )[0];
    Resolved::settled(&mut doc);
    let (_, bar) = *chart_of(&doc, c)
        .parts
        .iter()
        .find(|(k, _)| k.role == ondin_core::chart::PartRole::Bar)
        .unwrap();
    for (verb, args) in [
        (
            "set_fills",
            json!({ "nodes": [bar.to_wire()], "fills": [] }),
        ),
        ("set_name", json!({ "node": bar.to_wire(), "name": "Mine" })),
        ("delete_node", json!({ "nodes": [bar.to_wire()] })),
    ] {
        let done = answer(&doc, &mut ids, verb, args).unwrap_or_else(|e| panic!("{verb}: {e}"));
        let err = doc.apply(&done.tx).expect_err(verb);
        assert_eq!(
            err.to_string(),
            "Part of a chart — edit the chart, or flatten it first",
            "{verb}"
        );
    }
}

/// Two rectangles on the page, `a` (40 × 20) at the origin and `b` (20 × 40) at
/// `(x, y)`; answers both.
fn two_rects(
    doc: &mut Document,
    ids: &mut IdSource,
    page: NodeId,
    x: f64,
    y: f64,
) -> (NodeId, NodeId) {
    let p = page.to_wire();
    let a = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": p, "kind": rect(40.0, 20.0), "name": "A" }),
    )[0];
    let b = run(
        doc,
        ids,
        "create_node",
        json!({ "parent": p, "kind": rect(20.0, 40.0), "name": "B", "transform": Affine::translate((x, y)) }),
    )[0];
    (a, b)
}

/// `frame` wraps layers in a frame sized to them; `outline_shape` makes a
/// rectangle a path; `outline_stroke` refuses a layer with no stroke and outlines
/// one with a stroke; `outline_text` refuses a layer that is not text.
///
/// flip: `outline_stroke` without its `can_outline_stroke` check → fails at
/// "no stroke is refused", since the builder refuses as `Refused`, not `Nothing`.
#[test]
fn frame_and_the_outlines_rebuild_layers() {
    let (mut doc, mut ids, page) = blank();
    let (a, b) = two_rects(&mut doc, &mut ids, page, 60.0, 0.0);
    let f = run(
        &mut doc,
        &mut ids,
        "frame",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    )[0];
    assert!(matches!(
        doc.get(f).unwrap().kind(),
        NodeKind::Artboard { .. }
    ));
    assert_eq!(doc.get(a).unwrap().parent(), Some(f));
    let bounds = Resolved::rebuild(&doc).world_bounds(f).unwrap();
    assert_eq!((bounds.width(), bounds.height()), (80.0, 40.0));

    let path = run(
        &mut doc,
        &mut ids,
        "outline_shape",
        json!({ "node": a.to_wire() }),
    )[0];
    assert!(matches!(
        doc.get(path).unwrap().kind(),
        NodeKind::Path { .. }
    ));

    let bare = answer(
        &doc,
        &mut ids,
        "outline_stroke",
        json!({ "node": b.to_wire() }),
    );
    assert!(
        matches!(bare, Err(VerbError::Nothing(_))),
        "no stroke is refused: {bare:?}"
    );
    let strokes = verbs::example("[Stroke]").unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_strokes",
        json!({ "nodes": [b.to_wire()], "strokes": strokes }),
    );
    let made = run(
        &mut doc,
        &mut ids,
        "outline_stroke",
        json!({ "node": b.to_wire() }),
    );
    assert_eq!(made.len(), 1);
    assert!(
        doc.get(made[0]).unwrap().paint().strokes.is_empty(),
        "the stroke became a fill"
    );

    let not_text = answer(
        &doc,
        &mut ids,
        "outline_text",
        json!({ "node": path.to_wire() }),
    );
    assert!(not_text.is_err(), "a path is no text");
}

/// `divide` cuts two crossing rectangles into their four faces in a group,
/// and refuses two that do not touch.
///
/// flip: `divide` answering the first source rather than the group → fails at
/// "a group of faces".
#[test]
fn divide_cuts_overlapping_shapes_into_faces() {
    let (mut doc, mut ids, page) = blank();
    let (a, b) = two_rects(&mut doc, &mut ids, page, 400.0, 400.0);
    let apart = answer(
        &doc,
        &mut ids,
        "divide",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    );
    assert!(matches!(apart, Err(VerbError::Nothing(_))), "{apart:?}");

    let (c, d) = two_rects(&mut doc, &mut ids, page, 10.0, 0.0);
    let g = run(
        &mut doc,
        &mut ids,
        "divide",
        json!({ "nodes": [c.to_wire(), d.to_wire()] }),
    )[0];
    let group = doc.get(g).unwrap();
    assert!(matches!(group.kind(), NodeKind::Group), "a group of faces");
    assert_eq!(
        group.children().len(),
        4,
        "A's two sides, the overlap, B's foot"
    );
    assert!(
        doc.get(c).is_none() && doc.get(d).is_none(),
        "the sources go"
    );
}

/// `blend_shapes` puts its steps between two shapes in a new group, and refuses
/// a step count out of range.
#[test]
fn blend_shapes_steps_between_two() {
    let (mut doc, mut ids, page) = blank();
    let (a, b) = two_rects(&mut doc, &mut ids, page, 200.0, 0.0);
    let pair = json!([a.to_wire(), b.to_wire()]);
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "blend_shapes",
            json!({ "nodes": pair, "steps": 0 })
        ),
        "steps"
    );
    let g = run(
        &mut doc,
        &mut ids,
        "blend_shapes",
        json!({ "nodes": pair, "steps": 3 }),
    )[0];
    assert_eq!(
        doc.get(g).unwrap().children().len(),
        5,
        "the two and three steps"
    );
}

/// `tidy_up` evens the gaps of a row, then answers that the row is tidy.
///
/// flip: `tidy_up` answering an empty edit as done → fails at "already tidy".
#[test]
fn tidy_up_evens_a_row_once() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let row: Vec<NodeId> = [0.0, 50.0, 130.0]
        .into_iter()
        .map(|x| {
            run(
                &mut doc,
                &mut ids,
                "create_node",
                json!({ "parent": p, "kind": rect(20.0, 20.0), "transform": Affine::translate((x, 0.0)) }),
            )[0]
        })
        .collect();
    let wires: Vec<String> = row.iter().map(|i| i.to_wire()).collect();
    run(&mut doc, &mut ids, "tidy_up", json!({ "nodes": wires }));
    let res = Resolved::rebuild(&doc);
    let x: Vec<f64> = row
        .iter()
        .map(|i| res.world_bounds(*i).unwrap().x0)
        .collect();
    assert!(
        ((x[1] - x[0]) - (x[2] - x[1])).abs() < 1e-6,
        "even gaps: {x:?}"
    );
    let again = answer(&doc, &mut ids, "tidy_up", json!({ "nodes": wires }));
    assert!(
        matches!(again, Err(VerbError::Nothing("already tidy"))),
        "{again:?}"
    );
}

/// `slice` adds a slice over its layers' bounds and refuses to slice a slice.
#[test]
fn slice_fits_its_layers_and_skips_a_slice() {
    let (mut doc, mut ids, page) = blank();
    let (a, b) = two_rects(&mut doc, &mut ids, page, 60.0, 0.0);
    let s = run(
        &mut doc,
        &mut ids,
        "slice",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    )[0];
    assert!(matches!(doc.get(s).unwrap().kind(), NodeKind::Slice { .. }));
    let r = Resolved::rebuild(&doc).world_bounds(s).unwrap();
    assert_eq!((r.width(), r.height()), (80.0, 40.0));
    let again = answer(&doc, &mut ids, "slice", json!({ "nodes": [s.to_wire()] }));
    assert!(matches!(again, Err(VerbError::Nothing(_))), "{again:?}");
}

/// `push_to_main` writes an instance's overridden fill into its main, and
/// refuses a layer in no instance.
///
/// flip: `push_to_main` pushing `Kind::Order` for every scope → fails at "the
/// main took the fill".
#[test]
fn push_to_main_writes_an_override_into_the_main() {
    let (mut doc, mut ids, page) = blank();
    let (card, inner) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    let inst = run(
        &mut doc,
        &mut ids,
        "create_instance",
        json!({ "main": card.to_wire(), "parent": page.to_wire() }),
    )[0];
    let copy = doc.get(inst).unwrap().children()[0];
    let red = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::from_rgb8(200, 0, 0)).into(),
        visible: true,
    }])
    .unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [copy.to_wire()], "fills": red }),
    );
    assert!(
        doc.get(inner).unwrap().paint().fills.is_empty(),
        "fixture: the main is bare"
    );
    run(
        &mut doc,
        &mut ids,
        "push_to_main",
        json!({ "node": copy.to_wire() }),
    );
    assert_eq!(
        doc.get(inner).unwrap().paint().fills.len(),
        1,
        "the main took the fill"
    );

    let loose = answer(
        &doc,
        &mut ids,
        "push_to_main",
        json!({ "node": card.to_wire() }),
    );
    assert!(matches!(loose, Err(VerbError::Nothing(_))), "{loose:?}");
}

/// `add_variant` grows a set by one main; `duplicate_as_component` copies a main
/// as a new main, not an instance.
#[test]
fn add_variant_and_duplicate_as_component_make_mains() {
    let (mut doc, mut ids, page) = blank();
    let (a, _) = main_component(&mut doc, &mut ids, page, "On", 100.0);
    let (b, _) = main_component(&mut doc, &mut ids, page, "Off", 100.0);
    let set = run(
        &mut doc,
        &mut ids,
        "combine_as_variants",
        json!({ "nodes": [a.to_wire(), b.to_wire()] }),
    )[0];
    let new = run(
        &mut doc,
        &mut ids,
        "add_variant",
        json!({ "node": set.to_wire() }),
    )[0];
    assert!(doc.get(new).unwrap().component());
    assert_eq!(doc.get(set).unwrap().children().len(), 3);
    let (lone, _) = main_component(&mut doc, &mut ids, page, "Lone", 50.0);
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "add_variant",
            json!({ "node": lone.to_wire() })
        ),
        "node",
        "a main in no set"
    );

    let copy = run(
        &mut doc,
        &mut ids,
        "duplicate_as_component",
        json!({ "node": lone.to_wire(), "dx": 80 }),
    )[0];
    let node = doc.get(copy).unwrap();
    assert!(
        node.component() && node.link().is_none(),
        "a main, not an instance"
    );
    assert_ne!(copy, lone);
}

/// `import_svg` reads markup into one group placed at `x`/`y`, named as asked,
/// says what it left out, and refuses markup that draws nothing.
///
/// flip: `import_svg` leaving the root's transform alone → fails at "placed".
#[test]
fn import_svg_reads_markup_into_a_placed_group() {
    let (mut doc, mut ids, page) = blank();
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/><blink/></svg>"#;
    let args = json!({ "parent": page.to_wire(), "svg": svg, "x": 100, "y": 50, "name": "Icon" });
    let done = answer(&doc, &mut ids, "import_svg", args.clone()).unwrap();
    assert!(
        done.said.as_deref().is_some_and(|s| s.contains("blink")),
        "{:?}",
        done.said
    );
    let g = run(&mut doc, &mut ids, "import_svg", args)[0];
    assert_eq!(doc.get(g).unwrap().name(), "Icon");
    let r = Resolved::rebuild(&doc).world_bounds(g).unwrap();
    assert_eq!((r.x0, r.y0), (100.0, 50.0), "placed");

    let empty = answer(
        &doc,
        &mut ids,
        "import_svg",
        json!({ "parent": page.to_wire(), "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>" }),
    );
    assert!(matches!(empty, Err(VerbError::Nothing(_))), "{empty:?}");
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "import_svg",
            json!({ "parent": page.to_wire(), "svg": "<html/>" })
        ),
        "svg"
    );
}

/// `replace_text` rewrites every match in scope, keeps a styled run's styling,
/// leaves a locked layer alone and says so.
///
/// flip: `replace_text` writing through `set_text`'s empty runs → fails at "the
/// run survives".
#[test]
fn replace_text_rewrites_matches_and_keeps_their_styling() {
    let (mut doc, mut ids, page) = blank();
    let p = page.to_wire();
    let a = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": p, "text": "Sign in to Acme" }),
    )[0];
    let b = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": p, "text": "Acme rules" }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "style_text",
        json!({ "node": a.to_wire(), "attrs": [{ "Weight": 700 }], "start": 11, "end": 15 }),
    );
    let spans = |doc: &Document| match doc.get(a).unwrap().kind() {
        NodeKind::Text { spans, .. } => spans.as_slice().len(),
        _ => 0,
    };
    assert_eq!(spans(&doc), 1, "fixture: the name is bold");
    run(
        &mut doc,
        &mut ids,
        "set_locked",
        json!({ "nodes": [b.to_wire()], "locked": true }),
    );
    let done = answer(
        &doc,
        &mut ids,
        "replace_text",
        json!({ "find": "acme", "replace": "Ondin" }),
    )
    .unwrap();
    assert_eq!(
        done.said.as_deref(),
        Some("Replaced 1 match in 1 layer; 1 locked or in an instance left alone")
    );
    run(
        &mut doc,
        &mut ids,
        "replace_text",
        json!({ "find": "acme", "replace": "Ondin" }),
    );
    let text = |id: NodeId, doc: &Document| match doc.get(id).unwrap().kind() {
        NodeKind::Text { content, .. } => content.clone(),
        _ => String::new(),
    };
    assert_eq!(text(a, &doc), "Sign in to Ondin");
    assert_eq!(text(b, &doc), "Acme rules", "the locked layer");
    assert_eq!(spans(&doc), 1, "the run survives");
    let none = answer(
        &doc,
        &mut ids,
        "replace_text",
        json!({ "find": "zebra", "replace": "x" }),
    );
    assert!(matches!(none, Err(VerbError::Nothing(_))), "{none:?}");
}

/// `merge_colours` repaints a near-identical colour as the one named, and
/// refuses an `into` no paint uses.
///
/// flip: `merge_colours` merging into the other member rather than `into` →
/// fails at the last assertion: both still match, in the wrong colour.
#[test]
fn merge_colours_repaints_near_identical_colours() {
    let (mut doc, mut ids, page) = blank();
    let (a, b) = two_rects(&mut doc, &mut ids, page, 60.0, 0.0);
    let fill = |r, g, b| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(Color::from_rgb8(r, g, b)).into(),
            visible: true,
        }])
        .unwrap()
    };
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [a.to_wire()], "fills": fill(0x1f, 0x3a, 0x5f) }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [b.to_wire()], "fills": fill(0x20, 0x3b, 0x5e) }),
    );
    let (into, other) = ("#1f3a5fff", "#203b5e");
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "merge_colours",
            json!({ "colours": [into], "into": "#010203" })
        ),
        "into",
        "no paint is this colour"
    );
    assert_eq!(
        refused_param(
            &doc,
            &mut ids,
            "merge_colours",
            json!({ "colours": ["teal-ish"], "into": into })
        ),
        "colours"
    );
    run(
        &mut doc,
        &mut ids,
        "merge_colours",
        json!({ "colours": [other], "into": into }),
    );
    let solid = |id: NodeId| match &doc.get(id).unwrap().paint().fills[0].brush.literal() {
        Some(Brush::Solid(c)) => c.to_rgba8().to_u8_array(),
        _ => [0; 4],
    };
    assert_eq!(solid(a), solid(b), "both take `into`");
    assert_eq!(solid(b), [0x1f, 0x3a, 0x5f, 0xff]);
}

/// `merge_colours` into a colour a variable carries binds the raw paints to that
/// variable, as Colour review's default target does.
///
/// flip: `merge_colours` matching `into` on raw paints alone → fails at the
/// refusal, as no raw paint is that colour.
#[test]
fn merge_colours_into_a_bound_colour_binds_the_raw_paints() {
    let (mut doc, mut ids, _) = blank();
    let blue =
        serde_json::to_value(ondin_core::Cell::Color(Color::from_rgb8(0x33, 0x66, 0xcc))).unwrap();
    let fill = |r, g, b| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(Color::from_rgb8(r, g, b)).into(),
            visible: true,
        }])
        .unwrap()
    };
    let script = json!([
        { "verb": "add_variable", "args": { "kind": "color", "name": "brand/blue", "value": blue }, "as": "c" },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "x" },
        { "verb": "set_fills", "args": { "nodes": ["@x"], "fills": fill(0x33, 0x66, 0xcc) } },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": { "fill": 0 }, "variable": "@c" } },
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(40.0, 40.0) }, "as": "y" },
        { "verb": "set_fills", "args": { "nodes": ["@y"], "fills": fill(0x34, 0x67, 0xcb) } },
        { "verb": "merge_colours", "args": { "colours": ["#3467cbff"], "into": "#3366ccff" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let y = NodeId::from_wire(&made[4][0]).unwrap();
    assert!(
        doc.get(y).unwrap().paint().fills[0].brush.var().is_some(),
        "the raw paint is bound to the variable"
    );
}

/// `set_fills` over several layers writes as the Fill card writes a selection:
/// layers sharing a list keep their own item ids for its rows and share one id
/// for a row added, a group's shapes take it, and differing lists are replaced
/// position by position; an empty group, which takes no paint, is refused.
/// `set_exports` shares one id for a row added over a selection. The text setters
/// take text layers only.
///
/// flip: `set_fills` writing the anchor's ids onto every layer rather than
/// `retarget_fills_all` → fails at "b keeps its own id"; `set_fills` without
/// `painted` → fails at "an empty group takes no paint"; `set_exports` with a
/// `PastTheEnd` per layer → fails at "one id for the added row".
#[test]
fn set_fills_writes_a_selection_as_the_inspector_does() {
    let (mut doc, mut ids, page) = blank();
    let solid = |c: Color| Fill {
        brush: Brush::Solid(c).into(),
        visible: true,
    };
    let fills = |list: &[Fill]| serde_json::to_value(list).unwrap();
    let make = |doc: &mut Document, ids: &mut IdSource| {
        run(
            doc,
            ids,
            "create_node",
            json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
        )[0]
    };
    let (a, b) = (make(&mut doc, &mut ids), make(&mut doc, &mut ids));
    for id in [a, b] {
        run(
            &mut doc,
            &mut ids,
            "set_fills",
            json!({ "nodes": [id.to_wire()], "fills": fills(&[solid(Color::WHITE)]) }),
        );
    }
    let item_ids = |doc: &Document, id: NodeId| -> Vec<_> {
        doc.get(id)
            .unwrap()
            .paint()
            .fills
            .iter()
            .map(|f| f.id)
            .collect()
    };
    let (was_a, was_b) = (item_ids(&doc, a), item_ids(&doc, b));
    assert_ne!(was_a, was_b, "fills added apart have ids of their own");
    let two = fills(&[solid(Color::BLACK), solid(Color::WHITE)]);
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [a.to_wire(), b.to_wire()], "fills": two }),
    );
    let (now_a, now_b) = (item_ids(&doc, a), item_ids(&doc, b));
    assert_eq!(now_a[0], was_a[0], "a keeps its own id");
    assert_eq!(now_b[0], was_b[0], "b keeps its own id");
    assert_eq!(now_a[1], now_b[1], "the added row shares one id");
    assert_eq!(doc.get(b).unwrap().paint().fills.len(), 2);

    let g = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [a.to_wire()] }),
    )[0];
    let red = fills(&[solid(Color::from_rgb8(200, 0, 0))]);
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [g.to_wire(), b.to_wire()], "fills": red }),
    );
    for (id, own) in [(a, now_a[0]), (b, now_b[0])] {
        let f = &doc.get(id).unwrap().paint().fills;
        assert_eq!(f.len(), 1, "a group's shape takes it too");
        assert_eq!(f[0].id, own, "the first row keeps each layer's id");
    }

    let empty = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": "Group" } }),
    )[0];
    let args = json!({ "nodes": [empty.to_wire()], "fills": fills(&[solid(Color::WHITE)]) });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_fills", args),
        "nodes",
        "an empty group takes no paint"
    );

    // A row added over a selection shares one id, as the Export card adds it.
    let spec = verbs::example("[ExportSpec]").unwrap();
    run(
        &mut doc,
        &mut ids,
        "set_exports",
        json!({ "nodes": [a.to_wire(), b.to_wire()], "exports": spec }),
    );
    let export_id = |id: NodeId| doc.get(id).unwrap().exports()[0].id;
    assert_eq!(export_id(a), export_id(b), "one id for the added row");

    let args = json!({ "nodes": [b.to_wire()], "style": serde_json::to_value(ondin_core::TextStyle::default()).unwrap() });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_text_style", args),
        "nodes"
    );
}

/// `verbs::tree` reads a layer back in `insert_subtree`'s shape with what the
/// snapshot leaves out — a hidden fill, the strokes, effects and exports whole, a
/// text's runs, a mask's mode — and a bound width as drawn, so inserting what it
/// read makes a copy drawn the same that reads back the same.
///
/// flip: `tree` keeping visible fills only → fails at "the hidden fill is read";
/// `tree` writing the stored kind → fails at "the bound width as drawn".
#[test]
fn tree_reads_a_layer_back_as_insert_subtree_takes_it() {
    use ondin_core::typography::CharAttr;
    let (mut doc, mut ids, page) = blank();
    let f = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(200.0, 100.0), "name": "Card" }),
    )[0];
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": f.to_wire(), "text": "Hello world" }),
    )[0];
    let bold = serde_json::to_value(vec![CharAttr::Weight(700)]).unwrap();
    run(
        &mut doc,
        &mut ids,
        "style_text",
        json!({ "node": t.to_wire(), "attrs": bold, "start": 0, "end": 5 }),
    );
    let fills = serde_json::to_value(vec![
        Fill {
            brush: Brush::Solid(Color::WHITE).into(),
            visible: true,
        },
        Fill {
            brush: Brush::Solid(Color::BLACK).into(),
            visible: false,
        },
    ])
    .unwrap();
    let steps = json!([
        { "verb": "set_fills", "args": { "nodes": [f.to_wire()], "fills": fills } },
        { "verb": "set_strokes", "args": { "nodes": [f.to_wire()], "strokes": verbs::example("[Stroke]").unwrap() } },
        { "verb": "set_effects", "args": { "nodes": [f.to_wire()], "effects": verbs::example("[Effect]").unwrap() } },
        { "verb": "set_exports", "args": { "nodes": [f.to_wire()], "exports": verbs::example("[ExportSpec]").unwrap() } },
        { "verb": "create_node", "args": { "parent": f.to_wire(), "kind": rect(10.0, 10.0) }, "as": "m" },
        { "verb": "set_mask", "args": { "nodes": ["@m"], "mask": true, "mode": "luminance" } },
        { "verb": "add_variable", "args": { "kind": "number", "name": "w", "value": { "number": 240.0 } }, "as": "w" },
        { "verb": "bind_variable", "args": { "nodes": [f.to_wire()], "field": { "size": 0 }, "variable": "@w" } },
    ]);
    script(&mut doc, &mut ids, steps);
    let read = verbs::tree(&doc, f).expect("a layer");
    assert_eq!(
        read["kind"]["Artboard"]["size"]["width"],
        json!(240.0),
        "the bound width as drawn"
    );
    assert_eq!(
        read["fills"].as_array().map(Vec::len),
        Some(2),
        "the hidden fill is read"
    );
    assert!(read["children"][0]["kind"]["Text"].is_object(), "{read}");
    let copy = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": read }),
    )[0];
    assert_eq!(
        verbs::tree(&doc, copy),
        Some(read),
        "the copy reads back the same"
    );
    let NodeKind::Text { spans, .. } = doc
        .get(doc.get(copy).unwrap().children()[0])
        .unwrap()
        .kind()
    else {
        unreachable!()
    };
    assert!(!spans.as_slice().is_empty(), "the runs came across");
}

/// The full JSON of a solid colour, as the model writes it.
fn solid(c: Color) -> Value {
    serde_json::to_value(Brush::Solid(c)).unwrap()
}

/// A tree written in the short forms (hex colours, a bare-colour fill, a stroke
/// and a text style without their default fields) makes the layers its full form
/// makes.
///
/// flip: `loosen` leaving a bare-colour fill as it is → fails at the short tree's
/// `insert_subtree` panic.
#[test]
fn short_forms_make_what_full_forms_make() {
    let (mut doc, mut ids, page) = blank();
    let shadow = |color: Value| {
        json!([{ "visible": true, "kind": { "type": "drop_shadow", "color": color,
            "offset": { "x": 0.0, "y": 4.0 }, "blur": 8.0, "spread": 0.0 } }])
    };
    let short = json!({
        "kind": rect(40.0, 20.0),
        "name": "Box",
        "fills": ["#12352a"],
        "strokes": [{ "brush": "#ff7a1a80", "width": 2.0 }],
        "effects": shadow(json!("#00000033")),
        "children": [{ "kind": { "Text": { "content": "Hi", "sizing": "Auto",
            "style": { "font_size": 20.0, "color": "#fff" } } } }],
    });
    let c = |r: u8, g: u8, b: u8, a: u8| Color::from_rgba8(r, g, b, a);
    let full = json!({
        "kind": rect(40.0, 20.0),
        "name": "Box",
        "fills": [{ "brush": solid(c(0x12, 0x35, 0x2a, 255)), "visible": true }],
        "strokes": [{ "brush": solid(c(0xff, 0x7a, 0x1a, 0x80)), "width": 2.0, "visible": true,
            "join": "Miter", "cap": "Butt", "dashes": [], "dash_offset": 0.0 }],
        "effects": shadow(serde_json::to_value(c(0, 0, 0, 0x33)).unwrap()),
        "children": [{ "kind": { "Text": { "content": "Hi", "sizing": "Auto",
            "style": { "font_family": "Inter", "font_size": 20.0, "weight": 400, "italic": false,
                "color": serde_json::to_value(c(255, 255, 255, 255)).unwrap() } } } }],
    });
    let insert = |doc: &mut Document, ids: &mut IdSource, tree: &Value| {
        run(
            doc,
            ids,
            "insert_subtree",
            json!({ "parent": page.to_wire(), "tree": tree }),
        )[0]
    };
    let a = insert(&mut doc, &mut ids, &short);
    let b = insert(&mut doc, &mut ids, &full);
    let read = verbs::tree(&doc, a).unwrap();
    assert_eq!(
        Some(&read),
        verbs::tree(&doc, b).as_ref(),
        "the same layers"
    );
    assert_eq!(read["fills"], json!(["#12352a"]), "read back short: {read}");
    assert_eq!(
        read["strokes"],
        json!([{ "brush": "#ff7a1a80", "width": 2.0, "align": "Center", "miter_limit": 4.0 }]),
        "{read}"
    );
}

/// `tree` writes a colour between two bytes as its nearest hex, and the copy
/// that hex makes reads back as the same tree.
///
/// flip: `nearest_hex` refusing a colour that does not read back exactly → fails
/// at "the nearest byte".
#[test]
fn tree_writes_a_colour_as_its_nearest_hex() {
    let (mut doc, mut ids, page) = blank();
    let between = Color::new([0.123_456_7, 0.5, 0.25, 1.0]);
    let fills = |c: Color| {
        serde_json::to_value(vec![Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }])
        .unwrap()
    };
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_fills",
        json!({ "nodes": [r.to_wire()], "fills": fills(between) }),
    );
    let read = verbs::tree(&doc, r).unwrap();
    assert_eq!(read["fills"][0], json!("#1f8040"), "the nearest byte");
    let copy = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": read }),
    )[0];
    assert_eq!(
        verbs::tree(&doc, copy),
        Some(read),
        "and reads back the same"
    );
}

/// `like` gives a layer another's text style field by field, under what it
/// writes itself, and its fills.
///
/// flip: `expand_like` skipping the text style → fails at "the size comes from
/// the like".
#[test]
fn like_takes_another_layers_text_style_and_paint() {
    let (mut doc, mut ids, page) = blank();
    let src = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "fills": ["#12352a"],
            "kind": { "Text": { "content": "Title", "sizing": "Auto",
            "style": { "font_size": 48.0, "weight": 800, "letter_spacing": { "Px": -1.4 } } } } } }),
    )[0];
    let copy = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "like": src.to_wire(),
            "kind": { "Text": { "content": "Another", "sizing": "Auto",
            "style": { "weight": 600 } } } } }),
    )[0];
    let read = verbs::tree(&doc, copy).unwrap();
    let style = &read["kind"]["Text"]["style"];
    assert_eq!(
        style["font_size"],
        json!(48.0),
        "the size comes from the like"
    );
    assert_eq!(style["weight"], json!(600), "its own weight wins");
    assert_eq!(style["letter_spacing"], json!("-1.4px"), "{read}");
    assert_eq!(read["fills"], json!(["#12352a"]), "and the fills");
    assert!(
        refused_param(
            &doc,
            &mut ids,
            "insert_subtree",
            json!({ "parent": page.to_wire(), "tree": { "like": "zz", "kind": rect(1.0, 1.0) } }),
        ) == "tree"
    );
}

/// A text that gives its own fills does not take its `like`'s style colour, which
/// would draw over them; one that gives none takes it.
///
/// flip: `expand_like` copying the style's `color` whatever the layer gives →
/// fails at "its own fills are its colour".
#[test]
fn like_leaves_the_style_colour_to_a_text_with_its_own_fills() {
    let (mut doc, mut ids, page) = blank();
    let src = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": { "Text": { "content": "On dark",
            "sizing": "Auto", "style": { "font_size": 16.0, "color": "#ffffff" } } } } }),
    )[0];
    let text = |fills: Option<Value>| {
        let mut t = json!({ "like": src.to_wire(),
            "kind": { "Text": { "content": "Copy", "sizing": "Auto" } } });
        if let Some(f) = fills {
            t["fills"] = f;
        }
        json!({ "parent": page.to_wire(), "tree": t })
    };
    let own = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        text(Some(json!(["#12352a"]))),
    )[0];
    let read = verbs::tree(&doc, own).unwrap();
    let style = &read["kind"]["Text"]["style"];
    assert_eq!(style["font_size"], json!(16.0), "the like still reached it");
    assert!(
        style.get("color").is_none(),
        "its own fills are its colour: {read}"
    );
    assert_eq!(read["fills"], json!(["#12352a"]));
    let bare = run(&mut doc, &mut ids, "insert_subtree", text(None))[0];
    let read = verbs::tree(&doc, bare).unwrap();
    assert_eq!(
        read["kind"]["Text"]["style"]["color"],
        json!("#ffffff"),
        "{read}"
    );
}

/// `like` refuses a sample with nothing to give, naming what it is: a text
/// that names a group, and a shape that names a group with no paint. A filled
/// shape gives a text nothing either, since it has no text style.
///
/// flip: `expand_like` without its text check → fails at "a group gives a text
/// nothing", refused for its paint instead of for being no text.
#[test]
fn like_refuses_a_sample_with_nothing_to_give() {
    let (mut doc, mut ids, page) = blank();
    let filled = run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "kind": rect(10.0, 10.0), "fills": ["#ff7a1a"] } }),
    )[0];
    let group = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [filled.to_wire()] }),
    )[0];
    let text = |like: NodeId| {
        json!({ "parent": page.to_wire(), "tree": { "like": like.to_wire(),
            "kind": { "Text": { "content": "Hi", "sizing": "Auto" } } } })
    };
    let mut why = |args: Value| match answer(&doc, &mut ids, "insert_subtree", args) {
        Err(VerbError::Invalid { why, .. }) => why,
        other => panic!("{other:?}"),
    };
    let said = why(text(group));
    assert!(
        said.contains("is a group, not a text"),
        "a group gives a text nothing: {said}"
    );
    assert!(why(text(filled)).contains("is a rectangle, not a text"));
    let said = why(
        json!({ "parent": page.to_wire(), "tree": { "like": group.to_wire(),
        "kind": rect(10.0, 10.0) } }),
    );
    assert!(said.contains("no fill, stroke or effect"), "{said}");
    run(
        &mut doc,
        &mut ids,
        "insert_subtree",
        json!({ "parent": page.to_wire(), "tree": { "like": filled.to_wire(), "kind": rect(5.0, 5.0) } }),
    );
}

/// `set_mask` sets a mask's mode with it, and refuses a mode for a layer that
/// stops being one.
///
/// flip: `set_mask` dropping its `SetMaskMode` → fails at "alpha".
#[test]
fn set_mask_sets_the_mode_of_a_mask() {
    let (mut doc, mut ids, page) = blank();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_mask",
        json!({ "nodes": [r.to_wire()], "mask": true, "mode": "alpha" }),
    );
    let n = doc.get(r).unwrap();
    assert!(n.mask());
    assert_eq!(n.mask_mode(), ondin_core::MaskMode::Alpha, "alpha");
    let args = json!({ "nodes": [r.to_wire()], "mask": false, "mode": "shape" });
    assert_eq!(refused_param(&doc, &mut ids, "set_mask", args), "mode");
}

/// A frame is written as an element its kind offers and, as a link, takes an
/// address; a shape, an element a text takes, a link address on a layer that is
/// no link and an address a link cannot take are refused.
///
/// flip: `set_link` without its element check → fails at "no link yet".
#[test]
fn set_element_and_set_link_write_what_code_export_reads() {
    let (mut doc, mut ids, page) = blank();
    let f = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(100.0, 40.0) }),
    )[0];
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": rect(10.0, 10.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "set_element",
        json!({ "node": f.to_wire(), "element": "nav" }),
    );
    assert_eq!(
        doc.get(f).unwrap().element(),
        Some(ondin_core::element::HtmlElement::Nav)
    );
    for (node, element, param) in [(f, "h1", "element"), (r, "nav", "node")] {
        let args = json!({ "node": node.to_wire(), "element": element });
        assert_eq!(refused_param(&doc, &mut ids, "set_element", args), param);
    }
    let link = |href: &str| json!({ "node": f.to_wire(), "href": href });
    assert_eq!(
        refused_param(&doc, &mut ids, "set_link", link("/about")),
        "node",
        "no link yet"
    );
    run(
        &mut doc,
        &mut ids,
        "set_element",
        json!({ "node": f.to_wire(), "element": "a" }),
    );
    run(&mut doc, &mut ids, "set_link", link(" /about "));
    assert_eq!(doc.get(f).unwrap().href(), "/about");
    assert_eq!(
        refused_param(&doc, &mut ids, "set_link", link("javascript:alert(1)")),
        "href"
    );
    run(
        &mut doc,
        &mut ids,
        "set_element",
        json!({ "node": f.to_wire() }),
    );
    assert_eq!(doc.get(f).unwrap().element(), None, "back to Automatic");
}

/// A main takes a description and a documentation link, a bare host made an
/// https address; a style takes a description; a plain frame, two subjects and a
/// file address are refused.
///
/// flip: `set_doc_link` writing the address as typed → fails at the commit, which
/// refuses a link not in `web_link`'s form.
#[test]
fn descriptions_and_doc_links_are_set() {
    let (mut doc, mut ids, page) = blank();
    let (card, inside) = main_component(&mut doc, &mut ids, page, "Card", 100.0);
    run(
        &mut doc,
        &mut ids,
        "set_description",
        json!({ "node": card.to_wire(), "text": " A card. " }),
    );
    assert_eq!(doc.get(card).unwrap().description(), "A card.");
    run(
        &mut doc,
        &mut ids,
        "set_doc_link",
        json!({ "node": card.to_wire(), "url": "example.com/card" }),
    );
    assert_eq!(
        doc.get(card).unwrap().doc_link(),
        "https://example.com/card"
    );
    let fills = serde_json::to_value(vec![Fill {
        brush: Brush::Solid(Color::WHITE).into(),
        visible: true,
    }])
    .unwrap();
    let made = script(
        &mut doc,
        &mut ids,
        json!([
            { "verb": "add_style", "args": { "value": { "fills": fills } }, "as": "s" },
            { "verb": "set_description", "args": { "style": "@s", "text": "Paper" } },
        ]),
    );
    let style: ondin_core::style::StyleId = serde_json::from_value(json!(made[0][0])).unwrap();
    assert_eq!(doc.styles().get(style).unwrap().description, "Paper");
    for (args, param, verb) in [
        (
            json!({ "node": inside.to_wire(), "text": "x" }),
            "node",
            "set_description",
        ),
        (
            json!({ "node": card.to_wire(), "style": made[0][0], "text": "x" }),
            "node",
            "set_description",
        ),
        (
            json!({ "node": card.to_wire(), "url": "file:///x" }),
            "url",
            "set_doc_link",
        ),
    ] {
        assert_eq!(refused_param(&doc, &mut ids, verb, args), param, "{verb}");
    }
}

/// A group becomes a frame and back, each answering what it became; an empty
/// frame is refused. A frame fits its content once, then says it already fits.
///
/// flip: `fit_to_content` answering nothing to do where `fit_frame` declines →
/// fails at "already fits".
#[test]
fn containers_convert_and_fit_their_content() {
    let (mut doc, mut ids, page) = blank();
    let r = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({
            "parent": page.to_wire(),
            "kind": rect(10.0, 10.0),
            "transform": Affine::translate((20.0, 30.0)),
        }),
    )[0];
    let g = run(
        &mut doc,
        &mut ids,
        "group",
        json!({ "nodes": [r.to_wire()] }),
    )[0];
    let f = run(
        &mut doc,
        &mut ids,
        "convert_container",
        json!({ "node": g.to_wire() }),
    )[0];
    assert!(matches!(
        doc.get(f).unwrap().kind(),
        NodeKind::Artboard { .. }
    ));
    assert_eq!(doc.get(r).unwrap().parent(), Some(f), "its layers move in");
    let back = run(
        &mut doc,
        &mut ids,
        "convert_container",
        json!({ "node": f.to_wire() }),
    )[0];
    assert!(matches!(doc.get(back).unwrap().kind(), NodeKind::Group));
    let empty = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(50.0, 50.0) }),
    )[0];
    let args = json!({ "node": empty.to_wire() });
    assert_eq!(
        refused_param(&doc, &mut ids, "convert_container", args),
        "node"
    );

    let card = run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({ "parent": page.to_wire(), "kind": frame(300.0, 200.0) }),
    )[0];
    run(
        &mut doc,
        &mut ids,
        "create_node",
        json!({
            "parent": card.to_wire(),
            "kind": rect(40.0, 24.0),
            "transform": Affine::translate((16.0, 16.0)),
        }),
    );
    run(
        &mut doc,
        &mut ids,
        "fit_to_content",
        json!({ "node": card.to_wire() }),
    );
    let NodeKind::Artboard { size, .. } = doc.get(card).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!((size.width, size.height), (40.0, 24.0));
    let again = answer(
        &doc,
        &mut ids,
        "fit_to_content",
        json!({ "node": card.to_wire() }),
    );
    assert!(
        matches!(&again, Err(VerbError::Invalid { why, .. }) if why.contains("already fits")),
        "already fits: {again:?}"
    );
}

/// `replace_font` rewrites a layer's family, and refuses a family nothing sets
/// and a family replaced by itself, which would drop its axis settings.
///
/// flip: `replace_font` swapping `from` and `to` → fails at the first call, refused
/// as nothing is set in the new family yet; without its same-family check → fails
/// at "a family is not replaced by itself".
#[test]
fn replace_font_rewrites_a_family_everywhere() {
    let (mut doc, mut ids, page) = blank();
    let t = run(
        &mut doc,
        &mut ids,
        "create_text",
        json!({ "parent": page.to_wire(), "text": "Hi" }),
    )[0];
    let family = |doc: &Document| match doc.get(t).unwrap().kind() {
        NodeKind::Text { style, .. } => style.font_family.clone(),
        _ => unreachable!(),
    };
    let from = family(&doc);
    run(
        &mut doc,
        &mut ids,
        "replace_font",
        json!({ "from": from.to_uppercase(), "to": "Test Serif" }),
    );
    assert_eq!(family(&doc), "Test Serif");
    let gone = answer(
        &doc,
        &mut ids,
        "replace_font",
        json!({ "from": "Nowhere Sans", "to": "Test Serif" }),
    );
    assert!(matches!(gone, Err(VerbError::Nothing(_))), "{gone:?}");
    let args = json!({ "from": "test serif", "to": "Test Serif" });
    assert_eq!(
        refused_param(&doc, &mut ids, "replace_font", args),
        "to",
        "a family is not replaced by itself"
    );
}
