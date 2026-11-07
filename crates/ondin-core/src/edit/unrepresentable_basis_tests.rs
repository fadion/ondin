//! A member whose finite basis has no exact `orientation · scale` split, which gestures
//! must leave alone; distinct from `overflow_tests`, whose basis overflows to `inf`.

use super::*;
use crate::kurbo::{RoundedRectRadii, Size};
use crate::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

/// One rect storing a rank-1 transform (both columns on `y = 2x`), which is finite and
/// so stored by the op layer, beside an ordinary sibling.
fn a_rank_one_member() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5E2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0])),
            name: None,
        },
        // The ordinary sibling that still resizes.
        Operation::CreateNode {
            id: b,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((80.0, 0.0))),
            name: None,
        },
    ]))
    .expect("a rank-1 transform is finite, so the op layer stores it");
    let res = Resolved::rebuild(&doc);
    (doc, res, a, b)
}

/// A rotation or multi-select resize leaves a member with no exact basis split
/// untouched instead of collapsing it, while its ordinary sibling still resizes.
/// The determinant is pinned first so the fixture really is singular.
///
/// flip: `Basis::of` back inside `split_geometry_scale` → fails at "a basis with no exact split refuses the turn"
/// flip: the old splitter at `resize_selection`'s call site alone → fails at "the rank-1 member is untouched"
#[test]
fn a_rank_one_member_is_left_alone_rather_than_collapsed() {
    let (doc, res, a, b) = a_rank_one_member();
    let before = doc.get(a).expect("a is there").transform().as_coeffs();
    assert_eq!(
        Affine::new(before).determinant(),
        0.0,
        "the fixture is singular, or this test is about nothing"
    );

    let turned = rotate_node(&doc, &res, a, Point::new(0.0, 0.0), 0.5);
    assert!(
        turned.0.is_empty(),
        "a basis with no exact split refuses the turn; got {:?}",
        turned.0
    );

    let mut doc = doc.clone();
    let union = [a, b]
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|x, y| x.union(y))
        .expect("both resolve");
    let b_before = doc.get(b).expect("b is there").transform().as_coeffs();
    let tx = resize_selection(
        &doc.clone(),
        &res,
        &[a, b],
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(400.0, 400.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).expect("the surviving ops are all finite");
    assert_eq!(
        doc.get(a).expect("a is there").transform().as_coeffs(),
        before,
        "the rank-1 member is untouched rather than rewritten"
    );
    assert_ne!(
        doc.get(b).expect("b is there").transform().as_coeffs(),
        b_before,
        "and its ordinary sibling still resized"
    );
}
