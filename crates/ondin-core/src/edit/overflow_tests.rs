//! A resize whose product overflows, and the member that cannot take it.

use super::*;
use crate::kurbo::{RoundedRectRadii, Size};
use crate::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

/// Two rects in a frame: one whose world basis is `2¹⁰²³`, the largest finite power
/// of two, over a size of `25·2⁻¹⁰²¹` (world bounds `(0,0)–(100,100)`), and a plain
/// 20×20 at x=80.
///
/// The world scale is built from 53 nested groups of `2¹⁹` and the rect's own `2¹⁶`,
/// each within `build::GEOMETRY_LIMIT`: one stored coefficient cannot reach it, a
/// composition can. Powers of two keep every product exact.
fn a_pathological_member_and_an_ordinary_one() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5E1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
    let mut ops = vec![Operation::CreateNode {
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
    }];
    let mut parent = ab;
    for _ in 0..53 {
        let g = ids.mint();
        ops.push(Operation::CreateNode {
            id: g,
            parent,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::scale(f64::from(1u32 << 19))),
            name: None,
        });
        parent = g;
    }
    let side = 25.0 * 2f64.powi(-1021);
    ops.extend([
        Operation::CreateNode {
            id: a,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(side, side),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::scale(f64::from(1u32 << 16))),
            name: None,
        },
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
    ]);
    doc.apply(&Transaction(ops))
        .expect("the fixture is an ordinary, all-finite document");
    let res = Resolved::rebuild(&doc);
    (doc, res, a, b)
}

/// A member whose scaled basis overflows to `inf` sits the resize out alone: the ordinary
/// rect still scales, the pathological one is unchanged, and the transaction applies.
///
/// Coverage note: `scaled_about` returning `Some(out)` unchecked does not fail this,
/// because `split_geometry_scale` refuses the infinite basis next; the test pins the
/// pair, not either guard.
#[test]
fn one_overflowing_member_does_not_refuse_the_whole_resize() {
    let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
    let before = doc.get(a).expect("a is there").transform().as_coeffs();
    let union = [a, b]
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|x, y| x.union(y))
        .expect("both resolve");
    assert_eq!(
        (union.width(), union.height()),
        (100.0, 100.0),
        "the fixture's union has to be ordinary, or the drag below is not one"
    );

    let tx = resize_selection(
        &doc,
        &res,
        &[a, b],
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(200.0, 200.0),
        Resize::geometry(false, false),
    );
    let applied = doc.apply(&tx);

    assert!(
        tx.0.iter().any(|op| matches!(
            op,
            Operation::SetGeometry { id, .. } | Operation::SetTransform { id, .. } if *id == b
        )),
        "the ordinary member has to be scaled — refusing per gesture is what \
         this fixes: {:?}",
        tx.0
    );
    assert_eq!(
        doc.get(a).expect("a survives").transform().as_coeffs(),
        before,
        "and the member that cannot take the scale is left exactly as it was"
    );
    assert!(
        applied.is_ok(),
        "the transaction has to reach the document: {applied:?}"
    );
}

/// The document the resize leaves behind still saves and loads. Coverage note: this
/// passes without the per-member fix, because the op layer refuses non-finite transforms.
#[test]
fn the_document_a_resize_leaves_behind_still_loads() {
    let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
    let union = [a, b]
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|x, y| x.union(y))
        .expect("both resolve");
    let _ = doc.apply(&resize_selection(
        &doc.clone(),
        &res,
        &[a, b],
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(200.0, 200.0),
        Resize::geometry(false, false),
    ));
    let bytes = crate::io::save(&doc).expect("it saves");
    assert!(
        crate::io::load(&bytes).is_ok(),
        "a document the app just wrote has to be one the app can read"
    );
}
