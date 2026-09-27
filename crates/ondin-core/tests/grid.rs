//! Grid containers through the whole model (container layout's step 4, §15
//! D913, D914) — the document, `Resolved`'s layout pass and history together,
//! with incremental `update` checked against a full `rebuild` after every commit,
//! `tests/flex.rs`' harness.
//!
//! Every fixture's numbers can be checked by hand: tracks in px or in `fr` of a
//! round remainder, and items aligned to the start wherever a stretch would
//! muddy them — **on both axes, `justify-items` as well as `align-items`**,
//! since in a grid `align-items` is the vertical axis alone (the first cut of
//! the first test set only `align-items`, and item a came back 100 wide: CSS's
//! reading, and the reason `Grid::justify_items` exists, §15 D914).

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::{
    Document, GeometryPatch, History, IdSource, NodeId, NodeKind, OpError, Operation, Resolved,
    Transaction,
    container::{
        AlignContent, AlignItems, Display, Flex, Grid, GridAutoFlow, GridLines, GridPlacement,
        LayoutItem, Track, TrackBreadth, TrackSize,
    },
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

fn px(v: f64) -> Track {
    Track::Size(TrackSize::Breadth(TrackBreadth::Px(v)))
}

fn fr(v: f64) -> Track {
    Track::Size(TrackSize::Breadth(TrackBreadth::Fr(v)))
}

fn grid(columns: Vec<Track>, rows: Vec<Track>) -> Grid {
    Grid {
        columns,
        rows,
        ..Default::default()
    }
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

    fn try_commit(&mut self, ops: Vec<Operation>) -> Result<(), OpError> {
        let tx = ondin_core::build::keep_insets(&self.doc, &self.res, Transaction(ops));
        let tx = ondin_core::build::keep_flex_sizes(&self.doc, &self.res, tx);
        let dirty = self.history.commit(&mut self.doc, tx)?;
        self.res.update(&self.doc, &dirty);
        self.assert_equal_to_rebuild();
        Ok(())
    }

    fn commit(&mut self, ops: Vec<Operation>) {
        self.try_commit(ops).expect("the edit applies");
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

    fn item(&mut self, id: NodeId, f: impl FnOnce(&mut LayoutItem)) {
        let mut item = *self.doc.get(id).unwrap().item();
        f(&mut item);
        self.commit(vec![Operation::SetLayoutItem { id, item }]);
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

/// **A grid places its items in its tracks, row by row**: a 400-wide frame with
/// padding 20 and gaps 10 has 360 across, less two gaps is 340, less the 100px
/// column is 240 for `1fr 2fr` — so columns start at 20, 130 and 220 — and rows
/// of 50 and 40 start at 20 and 80. The fourth item wraps to the second row.
///
/// **Flip run**, `compute_child_layout` sending a grid container to
/// `compute_flexbox_layout` (the arm as it was before grid): fails on *"b, the
/// second column"* — b at x 50, the four laid as a flex row, 20 + 20 + 10 — the
/// predicted site.
#[test]
fn a_grid_places_its_items_in_its_tracks() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let [a, b, c, d] = [0; 4].map(|_| s.add(f, rect(20.0, 20.0), (300.0, 150.0)));
    s.display(
        f,
        Some(Display::Grid(Grid {
            column_gap: 10.0,
            row_gap: 10.0,
            padding: [20.0; 4],
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![px(100.0), fr(1.0), fr(2.0)], vec![px(50.0), px(40.0)])
        })),
    );
    assert_eq!(s.bounds(a), Rect::new(20.0, 20.0, 40.0, 40.0), "a");
    assert_eq!(
        s.bounds(b).origin(),
        (130.0, 20.0).into(),
        "b, the second column"
    );
    assert_eq!(s.bounds(c).origin(), (220.0, 20.0).into(), "c, the third");
    assert_eq!(
        s.bounds(d).origin(),
        (20.0, 80.0).into(),
        "d, the second row"
    );
    assert_eq!(
        s.doc.get(b).unwrap().transform(),
        Affine::translate((300.0, 150.0)),
        "placed in used geometry only: the document keeps what was drawn"
    );
}

/// **An item with lines takes its area, and a stretched one fills it.**
/// `grid-column: 2 / span 2` over the tracks above is 80 + 10 + 160 = 250 wide
/// from x 130, and `grid-row: 2` is the 40 row from y 80; the auto-placed item
/// skips nothing it does not have to and takes the first cell, stretched to
/// 100 × 50. Told `stretch` in so many words: under the default `normal` a
/// shape keeps its size (the next test, §15 D915).
#[test]
fn a_placed_item_takes_its_lines_and_its_span() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let placed = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let auto = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            column_gap: 10.0,
            row_gap: 10.0,
            padding: [20.0; 4],
            justify_items: Some(AlignItems::Stretch),
            align_items: Some(AlignItems::Stretch),
            ..grid(vec![px(100.0), fr(1.0), fr(2.0)], vec![px(50.0), px(40.0)])
        })),
    );
    s.item(placed, |i| {
        i.grid_column = GridLines {
            start: GridPlacement::Line(2),
            end: GridPlacement::Span(2),
        };
        i.grid_row.start = GridPlacement::Line(2);
    });
    assert_eq!(s.bounds(placed), Rect::new(130.0, 80.0, 380.0, 120.0));
    assert_eq!(s.bounds(auto), Rect::new(20.0, 20.0, 120.0, 70.0));
    assert_eq!(
        s.doc.get(placed).unwrap().kind(),
        &rect(20.0, 20.0),
        "stretched in used geometry only"
    );

    // `justify-self: start` keeps its own width at the area's start.
    s.item(placed, |i| i.justify_self = Some(AlignItems::Start));
    assert_eq!(s.bounds(placed), Rect::new(130.0, 80.0, 150.0, 120.0));
}

/// **`normal` is not `stretch`** (§15 D915, the maintainer's CSS-parity
/// ruling): in two 100 × 50 cells, a 20 × 20 rect — a replaced element (§15
/// D872) — keeps its size at the start of its cell, while a group with a layout
/// — a box, hugging a 10 × 10 rect — stretches to fill its own. Asked to hug
/// across (`fit-content`), the box keeps its 10 and still stretches down. Told
/// `stretch` in so many words, the rect fills its cell as CSS stretches a
/// replaced element; the hugging axis still does not (§15 D893's rule, in grid).
///
/// **Flip runs**, each predicted and each failing where predicted:
/// `container::grid_held` answering `(false, false)` — taffy's own `normal` —
/// fails on *"a shape keeps its size under normal"*, the rect 100 × 50, the
/// predicted site. Its `fit-content` clause dropped was predicted to fail on
/// *"fit-content is not stretched"*, under `stretch`, and fails one assertion
/// earlier — *"fit-content hugs under normal"*, at a width of 100 — since a box
/// under `normal` is stretched too; the later one would fail the same way.
#[test]
fn normal_holds_a_shape_at_the_start_and_stretches_a_box() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 50.0), (0.0, 0.0));
    let shape = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let boxed = s.add(f, NodeKind::Group, (0.0, 0.0));
    let _inner = s.add(boxed, rect(10.0, 10.0), (0.0, 0.0));
    s.display(boxed, Some(Display::Flex(Flex::default())));
    let cells = grid(vec![px(100.0), px(100.0)], vec![px(50.0)]);
    s.display(f, Some(Display::Grid(cells.clone())));
    assert_eq!(
        s.bounds(shape),
        Rect::new(0.0, 0.0, 20.0, 20.0),
        "a shape keeps its size under normal"
    );
    assert_eq!(
        s.res.used_frame(boxed),
        Some(Size::new(100.0, 50.0)),
        "a box stretches under normal"
    );
    assert_eq!(s.bounds(boxed).x0, 100.0);

    s.item(boxed, |i| {
        i.width = ondin_core::container::Dimension::FitContent
    });
    assert_eq!(
        s.res.used_frame(boxed),
        Some(Size::new(10.0, 50.0)),
        "fit-content hugs under normal"
    );

    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Stretch),
            align_items: Some(AlignItems::Stretch),
            ..cells
        })),
    );
    assert_eq!(
        s.bounds(shape),
        Rect::new(0.0, 0.0, 100.0, 50.0),
        "stretch stretches a shape"
    );
    assert_eq!(
        s.res.used_frame(boxed),
        Some(Size::new(10.0, 50.0)),
        "fit-content is not stretched"
    );
}

/// **Resizing a stretched grid item holds the size it was dragged to** (§15
/// D913's second ruling): the drag writes the size, and the stretch that would
/// undo it becomes `start` — or `end` when the drag moved the left or top edge
/// and held the other (D905's rule, on both axes). The item keeps its cells, and
/// an axis the drag left alone keeps its alignment. Two 100 × 50 cells told
/// `stretch`: a right-handle drag to 60 keeps x 0–60; a left-handle drag on the
/// second cell's item to 40 keeps its right edge at 200.
///
/// **Flip runs**: `flex_holds`' grid arm deleted (the code before grid) fails on
/// *"the dragged width held"* — the rect stretched straight back to 100, the
/// predicted site; `grid_resize_held`'s edge ignored (always `Start`) fails on
/// *"a left-handle drag keeps its right edge"* at x 100–140, as predicted.
#[test]
fn resizing_a_stretched_grid_item_holds_the_size_it_was_dragged_to() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 50.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Stretch),
            align_items: Some(AlignItems::Stretch),
            ..grid(vec![px(100.0), px(100.0)], vec![px(50.0)])
        })),
    );
    assert_eq!(s.bounds(a), Rect::new(0.0, 0.0, 100.0, 50.0), "the fixture");

    s.resize(a, 60.0, 50.0);
    assert_eq!(
        s.bounds(a),
        Rect::new(0.0, 0.0, 60.0, 50.0),
        "the dragged width held"
    );
    let item = *s.doc.get(a).unwrap().item();
    assert_eq!(item.justify_self, Some(AlignItems::Start));
    assert_eq!(item.align_self, None, "the height was not dragged");
    assert_eq!(item.grid_column, GridLines::default(), "its cells kept");

    // A left-handle drag: the tool writes the size and the slot shifted to hold
    // the right edge, as `tools::resize_box_to` does.
    s.commit(vec![
        Operation::SetGeometry {
            id: b,
            geometry: GeometryPatch::Size(Size::new(40.0, 50.0)),
        },
        Operation::SetTransform {
            id: b,
            transform: Affine::translate((160.0, 0.0)),
        },
    ]);
    assert_eq!(
        s.bounds(b),
        Rect::new(160.0, 0.0, 200.0, 50.0),
        "a left-handle drag keeps its right edge"
    );
    assert_eq!(
        s.doc.get(b).unwrap().item().justify_self,
        Some(AlignItems::End)
    );
}

/// **Under `normal` a resized shape needs no hold** — it was never stretched
/// (§15 D915) — so its item is left at CSS's defaults; **a laid group, a box, is
/// stretched under `normal`** and its resize writes px and releases the stretch
/// on the axis it changed (`build::sized_flex_item`, the tools' door for a group
/// with a layout).
#[test]
fn under_normal_a_shape_needs_no_hold_and_a_box_does() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 50.0), (0.0, 0.0));
    let shape = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let boxed = s.add(f, NodeKind::Group, (0.0, 0.0));
    let _inner = s.add(boxed, rect(10.0, 10.0), (0.0, 0.0));
    s.display(boxed, Some(Display::Flex(Flex::default())));
    s.display(
        f,
        Some(Display::Grid(grid(
            vec![px(100.0), px(100.0)],
            vec![px(50.0)],
        ))),
    );
    s.resize(shape, 30.0, 30.0);
    assert_eq!(s.bounds(shape), Rect::new(0.0, 0.0, 30.0, 30.0));
    assert!(
        s.doc.get(shape).unwrap().item().is_default(),
        "nothing held"
    );

    assert_eq!(
        s.res.used_frame(boxed),
        Some(Size::new(100.0, 50.0)),
        "stretched"
    );
    let item =
        ondin_core::build::sized_flex_item(&s.doc, &s.res, boxed, Size::new(60.0, 50.0), None)
            .expect("a group with a layout");
    assert_eq!(item.width, ondin_core::container::Dimension::Px(60.0));
    assert_eq!(item.justify_self, Some(AlignItems::Start));
    assert_eq!(item.align_self, None, "the height was not changed");
    s.commit(vec![Operation::SetLayoutItem { id: boxed, item }]);
    assert_eq!(s.res.used_frame(boxed), Some(Size::new(60.0, 50.0)));
}

/// `grid-auto-flow: column` fills a column before moving to the next.
#[test]
fn column_flow_fills_a_column_first() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 200.0), (0.0, 0.0));
    let [a, b, c] = [0; 3].map(|_| s.add(f, rect(20.0, 20.0), (0.0, 0.0)));
    s.display(
        f,
        Some(Display::Grid(Grid {
            auto_flow: GridAutoFlow::Column,
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![px(50.0), px(50.0)], vec![px(30.0), px(30.0)])
        })),
    );
    assert_eq!(s.bounds(a).origin(), (0.0, 0.0).into());
    assert_eq!(s.bounds(b).origin(), (0.0, 30.0).into(), "down the column");
    assert_eq!(s.bounds(c).origin(), (50.0, 0.0).into(), "then across");
}

/// **`auto` tracks share the free space by default** — a grid's `normal`
/// content distribution is `stretch` (§15 D914), which is why `Grid` takes
/// [`AlignContent`] on both axes: 360 across, less a gap of 10 and two 20-wide
/// items, leaves 310, 155 to each track, so they are 175 wide and the second
/// starts at 20 + 175 + 10 = 205. Told `start`, the tracks keep their content's
/// width and the second starts at 50.
///
/// **Flip run**, `style_of`'s grid arm handing taffy `FLEX_START` for
/// `justify-content` when the model says `Stretch` — the reading flex's
/// `JustifyContent`, which has no `stretch`, would have forced: fails on *"the
/// tracks stretch"* at 50, the predicted site, and nothing else in the file.
#[test]
fn auto_tracks_stretch_into_the_free_space() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 100.0), (0.0, 0.0));
    let _a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let auto = Track::Size(TrackSize::Breadth(TrackBreadth::Auto));
    let base = Grid {
        column_gap: 10.0,
        padding: [20.0; 4],
        justify_items: Some(AlignItems::Start),
        align_items: Some(AlignItems::Start),
        ..grid(vec![auto.clone(), auto], vec![])
    };
    s.display(f, Some(Display::Grid(base.clone())));
    assert_eq!(s.bounds(b).x0, 205.0, "the tracks stretch");
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_content: AlignContent::Start,
            ..base
        })),
    );
    assert_eq!(s.bounds(b).x0, 50.0, "held at their content");
}

/// `repeat(3, 50px) 20px` is four columns — the fourth item lands in the 20px
/// one at 3 × (50 + 10) = 180, and the fifth wraps.
#[test]
fn a_repeat_expands_into_its_tracks() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let items = [0; 5].map(|_| s.add(f, rect(10.0, 10.0), (0.0, 0.0)));
    s.display(
        f,
        Some(Display::Grid(Grid {
            column_gap: 10.0,
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(
                vec![
                    Track::Repeat {
                        repeat: 3,
                        tracks: vec![TrackSize::Breadth(TrackBreadth::Px(50.0))],
                    },
                    px(20.0),
                ],
                vec![px(30.0)],
            )
        })),
    );
    assert_eq!(s.bounds(items[3]).x0, 180.0);
    assert_eq!(s.bounds(items[4]).origin(), (0.0, 30.0).into());
}

/// **`minmax(100px, 1fr)` grows with the frame and stops at its minimum** —
/// resizing the frame reflows the grid: 400 wide leaves 350 beside a 50px
/// column, 120 wide would leave 70 and the track holds 100 instead.
#[test]
fn a_minmax_track_grows_with_the_frame_down_to_its_minimum() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 100.0), (0.0, 0.0));
    let _a = s.add(f, rect(10.0, 10.0), (0.0, 0.0));
    let b = s.add(f, rect(10.0, 10.0), (0.0, 0.0));
    let minmax = Track::Size(TrackSize::MinMax {
        min: TrackBreadth::Px(100.0),
        max: TrackBreadth::Fr(1.0),
    });
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![minmax, px(50.0)], vec![])
        })),
    );
    assert_eq!(s.bounds(b).x0, 350.0);
    s.resize(f, 120.0, 100.0);
    assert_eq!(s.bounds(b).x0, 100.0, "held at its minimum");
}

/// **A group with a grid is its tracks' box** (§15 D869's group, laid by grid):
/// 5 + 50 + 10 + 70 + 5 across and 5 + 30 + 5 down. **And it is one box to a
/// flex row around it**, so the row's next item starts after it: 20 + 140 + 10.
#[test]
fn a_group_with_a_grid_hugs_its_tracks_inside_a_flex_row() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    let _x = s.add(g, rect(10.0, 10.0), (0.0, 0.0));
    let _y = s.add(g, rect(10.0, 10.0), (0.0, 0.0));
    let after = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        g,
        Some(Display::Grid(Grid {
            column_gap: 10.0,
            padding: [5.0; 4],
            ..grid(vec![px(50.0), px(70.0)], vec![px(30.0)])
        })),
    );
    assert_eq!(s.res.used_frame(g), Some(Size::new(140.0, 40.0)));
    s.display(
        f,
        Some(Display::Flex(Flex {
            column_gap: 10.0,
            padding: [20.0; 4],
            align_items: AlignItems::Start,
            ..Default::default()
        })),
    );
    assert_eq!(s.bounds(g).x0, 20.0);
    assert_eq!(s.bounds(after).x0, 170.0, "the row makes room for the grid");
}

/// **One item record for both layouts** (§15 D914): lines written while the
/// parent is a flex row do nothing there, and take effect the moment it turns
/// into a grid — CSS keeps them on the element the same way.
#[test]
fn grid_lines_written_under_flex_take_effect_under_grid() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(f, Some(Display::Flex(Flex::default())));
    s.item(a, |i| i.grid_column.start = GridPlacement::Line(2));
    assert_eq!(s.bounds(a).x0, 0.0, "a flex row reads no lines");
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![px(50.0), px(50.0)], vec![])
        })),
    );
    assert_eq!(s.bounds(a).x0, 50.0, "the second column");
}

/// **What CSS refuses, the operations refuse** (`OpError::BadLayout`, §15 D914):
/// a negative track, an `fr` minimum, `repeat(0, …)`, an empty repeat, line 0
/// and `span 0` — and a template past `MAX_TRACKS` (§15 D919), one repeat at 1001
/// and two entries whose repeats only pass it together — and nothing is changed
/// by any of them.
#[test]
fn values_css_refuses_are_refused() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let auto = TrackSize::Breadth(TrackBreadth::Auto);
    let bad_tracks = [
        vec![px(-1.0)],
        vec![Track::Size(TrackSize::MinMax {
            min: TrackBreadth::Fr(1.0),
            max: TrackBreadth::Fr(2.0),
        })],
        vec![Track::Repeat {
            repeat: 0,
            tracks: vec![auto],
        }],
        vec![Track::Repeat {
            repeat: 2,
            tracks: vec![],
        }],
        vec![Track::Repeat {
            repeat: 1001,
            tracks: vec![auto],
        }],
        vec![
            Track::Repeat {
                repeat: 300,
                tracks: vec![auto, auto],
            },
            Track::Repeat {
                repeat: 401,
                tracks: vec![auto],
            },
        ],
    ];
    let at_the_cap = Track::Repeat {
        repeat: 1000,
        tracks: vec![auto],
    };
    s.try_commit(vec![Operation::SetDisplay {
        id: f,
        display: Some(Display::Grid(grid(vec![at_the_cap], vec![]))),
    }])
    .expect("exactly MAX_TRACKS is accepted");
    s.display(f, None);
    for t in bad_tracks {
        let r = s.try_commit(vec![Operation::SetDisplay {
            id: f,
            display: Some(Display::Grid(grid(t.clone(), vec![]))),
        }]);
        assert!(matches!(r, Err(OpError::BadLayout)), "{t:?}: {r:?}");
    }
    assert_eq!(s.doc.get(f).unwrap().display(), None, "nothing changed");

    for p in [GridPlacement::Line(0), GridPlacement::Span(0)] {
        let mut item = LayoutItem::default();
        item.grid_row.end = p;
        let r = s.try_commit(vec![Operation::SetLayoutItem { id: a, item }]);
        assert!(matches!(r, Err(OpError::BadLayout)), "{p:?}: {r:?}");
    }
    assert!(s.doc.get(a).unwrap().item().is_default(), "nothing changed");
}

/// **A grid and its lines survive a save and a load, in a form that reads like
/// the CSS**: untagged tracks, so `minmax(100px, 2fr)` is
/// `{"min":{"Px":100.0},"max":{"Fr":2.0}}` and `repeat(3, auto)` is
/// `{"repeat":3,"tracks":["Auto"]}`.
#[test]
fn a_grid_round_trips_through_the_save_format() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            auto_flow: GridAutoFlow::Column,
            ..grid(
                vec![
                    px(200.0),
                    fr(1.0),
                    Track::Size(TrackSize::MinMax {
                        min: TrackBreadth::Px(100.0),
                        max: TrackBreadth::Fr(2.0),
                    }),
                    Track::Repeat {
                        repeat: 3,
                        tracks: vec![TrackSize::Breadth(TrackBreadth::Auto)],
                    },
                ],
                vec![Track::Size(TrackSize::Breadth(TrackBreadth::MinContent))],
            )
        })),
    );
    s.item(a, |i| {
        i.grid_column = GridLines {
            start: GridPlacement::Line(-2),
            end: GridPlacement::Span(2),
        };
        i.justify_self = Some(AlignItems::Center);
    });
    let bytes = ondin_core::io::save(&s.doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    for piece in [
        r#"{"min":{"Px":100.0},"max":{"Fr":2.0}}"#,
        r#"{"repeat":3,"tracks":["Auto"]}"#,
        r#""rows":["MinContent"]"#,
        r#""grid_column":{"start":{"Line":-2},"end":{"Span":2}}"#,
    ] {
        assert!(flat.contains(piece), "{piece} in {flat}");
    }
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(
        back.get(f).unwrap().display(),
        s.doc.get(f).unwrap().display()
    );
    assert_eq!(back.get(a).unwrap().item(), s.doc.get(a).unwrap().item());
}

/// **A file carrying a value CSS refuses still opens, and is laid as though the
/// value were not there** — §15 D492's asymmetry, refused at the operation and
/// not at the door, though unlike D492's loader nothing is dropped: the value is
/// **kept as stored** and only read around at layout. `100px repeat(0, 50px)` is
/// one column, so the second item wraps to the next row instead of sitting at
/// x 100.
#[test]
fn a_file_with_a_refused_track_opens_and_lays_around_it() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let _a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(
                vec![
                    px(100.0),
                    Track::Repeat {
                        repeat: 3,
                        tracks: vec![TrackSize::Breadth(TrackBreadth::Px(50.0))],
                    },
                ],
                vec![px(30.0)],
            )
        })),
    );
    assert_eq!(
        s.bounds(b).x0,
        100.0,
        "the fixture: b in the repeat's first"
    );
    let text = String::from_utf8(ondin_core::io::save(&s.doc).unwrap()).unwrap();
    assert_eq!(
        text.matches(r#""repeat": 3"#).count(),
        1,
        "the fixture's spelling"
    );
    let broken = text.replace(r#""repeat": 3"#, r#""repeat": 0"#);
    let doc = ondin_core::io::load(broken.as_bytes()).expect("the file opens");
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(b).unwrap().origin(),
        (0.0, 30.0).into(),
        "one column, so b wraps"
    );
    let Some(Display::Grid(g)) = doc.get(f).unwrap().display() else {
        panic!("the grid survived the load");
    };
    assert!(
        matches!(g.columns[1], Track::Repeat { repeat: 0, .. }),
        "kept as stored, not dropped: {:?}",
        g.columns
    );
}

/// `s`'s document saved, `from` swapped for `to` in the file — exactly once —
/// and opened again: how a test gets a value the operations refuse into a
/// document, as another tool or a hand edit would.
fn reopened(s: &Scene, from: &str, to: &str) -> (Document, Resolved) {
    let text = String::from_utf8(ondin_core::io::save(&s.doc).unwrap()).unwrap();
    assert_eq!(
        text.matches(from).count(),
        1,
        "the fixture spells {from} once"
    );
    let doc = ondin_core::io::load(text.replace(from, to).as_bytes()).expect("the file opens");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// **Every value the operations refuse is read around in a file** (§15 D914's
/// rule, D919's cap), one case per value on columns `100px 50px 100px` over a
/// 40px row, 10 × 10 items at the start of their cells — `c` at x 150:
///
/// - a **negative** px track is read as 0: `-50px` puts `c` at x 100;
/// - **line 0** is read as `auto`: `d`, placed at column 3 and so wrapped to row
///   2 at x 150, is auto-placed instead, at x 0;
/// - **`span 0`** is read as `auto`, one track: `d` at `2 / span 2`, stretched,
///   is 50 wide rather than 150;
/// - an **`fr` minimum** is read as `auto` at that end — the file opens, lays out
///   and keeps the value as stored;
/// - a **repeat past the cap** repeats as many times as fit: `repeat(5000, 10px)`
///   after three tracks lays 1000 explicit columns.
///
/// An empty repeat goes through the same guard as `repeat(0)`, which
/// `a_file_with_a_refused_track_opens_and_lays_around_it` pins; the file's
/// pretty-printed list makes it a multi-line edit, so it is not spelled here.
///
/// **Flip run**, `template` given no budget: fails on *"laid to the cap"* at
/// 5003, the predicted site.
#[test]
fn every_value_css_refuses_is_read_around_in_a_file() {
    let base = || {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 100.0), (0.0, 0.0));
        let items = [0; 4].map(|_| s.add(f, rect(10.0, 10.0), (0.0, 0.0)));
        s.display(
            f,
            Some(Display::Grid(Grid {
                justify_items: Some(AlignItems::Start),
                align_items: Some(AlignItems::Start),
                ..grid(vec![px(100.0), px(50.0), px(100.0)], vec![px(40.0)])
            })),
        );
        (s, f, items)
    };
    let x0 = |res: &Resolved, id| res.world_bounds(id).unwrap().x0;

    let (s, _f, [_, _, c, _]) = base();
    assert_eq!(s.bounds(c).x0, 150.0, "the fixture");
    let (_doc, res) = reopened(&s, r#""Px": 50.0"#, r#""Px": -50.0"#);
    assert_eq!(x0(&res, c), 100.0, "a negative track is 0");

    let (mut s, _f, [.., d]) = base();
    s.item(d, |i| i.grid_column.start = GridPlacement::Line(3));
    assert_eq!(s.bounds(d).x0, 150.0, "the fixture: d at column 3");
    let (_doc, res) = reopened(&s, r#""Line": 3"#, r#""Line": 0"#);
    assert_eq!(x0(&res, d), 0.0, "line 0 is auto");

    let (mut s, _f, [.., d]) = base();
    s.item(d, |i| {
        i.grid_column = GridLines {
            start: GridPlacement::Line(2),
            end: GridPlacement::Span(2),
        };
        i.justify_self = Some(AlignItems::Stretch);
    });
    assert_eq!(s.bounds(d).width(), 150.0, "the fixture: d across two");
    let (_doc, res) = reopened(&s, r#""Span": 2"#, r#""Span": 0"#);
    assert_eq!(
        res.world_bounds(d).unwrap().width(),
        50.0,
        "span 0 is one track"
    );

    let (mut s, f, _) = base();
    let minmax = Track::Size(TrackSize::MinMax {
        min: TrackBreadth::Px(30.0),
        max: TrackBreadth::Fr(1.0),
    });
    s.display(f, Some(Display::Grid(grid(vec![minmax], vec![]))));
    let (doc, _res) = reopened(&s, r#""Px": 30.0"#, r#""Fr": 30.0"#);
    let Some(Display::Grid(g)) = doc.get(f).unwrap().display() else {
        panic!("the grid survived the load");
    };
    assert!(
        matches!(
            g.columns[0],
            Track::Size(TrackSize::MinMax {
                min: TrackBreadth::Fr(_),
                ..
            })
        ),
        "an fr minimum kept as stored: {:?}",
        g.columns
    );

    let (mut s, f, _) = base();
    let mut columns = vec![px(100.0), px(50.0), px(100.0)];
    columns.push(Track::Repeat {
        repeat: 3,
        tracks: vec![TrackSize::Breadth(TrackBreadth::Px(10.0))],
    });
    s.display(f, Some(Display::Grid(grid(columns, vec![]))));
    let (doc, _res) = reopened(&s, r#""repeat": 3"#, r#""repeat": 5000"#);
    let g = ondin_core::build::laid_grid(&doc, f).expect("a grid");
    assert_eq!(g.columns.explicit, 1000, "laid to the cap");
}

/// The first test's grid, with the four items at the start of their cells.
fn four_in_tracks(s: &mut Scene) -> (NodeId, [NodeId; 4]) {
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let items = [0; 4].map(|_| s.add(f, rect(20.0, 20.0), (300.0, 150.0)));
    s.display(
        f,
        Some(Display::Grid(Grid {
            column_gap: 10.0,
            row_gap: 10.0,
            padding: [20.0; 4],
            ..grid(vec![px(100.0), fr(1.0), fr(2.0)], vec![px(50.0), px(40.0)])
        })),
    );
    (f, items)
}

/// **A laid grid reports its tracks and its items' areas** (§15 D916) — the
/// first test's numbers read back: columns at 20–120, 130–210 and 220–380, rows
/// at 20–70 and 80–120, and the fourth item in column 1, row 2, as CSS lines.
#[test]
fn a_laid_grid_reports_its_tracks_and_areas() {
    let mut s = Scene::new();
    let (f, [a, _, _, d]) = four_in_tracks(&mut s);
    let g = ondin_core::build::laid_grid(&s.doc, f).expect("a grid");
    assert_eq!(
        g.columns.spans,
        vec![(20.0, 120.0), (130.0, 210.0), (220.0, 380.0)]
    );
    assert_eq!(g.rows.spans, vec![(20.0, 70.0), (80.0, 120.0)]);
    assert_eq!((g.columns.explicit, g.rows.explicit), (3, 2));
    let area = |id| g.areas.iter().find(|(n, _)| *n == id).unwrap().1;
    assert_eq!(area(a), [1, 2, 1, 2]);
    assert_eq!(area(d), [1, 2, 2, 3]);
    assert_eq!(
        g.columns.index_at(125.0),
        Some(0),
        "a gap goes to the nearer edge"
    );
    assert_eq!(g.columns.index_at(900.0), Some(2), "past the end, the end");
}

/// **A drop writes the lines of the cell the centre lands in** (§15 D913's
/// third ruling, `build::grid_drop`): the auto-placed first item, its centre at
/// (30, 30), dragged by (+200, +60) to (230, 90) — column 3, row 2 — is written
/// `3 / auto` and `2 / auto`, and lands at (220, 80); the three auto-placed
/// siblings flow round it. A drag short of the next track is no operation.
///
/// **Flip run**, the "no operation when the centre stays" guard removed: fails on
/// *"back in its own cell"*, `Some` for a 20 px nudge, the predicted site.
#[test]
fn a_drop_writes_the_lines_of_the_cell_it_lands_in() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let (_f, [a, b, ..]) = four_in_tracks(&mut s);
    assert_eq!(
        ondin_core::build::grid_drop(&s.doc, &s.res, a, Vec2::new(20.0, 10.0)),
        None,
        "back in its own cell"
    );
    let op = ondin_core::build::grid_drop(&s.doc, &s.res, a, Vec2::new(200.0, 60.0))
        .expect("a new cell");
    let Operation::SetLayoutItem { item, .. } = &op else {
        panic!("{op:?}")
    };
    assert_eq!(
        (item.grid_column, item.grid_row),
        (
            GridLines {
                start: GridPlacement::Line(3),
                end: GridPlacement::Auto
            },
            GridLines {
                start: GridPlacement::Line(2),
                end: GridPlacement::Auto
            }
        )
    );
    s.commit(vec![op]);
    assert_eq!(s.bounds(a).origin(), (220.0, 80.0).into(), "landed");
    assert_eq!(
        s.bounds(b).origin(),
        (20.0, 20.0).into(),
        "b took the first cell"
    );
}

/// **A dragged area keeps its span, in the author's spelling**: `1 / span 2`
/// dragged one column right is `2 / span 2`; `1 / 3` is `2 / 4`. The drag
/// is counted in tracks crossed by the centre, so an item spanning two columns
/// moves by one when its centre crosses one boundary, not by half its width.
#[test]
fn a_dropped_area_keeps_its_span_and_its_spelling() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
    let wide = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Stretch),
            ..grid(vec![px(100.0), px(100.0), px(100.0)], vec![px(50.0)])
        })),
    );
    for (end, moved) in [
        (GridPlacement::Span(2), GridPlacement::Span(2)),
        (GridPlacement::Line(3), GridPlacement::Line(4)),
    ] {
        s.item(wide, |i| {
            i.grid_column = GridLines {
                start: GridPlacement::Line(1),
                end,
            }
        });
        assert_eq!(s.bounds(wide).width(), 200.0, "the fixture: two columns");
        // Its centre at x 100, on the boundary; 60 right is inside column 2.
        let op = ondin_core::build::grid_drop(&s.doc, &s.res, wide, Vec2::new(60.0, 0.0))
            .expect("one column over");
        let Operation::SetLayoutItem { item, .. } = &op else {
            panic!("{op:?}")
        };
        assert_eq!(
            item.grid_column,
            GridLines {
                start: GridPlacement::Line(2),
                end: moved
            },
            "{end:?}"
        );
        s.commit(vec![op]);
        assert_eq!(s.bounds(wide).x0, 100.0, "{end:?}: moved one column");
    }
}

/// **Several items of one grid move as a block** (§15 D918, D902's reading in a
/// grid): `a` and `b` in the first two cells — drawn at (20, 20) and (130, 20),
/// their union's centre at (85, 30) — dragged by (+100, +60) to (185, 90), one
/// column and one row over, land in column 2 and column 3 of row 2, side by side
/// as they were; a nudge lands where it was.
///
/// **And a block pushed past line 1 stops there whole.** Four 50px columns, `a`
/// placed in column 2 and `b` in column 4: their union's centre is at x 110, in
/// column 3, and a drag of −110 puts it in column 1 — two columns back, where
/// `a` has room for one. The block moves one: `a` to column 1, `b` to column 3,
/// the gap between them kept, rather than `a` held at 1 and `b` moved two.
///
/// **Flip run**, `grid_drop_many`'s `room` answering the raw shift (each start
/// then clamped alone by `line`, as a single item's is): fails on *"stopped
/// whole at line 1"* with `b` at line 2, the predicted site.
#[test]
fn several_items_of_a_grid_move_as_a_block() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let (_f, [a, b, ..]) = four_in_tracks(&mut s);
    let ops = ondin_core::build::grid_drop_many(&s.doc, &s.res, &[a, b], Vec2::new(100.0, 60.0))
        .expect("one grid's items");
    assert_eq!(ops.len(), 2);
    s.commit(ops);
    assert_eq!(
        s.bounds(a).origin(),
        (130.0, 80.0).into(),
        "a: column 2, row 2"
    );
    assert_eq!(
        s.bounds(b).origin(),
        (220.0, 80.0).into(),
        "b: column 3, row 2"
    );
    assert_eq!(
        ondin_core::build::grid_drop_many(&s.doc, &s.res, &[a, b], Vec2::new(5.0, 5.0)),
        Some(Vec::new()),
        "a nudge lands where it was"
    );

    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 50.0), (0.0, 0.0));
    let [a, b] = [0; 2].map(|_| s.add(f, rect(20.0, 20.0), (0.0, 0.0)));
    s.display(
        f,
        Some(Display::Grid(grid(
            vec![px(50.0), px(50.0), px(50.0), px(50.0)],
            vec![px(50.0)],
        ))),
    );
    s.item(a, |i| i.grid_column.start = GridPlacement::Line(2));
    s.item(b, |i| i.grid_column.start = GridPlacement::Line(4));
    assert_eq!(
        (s.bounds(a).x0, s.bounds(b).x0),
        (50.0, 150.0),
        "the fixture"
    );
    let ops = ondin_core::build::grid_drop_many(&s.doc, &s.res, &[a, b], Vec2::new(-110.0, 0.0))
        .expect("one grid's items");
    s.commit(ops);
    let column = |id| s.doc.get(id).unwrap().item().grid_column.start;
    assert_eq!(
        (column(a), column(b)),
        (GridPlacement::Line(1), GridPlacement::Line(3)),
        "stopped whole at line 1"
    );
}

/// **An area before the explicit grid keeps the negative line that names it**
/// (§15 D918's amendment) — three 50px columns, and `a` placed at `-5`: line −1
/// is line 4, so −5 is the line before line 1, and the grid grows a leading
/// implicit track for it, 20 wide around `a`. Dragged one row down, `a` is
/// written `-5` again on the axis the drag left alone, and stays at x 0; the
/// first cut wrote line 1 there and moved it a column right.
///
/// **Flip run**, `grid_drop_many`'s `line` clamping to 1 as it did: fails on
/// *"its column kept"* with `Line(1)`, the predicted site.
#[test]
fn an_area_before_the_explicit_grid_keeps_its_negative_line() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(grid(
            vec![px(50.0), px(50.0), px(50.0)],
            vec![px(50.0), px(50.0)],
        ))),
    );
    s.item(a, |i| i.grid_column.start = GridPlacement::Line(-5));
    let g = ondin_core::build::laid_grid(&s.doc, f).expect("a grid");
    assert_eq!(
        g.columns.before, 1,
        "the fixture: one leading implicit track"
    );
    assert_eq!(s.bounds(a).x0, 0.0, "the fixture: a in it");

    let op =
        ondin_core::build::grid_drop(&s.doc, &s.res, a, Vec2::new(0.0, 50.0)).expect("a row down");
    s.commit(vec![op]);
    let item = *s.doc.get(a).unwrap().item();
    assert_eq!(
        item.grid_column.start,
        GridPlacement::Line(-5),
        "its column kept"
    );
    assert_eq!(item.grid_row.start, GridPlacement::Line(2));
    assert_eq!(s.bounds(a).origin(), (0.0, 50.0).into());
}

/// **A grid laid out more than once in one pass reports its last layout**
/// (`laid_grid`, §15 D916's *"last report wins"*, owed since as read and not
/// tested). The grid `g` grows in a hugging flex row `f` that aligns to the
/// baseline, and `f` grows in a 600-wide row `o`: `o` *measures* `f` first, and
/// taffy's baseline step lays `f`'s items out in full even in a measure — so `g`
/// is laid at its own 100 while `f` hugs, and again at 580 once `f` has grown to
/// 600 and `g` with it. The report is the second: columns 0–290 and 290–580.
///
/// **Flip run**, `set_detailed_grid_info` keeping the first report (`if
/// self.grid.is_none()` on the write): fails on *"the columns of the layout
/// that placed it"* with 0–50 and 50–100, the measure's — the predicted site.
#[test]
fn a_grid_laid_out_twice_reports_its_last_layout() {
    let mut s = Scene::new();
    let o = s.add(s.root, frame(600.0, 100.0), (0.0, 0.0));
    let f = s.add(o, frame(10.0, 100.0), (0.0, 0.0));
    let g = s.add(f, frame(100.0, 50.0), (0.0, 0.0));
    let beside = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(o, Some(Display::Flex(Flex::default())));
    s.display(
        f,
        Some(Display::Flex(Flex {
            align_items: AlignItems::Baseline,
            ..Default::default()
        })),
    );
    s.display(g, Some(Display::Grid(grid(vec![fr(1.0), fr(1.0)], vec![]))));
    s.item(f, |i| {
        i.width = ondin_core::container::Dimension::FitContent;
        i.grow = 1.0;
    });
    s.item(g, |i| i.grow = 1.0);
    assert_eq!(s.bounds(f).width(), 600.0, "the fixture: f grown across o");
    assert_eq!(
        s.bounds(g).width(),
        580.0,
        "and g across f, beside the rect"
    );
    assert_eq!(s.bounds(beside).x0, 580.0, "the fixture: the rect after g");
    let laid = ondin_core::build::laid_grid(&s.doc, g).expect("a grid");
    assert_eq!(
        laid.columns.spans,
        vec![(0.0, 290.0), (290.0, 580.0)],
        "the columns of the layout that placed it"
    );
}

/// An auto-width label, `Inter` 12.
fn label(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(ondin_core::TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            weight: 400,
            italic: false,
            line_height: Some(ondin_core::Length::Em(1.2)),
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// **An auto-width label stretched narrower than its line keeps its line** (§15
/// D917): in a 30px grid column, and in a 30-wide flex column, a label 161.6 wide
/// on one line is laid as a fixed box as wide as its line — overflowing its slot,
/// `white-space: nowrap`'s reading — and not as a 30-wide box that wraps it.
/// Auto width never wraps (D875, D913's first ruling); both layouts broke it the
/// same way, the stretch handing the text *less* room than it measured.
///
/// **Flip run**, `flexed_text`'s floor removed (the fixed box at the stretched
/// size, as it was): fails on *"grid: one line"* at a width of 30, the predicted
/// site. The flex half is not reached under that flip; the probe that found this
/// read flex's box as `Fixed(30 × 14.4)` before the repair.
#[test]
fn an_auto_width_label_stretched_narrower_keeps_its_line() {
    let width = |s: &Scene, t| match s.res.used_kind(&s.doc, t) {
        Some(NodeKind::Text { sizing, .. }) => match sizing {
            ondin_core::TextSizing::Fixed(size) => size.width,
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let t = s.add(f, label("a label wider than its column"), (0.0, 0.0));
    let line = s.bounds(t).width();
    assert!(line > 150.0, "the fixture: one long line, {line}");
    s.display(f, Some(Display::Grid(grid(vec![px(30.0)], vec![]))));
    assert_eq!(width(&s, t), line, "grid: one line");
    assert_eq!(s.bounds(t).x0, 0.0, "at the start of its cell, overflowing");

    s.resize(f, 30.0, 100.0);
    s.display(
        f,
        Some(Display::Flex(Flex {
            direction: ondin_core::container::FlexDirection::Column,
            ..Default::default()
        })),
    );
    assert_eq!(width(&s, t), line, "flex: one line");
}

/// **A track list reads and writes as CSS** (§15 D920) — the mockup's own line,
/// `200px 1fr minmax(100px, 2fr) repeat(3, auto)`, parses to its four entries and
/// writes back the same; keywords in any case, no space after a comma, a unitless
/// `0`, a percentage and the two content keywords all read; `none` and nothing
/// are the empty list, which writes as `none`.
#[test]
fn a_track_list_reads_and_writes_as_css() {
    use ondin_core::container::{parse_tracks, tracks_css};
    let line = "200px 1fr minmax(100px, 2fr) repeat(3, auto)";
    let tracks = parse_tracks(line).unwrap();
    assert_eq!(
        tracks,
        vec![
            px(200.0),
            fr(1.0),
            Track::Size(TrackSize::MinMax {
                min: TrackBreadth::Px(100.0),
                max: TrackBreadth::Fr(2.0),
            }),
            Track::Repeat {
                repeat: 3,
                tracks: vec![TrackSize::Breadth(TrackBreadth::Auto)],
            },
        ]
    );
    assert_eq!(tracks_css(&tracks), line);

    assert_eq!(
        parse_tracks("  MINMAX(0,Max-Content) 12.5%  repeat(2, 1.5fr min-content)  ").unwrap(),
        vec![
            Track::Size(TrackSize::MinMax {
                min: TrackBreadth::Px(0.0),
                max: TrackBreadth::MaxContent,
            }),
            Track::Size(TrackSize::Breadth(TrackBreadth::Percent(12.5))),
            Track::Repeat {
                repeat: 2,
                tracks: vec![
                    TrackSize::Breadth(TrackBreadth::Fr(1.5)),
                    TrackSize::Breadth(TrackBreadth::MinContent),
                ],
            },
        ]
    );
    assert_eq!(parse_tracks("none").unwrap(), vec![]);
    assert_eq!(parse_tracks("   ").unwrap(), vec![]);
    assert_eq!(tracks_css(&[]), "none");
}

/// **What is not a track list is an error, not a guess** — a number with no unit,
/// an unclosed `repeat(`, a fractional count, a word CSS does not have — and what
/// parses but CSS refuses is still refused at the operation, not here.
#[test]
fn what_is_not_a_track_list_is_an_error() {
    use ondin_core::container::parse_tracks;
    for bad in [
        "100",
        "repeat(3, auto",
        "repeat(1.5, auto)",
        "fit",
        "1px,2px",
    ] {
        assert!(parse_tracks(bad).is_err(), "{bad}");
    }
    // `minmax(1fr, 2fr)` reads — and `Grid::is_valid` is what refuses it.
    let t = parse_tracks("minmax(1fr, 2fr)").unwrap();
    assert!(
        !Grid {
            columns: t,
            ..Default::default()
        }
        .is_valid()
    );
}

/// **A line reads and writes as CSS**: `auto`, a line, a negative line, `span n`,
/// in any case; line 0 and `span 0`, which CSS refuses, do not read at all.
#[test]
fn a_placement_reads_and_writes_as_css() {
    use ondin_core::container::{parse_placement, placement_css};
    for (text, p) in [
        ("auto", GridPlacement::Auto),
        ("3", GridPlacement::Line(3)),
        ("-1", GridPlacement::Line(-1)),
        ("span 2", GridPlacement::Span(2)),
    ] {
        assert_eq!(parse_placement(text), Some(p), "{text}");
        assert_eq!(placement_css(p), text);
    }
    assert_eq!(parse_placement(" SPAN  4 "), Some(GridPlacement::Span(4)));
    for bad in ["0", "span 0", "span", "two", ""] {
        assert_eq!(parse_placement(bad), None, "{bad}");
    }
}
