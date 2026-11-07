//! The verb registry (`verbs`): every verb is well formed, arguments are checked
//! against its parameters before it runs, and a run of verbs edits a document as
//! the canvas would.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::svg_in::ImageSink;
use ondin_core::verbs::{self, Ctx, Done, ParamKind, VerbError};
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
        images: None,
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
            images: None,
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
        },
        "insert_subtree",
        &json!({ "parent": page.to_wire(), "tree": { "kind": rect(1.0, 1.0), "colour": "red" } }),
    );
    assert!(
        matches!(refused, Err(VerbError::Invalid { param: "tree", .. })),
        "{refused:?}"
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
        },
        "style_text",
        &json!({ "node": w, "attrs": [], "end": 99 }),
    );
    assert!(
        matches!(past, Err(VerbError::Invalid { param: "end", .. })),
        "{past:?}"
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
        },
        "delete_page",
        &json!({ "page": first.to_wire() }),
    );
    assert!(matches!(last, Err(VerbError::Nothing(_))), "{last:?}");
}

/// The component verbs: a rectangle wrapped and made a main, an instance placed
/// centred on a point that follows its main's edit, swapped to another main, and
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
        res.world_bounds(inst).unwrap().center(),
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
        json!({ "node": inner.to_wire(), "fills": red }),
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
        },
        "make_component",
        &json!({ "nodes": [card.to_wire()] }),
    );
    assert!(
        matches!(again, Err(VerbError::Nothing(why)) if why.contains("already")),
        "{again:?}"
    );
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
        { "verb": "set_fills", "args": { "node": "@r", "fills": red } },
    ]);
    let made =
        verbs::run_script(&mut doc, &mut ids, None, &script).unwrap_or_else(|e| panic!("{e}"));
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
    let made =
        verbs::run_script(&mut doc, &mut ids, None, &script).unwrap_or_else(|e| panic!("{e}"));
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
        verbs::run_script(doc, ids, None, &json!([step]))
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
    verbs::run_script(&mut doc, &mut ids, None, &tidy).unwrap_or_else(|e| panic!("{e}"));
    assert!(doc.variables().get(var).is_none());
    assert!(doc.variables().mode(dark).is_none());
    assert!(
        doc.get(f).unwrap().modes().is_empty(),
        "the frame inherits again"
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
        { "verb": "set_fills", "args": { "node": "@x", "fills": fill } },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": { "fill": 0 }, "variable": "@c" } },
        { "verb": "bind_variable", "args": { "nodes": ["@x"], "field": "radius", "variable": "@n" } },
    ]);
    let made =
        verbs::run_script(&mut doc, &mut ids, None, &script).unwrap_or_else(|e| panic!("{e}"));
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
    verbs::run_script(&mut doc, &mut ids, None, &detach).unwrap_or_else(|e| panic!("{e}"));
    let node = doc.get(x).unwrap();
    assert!(node.corner_vars().iter().all(Option::is_none));
    let NodeKind::Rect { corner_radii, .. } = node.kind() else {
        unreachable!()
    };
    assert_eq!(corner_radii.top_left, 8.0, "detached to what it drew");

    let past = json!([{ "verb": "bind_variable", "args": { "nodes": [x.to_wire()], "field": { "corner": 4 } } }]);
    let err = verbs::run_script(&mut doc, &mut ids, None, &past).expect_err("no fifth corner");
    assert!(err.why.contains("no such place"), "{err}");
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
    let made =
        verbs::run_script(&mut doc, &mut ids, None, &script).unwrap_or_else(|e| panic!("{e}"));
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
    let made = verbs::run_script(&mut doc, &mut ids, None, &more).unwrap_or_else(|e| panic!("{e}"));
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
        None,
        &json!([{ "verb": "add_style", "args": { "value": { "fills": fills(1) }, "name": "Accent" } }]),
    )
    .expect_err("a taken name");
    assert!(taken.why.contains("already exists"), "{taken}");

    verbs::run_script(
        &mut doc,
        &mut ids,
        None,
        &json!([{ "verb": "remove_style", "args": { "style": s } }]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(doc.styles().get(style).is_none());
    assert_eq!(colour(&doc, a), 200, "a keeps the look it drew");
    assert!(doc.get(page).is_some());
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
        },
        "set_instance_property",
        &json!({ "node": inst.to_wire(), "property": "Badge", "value": "yes" }),
    );
    assert!(
        matches!(wrong, Err(VerbError::Invalid { param: "value", .. })),
        "{wrong:?}"
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
        json!({ "node": copy.to_wire(), "fills": red }),
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
        },
        "reset_overrides",
        &json!({ "nodes": [card.to_wire()] }),
    );
    assert!(
        matches!(loose, Err(VerbError::Nothing(_))),
        "a loose layer is refused: {loose:?}"
    );
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
        { "verb": "set_fills", "args": { "node": "@r", "fills": [fill] } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Some(&mut Pictures), &script)
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
    let made = verbs::run_script(&mut doc, &mut ids, Some(&mut Pictures), &again)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(made[0], std::slice::from_ref(&id.0));
    assert_eq!(doc.images().count(), 1);

    let err = verbs::run_script(&mut doc, &mut ids, None, &again).expect_err("no sink");
    assert!(err.why.contains("cannot read pictures"), "{err}");
    let text = base64::engine::general_purpose::STANDARD.encode(b"hello");
    let err = verbs::run_script(
        &mut doc,
        &mut ids,
        Some(&mut Pictures),
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
        { "verb": "set_fills", "args": { "node": "@r", "fills": [fill] } },
        { "verb": "replace_image", "args": { "node": "@r", "image": "@b" } },
    ]);
    let made = verbs::run_script(&mut doc, &mut ids, Some(&mut Pictures), &script)
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
    let err = verbs::run_script(&mut doc, &mut ids, None, &missing).expect_err("no such picture");
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
    let made =
        verbs::run_script(&mut doc, &mut ids, None, &script).unwrap_or_else(|e| panic!("{e}"));
    let pair = NodeId::from_wire(&made[2][0]).expect("a node's wire form");
    assert_eq!(doc.get(pair).unwrap().name(), "Pair");
    assert_eq!(doc.get(page).unwrap().children(), [pair]);

    // Labels belong to one script: this one knows `page` and its own, not `pair`.
    let broken = json!([
        { "verb": "create_node", "args": { "parent": "@page", "kind": rect(1.0, 1.0) }, "as": "c" },
        { "verb": "set_name", "args": { "node": "@pair", "name": "x" } },
    ]);
    let err = verbs::run_script(&mut doc, &mut ids, None, &broken).expect_err("an unknown label");
    assert_eq!(err.step, 2, "{err}");
    assert!(err.why.contains("pair"), "{err}");
    assert_eq!(
        doc.get(page).unwrap().children().len(),
        2,
        "the step before the refusal stands"
    );
}
