//! The verb registry (`verbs`): every verb is well formed, arguments are checked
//! against its parameters before it runs, and a run of verbs edits a document as
//! the canvas would.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::verbs::{self, Ctx, Done, ParamKind, VerbError};
use ondin_core::{
    Brush, Document, Fill, Guide, GuideAxis, GuideId, IdSource, NodeId, NodeKind, Operation,
    Resolved, Transaction,
};
use serde_json::{Value, json};

/// A document with one page and nothing on it, the id source that made it, and the page.
fn blank() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7E5B);
    let page = ids.mint();
    (Document::new(page), ids, page)
}

/// Run `name` with `args` on `doc` and commit its edit; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Vec<NodeId> {
    let res = Resolved::rebuild(doc);
    let Done { tx, created } = verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
        },
        name,
        &args,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    doc.apply(&tx)
        .unwrap_or_else(|e| panic!("{name} commits: {e}"));
    created
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
        json!({ "node": a.to_wire(), "fills": [serde_json::to_value(&red).unwrap()] }),
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
        json!({ "node": a.to_wire(), "fills": fills }),
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
    let made = verbs::run_script(&mut doc, &mut ids, &script).unwrap_or_else(|e| panic!("{e}"));
    let pair = made[2][0];
    assert_eq!(doc.get(pair).unwrap().name(), "Pair");
    assert_eq!(doc.get(page).unwrap().children(), [pair]);

    // Labels belong to one script: this one knows `page` and its own, not `pair`.
    let broken = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(1.0, 1.0) }, "as": "c" },
        { "verb": "set_name", "args": { "node": "@pair", "name": "x" } },
    ]);
    let err = verbs::run_script(&mut doc, &mut ids, &broken).expect_err("an unknown label");
    assert_eq!(err.step, 2, "{err}");
    assert!(err.why.contains("pair"), "{err}");
    assert_eq!(
        doc.get(page).unwrap().children().len(),
        2,
        "the step before the refusal stands"
    );
}
