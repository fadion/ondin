//! `merge::merge`, the three-way merge behind `ondin merge`: two edits of one
//! saved document combined, each case built as a base, ours and theirs.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::merge::{merge, merge_order};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, GeometryPatch, IdSource, NodeId, NodeKind, Operation, TextSizing,
    TextStyle, Transaction, io, keyed_by_position,
};

/// A page holding a frame with two rects and a text, and the ids.
struct Base {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    a: NodeId,
    b: NodeId,
    text: NodeId,
}

fn base() -> Base {
    let mut ids = IdSource::new(0x3E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, a, b, text) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let rect = |w| NodeKind::Rect {
        size: Size::new(w, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    };
    let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::IDENTITY),
        name: Some(name.into()),
    };
    doc.apply(&Transaction(vec![
        create(
            frame,
            root,
            0,
            NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
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
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)).into(),
                visible: true,
            }]),
        },
    ]))
    .unwrap();
    Base {
        doc,
        ids,
        frame,
        a,
        b,
        text,
    }
}

fn save(doc: &Document) -> Vec<u8> {
    io::save(doc).unwrap()
}

fn edited(doc: &Document, ops: Vec<Operation>) -> Document {
    let mut d = doc.clone();
    d.apply(&Transaction(ops)).unwrap();
    d
}

/// `base`, `ours` and `theirs` merged and loaded back.
fn merged(base: &Document, ours: &Document, theirs: &Document) -> (Document, Vec<String>) {
    let m = merge(&save(base), &save(ours), &save(theirs)).unwrap();
    (io::load(&m.bytes).unwrap(), m.conflicts)
}

fn name(doc: &Document, id: NodeId) -> &str {
    doc.get(id).unwrap().name()
}

fn rename(id: NodeId, name: &str) -> Operation {
    Operation::SetName {
        id,
        name: name.into(),
    }
}

/// Edits to different layers both land, with no conflict.
///
#[test]
fn edits_to_different_layers_both_land() {
    let f = base();
    let ours = edited(&f.doc, vec![rename(f.a, "Ours")]);
    let theirs = edited(
        &f.doc,
        vec![Operation::SetTransform {
            id: f.b,
            transform: Affine::translate((40.0, 0.0)),
        }],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(name(&doc, f.a), "Ours");
    assert_eq!(
        doc.get(f.b).unwrap().transform(),
        Affine::translate((40.0, 0.0))
    );
}

/// Different fields of one layer both land.
#[test]
fn different_fields_of_one_layer_both_land() {
    let f = base();
    let ours = edited(&f.doc, vec![rename(f.a, "Ours")]);
    let theirs = edited(
        &f.doc,
        vec![Operation::SetGeometry {
            id: f.a,
            geometry: GeometryPatch::Size(Size::new(99.0, 10.0)),
        }],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(name(&doc, f.a), "Ours");
    assert_eq!(
        doc.get(f.a).unwrap().kind().box_size(),
        Some(Size::new(99.0, 10.0))
    );
}

/// One field changed differently on both sides is a conflict: ours is kept and the
/// place is named.
#[test]
fn one_field_changed_differently_keeps_ours_and_says_so() {
    let f = base();
    let ours = edited(&f.doc, vec![rename(f.a, "Ours")]);
    let theirs = edited(&f.doc, vec![rename(f.a, "Theirs")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert_eq!(name(&doc, f.a), "Ours");
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert!(conflicts[0].contains(".name"), "{conflicts:?}");
    assert!(conflicts[0].contains(&f.a.to_wire()), "{conflicts:?}");
}

/// The same change on both sides is no conflict.
#[test]
fn the_same_change_on_both_sides_is_clean() {
    let f = base();
    let ours = edited(&f.doc, vec![rename(f.a, "Same")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &ours.clone());
    assert!(conflicts.is_empty());
    assert_eq!(name(&doc, f.a), "Same");
}

/// A layer each side added to the same frame both arrive, theirs after the layer
/// it follows in theirs.
///
/// flip: `merge_order` dropping theirs' additions → theirs' layer is unreachable,
/// the merge keeps ours whole, and fails at the no-conflict assertion.
#[test]
fn layers_added_on_both_sides_both_arrive() {
    let mut f = base();
    let (x, y) = (f.ids.mint(), f.ids.mint());
    let add = |id, index, name: &str| Operation::CreateNode {
        id,
        parent: f.frame,
        index,
        kind: NodeKind::Ellipse {
            size: Size::new(5.0, 5.0),
        },
        transform: None,
        name: Some(name.into()),
    };
    let ours = edited(&f.doc, vec![add(x, 3, "X")]);
    let theirs = edited(&f.doc, vec![add(y, 1, "Y")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(
        doc.get(f.frame).unwrap().children(),
        &[f.a, y, f.b, f.text, x]
    );
}

/// A layer one side deleted and the other left alone is gone.
#[test]
fn a_layer_deleted_on_one_side_is_gone() {
    let f = base();
    let ours = edited(&f.doc, vec![Operation::DeleteNode { id: f.b }]);
    let theirs = edited(&f.doc, vec![rename(f.a, "Theirs")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert!(doc.get(f.b).is_none());
    assert_eq!(name(&doc, f.a), "Theirs");
}

/// A layer deleted by ours and edited by theirs cannot stand in two places at
/// once: the parent's list dropped it while its record survives, so the merge
/// keeps ours whole and says so — and still loads.
#[test]
fn deleted_by_ours_and_edited_by_theirs_is_a_conflict() {
    let f = base();
    let ours = edited(&f.doc, vec![Operation::DeleteNode { id: f.b }]);
    let theirs = edited(&f.doc, vec![rename(f.b, "Edited")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(!conflicts.is_empty());
    assert!(doc.get(f.b).is_none(), "ours, whole");
}

/// One layer moved into two different parents does not hold together: ours is
/// kept whole, with a conflict saying why.
#[test]
fn one_layer_moved_two_ways_keeps_ours_whole() {
    let mut f = base();
    let (g1, g2) = (f.ids.mint(), f.ids.mint());
    let group = |id, index| Operation::CreateNode {
        id,
        parent: f.frame,
        index,
        kind: NodeKind::Group,
        transform: None,
        name: None,
    };
    let start = edited(&f.doc, vec![group(g1, 3), group(g2, 4)]);
    let into = |p| Operation::Reparent {
        id: f.a,
        new_parent: p,
        index: 0,
    };
    let ours = edited(&start, vec![into(g1), rename(f.text, "Ours")]);
    let theirs = edited(&start, vec![into(g2), rename(f.b, "Theirs")]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(
        conflicts
            .iter()
            .any(|c| c.contains("does not hold together")),
        "{conflicts:?}"
    );
    assert_eq!(doc.get(f.a).unwrap().parent(), Some(g1));
    assert_eq!(name(&doc, f.b), "B", "theirs is not half applied");
}

/// A value that switched variant on one side and changed inside on the other is
/// one value in conflict, not a blend of two variants, so the rest of theirs still
/// lands.
///
/// flip: `variant_differs` answering `false` → the sizing blends into an object of
/// two variants, the merge does not load, and fails at the conflict count.
#[test]
fn a_switched_variant_is_one_value() {
    let f = base();
    let sizing = |s| Operation::SetGeometry {
        id: f.text,
        geometry: GeometryPatch::TextSizing(s),
    };
    let ours = edited(&f.doc, vec![sizing(TextSizing::AutoHeight(120.0))]);
    let theirs = edited(
        &f.doc,
        vec![
            sizing(TextSizing::Fixed(Size::new(100.0, 50.0))),
            rename(f.a, "Theirs"),
        ],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(name(&doc, f.a), "Theirs");
    let NodeKind::Text { sizing, .. } = doc.get(f.text).unwrap().kind() else {
        panic!()
    };
    assert_eq!(*sizing, TextSizing::AutoHeight(120.0));
}

/// A fill added by one side and another fill recoloured by the other both land:
/// list items merge by id.
#[test]
fn list_items_merge_by_id() {
    let f = base();
    let fills = f.doc.get(f.a).unwrap().paint().fills.clone();
    let mut recoloured = fills.clone();
    recoloured[0].value.brush = Brush::Solid(Color::from_rgba8(0, 0, 255, 255)).into();
    let mut added = fills.clone();
    added.push(ondin_core::Keyed::new(
        ondin_core::ItemId(f.text),
        Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 255, 0, 255)).into(),
            visible: true,
        },
    ));
    let ours = edited(
        &f.doc,
        vec![Operation::SetFills {
            id: f.a,
            fills: recoloured,
        }],
    );
    let theirs = edited(
        &f.doc,
        vec![Operation::SetFills {
            id: f.a,
            fills: added,
        }],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    let got = &doc.get(f.a).unwrap().paint().fills;
    assert_eq!(got.len(), 2);
    assert_eq!(
        got[0].value.brush,
        Brush::Solid(Color::from_rgba8(0, 0, 255, 255)).into()
    );
}

/// Ours untouched gives theirs exactly, and theirs untouched gives ours.
#[test]
fn an_untouched_side_gives_the_other() {
    let f = base();
    let theirs = edited(&f.doc, vec![rename(f.a, "Theirs")]);
    let m = merge(&save(&f.doc), &save(&f.doc), &save(&theirs)).unwrap();
    assert_eq!(m.bytes, save(&theirs));
    let m = merge(&save(&f.doc), &save(&theirs), &save(&f.doc)).unwrap();
    assert_eq!(m.bytes, save(&theirs));
}

/// The ordered merge of id lists, case by case.
#[test]
fn merge_order_cases() {
    let v = |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_string).collect() };
    for (o, a, b, want) in [
        // Ours unchanged: theirs, reorders included.
        ("p q r", "p q r", "r q p", "r q p"),
        // Theirs unchanged: ours.
        ("p q r", "q p r", "p q r", "q p r"),
        // Ours reordered, theirs added after q.
        ("p q r", "r q p", "p q x r", "r q x p"),
        // Theirs removed q, ours added y first.
        ("p q r", "y p q r", "p r", "y p r"),
        // Theirs added at the front.
        ("p q", "q p", "x p q", "x q p"),
        // Both added the same id.
        ("p", "p x", "x p", "p x"),
    ] {
        assert_eq!(merge_order(&v(o), &v(a), &v(b)), v(want), "{o} / {a} / {b}");
    }
}
