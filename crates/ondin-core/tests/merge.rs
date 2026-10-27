//! `merge::merge`, the three-way merge behind `ondin merge`: two edits of one
//! saved document combined, each case built as a base, ours and theirs.

use ondin_core::kurbo::{Affine, BezPath, RoundedRectRadii, Size};
use ondin_core::merge::{merge, merge_order};
use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleId, StyleValue};
use ondin_core::variant::{VariantProp, VariantSet};
use ondin_core::{
    Brush, Cell, CharAttr, CharSpans, Document, Fill, GeometryPatch, Guide, GuideAxis, GuideId,
    IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, Mode, ModeId, NodeId, NodeKind,
    Operation, TextSizing, TextStyle, Transaction, VarId, VarKind, Variable, image_brush, io,
    keyed_by_position,
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
        // Theirs reordered, ours added at the end: theirs' order, ours' id last.
        ("p q r", "p q r y", "r p q", "r p q y"),
        // Theirs reordered, ours added after p.
        ("p q r", "p y q r", "r p q", "r p y q"),
        // Theirs reordered, ours removed q.
        ("p q r", "p r", "r q p", "r p"),
        // Both reordered alike, ours added.
        ("p q r", "r q p y", "r q p", "r q p y"),
    ] {
        let (got, clash) = merge_order(&v(o), &v(a), &v(b));
        assert_eq!(got, v(want), "{o} / {a} / {b}");
        assert!(!clash, "{o} / {a} / {b}");
    }
}

/// Both sides reordering differently is a conflict, ours' order kept.
///
/// flip: `merge_order` never reporting a clash → fails at the clash assertion.
#[test]
fn merge_order_both_reordered_differently_clashes() {
    let v = |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_string).collect() };
    let (got, clash) = merge_order(&v("p q r"), &v("q p r"), &v("p r q"));
    assert_eq!(got, v("q p r"));
    assert!(clash);
}

/// A tiny embedded picture's table entry.
fn picture(bytes: &[u8]) -> ImageEntry {
    ImageEntry {
        source: ImageSource::Embedded(bytes.to_vec().into()),
        format: ImageFormat::Png,
        width: 4,
        height: 4,
    }
}

fn image_fill(id: NodeId, image: &ImageId) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: image_brush(image.clone()).into(),
            visible: true,
        }]),
    }
}

/// Ours replaced a picture (dropping the old table entry) while theirs put the
/// old picture on a new layer: the entry theirs still uses survives the merge.
///
/// flip: `keep_used_images` doing nothing → the record is merged away and fails
/// at `has_image(old)`.
#[test]
fn a_removed_image_a_new_layer_uses_is_kept() {
    let mut f = base();
    let (old, new) = (ImageId("sha:old".into()), ImageId("sha:new".into()));
    let start = edited(
        &f.doc,
        vec![
            Operation::AddImage {
                id: old.clone(),
                entry: picture(&[1, 2, 3]),
            },
            image_fill(f.a, &old),
        ],
    );
    let ours = edited(
        &start,
        vec![
            Operation::AddImage {
                id: new.clone(),
                entry: picture(&[4, 5, 6]),
            },
            image_fill(f.a, &new),
            Operation::RemoveImage { id: old.clone() },
        ],
    );
    let n2 = f.ids.mint();
    let theirs = edited(
        &start,
        vec![
            Operation::CreateNode {
                id: n2,
                parent: f.frame,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(5.0, 5.0),
                },
                transform: None,
                name: Some("N2".into()),
            },
            image_fill(n2, &old),
        ],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert!(doc.get(n2).is_some());
    assert!(doc.has_image(&old), "theirs' layer still draws it");
    assert!(doc.has_image(&new));
    assert_eq!(doc.image(&old), start.image(&old));
}

/// A picture nothing uses any more is still removed by the side that removed it.
#[test]
fn an_unused_image_removed_by_one_side_is_gone() {
    let f = base();
    let old = ImageId("sha:old".into());
    let start = edited(
        &f.doc,
        vec![Operation::AddImage {
            id: old.clone(),
            entry: picture(&[1, 2, 3]),
        }],
    );
    let ours = edited(&start, vec![Operation::RemoveImage { id: old.clone() }]);
    let theirs = edited(&start, vec![rename(f.a, "Theirs")]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert!(!doc.has_image(&old));
    assert_eq!(name(&doc, f.a), "Theirs");
}

/// `f`'s text set to `content` with no overrides.
fn retyped(f: &Base, doc: &Document, content: &str) -> Document {
    edited(
        doc,
        vec![Operation::SetText {
            id: f.text,
            content: content.into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }],
    )
}

fn text_of(doc: &Document, id: NodeId) -> (String, CharSpans) {
    let NodeKind::Text { content, spans, .. } = doc.get(id).unwrap().kind() else {
        panic!("a text")
    };
    (content.clone(), spans.clone())
}

/// Text retyped on one side and styled on the other is one value: the byte
/// ranges of theirs' spans do not land on ours' characters.
///
/// flip: `coupled` answering no group for a text → the spans of theirs land on
/// ours' content cleanly and fails at the conflict count.
#[test]
fn text_content_and_spans_are_one_value() {
    let f = base();
    let start = retyped(&f, &f.doc, "Hello world");
    let ours = retyped(&f, &start, "Hi there, world");
    let NodeKind::Text { style, .. } = start.get(f.text).unwrap().kind() else {
        panic!()
    };
    let mut bold = CharSpans::default();
    bold.set(6..11, CharAttr::Weight(700), style);
    let theirs = edited(
        &start,
        vec![
            Operation::SetTextSpans {
                id: f.text,
                spans: bold,
            },
            rename(f.a, "Theirs"),
        ],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(text_of(&doc, f.text), text_of(&ours, f.text));
    assert_eq!(name(&doc, f.a), "Theirs", "the rest of theirs lands");
}

/// Spans styled on one side alone land with the base's text.
#[test]
fn spans_changed_on_one_side_land() {
    let f = base();
    let start = retyped(&f, &f.doc, "Hello world");
    let NodeKind::Text { style, .. } = start.get(f.text).unwrap().kind() else {
        panic!()
    };
    let mut bold = CharSpans::default();
    bold.set(6..11, CharAttr::Weight(700), style);
    let theirs = edited(
        &start,
        vec![Operation::SetTextSpans {
            id: f.text,
            spans: bold,
        }],
    );
    let ours = edited(&start, vec![rename(f.a, "Ours")]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(text_of(&doc, f.text), text_of(&theirs, f.text));
}

/// A path's outline and its per-anchor radii are one value.
#[test]
fn a_paths_outline_and_radii_are_one_value() {
    let mut f = base();
    let p = f.ids.mint();
    let square = |extra: bool| {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 0.0));
        if extra {
            b.line_to((10.0, 5.0));
        }
        b.line_to((10.0, 10.0));
        b.line_to((0.0, 10.0));
        b.close_path();
        b
    };
    let start = edited(
        &f.doc,
        vec![Operation::CreateNode {
            id: p,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Path {
                path: square(false),
                corner_radii: Vec::new(),
            },
            transform: None,
            name: Some("P".into()),
        }],
    );
    let geometry = |path, corner_radii| Operation::SetGeometry {
        id: p,
        geometry: GeometryPatch::Path { path, corner_radii },
    };
    let ours = edited(&start, vec![geometry(square(true), Vec::new())]);
    let theirs = edited(
        &start,
        vec![geometry(square(false), vec![0.0, 0.0, 3.0, 0.0])],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(doc.get(p).unwrap().kind(), ours.get(p).unwrap().kind());
}

/// Theirs brought a layer to the bottom while ours added one on top: both land.
///
/// flip: `merge_order` keeping ours' order when only theirs reordered → fails at
/// the children assertion.
#[test]
fn theirs_reorder_lands_beside_ours_addition() {
    let mut f = base();
    let y = f.ids.mint();
    let ours = edited(
        &f.doc,
        vec![Operation::CreateNode {
            id: y,
            parent: f.frame,
            index: 3,
            kind: NodeKind::Ellipse {
                size: Size::new(5.0, 5.0),
            },
            transform: None,
            name: Some("Y".into()),
        }],
    );
    let theirs = edited(
        &f.doc,
        vec![Operation::Reparent {
            id: f.text,
            new_parent: f.frame,
            index: 0,
        }],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(doc.get(f.frame).unwrap().children(), &[f.text, f.a, f.b, y]);
}

/// Both sides reordering one list differently is reported, ours' order kept.
#[test]
fn both_reordering_layers_is_a_conflict() {
    let f = base();
    let to = |id, index| Operation::Reparent {
        id,
        new_parent: f.frame,
        index,
    };
    let ours = edited(&f.doc, vec![to(f.text, 0)]);
    let theirs = edited(&f.doc, vec![to(f.a, 2)]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert!(conflicts[0].contains("children"), "{conflicts:?}");
    assert_eq!(
        doc.get(f.frame).unwrap().children(),
        ours.get(f.frame).unwrap().children()
    );
}

fn add_page(id: NodeId, index: usize, name: &str) -> Operation {
    Operation::AddPage {
        id,
        index,
        name: name.into(),
        ground: Color::from_rgba8(240, 240, 240, 255),
        draft: false,
        mode: None,
    }
}

fn page_ids(doc: &Document) -> Vec<NodeId> {
    doc.pages().iter().map(|p| p.id).collect()
}

/// A page each side added at the end both arrive, theirs after ours.
#[test]
fn pages_added_on_both_sides_both_arrive() {
    let mut f = base();
    let (p1, p2) = (f.ids.mint(), f.ids.mint());
    let first = f.doc.first_page();
    let ours = edited(&f.doc, vec![add_page(p1, 1, "Ours")]);
    let theirs = edited(&f.doc, vec![add_page(p2, 1, "Theirs")]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(page_ids(&doc), [first, p1, p2]);
}

/// Theirs moved a page while ours added one: theirs' order, ours' page in it.
#[test]
fn theirs_page_move_lands_beside_ours_new_page() {
    let mut f = base();
    let (p1, p2) = (f.ids.mint(), f.ids.mint());
    let first = f.doc.first_page();
    let start = edited(&f.doc, vec![add_page(p1, 1, "Second")]);
    let ours = edited(&start, vec![add_page(p2, 2, "Third")]);
    let theirs = edited(&start, vec![Operation::MovePage { id: p1, index: 0 }]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(page_ids(&doc), [p1, first, p2]);
}

/// A page ours removed while theirs added a layer on it cannot hold together:
/// ours whole, reported.
#[test]
fn a_page_removed_by_ours_and_used_by_theirs_is_a_conflict() {
    let mut f = base();
    let (p1, r) = (f.ids.mint(), f.ids.mint());
    let start = edited(&f.doc, vec![add_page(p1, 1, "Second")]);
    let ours = edited(&start, vec![Operation::RemovePage { id: p1 }]);
    let theirs = edited(
        &start,
        vec![Operation::CreateNode {
            id: r,
            parent: p1,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(5.0, 5.0),
            },
            transform: None,
            name: Some("R".into()),
        }],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(!conflicts.is_empty());
    assert_eq!(page_ids(&doc), page_ids(&ours));
}

/// `f.doc` with one mode, `Light`.
fn with_a_mode(f: &mut Base) -> (Document, ModeId) {
    let light = f.ids.mint_mode();
    let doc = edited(
        &f.doc,
        vec![Operation::AddMode {
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: Vec::new(),
        }],
    );
    (doc, light)
}

fn number_var(id: VarId, name: &str, mode: ModeId) -> Operation {
    Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::Number,
            cells: [(mode, Cell::Number(4.0))].into(),
            origin: None,
            forked: None,
            description: String::new(),
        },
    }
}

/// A variable each side added both arrive.
#[test]
fn variables_added_on_both_sides_both_arrive() {
    let mut f = base();
    let (start, light) = with_a_mode(&mut f);
    let (v1, v2) = (f.ids.mint_var(), f.ids.mint_var());
    let ours = edited(&start, vec![number_var(v1, "space/s", light)]);
    let theirs = edited(&start, vec![number_var(v2, "space/m", light)]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert!(doc.variables().get(v1).is_some());
    assert!(doc.variables().get(v2).is_some());
}

/// A variable added by ours lacks a cell in the mode theirs added: the table
/// does not hold together, so ours is kept whole and reported.
#[test]
fn a_variable_against_a_new_mode_is_a_conflict() {
    let mut f = base();
    let (start, light) = with_a_mode(&mut f);
    let (v1, dark) = (f.ids.mint_var(), f.ids.mint_mode());
    let ours = edited(&start, vec![number_var(v1, "space/s", light)]);
    let theirs = edited(
        &start,
        vec![Operation::AddMode {
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: Vec::new(),
        }],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(!conflicts.is_empty());
    assert!(doc.variables().get(v1).is_some(), "ours, whole");
    assert!(!doc.variables().has_mode(dark));
}

fn fill_style(id: StyleId, name: &str) -> Style {
    Style {
        id,
        name: name.into(),
        value: StyleValue::Fills(keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 255, 255)).into(),
            visible: true,
        }])),
        origin: None,
        forked: None,
        description: String::new(),
    }
}

/// A style each side added both arrive.
#[test]
fn styles_added_on_both_sides_both_arrive() {
    let mut f = base();
    let (s1, s2) = (f.ids.mint_style(), f.ids.mint_style());
    let ours = edited(
        &f.doc,
        vec![Operation::AddStyle {
            style: fill_style(s1, "brand/one"),
        }],
    );
    let theirs = edited(
        &f.doc,
        vec![Operation::AddStyle {
            style: fill_style(s2, "brand/two"),
        }],
    );
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert!(doc.styles().get(s1).is_some());
    assert!(doc.styles().get(s2).is_some());
}

/// A style ours deleted while theirs applied it is reported, ours whole.
#[test]
fn a_style_deleted_against_applied_is_a_conflict() {
    let mut f = base();
    let s = f.ids.mint_style();
    let start = edited(
        &f.doc,
        vec![Operation::AddStyle {
            style: fill_style(s, "brand/one"),
        }],
    );
    let ours = edited(&start, vec![Operation::RemoveStyle { id: s }]);
    let theirs = edited(
        &start,
        vec![Operation::SetFillStyle {
            id: f.a,
            style: Some(s),
        }],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(!conflicts.is_empty());
    assert!(doc.styles().get(s).is_none(), "ours, whole");
}

/// A guide added by one side and another moved by the other both land.
#[test]
fn guides_merge_by_id() {
    let mut f = base();
    let guide = |id, position| Guide {
        id: GuideId(id),
        axis: GuideAxis::Horizontal,
        position,
        color: None,
        owner: f.doc.first_page(),
    };
    let (g0, g1) = (f.ids.mint(), f.ids.mint());
    let start = edited(
        &f.doc,
        vec![Operation::AddGuide {
            guide: guide(g0, 10.0),
        }],
    );
    let ours = edited(
        &start,
        vec![Operation::AddGuide {
            guide: guide(g1, 50.0),
        }],
    );
    let theirs = edited(
        &start,
        vec![Operation::SetGuidePosition {
            id: GuideId(g0),
            position: 20.0,
        }],
    );
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(doc.guide(GuideId(g0)).unwrap().position, 20.0);
    assert_eq!(doc.guide(GuideId(g1)).unwrap().position, 50.0);
}

/// A vault theirs opened arrives beside ours' edit.
#[test]
fn a_vault_opened_by_one_side_arrives() {
    let mut f = base();
    let v = f.ids.mint();
    let ours = edited(&f.doc, vec![rename(f.a, "Ours")]);
    let theirs = edited(&f.doc, vec![Operation::OpenVault { id: v }]);
    let (doc, conflicts) = merged(&f.doc, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(doc.vault(), Some(v));
    assert_eq!(name(&doc, f.a), "Ours");
}

/// A variant's values are positional, one per property: ours' change of one
/// property and theirs' of another both land.
///
/// flip: `merge3` merging every list of distinct strings as an ordered set →
/// the values scramble, the set refuses them, and fails at the no-conflict
/// assertion.
#[test]
fn a_variants_values_merge_by_position() {
    let mut f = base();
    let (set, var) = (f.ids.mint(), f.ids.mint());
    let start = edited(
        &f.doc,
        vec![
            Operation::CreateNode {
                id: set,
                parent: f.doc.first_page(),
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 100.0),
                },
                transform: None,
                name: Some("Button".into()),
            },
            Operation::CreateNode {
                id: var,
                parent: set,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 30.0),
                },
                transform: None,
                name: Some("Variant".into()),
            },
            Operation::SetComponent {
                id: var,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![
                        VariantProp {
                            name: "Size".into(),
                            values: vec!["S".into(), "L".into()],
                        },
                        VariantProp {
                            name: "Tone".into(),
                            values: vec!["Light".into(), "Dark".into()],
                        },
                    ],
                }),
            },
            Operation::SetVariant {
                id: var,
                values: vec!["S".into(), "Light".into()],
            },
        ],
    );
    let values = |a: &str, b: &str| Operation::SetVariant {
        id: var,
        values: vec![a.into(), b.into()],
    };
    let ours = edited(&start, vec![values("L", "Light")]);
    let theirs = edited(&start, vec![values("S", "Dark"), rename(f.a, "Theirs")]);
    let (doc, conflicts) = merged(&start, &ours, &theirs);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    assert_eq!(doc.get(var).unwrap().variant(), ["L", "Dark"]);
    assert_eq!(name(&doc, f.a), "Theirs");
}

/// An empty base — git's for a file both branches added — merges two ways: what
/// only one side holds arrives, and every value the sides hold differently is a
/// conflict, ours kept.
///
/// flip: reading the empty base as JSON → `merge` errs and fails at `unwrap`.
#[test]
fn an_empty_base_merges_two_ways() {
    let mut f = base();
    let y = f.ids.mint();
    let ours = edited(&f.doc, vec![rename(f.a, "Ours")]);
    let theirs = edited(
        &f.doc,
        vec![Operation::CreateNode {
            id: y,
            parent: f.frame,
            index: 3,
            kind: NodeKind::Ellipse {
                size: Size::new(5.0, 5.0),
            },
            transform: None,
            name: Some("Y".into()),
        }],
    );
    let m = merge(b"", &save(&ours), &save(&theirs)).unwrap();
    let doc = io::load(&m.bytes).unwrap();
    assert_eq!(name(&doc, f.a), "Ours");
    assert!(doc.get(y).is_some(), "theirs' layer arrives");
    assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
    assert!(m.conflicts[0].contains(".name"), "{:?}", m.conflicts);
}
