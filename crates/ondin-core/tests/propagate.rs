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
