//! `Document::apply` is all-or-nothing: it applies in place and puts a refused
//! transaction back from a journal, so each test here refuses one and compares
//! the whole document with the one it started from. In a debug build `apply`
//! makes the same comparison itself and panics first; the flips below name the
//! assertion a release run fails at.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleId, StyleValue};
use ondin_core::{
    Brush, Cell, CharSpans, Described, Document, Fill, Guide, GuideAxis, GuideId, IdSource,
    ImageEntry, ImageFormat, ImageId, ImageSource, Mode, ModeId, NodeId, NodeKind, Operation,
    ParaSpans, TextSizing, TextStyle, Transaction, VarId, VarKind, Variable, image_brush,
    keyed_by_position,
};

fn rect(w: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn create(id: NodeId, parent: NodeId, index: usize, kind: NodeKind, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::IDENTITY),
        name: Some(name.into()),
    }
}

fn solid(r: u8) -> Vec<ondin_core::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(Color::from_rgba8(r, 0, 0, 255)).into(),
        visible: true,
    }])
}

fn picture(bytes: &[u8]) -> ImageEntry {
    ImageEntry {
        source: ImageSource::Embedded(bytes.to_vec().into()),
        format: ImageFormat::Png,
        width: 4,
        height: 4,
    }
}

fn number_var(id: VarId, name: &str, cells: &[(ModeId, f64)]) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind: VarKind::Number,
        cells: cells.iter().map(|&(m, v)| (m, Cell::Number(v))).collect(),
        scope: None,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

fn fill_style(id: StyleId, name: &str) -> Style {
    Style {
        id,
        name: name.into(),
        value: StyleValue::Fills(solid(9)),
        origin: None,
        forked: None,
        description: String::new(),
    }
}

/// A document with something in every table an operation writes — nodes, a
/// mode, a variable, a style, an image, a guide — and a transaction of ops
/// across all of them, each valid after the ones before it, that the document
/// takes whole.
struct Fixture {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    a: NodeId,
    light: ModeId,
    v0: VarId,
    s0: StyleId,
    tx: Vec<Operation>,
}

fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x7B);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, a, b, text) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let (light, v0, s0) = (ids.mint_mode(), ids.mint_var(), ids.mint_style());
    let (img0, g0) = (ImageId("sha:zero".into()), GuideId(ids.mint()));
    let guide = |id, position| Guide {
        id,
        axis: GuideAxis::Horizontal,
        position,
        color: None,
        owner: root,
    };
    doc.apply(&Transaction(vec![
        create(
            frame,
            root,
            0,
            NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            "Frame",
        ),
        create(a, frame, 0, rect(10.0), "A"),
        create(b, frame, 1, rect(20.0), "B"),
        create(
            text,
            frame,
            2,
            NodeKind::Text {
                content: "hi".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 12.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::AutoHeight(100.0),
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            "T",
        ),
        Operation::SetFills {
            id: a,
            fills: solid(255),
        },
        Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: Vec::new(),
        },
        Operation::AddVariable {
            var: number_var(v0, "space/s", &[(light, 4.0)]),
        },
        Operation::AddStyle {
            style: fill_style(s0, "Brand"),
        },
        Operation::AddImage {
            id: img0.clone(),
            entry: picture(&[1, 2, 3]),
        },
        Operation::AddGuide {
            guide: guide(g0, 10.0),
        },
    ]))
    .expect("the starting document");

    let (c, page2, on_page2) = (ids.mint(), ids.mint(), ids.mint());
    let (g1, img1, dark) = (
        GuideId(ids.mint()),
        ImageId("sha:one".into()),
        ids.mint_mode(),
    );
    let (v1, s1, vault) = (ids.mint_var(), ids.mint_style(), ids.mint());
    let tx = vec![
        create(c, frame, 0, rect(30.0), "C"),
        Operation::SetName {
            id: a,
            name: "A2".into(),
        },
        Operation::SetFills {
            id: b,
            fills: solid(128),
        },
        Operation::Reorder { id: text, index: 0 },
        Operation::SetTransform {
            id: a,
            transform: Affine::translate((5.0, 7.0)),
        },
        Operation::DeleteNode { id: b },
        Operation::SetText {
            id: text,
            content: "hello".into(),
            spans: CharSpans::default(),
            para_spans: ParaSpans::default(),
        },
        Operation::SetOpacity {
            id: text,
            opacity: 0.5,
        },
        Operation::SetVisible {
            id: c,
            visible: false,
        },
        Operation::SetDescription {
            of: Described::Node(a),
            text: "the first".into(),
        },
        Operation::AddPage {
            id: page2,
            index: 1,
            name: "Second".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
        },
        Operation::MovePage {
            id: page2,
            index: 0,
        },
        Operation::SetPageGround {
            id: page2,
            ground: Some(Color::from_rgba8(240, 240, 240, 255)),
        },
        create(on_page2, page2, 0, rect(40.0), "On two"),
        Operation::AddGuide {
            guide: guide(g1, 50.0),
        },
        Operation::SetGuidePosition {
            id: g0,
            position: 30.0,
        },
        Operation::RemoveGuide { id: g0 },
        Operation::AddImage {
            id: img1.clone(),
            entry: picture(&[4, 5, 6]),
        },
        Operation::SetFills {
            id: c,
            fills: keyed_by_position([Fill {
                brush: image_brush(img1.clone()).into(),
                visible: true,
            }]),
        },
        Operation::RemoveImage { id: img0 },
        Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![(v0, Cell::Number(8.0))],
        },
        Operation::SetVariableCell {
            id: v0,
            mode: light,
            cell: Cell::Number(6.0),
        },
        Operation::AddVariable {
            var: number_var(v1, "space/m", &[(light, 12.0), (dark, 16.0)]),
        },
        Operation::SetVariableName {
            id: v0,
            name: "space/xs".into(),
        },
        Operation::SetModeName {
            id: dark,
            name: "Night".into(),
        },
        Operation::AddStyle {
            style: fill_style(s1, "Accent"),
        },
        Operation::SetStyleName {
            id: s0,
            name: "Brand 2".into(),
        },
        Operation::SetFillStyle {
            id: a,
            style: Some(s0),
        },
        Operation::RemoveStyle { id: s1 },
        Operation::OpenVault { id: vault },
        Operation::DeleteNode { id: on_page2 },
        Operation::RemovePage { id: page2 },
    ];
    let mut whole = doc.clone();
    whole
        .apply(&Transaction(tx.clone()))
        .expect("the fixture's transaction is valid whole");
    Fixture {
        doc,
        ids,
        frame,
        a,
        light,
        v0,
        s0,
        tx,
    }
}

/// `tx[..k]` followed by `last`, for every `k`, refused and leaving `f.doc`
/// exactly as it was.
fn refused_after_each_prefix(f: &Fixture, last: &Operation) {
    for k in 0..=f.tx.len() {
        let mut doc = f.doc.clone();
        let mut ops = f.tx[..k].to_vec();
        ops.push(last.clone());
        let got = doc.apply(&Transaction(ops));
        assert!(got.is_err(), "refused after {k} ops: {got:?}");
        assert!(doc == f.doc, "rolled back exactly after {k} ops");
    }
}

/// An op refused in the loop, after every prefix of the transaction.
///
/// flip: `NodeTable::note` recording nothing → fails at "rolled back exactly
/// after 1 ops" (the created node stays).
#[test]
fn an_op_refused_after_each_prefix_leaves_the_document_as_it_was() {
    let mut f = fixture();
    let missing = f.ids.mint();
    refused_after_each_prefix(
        &f,
        &Operation::SetName {
            id: missing,
            name: "x".into(),
        },
    );
}

/// An op that writes before it refuses — `AddMode` fills the first variable's
/// cell, then meets a variable that does not exist — after every prefix: the
/// rollback restores state, it does not replay inverses.
///
/// flip: `InPlace`'s drop not restoring `variables` → fails at "rolled back
/// exactly after 0 ops".
#[test]
fn an_op_that_writes_before_refusing_is_rolled_back() {
    let mut f = fixture();
    let (late, missing) = (f.ids.mint_mode(), f.ids.mint_var());
    let last = Operation::AddMode {
        collection: ondin_core::variable::CollectionId::FIRST,
        mode: Mode {
            id: late,
            name: "Late".into(),
        },
        index: 0,
        cells: vec![(f.v0, Cell::Number(1.0)), (missing, Cell::Number(2.0))],
    };
    assert!(
        f.doc.variables().get(f.v0).is_some() && f.doc.variables().get(missing).is_none(),
        "the op's first variable exists and its second does not"
    );
    refused_after_each_prefix(&f, &last);
}

/// A node op that writes before it refuses — `Reparent` takes the layer out of
/// its parent's children, then finds the index out of range — after every
/// prefix: the parent's list gets the layer back in its place.
///
/// flip: `NodeTable::note` recording nothing → fails at "rolled back exactly
/// after 0 ops".
#[test]
fn a_node_op_that_writes_before_refusing_is_rolled_back() {
    let f = fixture();
    refused_after_each_prefix(
        &f,
        &Operation::Reparent {
            id: f.a,
            new_parent: f.frame,
            index: 99,
        },
    );
}

/// A transaction every op of which succeeds, refused by a post-condition — a
/// layer bound to a style that does not exist — after every prefix.
///
/// flip: `apply_journaled` ignoring `style::check`'s refusal → fails at
/// "refused after 0 ops".
#[test]
fn a_post_condition_refusal_after_each_prefix_leaves_the_document_as_it_was() {
    let mut f = fixture();
    let missing = f.ids.mint_style();
    refused_after_each_prefix(
        &f,
        &Operation::SetFillStyle {
            id: f.a,
            style: Some(missing),
        },
    );
}

/// The undo of the whole transaction, applied as history applies it, gives back
/// the starting document.
#[test]
fn the_whole_transaction_undoes_to_the_start() {
    let f = fixture();
    let mut doc = f.doc.clone();
    let outcome = doc.apply(&Transaction(f.tx.clone())).unwrap();
    assert!(doc != f.doc, "the transaction changed something");
    assert!(doc.variables().get(f.v0).is_some());
    assert_eq!(doc.styles().get(f.s0).unwrap().name, "Brand 2");
    assert_eq!(doc.get(f.frame).unwrap().children().len(), 3);
    doc.apply_restoring(&outcome.inverse).unwrap();
    assert!(doc == f.doc, "undone exactly");
}

/// A refused transaction leaves the document usable: the next one applies, and
/// its undo returns to the start, so the journal was closed by the refusal.
///
/// flip: `InPlace`'s drop not rolling the node table back → the journal stays
/// open and the next apply panics at "already open".
#[test]
fn a_refusal_leaves_the_document_ready_for_the_next_transaction() {
    let mut f = fixture();
    let missing = f.ids.mint();
    let mut doc = f.doc.clone();
    let mut ops = f.tx.clone();
    ops.push(Operation::DeleteNode { id: missing });
    assert!(doc.apply(&Transaction(ops)).is_err());
    let outcome = doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: f.v0,
            mode: f.light,
            cell: Cell::Number(2.0),
        }]))
        .unwrap();
    doc.apply_restoring(&outcome.inverse).unwrap();
    assert!(doc == f.doc);
}
