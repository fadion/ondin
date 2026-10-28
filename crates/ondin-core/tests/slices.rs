//! Slices (`NodeKind::Slice`): a box that is hit only along its edge, lends its
//! group no bounds, stays out of a layout's flow, and survives a save.

use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction, hit_test, io,
};

/// A page holding a group of a 100×100 rect at the origin and a 100×100 slice at
/// (50, 50), half over it.
fn fixture() -> (Document, NodeId, NodeId, NodeId, NodeId) {
    let mut ids = IdSource::new(0x511C);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (group, rect, slice) = (ids.mint(), ids.mint(), ids.mint());
    let square = Size::new(100.0, 100.0);
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: page,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: square,
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
    ]))
    .expect("the group and rect");
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: slice,
        parent: group,
        index: 1,
        kind: NodeKind::Slice { size: square },
        transform: Some(Affine::translate((50.0, 50.0))),
        name: None,
    }]))
    .expect("the slice");
    (doc, page, group, rect, slice)
}

/// A click inside a slice picks what is under it; only a band along its edge picks
/// the slice, and an exact test never does.
/// flip: `contains_local`'s slice arm testing the whole box → fails at "passes
/// through".
#[test]
fn a_slice_is_hit_only_along_its_edge() {
    let (doc, page, _, rect, slice) = fixture();
    let res = Resolved::rebuild(&doc);
    let at = |x: f64, y: f64, slop: f64| hit_test(&res, &doc, page, (x, y).into(), slop);
    assert_eq!(
        at(70.0, 70.0, 4.0),
        vec![rect],
        "inside, a click passes through"
    );
    assert_eq!(at(148.0, 120.0, 4.0), vec![slice], "on the edge, the slice");
    assert!(
        at(148.0, 120.0, 0.0).is_empty(),
        "an exact test never lands on one"
    );
}

/// A slice adds nothing to its group's box.
/// flip: `lends_bounds` answering `true` → fails at the group's bounds.
#[test]
fn a_slice_lends_its_group_no_bounds() {
    let (doc, _, group, _, slice) = fixture();
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(group),
        Some(Rect::new(0.0, 0.0, 100.0, 100.0))
    );
    assert_eq!(
        res.world_bounds(slice),
        Some(Rect::new(50.0, 50.0, 150.0, 150.0)),
        "it still has its own"
    );
    assert_eq!(
        ondin_core::local_box(&doc, &res, group),
        Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
        "nor to the local box the handles are drawn on"
    );
}

/// A slice saves and loads as it was.
#[test]
fn a_slice_survives_a_save() {
    let (doc, _, _, _, slice) = fixture();
    let back = io::load(&io::save(&doc).expect("save")).expect("load");
    assert_eq!(
        back.get(slice).map(|n| n.kind().clone()),
        Some(NodeKind::Slice {
            size: Size::new(100.0, 100.0)
        })
    );
}
