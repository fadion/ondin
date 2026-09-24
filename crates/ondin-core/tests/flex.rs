//! Flex containers through the whole model (container layout's step 3, §15
//! D867, D869, D875) — the document, `Resolved`'s layout pass and history
//! together, with incremental `update` checked against a full `rebuild` after
//! every commit.
//!
//! `container`'s own `flex_tests` pin what the engine computes; these pin that it
//! is wired: an item's change moves its siblings (the chain-root widening), a
//! frame resize reflows, a group with a layout is its box, and taking a layout
//! away puts every child back where its transform says.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::{
    Document, GeometryPatch, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    container::{AlignItems, Display, Flex, FlexDirection, FlexItem},
};

fn rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
    }
}

/// A row: gap 10, padding 20, items aligned to the start so no cross-axis
/// stretch muddies the numbers.
fn row() -> Option<Display> {
    Some(Display::Flex(Flex {
        column_gap: 10.0,
        row_gap: 10.0,
        padding: [20.0; 4],
        align_items: AlignItems::Start,
        ..Default::default()
    }))
}

struct Scene {
    doc: Document,
    res: Resolved,
    history: History,
    ids: IdSource,
    root: NodeId,
}

impl Scene {
    fn new() -> Self {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let doc = Document::new(root);
        let res = Resolved::rebuild(&doc);
        Scene {
            doc,
            res,
            history: History::default(),
            ids,
            root,
        }
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind, at: (f64, f64)) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }]);
        id
    }

    fn commit(&mut self, ops: Vec<Operation>) {
        let tx = ondin_core::build::keep_insets(&self.doc, &self.res, Transaction(ops));
        let tx = ondin_core::build::keep_flex_sizes(&self.doc, &self.res, tx);
        let dirty = self
            .history
            .commit(&mut self.doc, tx)
            .expect("the edit applies");
        self.res.update(&self.doc, &dirty);
        self.assert_equal_to_rebuild();
    }

    fn assert_equal_to_rebuild(&self) {
        let fresh = Resolved::rebuild(&self.doc);
        for id in ondin_core::subtree_nodes(&self.doc, &[self.root]) {
            assert_eq!(
                self.res.used_local(&self.doc, id),
                fresh.used_local(&self.doc, id),
                "used local of {id:?}"
            );
            assert_eq!(
                self.res.used_kind(&self.doc, id),
                fresh.used_kind(&self.doc, id),
                "used kind of {id:?}"
            );
            assert_eq!(
                self.res.used_frame(id),
                fresh.used_frame(id),
                "used frame of {id:?}"
            );
            assert_eq!(
                self.res.world_bounds(id),
                fresh.world_bounds(id),
                "bounds of {id:?}"
            );
        }
    }

    fn display(&mut self, id: NodeId, display: Option<Display>) {
        self.commit(vec![Operation::SetDisplay { id, display }]);
    }

    fn item(&mut self, id: NodeId, f: impl FnOnce(&mut FlexItem)) {
        let mut item = *self.doc.get(id).unwrap().item();
        f(&mut item);
        self.commit(vec![Operation::SetFlexItem { id, item }]);
    }

    fn resize(&mut self, id: NodeId, w: f64, h: f64) {
        self.commit(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(w, h)),
        }]);
    }

    fn bounds(&self, id: NodeId) -> Rect {
        self.res.world_bounds(id).expect("measured")
    }
}

/// Giving a frame a row lays its children out; resizing one item moves the
/// next, **though the next was never edited** — the widening step 1 owed, held by
/// `update` staying equal to `rebuild` across it.
///
/// **Flip:** collecting `affected` from each dirty node instead of its
/// `chain_root` fails the rebuild comparison — **on the resized item itself**, not
/// on the sibling as predicted: with the frame outside `affected` its pass never
/// runs, so there is no laid result for anything, and even the edited item falls
/// back to its stored transform. It fails this test, the pinned-child test and
/// the nested-group test alike.
#[test]
fn resizing_one_item_moves_its_siblings() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (300.0, 150.0));
    let b = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    assert_eq!(s.bounds(a), Rect::new(20.0, 20.0, 60.0, 50.0));
    assert_eq!(s.bounds(b), Rect::new(70.0, 20.0, 130.0, 50.0));

    s.resize(a, 100.0, 30.0);
    assert_eq!(
        s.bounds(b).x0,
        130.0,
        "the sibling moved along: 20 + 100 + 10"
    );
}

/// A growing item follows the frame when the frame is resized.
#[test]
fn a_growing_item_follows_the_frame() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let grow = s.add(f, rect(10.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    s.item(grow, |i| i.grow = 1.0);
    assert_eq!(s.bounds(grow).width(), 400.0 - 20.0 - 40.0 - 10.0 - 20.0);
    s.resize(f, 600.0, 200.0);
    assert_eq!(s.bounds(grow).width(), 600.0 - 20.0 - 40.0 - 10.0 - 20.0);
    assert_eq!(
        s.doc.get(grow).unwrap().kind(),
        &rect(10.0, 30.0),
        "grown in used geometry only: the document keeps what was drawn"
    );
    let _ = a;
}

/// Resizing a growing item holds the size it was dragged to — `keep_flex_sizes`
/// turns growth off for it (§15 D875) — and resizing the cross axis of a
/// stretched one holds too, by taking it out of the stretch.
///
/// **Flip:** without `keep_flex_sizes` in the commit path the first assertion
/// fails — the item grows straight back to fill the row.
#[test]
fn resizing_a_growing_item_holds_the_size_it_was_dragged_to() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let grow = s.add(f, rect(10.0, 30.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(Flex {
            padding: [20.0; 4],
            ..Default::default()
        })),
    );
    s.item(grow, |i| i.grow = 1.0);
    assert_eq!(s.bounds(grow).width(), 360.0, "grown to fill");
    assert_eq!(s.bounds(grow).height(), 160.0, "and stretched across");

    s.resize(grow, 150.0, 160.0);
    assert_eq!(s.bounds(grow).width(), 150.0, "the dragged width held");
    let item = *s.doc.get(grow).unwrap().item();
    assert_eq!((item.grow, item.shrink), (0.0, 0.0));

    s.resize(grow, 150.0, 70.0);
    assert_eq!(s.bounds(grow).height(), 70.0, "the dragged height held");
    assert_eq!(
        s.doc.get(grow).unwrap().item().align_self,
        Some(AlignItems::Start)
    );
}

/// A group with a layout is a box: its bounds are padding plus items plus gaps,
/// and it grows when an item is added.
#[test]
fn a_group_with_a_layout_is_its_box() {
    let mut s = Scene::new();
    let g = s.add(s.root, NodeKind::Group, (50.0, 50.0));
    s.add(g, rect(40.0, 30.0), (0.0, 0.0));
    s.display(g, row());
    assert_eq!(
        s.bounds(g),
        Rect::new(
            50.0,
            50.0,
            50.0 + 20.0 + 40.0 + 20.0,
            50.0 + 20.0 + 30.0 + 20.0
        )
    );
    s.add(g, rect(60.0, 30.0), (999.0, 999.0));
    assert_eq!(
        s.bounds(g).width(),
        20.0 + 40.0 + 10.0 + 60.0 + 20.0,
        "the group grew to hold the new item, placed in the row whatever its transform"
    );
}

/// **A growing group with a layout, resized, holds the size it was given** —
/// `build::sized_flex_item`, the resize of a box with no size field (§15 D875).
/// Its `width`/`height` go to px, and because a transaction's own `SetFlexItem`
/// is left alone by `keep_flex_sizes`, the growth it would have stopped is
/// stopped here: `flex-grow`/`flex-shrink` 0 on the main axis, `align-self: start`
/// on a stretched cross axis — `resizing_a_growing_item_holds_the_size_it_was_
/// dragged_to`'s two holds, for a group.
///
/// **Flip run**, `sized_flex_item`'s `held` call deleted: fails on *"the dragged
/// width held"* at 360, the group growing straight back to fill the row — the
/// predicted site.
#[test]
fn resizing_a_growing_group_with_a_layout_holds_its_box() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(40.0, 30.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(Flex {
            padding: [20.0; 4],
            ..Default::default()
        })),
    );
    s.display(g, row());
    s.item(g, |i| i.grow = 1.0);
    assert_eq!(s.bounds(g).width(), 360.0, "grown to fill");
    assert_eq!(s.bounds(g).height(), 160.0, "and stretched across");

    let item = ondin_core::build::sized_flex_item(&s.doc, &s.res, g, Size::new(150.0, 70.0))
        .expect("a group with a layout takes a size");
    s.commit(vec![Operation::SetFlexItem { id: g, item }]);
    assert_eq!(s.bounds(g).width(), 150.0, "the dragged width held");
    assert_eq!(s.bounds(g).height(), 70.0, "the dragged height held");
    let item = *s.doc.get(g).unwrap().item();
    assert_eq!((item.grow, item.shrink), (0.0, 0.0));
    assert_eq!(item.align_self, Some(AlignItems::Start));
}

/// A pinned child of a flex frame is out of the flow and placed against the
/// frame's box; the row closes up without it.
#[test]
fn a_pinned_child_of_a_flex_frame_is_placed_against_the_frame() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let pinned = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let kept = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    s.commit(vec![Operation::SetInsets {
        id: pinned,
        insets: ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(0.0)),
            bottom: Some(ondin_core::LengthPct::Px(0.0)),
            ..Default::default()
        },
    }]);
    assert_eq!(s.bounds(pinned), Rect::new(360.0, 170.0, 400.0, 200.0));
    assert_eq!(s.bounds(kept).x0, 20.0, "the row closed up");
}

/// A group with a layout nested in a frame's row takes its slot from the row and
/// lays out its own column inside it.
#[test]
fn a_nested_group_lays_out_inside_its_slot() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let first = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    let top = s.add(g, rect(20.0, 20.0), (0.0, 0.0));
    let under = s.add(g, rect(20.0, 20.0), (0.0, 0.0));
    s.display(f, row());
    s.display(
        g,
        Some(Display::Flex(Flex {
            direction: FlexDirection::Column,
            row_gap: 5.0,
            align_items: AlignItems::Start,
            ..Default::default()
        })),
    );
    assert_eq!(
        s.bounds(g).origin(),
        (70.0, 20.0).into(),
        "after the first item"
    );
    assert_eq!(s.bounds(top).origin(), (70.0, 20.0).into());
    assert_eq!(
        s.bounds(under).origin(),
        (70.0, 45.0).into(),
        "a column, 5 apart"
    );
    // And a change deep in the chain re-lays it from the frame.
    s.resize(first, 80.0, 30.0);
    assert_eq!(s.bounds(under).x0, 110.0, "moved along with its group");
}

/// Taking a frame's layout away puts every child back where its transform says.
#[test]
fn removing_a_layout_puts_children_back_by_their_transforms() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (300.0, 150.0));
    s.display(f, row());
    assert_eq!(s.bounds(a).x0, 20.0);
    s.display(f, None);
    assert_eq!(s.bounds(a), Rect::new(300.0, 150.0, 340.0, 180.0));
}

/// Layout and item properties survive a save and a load, and a layer with
/// neither writes neither.
#[test]
fn display_and_item_round_trip_through_the_save_format() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let plain = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    s.item(a, |i| {
        i.grow = 2.0;
        i.align_self = Some(AlignItems::Center);
    });
    let bytes = ondin_core::io::save(&s.doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert_eq!(text.matches("\"display\"").count(), 1);
    assert_eq!(text.matches("\"item\"").count(), 1);
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(
        back.get(f).unwrap().display(),
        s.doc.get(f).unwrap().display()
    );
    assert_eq!(back.get(a).unwrap().item(), s.doc.get(a).unwrap().item());
    assert!(back.get(plain).unwrap().item().is_default());
}

/// **Dragging a flex item reorders it by where its centre falls in the flow**
/// (`build::flex_reorder`, §15 D877) — a row of three at x 20–60, 70–130 and
/// 140–160, so centres 40, 100 and 150, and a pinned fourth child out of the flow.
///
/// Past the second item's centre it takes the second place; past the last it
/// goes to the end of the flow, just after the last item in it — the pinned child
/// keeps its place after that (the first draft of this test expected the item
/// after the pinned child, which is not the rule `flex_reorder`'s doc states); short of
/// the next centre it lands where it was, which is no operation at all; the last
/// item dragged to the front goes first. Committed, the row lays out in the new
/// order. And `row-reverse` reads the main axis backwards: the same rightward
/// drag, in a row drawn right to left, moves the item towards the *start*.
///
/// **Flip run**, the `reversed` comparison dropped (`s_main < at_main` always):
/// fails on *"row-reverse reads backwards"*, the predicted site; the forward rows
/// pass either way, which is why the reverse case is here.
#[test]
fn a_dragged_flex_item_reorders_by_where_its_centre_falls() {
    use ondin_core::kurbo::Vec2;
    use ondin_core::{Insets, LengthPct as Length};
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let b = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    let c = s.add(f, rect(20.0, 30.0), (0.0, 0.0));
    let pinned = s.add(f, rect(10.0, 10.0), (0.0, 0.0));
    s.commit(vec![Operation::SetInsets {
        id: pinned,
        insets: Insets {
            top: Some(Length::Px(0.0)),
            left: Some(Length::Px(0.0)),
            ..Default::default()
        },
    }]);
    s.display(f, row());
    assert_eq!(s.bounds(b).x0, 70.0, "the fixture's row");

    let reorder = |s: &Scene, id, dx| match ondin_core::build::flex_reorder(
        &s.doc,
        &s.res,
        id,
        Vec2::new(dx, 0.0),
    ) {
        Some(Operation::Reorder { index, .. }) => Some(index),
        None => None,
        Some(other) => panic!("not a reorder: {other:?}"),
    };
    assert_eq!(
        reorder(&s, a, 70.0),
        Some(1),
        "past the second item's centre"
    );
    assert_eq!(
        reorder(&s, a, 200.0),
        Some(2),
        "past the last: the end of the flow, the pinned child keeping its place after it"
    );
    assert_eq!(
        reorder(&s, a, 20.0),
        None,
        "short of the next centre: no change"
    );
    assert_eq!(
        reorder(&s, c, -130.0),
        Some(0),
        "the last dragged to the front"
    );

    s.commit(vec![Operation::Reorder { id: a, index: 1 }]);
    assert_eq!(s.bounds(b).x0, 20.0, "committed: b leads the row now");
    assert_eq!(s.bounds(a).x0, 90.0, "and a follows it");

    s.display(
        f,
        Some(Display::Flex(Flex {
            direction: FlexDirection::RowReverse,
            column_gap: 10.0,
            padding: [20.0; 4],
            align_items: AlignItems::Start,
            ..Default::default()
        })),
    );
    // Right to left now: b at the right edge, a left of it, c left of that.
    assert!(s.bounds(b).x0 > s.bounds(a).x0, "the fixture reversed");
    assert_eq!(
        reorder(&s, a, 100.0),
        Some(0),
        "row-reverse reads backwards: right is the start"
    );
}

/// **A tool's `SetTransform` on an in-flow item keeps the item's stored
/// translation** (`build::keep_flex_sizes`' `kept_flow_translations`, §15 D877's
/// amendment) — a 40-wide rect stored at (300, 150) and laid at (20, 20).
///
/// The transaction is the one a left-handle resize to 60 wide was measured to
/// write: its `SetGeometry` and `SetTransform` translate(0, 20), the slot shifted.
/// Committed, the size lands and the translation does not — the write is dropped,
/// changing nothing once kept — so taking the layout away puts the rect back at
/// (300, 150), not at (0, 20). A rotation keeps its turn and loses its shift. And
/// an item the same transaction hides leaves the flow, so its transform stands.
///
/// **Flip run**, `keep_flex_sizes` handing `tx` on without the rewrite: fails on
/// *"the stored translation survives the resize"* at (0, 20) — the predicted
/// site.
#[test]
fn a_tools_transform_on_a_flex_item_keeps_its_stored_translation() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (300.0, 150.0));
    let b = s.add(f, rect(60.0, 30.0), (300.0, 150.0));
    s.display(f, row());
    assert_eq!(s.bounds(a).x0, 20.0, "the fixture lays it at the slot");

    s.commit(vec![
        Operation::SetGeometry {
            id: a,
            geometry: GeometryPatch::Size(Size::new(60.0, 30.0)),
        },
        Operation::SetTransform {
            id: a,
            transform: Affine::translate((0.0, 20.0)),
        },
    ]);
    assert_eq!(s.bounds(a).width(), 60.0, "the size landed");
    assert_eq!(
        s.doc.get(a).unwrap().transform().translation(),
        Vec2::new(300.0, 150.0),
        "the stored translation survives the resize"
    );

    s.commit(vec![Operation::SetTransform {
        id: a,
        transform: Affine::translate((5.0, 5.0)) * Affine::rotate(0.5),
    }]);
    let t = s.doc.get(a).unwrap().transform();
    assert_eq!(
        t.translation(),
        Vec2::new(300.0, 150.0),
        "a turn loses its shift"
    );
    assert!(
        (t.as_coeffs()[1] - 0.5f64.sin()).abs() < 1e-12,
        "and keeps its turn"
    );

    s.commit(vec![
        Operation::SetTransform {
            id: b,
            transform: Affine::translate((7.0, 7.0)),
        },
        Operation::SetVisible {
            id: b,
            visible: false,
        },
    ]);
    assert_eq!(
        s.doc.get(b).unwrap().transform().translation(),
        Vec2::new(7.0, 7.0),
        "hidden in the same edit, it leaves the flow and its transform stands"
    );

    s.display(f, None);
    assert!(
        s.bounds(a).x0 > 250.0,
        "the rect goes back by its own transform, near (300, 150), not to (0, 20)"
    );
}
