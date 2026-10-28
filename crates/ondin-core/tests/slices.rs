//! Slices (`NodeKind::Slice`): a box that is hit only along its edge, lends its
//! group no bounds, stays out of a layout's flow, and survives a save.

use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction, build, hit_test, io,
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

/// *Export all* takes a slice with settings while it shows and leaves it out once
/// hidden; any other hidden layer with settings still exports.
/// flip: `exported_in_bulk` ignoring visibility → fails at "a hidden slice".
#[test]
fn export_all_leaves_out_a_hidden_slice_only() {
    let (mut doc, _, group, rect, slice) = fixture();
    let png = ondin_core::ExportSpec::new(
        ondin_core::ExportFormat::Png,
        ondin_core::ExportScale::Times(1.0),
    );
    let mut ops = Vec::new();
    for id in [rect, slice] {
        ops.push(Operation::SetExports {
            id,
            exports: ondin_core::keyed_by_position([png.clone()]),
        });
        ops.push(Operation::SetVisible { id, visible: true });
    }
    doc.apply(&Transaction(ops)).expect("settings");
    assert!(build::exported_in_bulk(&doc, slice), "a slice that shows");
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: group,
        visible: false,
    }]))
    .expect("hide the group");
    assert!(
        !build::exported_in_bulk(&doc, slice),
        "a slice in a hidden group"
    );
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: group,
        visible: true,
    }]))
    .expect("show the group");
    for id in [rect, slice] {
        doc.apply(&Transaction(vec![Operation::SetVisible {
            id,
            visible: false,
        }]))
        .expect("hide");
    }
    assert!(!build::exported_in_bulk(&doc, slice), "a hidden slice");
    assert!(
        build::exported_in_bulk(&doc, rect),
        "a hidden rect still exports"
    );
}

/// *Slice selection* fits the selection's bounds rounded out to whole units, goes
/// just above the topmost selected layer in their common parent, and takes a lone
/// layer's name.
/// flip: `slice_selection`'s index without its `+ 1` → fails at "above the
/// topmost".
#[test]
fn slice_selection_fits_rounds_out_and_sits_above_the_topmost() {
    let (mut doc, page, group, rect, _) = fixture();
    let other = IdSource::new(0x0711).mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: other,
        parent: page,
        index: 1,
        kind: NodeKind::Rect {
            size: Size::new(20.0, 20.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((200.3, 10.6))),
        name: Some("Badge".into()),
    }]))
    .expect("a second rect");
    let res = Resolved::rebuild(&doc);

    let new = IdSource::new(0x0712).mint();
    let tx = build::slice_selection(&doc, &res, &[rect, other], new).expect("a slice");
    doc.apply(&tx).expect("apply");
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(new),
        Some(Rect::new(0.0, 0.0, 221.0, 100.0)),
        "rounded out"
    );
    let children = doc.get(page).unwrap().children().to_vec();
    assert_eq!(
        children,
        vec![group, other, new],
        "in their common parent, above the topmost"
    );
    let name = doc.get(new).unwrap().name().to_string();
    assert!(name.starts_with("Slice"), "a slice's own name: {name}");

    let lone = IdSource::new(0x0713).mint();
    let tx = build::slice_selection(&doc, &res, &[other], lone).expect("a slice");
    doc.apply(&tx).expect("apply");
    assert_eq!(
        doc.get(lone).unwrap().name(),
        "Badge",
        "a lone layer's name"
    );
    assert_eq!(
        ondin_core::item::values(doc.get(lone).unwrap().exports()).len(),
        1,
        "one export spec"
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
