//! A main's edits reaching its instances (§5.3d build step 3,
//! `ondin_core::propagate::propagate`): a copy follows wherever it still holds the
//! main's old value and keeps its own everywhere else.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, IdSource, Keyed, NodeId, NodeKind, Operation, Placement, TextSizing, TextStyle,
    Transaction, keyed_by_position,
};

fn solid(r: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, 0, 0, 255))
}

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    /// The main, its rect and its text.
    m: NodeId,
    a: NodeId,
    t: NodeId,
    /// An instance of it, made by duplicating the main, and its two copies.
    i: NodeId,
    ia: NodeId,
    it: NodeId,
}

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xAB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [m, a, t] = [(); 3].map(|_| ids.mint());
    let create = |id, parent, index, kind| Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: None,
        name: None,
    };
    doc.apply(&Transaction(vec![
        create(
            m,
            root,
            0,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
        ),
        create(
            a,
            m,
            0,
            NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
        ),
        create(t, m, 1, text_kind("Label")),
        Operation::SetFills {
            id: a,
            fills: keyed_by_position([
                Fill {
                    brush: solid(10),
                    visible: true,
                },
                Fill {
                    brush: solid(20),
                    visible: true,
                },
            ]),
        },
        Operation::SetComponent {
            id: m,
            component: true,
        },
    ]))
    .expect("a main");
    // The instance, through the copy path, so its item ids are the main's.
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(m).unwrap(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).expect("an instance");
    let i = made[0];
    let kids = doc.get(i).unwrap().children().to_vec();
    F {
        doc,
        ids,
        root,
        m,
        a,
        t,
        i,
        ia: kids[0],
        it: kids[1],
    }
}

/// `ops` and what they owe the instances, applied — the commit's assembly.
fn commit(doc: &mut Document, ops: Vec<Operation>) {
    let mut tx = Transaction(ops);
    let follows = ondin_core::propagate::propagate(doc, &tx);
    tx.0.extend(follows);
    doc.apply(&tx).expect("the edit and its propagation");
}

fn fills(doc: &Document, id: NodeId) -> Vec<Keyed<Fill>> {
    doc.get(id).unwrap().paint().fills.clone()
}

/// The plain case: the main's fill changes, the instance's follows. Flip:
/// `propagate` answering nothing leaves the instance's red at 10.
#[test]
fn a_main_fill_reaches_its_instance() {
    let mut f = fixture();
    let mut next = fills(&f.doc, f.a);
    next[0].brush = solid(99);
    commit(
        &mut f.doc,
        vec![Operation::SetFills {
            id: f.a,
            fills: next,
        }],
    );
    assert_eq!(fills(&f.doc, f.ia)[0].brush, solid(99));
}

/// Item by item and field by field: the instance recoloured fill 0; the main
/// recolours fill 0 too and hides fill 1. The instance keeps its colour, and
/// fill 1 follows. Flip: comparing the list whole leaves fill 1 visible.
#[test]
fn an_overridden_fill_is_kept_while_its_sibling_follows() {
    let mut f = fixture();
    let mut mine = fills(&f.doc, f.ia);
    mine[0].brush = solid(55);
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.ia,
            fills: mine,
        }]))
        .unwrap();
    let mut next = fills(&f.doc, f.a);
    next[0].brush = solid(99);
    next[1].visible = false;
    commit(
        &mut f.doc,
        vec![Operation::SetFills {
            id: f.a,
            fills: next,
        }],
    );
    let got = fills(&f.doc, f.ia);
    assert_eq!(got[0].brush, solid(55), "the override is kept");
    assert!(!got[1].visible, "the untouched fill follows");
}

/// A text style follows field by field: the instance's own size stays when the
/// main changes family.
#[test]
fn a_text_style_follows_field_by_field() {
    let mut f = fixture();
    let style_of = |doc: &Document, id| match &doc.get(id).unwrap().kind() {
        NodeKind::Text { style, .. } => (**style).clone(),
        _ => unreachable!(),
    };
    let mut mine = style_of(&f.doc, f.it);
    mine.font_size = 30.0;
    f.doc
        .apply(&Transaction(vec![Operation::SetTextStyle {
            id: f.it,
            style: mine,
            spans: None,
        }]))
        .unwrap();
    let mut next = style_of(&f.doc, f.t);
    next.font_family = "Georgia".into();
    commit(
        &mut f.doc,
        vec![Operation::SetTextStyle {
            id: f.t,
            style: next,
            spans: None,
        }],
    );
    let got = style_of(&f.doc, f.it);
    assert_eq!(got.font_family, "Georgia", "the family follows");
    assert_eq!(got.font_size, 30.0, "the instance's size stays");
}

/// An instance root's placement is its own: moving or hiding the main moves and
/// hides nothing else. A member's transform follows. Flip: dropping
/// `is_placement` moves the instance onto the main.
#[test]
fn an_instance_keeps_its_own_place_while_its_members_follow() {
    let mut f = fixture();
    let at = f.doc.get(f.i).unwrap().transform();
    commit(
        &mut f.doc,
        vec![
            Operation::SetTransform {
                id: f.m,
                transform: Affine::translate((500.0, 0.0)),
            },
            Operation::SetVisible {
                id: f.m,
                visible: false,
            },
            Operation::SetTransform {
                id: f.a,
                transform: Affine::translate((5.0, 5.0)),
            },
        ],
    );
    assert_eq!(
        f.doc.get(f.i).unwrap().transform(),
        at,
        "the instance stays put"
    );
    assert!(f.doc.get(f.i).unwrap().visible(), "and stays visible");
    assert_eq!(
        f.doc.get(f.ia).unwrap().transform(),
        Affine::translate((5.0, 5.0)),
        "the member follows"
    );
}

/// The user's own edit to the instance in the same transaction wins.
#[test]
fn an_edit_to_the_instance_in_the_same_transaction_wins() {
    let mut f = fixture();
    let mut theirs = fills(&f.doc, f.a);
    theirs[0].brush = solid(99);
    let mut mine = fills(&f.doc, f.ia);
    mine[0].brush = solid(77);
    commit(
        &mut f.doc,
        vec![
            Operation::SetFills {
                id: f.a,
                fills: theirs,
            },
            Operation::SetFills {
                id: f.ia,
                fills: mine,
            },
        ],
    );
    assert_eq!(fills(&f.doc, f.ia)[0].brush, solid(77));
}

/// Through a nested chain: main `outer` holds an instance of `m`; an instance of
/// `outer` holds the copy of that. An edit to `m` reaches all three levels.
#[test]
fn an_edit_reaches_through_a_nested_instance() {
    let mut f = fixture();
    let outer = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: outer,
                parent: f.root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: outer,
                index: 0,
            },
            Operation::SetComponent {
                id: outer,
                component: true,
            },
        ]))
        .expect("an outer main holding the instance");
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(outer).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).expect("an instance of the outer main");
    let outer_copy = made[0];
    let nested_copy = f.doc.get(outer_copy).unwrap().children()[0];
    let deep = f.doc.get(nested_copy).unwrap().children()[0];

    let mut next = fills(&f.doc, f.a);
    next[0].brush = solid(99);
    commit(
        &mut f.doc,
        vec![Operation::SetFills {
            id: f.a,
            fills: next,
        }],
    );
    assert_eq!(fills(&f.doc, f.ia)[0].brush, solid(99), "one level");
    assert_eq!(fills(&f.doc, deep)[0].brush, solid(99), "two levels");
}

/// Text spans index into the content, so they follow only onto a copy whose
/// content is the main's. The instance retyped its label; the main's spans do
/// not land on it.
#[test]
fn spans_follow_only_onto_the_same_content() {
    let mut f = fixture();
    let text_of = |doc: &Document, id| match &doc.get(id).unwrap().kind() {
        NodeKind::Text { content, spans, .. } => (content.clone(), spans.clone()),
        _ => unreachable!(),
    };
    let (_, spans) = text_of(&f.doc, f.it);
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.it,
            content: "Mine".into(),
            spans: spans.clone(),
            para_spans: Default::default(),
        }]))
        .unwrap();
    // The main bolds its first two characters — a real span edit.
    let bold: ondin_core::node::CharSpans = serde_json::from_value(serde_json::json!([
        { "start": 0, "end": 2, "attr": { "Weight": 700 } }
    ]))
    .expect("a span");
    commit(
        &mut f.doc,
        vec![Operation::SetTextSpans {
            id: f.t,
            spans: bold.clone(),
        }],
    );
    assert_eq!(
        text_of(&f.doc, f.t).1,
        bold,
        "the fixture: the main's span landed"
    );
    let (content, spans) = text_of(&f.doc, f.it);
    assert_eq!(content, "Mine", "the instance's text is its own");
    assert_eq!(
        spans,
        Default::default(),
        "and the main's span did not land on it"
    );
}

/// The control for the test above: with the content the main's, the span follows.
/// Flip: `writes_spans`' content gate inverted fails one of the two.
#[test]
fn spans_follow_onto_the_same_content() {
    let mut f = fixture();
    let bold: ondin_core::node::CharSpans = serde_json::from_value(serde_json::json!([
        { "start": 0, "end": 2, "attr": { "Weight": 700 } }
    ]))
    .expect("a span");
    commit(
        &mut f.doc,
        vec![Operation::SetTextSpans {
            id: f.t,
            spans: bold.clone(),
        }],
    );
    match &f.doc.get(f.it).unwrap().kind() {
        NodeKind::Text { spans, .. } => assert_eq!(*spans, bold),
        _ => unreachable!(),
    }
}

// ── Structure (build step 4) ───────────────────────────────────────────────────

/// `ops` with everything `commit_inner` appends (`propagate::owed`).
fn commit_all(f: &mut F, ops: Vec<Operation>) {
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).expect("the edit and everything it owes");
}

fn rect_kind() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(5.0, 5.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn kids(doc: &Document, id: NodeId) -> Vec<NodeId> {
    doc.get(id).unwrap().children().to_vec()
}

fn link(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.get(id).unwrap().link()
}

/// A child the main gains is copied into the instance, linked to it, placed after
/// its preceding sibling's counterpart — and two gained at once keep their order.
/// Flip: `propagate_structure` answering nothing leaves the instance at two.
#[test]
fn children_a_main_gains_reach_its_instance_in_order() {
    let mut f = fixture();
    let [x, y] = [(); 2].map(|_| f.ids.mint());
    let (m, a) = (f.m, f.a);
    commit_all(
        &mut f,
        vec![
            Operation::CreateNode {
                id: x,
                parent: m,
                index: 1,
                kind: rect_kind(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: y,
                parent: m,
                index: 2,
                kind: rect_kind(),
                transform: None,
                name: None,
            },
        ],
    );
    assert_eq!(kids(&f.doc, m), vec![a, x, y, f.t], "the fixture");
    let links: Vec<_> = kids(&f.doc, f.i).iter().map(|k| link(&f.doc, *k)).collect();
    assert_eq!(links, vec![Some(a), Some(x), Some(y), Some(f.t)]);
}

/// A child the main loses takes an **untouched** counterpart with it and leaves a
/// **changed** one as the instance's own layer (§15 D979 (b)). Flip: deleting
/// regardless of `untouched` deletes the changed one too.
#[test]
fn a_lost_child_takes_untouched_counterparts_and_leaves_changed_ones() {
    let mut f = fixture();
    let i = f.i;
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(f.m).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    let changed_copy = kids(&f.doc, made[0])[0];
    f.doc
        .apply(&Transaction(vec![Operation::SetName {
            id: changed_copy,
            name: "mine".into(),
        }]))
        .unwrap();
    let a = f.a;
    commit_all(&mut f, vec![Operation::DeleteNode { id: a }]);
    let ia_gone = !kids(&f.doc, i).iter().any(|k| link(&f.doc, *k) == Some(a));
    assert!(
        ia_gone && f.doc.get(f.ia).is_none(),
        "the untouched counterpart went"
    );
    assert!(f.doc.get(changed_copy).is_some(), "the changed one stays");
    assert_eq!(
        link(&f.doc, changed_copy),
        None,
        "as the instance's own layer"
    );
}

/// A reorder in the main follows onto an instance still in the old order.
#[test]
fn a_reorder_in_the_main_follows() {
    let mut f = fixture();
    let (t, m) = (f.t, f.m);
    commit_all(&mut f, vec![Operation::Reorder { id: t, index: 0 }]);
    assert_eq!(kids(&f.doc, m), vec![f.t, f.a], "the fixture");
    assert_eq!(kids(&f.doc, f.i), vec![f.it, f.ia]);
}

/// A child gained by an inner main reaches the outer main's instances through
/// the nested copy, two levels down, each linked one level up.
#[test]
fn a_gained_child_reaches_through_a_nested_instance() {
    let mut f = fixture();
    let outer = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: outer,
                parent: f.root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: outer,
                index: 0,
            },
            Operation::SetComponent {
                id: outer,
                component: true,
            },
        ]))
        .unwrap();
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(outer).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    let nested_copy = kids(&f.doc, made[0])[0];
    let x = f.ids.mint();
    let m = f.m;
    commit_all(
        &mut f,
        vec![Operation::CreateNode {
            id: x,
            parent: m,
            index: 2,
            kind: rect_kind(),
            transform: None,
            name: None,
        }],
    );
    let in_i = *kids(&f.doc, f.i).last().unwrap();
    assert_eq!(link(&f.doc, in_i), Some(x), "one level");
    let deep = *kids(&f.doc, nested_copy).last().unwrap();
    assert_eq!(
        link(&f.doc, deep),
        Some(in_i),
        "two levels, linked one level up"
    );
}

/// A nested instance moved inside its outer main moves in every outer instance:
/// its place there is the outer main's, not its own. Only an instance linked
/// **straight to a main** keeps its placement. Flip: asking `instance_root` for
/// the placement rule (the first build) leaves the outer instance's copy put.
#[test]
fn a_nested_instance_moved_inside_its_outer_main_moves_in_its_instances() {
    let mut f = fixture();
    let outer = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: outer,
                parent: f.root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: outer,
                index: 0,
            },
            Operation::SetComponent {
                id: outer,
                component: true,
            },
        ]))
        .unwrap();
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(outer).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    let nested_copy = kids(&f.doc, made[0])[0];
    let to = Affine::translate((40.0, 0.0));
    let i = f.i;
    commit_all(
        &mut f,
        vec![Operation::SetTransform {
            id: i,
            transform: to,
        }],
    );
    assert_eq!(f.doc.get(nested_copy).unwrap().transform(), to);
}

/// *Group selection* inside a main: the instance's **own** counterpart moves into
/// a copy of the new group — it is not replaced by a fresh copy, which would lose
/// whatever the instance had changed on it (the first build's "loss plus gain").
/// Then *Ungroup* takes it back out, and the commit goes through (the first build
/// deleted the untouched group copy, and the counterpart with it, before moving
/// it — `NoSuchNode`).
#[test]
fn grouping_and_ungrouping_inside_a_main_moves_the_instances_own_layers() {
    let mut f = fixture();
    // The instance's own change to its counterpart, which must survive both.
    f.doc
        .apply(&Transaction(vec![Operation::SetName {
            id: f.ia,
            name: "mine".into(),
        }]))
        .unwrap();
    let (g, m, a, i, ia) = (f.ids.mint(), f.m, f.a, f.i, f.ia);
    commit_all(
        &mut f,
        vec![
            Operation::CreateNode {
                id: g,
                parent: m,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: a,
                new_parent: g,
                index: 0,
            },
        ],
    );
    let gc = f
        .doc
        .get(ia)
        .expect("the counterpart survives")
        .parent()
        .unwrap();
    assert_eq!(link(&f.doc, gc), Some(g), "it moved into the group's copy");
    assert_eq!(f.doc.get(gc).unwrap().parent(), Some(i));
    assert_eq!(link(&f.doc, ia), Some(a), "still linked");
    assert_eq!(f.doc.get(ia).unwrap().name(), "mine", "its change kept");

    commit_all(
        &mut f,
        vec![
            Operation::Reparent {
                id: a,
                new_parent: m,
                index: 0,
            },
            Operation::DeleteNode { id: g },
        ],
    );
    assert_eq!(
        f.doc.get(ia).unwrap().parent(),
        Some(i),
        "back out of the group"
    );
    assert!(
        f.doc.get(gc).is_none(),
        "the empty group copy went with the group"
    );
    assert_eq!(link(&f.doc, ia), Some(a));
}

/// A move into a parent the instance has deleted is a removal from that instance
/// (§5.3d): the untouched counterpart goes. Flip: leaving it where it was keeps a
/// layer the main no longer has there.
#[test]
fn a_move_into_a_parent_the_instance_lacks_removes_the_counterpart() {
    let mut f = fixture();
    let (g, m) = (f.ids.mint(), f.m);
    commit_all(
        &mut f,
        vec![Operation::CreateNode {
            id: g,
            parent: m,
            index: 2,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }],
    );
    let gc = *kids(&f.doc, f.i).last().unwrap();
    assert_eq!(link(&f.doc, gc), Some(g), "the fixture: the group's copy");
    // The instance deletes its copy of the group (free structure).
    commit_all(&mut f, vec![Operation::DeleteNode { id: gc }]);
    let (a, ia) = (f.a, f.ia);
    commit_all(
        &mut f,
        vec![Operation::Reparent {
            id: a,
            new_parent: g,
            index: 0,
        }],
    );
    assert!(f.doc.get(ia).is_none(), "the untouched counterpart went");
}

/// **Deleting a main through the commit keeps its instances' layers** (§15 D984):
/// the instance detaches and keeps its look (§15 D979 (c)). The structural pass
/// read a deleted main as a parent that had lost every child, and deleted each
/// untouched counterpart — so the instance came back an empty frame, while the
/// app's own test asserted only that its link was cut.
///
/// Flip: counting a deleted parent's children as lost unconditionally (the first
/// build) fails the child count at 0.
#[test]
fn deleting_a_main_through_the_commit_keeps_its_instances_layers() {
    let mut f = fixture();
    let (m, i, ia, it) = (f.m, f.i, f.ia, f.it);
    let mut ops = ondin_core::component::relink_for_delete(&f.doc, &[m]);
    ops.push(Operation::DeleteNode { id: m });
    commit_all(&mut f, ops);
    assert!(f.doc.get(m).is_none(), "the main is gone");
    assert_eq!(link(&f.doc, i), None, "the instance detached");
    assert_eq!(kids(&f.doc, i), vec![ia, it], "with every layer it had");
    assert_eq!(link(&f.doc, ia), None);
}

// ── The release review's blockers (`v0.4.1..7d0c666`) ─────────────────────────

/// The fixture's instance nested in an outer main, which gets an instance of its
/// own: the nested copy there, holding copies of `i`'s children.
fn nest(f: &mut F) -> NodeId {
    let outer = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: outer,
                parent: f.root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.i,
                new_parent: outer,
                index: 0,
            },
            Operation::SetComponent {
                id: outer,
                component: true,
            },
        ]))
        .unwrap();
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[Placement {
            nodes: f.doc.capture_subtree(outer).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    kids(&f.doc, made[0])[0]
}

/// **A value the loader admits and the operations refuse does not take the
/// commit down** (`[R1-L2-01]`). A gradient's opacity of 5 loads — its reader
/// clamps it, §15 D767 — and editing that fill on a main read the old value by
/// applying the edit and re-applying its inverse, which `op_set_fills` refuses:
/// `peek` asserted it took, and the commit panicked, as did every preview frame of
/// a scrub on the same fill. `peek` reads off a one-node scratch now and restores
/// nothing.
///
/// Flip: `peek` re-applying the inverse with the `expect` back panics here,
/// *"an inverse applies to the state it was taken from: NonFinite"*.
#[test]
fn a_loaded_value_the_operations_refuse_does_not_panic_a_mains_edit() {
    let mut f = fixture();
    let gradient = |opacity| {
        keyed_by_position([Fill {
            brush: Brush::Gradient(ondin_core::GradientBrush {
                gradient: ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0)),
                transform: Affine::IDENTITY,
                opacity,
            }),
            visible: true,
        }])
    };
    let a = f.a;
    commit_all(
        &mut f,
        vec![Operation::SetFills {
            id: a,
            fills: gradient(0.25),
        }],
    );
    let text = String::from_utf8(ondin_core::io::save(&f.doc).unwrap()).unwrap();
    assert_eq!(text.matches("0.25").count(), 2, "the main's and the copy's");
    f.doc = ondin_core::io::load(text.replace("0.25", "5.0").as_bytes()).expect("it loads");
    let new = keyed_by_position([Fill {
        brush: solid(90),
        visible: true,
    }]);
    commit_all(
        &mut f,
        vec![Operation::SetFills {
            id: a,
            fills: new.clone(),
        }],
    );
    assert_eq!(fills(&f.doc, f.a), new);
    assert_eq!(
        fills(&f.doc, f.ia),
        new,
        "and the copy, which shared it, follows"
    );
}

/// **A layer dragged out of a main leaves its instances, and the drag commits**
/// (`[X3-L1-01]`, §15 D979 (xiv)). The canvas writes the move as a `Reparent`
/// and a `SetTransform`; the structural pass deletes the untouched counterpart,
/// and the field pass — whose copy map is the document before the edit — then
/// wrote the transform onto the deleted node, and `apply` refused the whole drag
/// with `NoSuchNode`. A counterpart the instance had changed is kept and cut, and
/// the field pass moved it 300 units out of its 100×100 instance.
///
/// Flip: dropping `propagate`'s `gone` check fails the first commit with
/// `NoSuchNode`, the `expect` in `commit_all`.
#[test]
fn a_member_dragged_out_of_a_main_leaves_its_instances() {
    let out = Affine::translate((300.0, 40.0));
    let drag = |f: &F| {
        vec![
            Operation::Reparent {
                id: f.a,
                new_parent: f.root,
                index: 0,
            },
            Operation::SetTransform {
                id: f.a,
                transform: out,
            },
        ]
    };
    // Untouched: the counterpart goes with it.
    let mut f = fixture();
    let ops = drag(&f);
    commit_all(&mut f, ops);
    assert_eq!(f.doc.get(f.a).unwrap().parent(), Some(f.root), "it left");
    assert!(f.doc.get(f.ia).is_none(), "the untouched counterpart went");

    // Touched: kept, cut, and where it was.
    let mut f = fixture();
    let was = f.doc.get(f.ia).unwrap().transform();
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.ia,
            fills: keyed_by_position([Fill {
                brush: solid(77),
                visible: true,
            }]),
        }]))
        .unwrap();
    let ops = drag(&f);
    commit_all(&mut f, ops);
    let ia = f.doc.get(f.ia).expect("the changed counterpart is kept");
    assert_eq!(ia.parent(), Some(f.i));
    assert_eq!(ia.link(), None, "cut");
    assert_eq!(
        ia.transform(),
        was,
        "at its own place, not the main's new one"
    );
}

/// **Deleting a main's child together with its counterpart, or with the whole
/// instance, commits** (`[X2-L1-02]`). The structural pass read the counterpart
/// as untouched in the document before the edit and deleted it a second time.
///
/// Flip: dropping the lost queue's `after_nodes` check fails both commits with
/// `NoSuchNode`.
#[test]
fn deleting_a_child_with_its_counterpart_or_its_instance_commits() {
    let mut f = fixture();
    let (a, ia) = (f.a, f.ia);
    commit_all(
        &mut f,
        vec![
            Operation::DeleteNode { id: a },
            Operation::DeleteNode { id: ia },
        ],
    );
    assert!(f.doc.get(a).is_none() && f.doc.get(ia).is_none());

    let mut f = fixture();
    let (a, i) = (f.a, f.i);
    commit_all(
        &mut f,
        vec![
            Operation::DeleteNode { id: a },
            Operation::DeleteNode { id: i },
        ],
    );
    assert!(f.doc.get(a).is_none() && f.doc.get(i).is_none());
}

/// **A layer dragged into a main from outside it reaches the main's instances**
/// (`[X2-L1-01]`, §5.3d build step 4: *"a `Reparent` into the main"*). The gained
/// subtree was pruned to what the edit had *made*, so a layer that existed before
/// — drawn on the canvas and then dragged in — was never copied, and each
/// instance silently stopped matching its main.
///
/// Flip: `only_new` admitting every node that existed before (the old predicate)
/// fails the child count, 2 against 3.
#[test]
fn a_layer_dragged_into_a_main_reaches_its_instances() {
    let mut f = fixture();
    let r = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: r,
            parent: f.root,
            index: 0,
            kind: rect_kind(),
            transform: None,
            name: None,
        }]))
        .unwrap();
    let m = f.m;
    commit_all(
        &mut f,
        vec![Operation::Reparent {
            id: r,
            new_parent: m,
            index: 2,
        }],
    );
    assert_eq!(kids(&f.doc, f.i).len(), 3);
    let copy = *kids(&f.doc, f.i).last().unwrap();
    assert_eq!(link(&f.doc, copy), Some(r), "a copy linked to the layer");
}

/// **An ungroup inside a main keeps an instance's changed copy of the group as
/// a local wrapper, its children inside it and still linked** (`[R3-L5-01]`,
/// §15 D1003 (2)). The moves loop lifted the counterparts out whatever the copy
/// held: they drew as the main did — 30 units left of where the instance had
/// put them, at full opacity — and the moved, faded group stayed behind empty.
/// The group's transform the ungroup bakes into the child is the main's, not this
/// copy's, so the child keeps its own local transform too.
///
/// Flip: `wrapper` answering `false` fails "still inside its group", the copy's
/// parent coming back as the instance. Pinning nothing for a kept child fails
/// "and where it drew" at `(15, 5)`: the field pass carried the main's baked
/// transform into the copy's group.
#[test]
fn an_ungroup_in_a_main_keeps_a_changed_group_copy_as_a_local_wrapper() {
    let mut f = fixture();
    let (g, m, a, i, ia) = (f.ids.mint(), f.m, f.a, f.i, f.ia);
    let in_group = Affine::translate((5.0, 5.0));
    commit_all(
        &mut f,
        vec![
            Operation::CreateNode {
                id: g,
                parent: m,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((10.0, 0.0))),
                name: None,
            },
            Operation::Reparent {
                id: a,
                new_parent: g,
                index: 0,
            },
            Operation::SetTransform {
                id: a,
                transform: in_group,
            },
        ],
    );
    let gc = f.doc.get(ia).unwrap().parent().unwrap();
    assert_eq!(link(&f.doc, gc), Some(g), "the fixture: the group's copy");
    // The instance moves and fades its copy of the group.
    f.doc
        .apply(&Transaction(vec![
            Operation::SetTransform {
                id: gc,
                transform: Affine::translate((40.0, 0.0)),
            },
            Operation::SetOpacity {
                id: gc,
                opacity: 0.5,
            },
        ]))
        .unwrap();
    let local = f.doc.get(ia).unwrap().transform();
    assert_eq!(
        local, in_group,
        "the fixture: the child followed into the group"
    );
    // Ungroup in the main, as `build::ungroup` writes it.
    commit_all(
        &mut f,
        vec![
            Operation::SetTransform {
                id: a,
                transform: Affine::translate((10.0, 0.0)) * in_group,
            },
            Operation::Reparent {
                id: a,
                new_parent: m,
                index: 0,
            },
            Operation::DeleteNode { id: g },
        ],
    );
    assert_eq!(
        f.doc.get(a).unwrap().parent(),
        Some(m),
        "the main ungrouped"
    );
    let w = f.doc.get(gc).expect("the changed copy stays");
    assert_eq!(
        f.doc.get(ia).unwrap().parent(),
        Some(gc),
        "still inside its group"
    );
    assert_eq!(w.parent(), Some(i));
    assert_eq!(w.link(), None, "a local wrapper");
    assert_eq!(w.opacity(), 0.5, "with the instance's change");
    assert_eq!(link(&f.doc, ia), Some(a), "the child still linked");
    assert_eq!(
        f.doc.get(ia).unwrap().transform(),
        local,
        "and where it drew"
    );
}

/// **A reorder in a main reaches through a nested instance** (`[X3-L2-02]`,
/// §5.3d *Linking*: transitive). The reorder arm handled one level, so the outer
/// instance's nested copy kept the old order and refused every later reorder as
/// one it had made itself.
///
/// Flip: dropping the `reorders.push` leaves the nested copy at `[a, t]`.
#[test]
fn a_reorder_reaches_through_a_nested_instance() {
    let mut f = fixture();
    let nc = nest(&mut f);
    let (t, m) = (f.t, f.m);
    commit_all(&mut f, vec![Operation::Reorder { id: t, index: 0 }]);
    assert_eq!(kids(&f.doc, m), vec![f.t, f.a], "the fixture");
    assert_eq!(kids(&f.doc, f.i), vec![f.it, f.ia], "one level");
    let deep: Vec<_> = kids(&f.doc, nc)
        .into_iter()
        .map(|k| link(&f.doc, k).unwrap())
        .collect();
    assert_eq!(deep, vec![f.it, f.ia], "two levels");
}
