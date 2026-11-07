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

/// Run `name` with `args` on `doc` and commit its edit; answers what it created.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Vec<NodeId> {
    let res = Resolved::rebuild(doc);
    let Done { tx, created, .. } = verbs::call(
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
