//! Flex containers through the whole model (container layout's step 3, §15
//! D867, D869, D875) — the document, `Resolved`'s layout pass and history
//! together, with incremental `update` checked against a full `rebuild` after
//! every commit.
//!
//! `container`'s own `flex_tests` pin what the engine computes; these pin that it
//! is wired: an item's change moves its siblings (the chain-root widening), a
//! frame resize reflows, a group with a layout is its box, and taking a layout
//! away puts every child back where its transform says.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size, Vec2};
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

/// **A stretched item dragged by its top keeps its bottom** (§15 D905): the
/// released stretch aligns to the end of the line, not the start, so the edge the
/// user did not touch is where it lands. Dragged by its bottom it aligns to the
/// start as before; a laid group resized by its top does the same as a shape
/// (`build::sized_flex_item`'s `to`). The top-handle resize is written as the tool
/// writes it — the size, and the slot's transform shifted down by what the height
/// lost (`tools::resize_geometry`).
///
/// **Flip run**, `resized_from_cross_start` answering `false`: fails on *"the
/// bottom held"*, the rect at y 20..90 — the predicted site.
#[test]
fn a_stretched_item_dragged_by_its_top_aligns_to_the_end() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let top = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let bottom = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(Flex {
            column_gap: 10.0,
            padding: [20.0; 4],
            ..Default::default()
        })),
    );
    s.display(g, row());
    assert_eq!(s.bounds(top).height(), 160.0, "stretched across");

    let slot = s.res.used_local(&s.doc, top).unwrap();
    s.commit(vec![
        Operation::SetGeometry {
            id: top,
            geometry: GeometryPatch::Size(Size::new(40.0, 70.0)),
        },
        Operation::SetTransform {
            id: top,
            transform: slot * Affine::translate((0.0, 90.0)),
        },
    ]);
    assert_eq!(
        (s.bounds(top).y0, s.bounds(top).y1),
        (110.0, 180.0),
        "the bottom held"
    );
    assert_eq!(
        s.doc.get(top).unwrap().item().align_self,
        Some(AlignItems::End)
    );

    s.resize(bottom, 40.0, 70.0);
    assert_eq!(
        (s.bounds(bottom).y0, s.bounds(bottom).y1),
        (20.0, 90.0),
        "dragged by its bottom, the top held"
    );
    assert_eq!(
        s.doc.get(bottom).unwrap().item().align_self,
        Some(AlignItems::Start)
    );

    let slot = s.res.used_local(&s.doc, g).unwrap();
    let to = slot * Affine::translate((0.0, 60.0));
    let item = ondin_core::build::sized_flex_item(
        &s.doc,
        &s.res,
        g,
        Size::new(s.bounds(g).width(), 100.0),
        Some(to),
    )
    .expect("a group with a layout takes a size");
    assert_eq!(item.align_self, Some(AlignItems::End), "and a laid group");
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

    let item = ondin_core::build::sized_flex_item(&s.doc, &s.res, g, Size::new(150.0, 70.0), None)
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

/// **A pinned child of a group with a layout is placed against the group's box,
/// and a tool that moves it re-pins it where it was put** (`build::keep_insets`,
/// §15 D887). The group is 100 × 70 — padding 20 round the one 60 × 30 item left
/// in its row — so right 0, bottom 0 puts the pinned 40 × 30 at (60, 40) inside
/// it, (110, 90) in the world; a move to (10, 10) is right 50, bottom 30.
///
/// **Flip run**, `keep_insets`' group arm deleted (a laid group's child skipped as
/// it was before D887): fails on *"it stays where the move put it"*, the rect
/// back at (110, 90) — the predicted site. The first two assertions stay green,
/// since the placement in `resolve` was never frame-only.
#[test]
fn a_pinned_child_of_a_flex_group_is_placed_against_its_box_and_re_pinned_when_moved() {
    let mut s = Scene::new();
    let g = s.add(s.root, NodeKind::Group, (50.0, 50.0));
    s.add(g, rect(60.0, 30.0), (0.0, 0.0));
    let pinned = s.add(g, rect(40.0, 30.0), (0.0, 0.0));
    s.display(g, row());
    s.commit(vec![Operation::SetInsets {
        id: pinned,
        insets: ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(0.0)),
            bottom: Some(ondin_core::LengthPct::Px(0.0)),
            ..Default::default()
        },
    }]);
    assert_eq!(
        s.bounds(g),
        Rect::new(50.0, 50.0, 150.0, 120.0),
        "the fixture"
    );
    assert_eq!(s.bounds(pinned), Rect::new(110.0, 90.0, 150.0, 120.0));

    s.commit(vec![Operation::SetTransform {
        id: pinned,
        transform: Affine::translate((10.0, 10.0)),
    }]);
    assert_eq!(
        s.bounds(pinned),
        Rect::new(60.0, 60.0, 100.0, 90.0),
        "it stays where the move put it"
    );
    let insets = *s.doc.get(pinned).unwrap().insets();
    assert_eq!(insets.right, Some(ondin_core::LengthPct::Px(50.0)));
    assert_eq!(insets.bottom, Some(ondin_core::LengthPct::Px(30.0)));
}

/// **A frame that hugs across the line is not stretched** (§15 D893) — CSS's
/// `stretch` applying only to an `auto` cross size, and Figma's *Hug* staying
/// hugged. In a 400 × 200 row with padding 20 and the default `align-items:
/// stretch`, a card frame holding a 40 × 30 rect and set to `fit-content` both
/// ways stays 30 tall; a rect beside it, whose cross size is `auto`, still
/// stretches to 160 (§15 D872). A frame is no control here: its `auto` is its
/// stored size, which is definite and never stretched either (§15 D879) — the
/// first draft of this test used one and found exactly that.
///
/// **Flip run**, the `fit_content_across` patch in `FlexTree::push` deleted:
/// fails on *"the hugging card keeps hugging"*, 160 against 30 — the predicted
/// site; the stretching rect stays green, as it should.
#[test]
fn a_frame_that_hugs_across_the_line_is_not_stretched() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let hug = s.add(f, frame(10.0, 10.0), (0.0, 0.0));
    s.add(hug, rect(40.0, 30.0), (0.0, 0.0));
    let tall = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(Flex {
            padding: [20.0; 4],
            ..Default::default()
        })),
    );
    s.display(hug, Some(Display::Flex(Flex::default())));
    s.item(hug, |i| {
        i.width = ondin_core::container::Dimension::FitContent;
        i.height = ondin_core::container::Dimension::FitContent;
    });
    assert_eq!(
        s.bounds(hug).width(),
        40.0,
        "the fixture hugs along the row"
    );
    assert_eq!(
        s.bounds(hug).height(),
        30.0,
        "the hugging card keeps hugging across the line"
    );
    assert_eq!(
        s.bounds(tall).height(),
        160.0,
        "an `auto` height still stretches: 200 less 20 twice"
    );
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
    // Right to left now: b at the right edge, a left of it, c left of that —
    // `flex-start` in a reversed row is its right (§15 D909; this comment was true
    // of CSS and false of the engine until then, which packed the row left).
    assert!(s.bounds(b).x0 > s.bounds(a).x0, "the fixture reversed");
    assert_eq!(s.bounds(b).x1, 380.0, "b at the right edge: 400 less 20");
    assert_eq!(
        reorder(&s, a, 100.0),
        Some(0),
        "row-reverse reads backwards: right is the start"
    );
}

/// **`Start` is CSS's `flex-start`, which follows both reversals** (§15 D909) —
/// the cards' word and picture, and now the engine's. A 400 × 200 frame, padding
/// 20, holding a 40 × 30 and a 60 × 50: in `row-reverse` justified to the start the
/// row packs against the right; in `wrap-reverse` aligned to the start, lines and
/// items against the bottom. And a stretched item there, dragged by its **top**,
/// held the flow's cross-start — the bottom — so it aligns to the start, not the
/// end (§15 D905 under the reversal).
///
/// **Flip runs**: `justify-content`'s mapping back to taffy's `START` fails on
/// *"row-reverse packs right"*, a at 90..130 — and so does
/// `a_dragged_flex_item_reorders_by_where_its_centre_falls` on its new right-edge
/// assertion, b ending at 160; `align_items`' and `align-content`'s
/// back to `START` fail on *"wrap-reverse sits at the bottom"*, a at y 20..50 —
/// the predicted sites. `resized_from_cross_start` without its `WrapReverse` swap
/// fails on *"the flow's start held"*, `End` — predicted.
#[test]
fn flex_start_follows_the_reversals() {
    use ondin_core::container::{AlignContent, FlexWrap};
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    let b = s.add(f, rect(60.0, 50.0), (0.0, 0.0));
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
    assert_eq!(
        (s.bounds(a).x0, s.bounds(a).x1),
        (340.0, 380.0),
        "row-reverse packs right: a first, against the right padding"
    );
    assert_eq!(s.bounds(b).x1, 330.0, "b left of it");

    s.display(
        f,
        Some(Display::Flex(Flex {
            wrap: FlexWrap::WrapReverse,
            column_gap: 10.0,
            padding: [20.0; 4],
            align_items: AlignItems::Start,
            align_content: AlignContent::Start,
            ..Default::default()
        })),
    );
    assert_eq!(
        (s.bounds(a).y0, s.bounds(a).y1),
        (150.0, 180.0),
        "wrap-reverse sits at the bottom"
    );
    assert_eq!(s.bounds(b).y1, 180.0, "b too, its line's start");

    // Stretched across one line of the reversed wrap, then dragged by its top.
    s.display(
        f,
        Some(Display::Flex(Flex {
            wrap: FlexWrap::WrapReverse,
            column_gap: 10.0,
            padding: [20.0; 4],
            align_content: AlignContent::Stretch,
            ..Default::default()
        })),
    );
    assert_eq!(s.bounds(a).height(), 160.0, "stretched across");
    let slot = s.res.used_local(&s.doc, a).unwrap();
    s.commit(vec![
        Operation::SetGeometry {
            id: a,
            geometry: GeometryPatch::Size(Size::new(40.0, 70.0)),
        },
        Operation::SetTransform {
            id: a,
            transform: slot * Affine::translate((0.0, 90.0)),
        },
    ]);
    assert_eq!(
        s.doc.get(a).unwrap().item().align_self,
        Some(AlignItems::Start),
        "the flow's start held"
    );
    assert_eq!(
        (s.bounds(a).y0, s.bounds(a).y1),
        (110.0, 180.0),
        "and the bottom stayed"
    );
}

/// **A released stretch keeps the edge the user held, on every axis** (§15 D905
/// under D909) — one stretched 40 × 30 rect in a 400 × 200 frame, padding 20,
/// resized across its line from each cross edge, in a row and a column, with and
/// without `wrap-reverse`. Dragged by the edge the flow calls its cross-start —
/// top or left, or bottom or right when reversed — it aligns to `flex-end`;
/// dragged by the other, to `flex-start`; and either way the edge it did not drag
/// stays where it was.
///
/// **Flip run**, `resized_from_cross_start` answering `false`: fails on the first
/// start-edge case, *"row, top"*, `Start` — predicted. The swap under
/// `wrap-reverse` dropped fails on *"row, wrap-reverse, top"* — predicted.
#[test]
fn a_released_stretch_keeps_the_held_edge_on_every_axis() {
    use ondin_core::container::FlexWrap;
    // (what, direction, wrap, dragged the top/left edge, the keyword, the box's
    // cross extent afterwards)
    let cases = [
        (
            "row, top",
            FlexDirection::Row,
            FlexWrap::NoWrap,
            true,
            AlignItems::End,
            (110.0, 180.0),
        ),
        (
            "row, bottom",
            FlexDirection::Row,
            FlexWrap::NoWrap,
            false,
            AlignItems::Start,
            (20.0, 90.0),
        ),
        (
            "row, wrap-reverse, top",
            FlexDirection::Row,
            FlexWrap::WrapReverse,
            true,
            AlignItems::Start,
            (110.0, 180.0),
        ),
        (
            "row, wrap-reverse, bottom",
            FlexDirection::Row,
            FlexWrap::WrapReverse,
            false,
            AlignItems::End,
            (20.0, 90.0),
        ),
        (
            "column, left",
            FlexDirection::Column,
            FlexWrap::NoWrap,
            true,
            AlignItems::End,
            (280.0, 380.0),
        ),
        (
            "column, right",
            FlexDirection::Column,
            FlexWrap::NoWrap,
            false,
            AlignItems::Start,
            (20.0, 120.0),
        ),
        (
            "column, wrap-reverse, left",
            FlexDirection::Column,
            FlexWrap::WrapReverse,
            true,
            AlignItems::Start,
            (280.0, 380.0),
        ),
        (
            "column, wrap-reverse, right",
            FlexDirection::Column,
            FlexWrap::WrapReverse,
            false,
            AlignItems::End,
            (20.0, 120.0),
        ),
    ];
    for (what, direction, wrap, from_start, keyword, extent) in cases {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
        let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
        s.display(
            f,
            Some(Display::Flex(Flex {
                direction,
                wrap,
                padding: [20.0; 4],
                ..Default::default()
            })),
        );
        let row = direction.is_row();
        let across = |b: Rect| if row { (b.y0, b.y1) } else { (b.x0, b.x1) };
        assert_eq!(
            across(s.bounds(a)),
            if row { (20.0, 180.0) } else { (20.0, 380.0) },
            "{what}: stretched across"
        );
        // Shrunk to 70 tall in a row, 100 wide in a column; from the top or left
        // edge the tool shifts the origin by what the size lost, as it writes it.
        let (size, lost) = if row {
            (Size::new(40.0, 70.0), Vec2::new(0.0, 90.0))
        } else {
            (Size::new(100.0, 30.0), Vec2::new(260.0, 0.0))
        };
        let mut ops = vec![Operation::SetGeometry {
            id: a,
            geometry: GeometryPatch::Size(size),
        }];
        if from_start {
            let slot = s.res.used_local(&s.doc, a).unwrap();
            ops.push(Operation::SetTransform {
                id: a,
                transform: slot * Affine::translate(lost),
            });
        }
        s.commit(ops);
        assert_eq!(
            s.doc.get(a).unwrap().item().align_self,
            Some(keyword),
            "{what}"
        );
        assert_eq!(across(s.bounds(a)), extent, "{what}: the held edge stayed");
    }
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

/// **A resize writes px over a size keyword** (`build::keep_flex_sizes`, §15 D879)
/// — a frame at the top of the page hugging its row with `width: fit-content`, and
/// an item in that row at `width: 50%`. Each resized, each holds the size it was
/// given and its keyword goes back to `auto`, which for a kind with a stored size
/// is that size; the axis the resize did not change keeps its keyword.
///
/// **Flip run**, `sized_in_px` not called: fails on *"the dragged width held"* at
/// 80 — the frame snapping back to hug its row — the predicted site.
#[test]
fn a_resize_writes_px_over_a_size_keyword() {
    use ondin_core::container::Dimension;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    s.item(f, |i| {
        i.width = Dimension::FitContent;
        i.height = Dimension::FitContent;
    });
    assert_eq!(s.bounds(f).width(), 80.0, "the fixture hugs: 20 + 40 + 20");

    s.resize(f, 300.0, 70.0);
    assert_eq!(s.bounds(f).width(), 300.0, "the dragged width held");
    let item = *s.doc.get(f).unwrap().item();
    assert_eq!(item.width, Dimension::Auto, "px now: its stored size");
    assert_eq!(
        item.height,
        Dimension::FitContent,
        "the axis the resize left alone keeps its keyword"
    );

    s.item(a, |i| i.width = Dimension::Percent(50.0));
    assert_ne!(s.bounds(a).width(), 40.0, "the fixture sizes by percentage");
    s.resize(a, 70.0, 30.0);
    assert_eq!(s.bounds(a).width(), 70.0, "the item's dragged width held");
    assert_eq!(s.doc.get(a).unwrap().item().width, Dimension::Auto);
}

/// **A laid group resized along one axis keeps hugging along the other**
/// (`build::sized_flex_item`, §15 D879) — a side handle, or a W typed in the
/// Transform card, writes `width` in px and leaves `height` at `auto`, the same
/// per-axis rule `keep_flex_sizes` applies to a kind with a stored size.
///
/// **Flip run**, both axes written unconditionally (the rule before D879): fails
/// on *"the untouched axis still hugs"*, `Px(70)` — the predicted site.
#[test]
fn a_laid_group_resized_along_one_axis_keeps_hugging_along_the_other() {
    use ondin_core::container::Dimension;
    let mut s = Scene::new();
    let g = s.add(s.root, NodeKind::Group, (0.0, 0.0));
    let first = s.add(g, rect(40.0, 30.0), (0.0, 0.0));
    s.display(g, row());
    let hugged = s.bounds(g).height();
    assert_eq!(hugged, 70.0, "the fixture hugs: 20 + 30 + 20");

    let item =
        ondin_core::build::sized_flex_item(&s.doc, &s.res, g, Size::new(200.0, hugged), None)
            .expect("a group with a layout takes a size");
    s.commit(vec![Operation::SetFlexItem { id: g, item }]);
    let item = *s.doc.get(g).unwrap().item();
    assert_eq!(item.width, Dimension::Px(200.0));
    assert_eq!(
        item.height,
        Dimension::Auto,
        "the untouched axis still hugs"
    );

    s.resize(first, 40.0, 50.0);
    assert_eq!(
        s.bounds(g).height(),
        90.0,
        "and goes on hugging: 20 + 50 + 20"
    );
}

/// **`wrap-reverse` reorders with its lines read bottom-up** (`build::flex_reorder`,
/// §15 D883) — a 200-wide row of four 60-wide rects wrapping two to a line, the
/// first line (`a`, `b`) at the bottom.
///
/// `c`, on the upper line, dragged down to the left of `a` lands at the front of
/// the flow: `d` beside it on the upper line is a *later* line, so nothing comes
/// before it.
///
/// **Flip run**, the `lines_reversed` arm dropped (lines compared top-down, the
/// reading before D883): answers `Some(1)` — `d`, above, counted as an earlier line
/// — on the predicted assertion.
#[test]
fn wrap_reverse_reorders_with_its_lines_read_bottom_up() {
    use ondin_core::container::FlexWrap;
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    let _b = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    let c = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    let _d = s.add(f, rect(60.0, 30.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(Flex {
            wrap: FlexWrap::WrapReverse,
            column_gap: 10.0,
            row_gap: 10.0,
            padding: [20.0; 4],
            align_items: AlignItems::Start,
            ..Default::default()
        })),
    );
    assert!(
        s.bounds(a).y0 > s.bounds(c).y0,
        "the fixture: the first line is at the bottom"
    );
    assert_eq!(s.bounds(a).x0, s.bounds(c).x0, "and two to a line");

    let target = (s.bounds(a).x0 + 5.0, s.bounds(a).center().y);
    let delta = Vec2::new(
        target.0 - s.bounds(c).center().x,
        target.1 - s.bounds(c).center().y,
    );
    assert_eq!(
        ondin_core::build::flex_reorder(&s.doc, &s.res, c, delta),
        Some(Operation::Reorder { id: c, index: 0 }),
        "dropped before a, on the first line: nothing precedes it"
    );
}

/// **A layout nested in a plain group is measured by that layout** (§15 D899).
/// In a row with padding 20 and gap 10, a group with no layout holds a flex
/// group — padding 10 round a 40 × 30 rect, so 60 × 50 — and the next item sits
/// after the plain group's box: at 20 + 60 + 10 = 90, the plain group 50 tall.
/// `atomic_box` measured the nested layout by its children's stored union, 40 ×
/// 30, which put the next item at 70 — §15 D875's ⚠️.
///
/// **Flip run**, `atomic_box`'s early return for a laid container deleted: fails
/// on *"after the nested layout's padded box"*, 70 against 90 — the predicted
/// site. The randomized guard cannot see this one: `update` and `rebuild` both
/// measure through the same function.
#[test]
fn a_layout_nested_in_a_plain_group_is_measured_by_its_layout() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let plain = s.add(f, NodeKind::Group, (0.0, 0.0));
    let nested = s.add(plain, NodeKind::Group, (0.0, 0.0));
    s.add(nested, rect(40.0, 30.0), (0.0, 0.0));
    let next = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        nested,
        Some(Display::Flex(Flex {
            padding: [10.0; 4],
            ..Default::default()
        })),
    );
    s.display(f, row());
    assert_eq!(
        s.bounds(plain).height(),
        50.0,
        "the plain group is the nested layout's box"
    );
    assert_eq!(
        s.bounds(next).x0,
        90.0,
        "after the nested layout's padded box"
    );
}

/// **An in-flow item turns about its box centre in its slot, its stored
/// translation ignored** (§15 D896's ruling, the half no test checked: the
/// preview differential proves preview and commit agree, both through
/// `container::item_placed`, not *where* they draw). A 40 × 30 rect first in a
/// row with padding 20 has its slot at (20, 20), its centre at (40, 35); stored
/// turned a quarter about a point far away, translation and all, it is drawn as a
/// 30 × 40 box about that same centre — (25, 15) to (55, 55).
///
/// **Flip run**, `item_placed` turning about the box's top-left corner rather
/// than its centre (`translate(slot) * linear * translate(−origin)`, the version
/// somebody would write): fails on *"turned about its box centre"*, the box at
/// (−10, 20)–(20, 60) — the predicted site. The stored translation is not
/// flipped here: the commit door keeps the stored one (§15 D877's second
/// amendment), so it is (0, 0) and could not show a difference.
#[test]
fn an_in_flow_item_turns_about_its_box_centre_in_its_slot() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let turned = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
    s.display(f, row());
    s.commit(vec![Operation::SetTransform {
        id: turned,
        transform: Affine::translate((300.0, 150.0)) * Affine::rotate(90f64.to_radians()),
    }]);
    let b = s.bounds(turned);
    let near = |a: f64, e: f64| (a - e).abs() < 1e-6;
    assert!(
        near(b.x0, 25.0) && near(b.y0, 15.0) && near(b.x1, 55.0) && near(b.y1, 55.0),
        "turned about its box centre in its slot: {b:?}"
    );
}

/// **A pinned child of a laid group nested in a plain group moves where it is
/// dragged** (§15 D910) — a laid group `g`, a row with padding 5, inside a plain
/// group that is an item of a flex row, holding a 40 × 30 and a 10 × 10 pinned to
/// its bottom-right corner at (60, 50). Moved 20 left and 10 up, it lands at
/// (40, 40), pinned 20 from the right and 10 from the bottom.
///
/// `keep_insets` measures a laid group by `laid_group_box`, which ran the pass of
/// the group's chain root — the flex row, whose pass stops at the plain group —
/// found nothing, and skipped the child: the move was written as a transform the
/// insets then overrode, and it snapped back to the corner.
///
/// **Flip run**, `laid_group_box` without its own-pass fallback: fails on *"where
/// it was dragged"*, at (60, 50) — the predicted site.
#[test]
fn a_pinned_child_of_a_laid_group_in_a_plain_group_moves_where_dragged() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let p = s.add(f, NodeKind::Group, (0.0, 0.0));
    let g = s.add(p, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(40.0, 30.0), (0.0, 0.0));
    let k = s.add(g, rect(10.0, 10.0), (0.0, 0.0));
    s.display(f, row());
    s.display(
        g,
        Some(Display::Flex(Flex {
            padding: [5.0; 4],
            ..Default::default()
        })),
    );
    s.commit(vec![Operation::SetInsets {
        id: k,
        insets: ondin_core::Insets {
            right: Some(ondin_core::LengthPct::Px(0.0)),
            bottom: Some(ondin_core::LengthPct::Px(0.0)),
            ..Default::default()
        },
    }]);
    assert_eq!(
        s.bounds(g),
        Rect::new(20.0, 20.0, 70.0, 60.0),
        "the fixture"
    );
    assert_eq!(
        s.bounds(k),
        Rect::new(60.0, 50.0, 70.0, 60.0),
        "in its corner"
    );
    let local = s.res.used_local(&s.doc, k).unwrap();
    s.commit(vec![Operation::SetTransform {
        id: k,
        transform: Affine::translate((-20.0, -10.0)) * local,
    }]);
    assert_eq!(
        s.bounds(k),
        Rect::new(40.0, 40.0, 50.0, 50.0),
        "where it was dragged"
    );
    let insets = s.doc.get(k).unwrap().insets();
    assert_eq!(
        (insets.right, insets.bottom),
        (
            Some(ondin_core::LengthPct::Px(20.0)),
            Some(ondin_core::LengthPct::Px(10.0))
        ),
        "pinned where it now is"
    );
}

/// **The same move holds for a laid group out of its row's flow — hidden, or a
/// mask** (§15 D910's other half, untested until now). Each takes `g` out of the
/// row, so the row's pass never lays it and `laid_group_box` has to run `g`'s own:
/// the pinned child, moved 20 left and 10 up, lands there and is pinned 20 from
/// the right and 10 from the bottom.
///
/// **Flip runs**, `laid_group_box` without its own-pass fallback: fails on *"hidden:
/// where it was dragged"*, the child back in its corner at (140, 110) — predicted.
/// Run with the mask case first, it fails on *"a mask: where it was dragged"* at the
/// same place, so each case bites on its own.
#[test]
fn a_pinned_child_of_a_laid_group_out_of_the_flow_moves_where_dragged() {
    let hide: fn(NodeId) -> Operation = |id| Operation::SetVisible { id, visible: false };
    let mask: fn(NodeId) -> Operation = |id| Operation::SetMask { id, mask: true };
    for (what, leave) in [("hidden", hide), ("a mask", mask)] {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
        s.add(f, rect(40.0, 30.0), (0.0, 0.0));
        let g = s.add(f, NodeKind::Group, (100.0, 80.0));
        s.add(g, rect(40.0, 30.0), (0.0, 0.0));
        let k = s.add(g, rect(10.0, 10.0), (0.0, 0.0));
        s.display(f, row());
        s.display(
            g,
            Some(Display::Flex(Flex {
                padding: [5.0; 4],
                ..Default::default()
            })),
        );
        s.commit(vec![
            Operation::SetInsets {
                id: k,
                insets: ondin_core::Insets {
                    right: Some(ondin_core::LengthPct::Px(0.0)),
                    bottom: Some(ondin_core::LengthPct::Px(0.0)),
                    ..Default::default()
                },
            },
            leave(g),
        ]);
        let before = s.bounds(k);
        let local = s.res.used_local(&s.doc, k).unwrap();
        s.commit(vec![Operation::SetTransform {
            id: k,
            transform: Affine::translate((-20.0, -10.0)) * local,
        }]);
        assert_eq!(
            s.bounds(k),
            before - Vec2::new(20.0, 10.0),
            "{what}: where it was dragged"
        );
        let insets = s.doc.get(k).unwrap().insets();
        assert_eq!(
            (insets.right, insets.bottom),
            (
                Some(ondin_core::LengthPct::Px(20.0)),
                Some(ondin_core::LengthPct::Px(10.0))
            ),
            "{what}: pinned where it now is"
        );
    }
}

/// **Several items dragged together reorder as a block, keeping their order**
/// (§15 D902, `build::flex_reorder_many`) — a row of four 20-wide rects at x 20,
/// 50, 80 and 110 (padding 20, gap 10). `a` and `c`, not adjacent, dragged right
/// past `d` land after it as `a, c`: `b, d, a, c`. `c` and `d` dragged left past
/// `a` land first: `c, d, a, b`. A drag short of any sibling commits nothing, and
/// items of two containers are not a block.
///
/// **Flip run**, the ops placing each dragged item at its final index in turn
/// rather than walking the target order: fails on *"a and c after d, in their
/// order"* — the predicted site, the pair ending interleaved with `d`.
#[test]
fn several_items_dragged_together_reorder_as_a_block() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let [a, b, c, d] = [0, 1, 2, 3].map(|_| s.add(f, rect(20.0, 30.0), (0.0, 0.0)));
    s.display(f, row());
    assert_eq!(s.bounds(d).x0, 110.0, "the fixture");
    let order = |s: &Scene| s.doc.get(f).unwrap().children().to_vec();

    let ops = ondin_core::build::flex_reorder_many(&s.doc, &s.res, &[a, c], Vec2::new(80.0, 0.0))
        .expect("a block");
    s.commit(ops);
    assert_eq!(
        order(&s),
        vec![b, d, a, c],
        "a and c after d, in their order"
    );

    let ops = ondin_core::build::flex_reorder_many(&s.doc, &s.res, &[a, c], Vec2::new(-80.0, 0.0))
        .expect("a block");
    s.commit(ops);
    assert_eq!(order(&s), vec![a, c, b, d], "and back to the front");

    assert_eq!(
        ondin_core::build::flex_reorder_many(&s.doc, &s.res, &[b, d], Vec2::new(2.0, 0.0)),
        Some(Vec::new()),
        "short of any sibling: nothing"
    );

    let g = s.add(s.root, frame(100.0, 100.0), (500.0, 0.0));
    let e = s.add(g, rect(20.0, 30.0), (0.0, 0.0));
    s.display(g, row());
    assert_eq!(
        ondin_core::build::flex_reorder_many(&s.doc, &s.res, &[a, e], Vec2::new(10.0, 0.0)),
        None,
        "items of two containers are not a block"
    );
}
