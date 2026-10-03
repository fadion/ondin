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
/// `AlignContent` on both axes: 360 across, less a gap of 10 and two 20-wide
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
/// and `span 0` — and a template past `MAX_TEMPLATE_TRACKS` (§15 D919), one
/// repeat at 1001 and two entries whose repeats only pass it together — and
/// nothing is changed by any of them.
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
    .expect("exactly MAX_TEMPLATE_TRACKS is accepted");
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
/// **Flip run**, the backward half of `grid_drop_many`'s `room` answering the
/// raw shift: fails on *"stopped whole at line 1"* with `a` written as line −6
/// and `b` at line 2 — the predicted site. (This read *"each start then clamped
/// alone by `line`"*; `line` clamps nothing since §15 D918's amendment, and a
/// start before the grid is written as a negative line — `[R2-L8-02]`.)
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

/// **A spanning item dropped on the last column stops with its span inside the
/// grid** (§15 D927, the maintainer's test 1 of 2026-10-02): over three `1fr`
/// columns, `a` spanning two is dropped with its centre on the third and lands
/// as `2 / span 2` — the last two columns — where it used to be written
/// `3 / span 2` and hang into an implicit fourth column the `fr` tracks size to
/// nothing, drawn spanning one. A drop that has room is not clamped: `b` alone
/// to the third column is `3`.
///
/// **Flip run**, the forward half of `grid_drop_many`'s `room` answering `by`
/// unclamped: fails on *"the span kept inside"* with line 3 — the predicted
/// site.
#[test]
fn a_spanning_item_dropped_on_the_last_column_keeps_its_span_inside() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(grid(vec![fr(1.0), fr(1.0), fr(1.0)], vec![]))),
    );
    s.item(a, |i| i.grid_column.end = GridPlacement::Span(2));
    assert_eq!(
        s.bounds(b).x0,
        200.0,
        "the fixture: b after a's two columns"
    );

    let op = ondin_core::build::grid_drop(&s.doc, &s.res, a, Vec2::new(250.0, 0.0))
        .expect("a drop to the last column");
    s.commit(vec![op]);
    let lines = s.doc.get(a).unwrap().item().grid_column;
    assert_eq!(
        (lines.start, lines.end),
        (GridPlacement::Line(2), GridPlacement::Span(2)),
        "the span kept inside"
    );
    assert_eq!(s.bounds(a).x0, 100.0, "drawn from the second column");

    let (mut s, f2) = {
        let mut s = Scene::new();
        let f2 = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
        s.display(
            f2,
            Some(Display::Grid(grid(vec![fr(1.0), fr(1.0), fr(1.0)], vec![]))),
        );
        (s, f2)
    };
    let b = s.add(f2, rect(20.0, 20.0), (0.0, 0.0));
    let op = ondin_core::build::grid_drop(&s.doc, &s.res, b, Vec2::new(250.0, 0.0))
        .expect("a drop to the last column");
    s.commit(vec![op]);
    assert_eq!(
        s.doc.get(b).unwrap().item().grid_column.start,
        GridPlacement::Line(3),
        "room to spare, no clamp"
    );
}

/// **A block dropped at the bottom-right stops whole inside, on both axes**
/// (§15 D927's row axis and its block case, *"read, not tested"* until
/// `[X4.1-L6-01]`): over a 3 × 3 grid of `1fr`, `a` spanning two rows and `b`
/// beside it are dragged together until the block's centre is on the last
/// cell. The block moves one column and one row — as far as `b`'s column and
/// `a`'s rows have room for — so `a` lands `2 / span 2` down and in column 2,
/// `b` in column 3 row 2, neither hanging past the last line.
///
/// **Flip runs**: `room`'s forward arm clamping columns only (`axis == 0`)
/// fails on *"a stopped at the last row"*, line 3 — the predicted site. `room`
/// reading `last` off the first area rather than the furthest was predicted to
/// fail on *"b stopped at the last column"* and fails one assertion sooner, on
/// *"a moved one column"* with line 3: the block moved two, `a` with it.
#[test]
fn a_block_dropped_at_the_bottom_right_stops_whole_inside() {
    use ondin_core::kurbo::Vec2;
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 300.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let tracks = || vec![fr(1.0), fr(1.0), fr(1.0)];
    s.display(f, Some(Display::Grid(grid(tracks(), tracks()))));
    s.item(a, |i| i.grid_row.end = GridPlacement::Span(2));
    assert_eq!(
        (s.bounds(b).x0, s.bounds(b).y0),
        (100.0, 0.0),
        "the fixture: b beside a, in the first row"
    );

    // The block's centre, (60, 10), carried to (280, 260): the last cell.
    let ops = ondin_core::build::grid_drop_many(&s.doc, &s.res, &[a, b], Vec2::new(220.0, 250.0))
        .expect("a block drop");
    s.commit(ops);
    let lines = |id| {
        let item = s.doc.get(id).unwrap().item();
        (
            item.grid_column.start,
            item.grid_row.start,
            item.grid_row.end,
        )
    };
    let (a_col, a_row, a_span) = lines(a);
    assert_eq!(a_row, GridPlacement::Line(2), "a stopped at the last row");
    assert_eq!(a_span, GridPlacement::Span(2), "and keeps its span");
    assert_eq!(a_col, GridPlacement::Line(2), "a moved one column");
    let (b_col, b_row, _) = lines(b);
    assert_eq!(
        b_col,
        GridPlacement::Line(3),
        "b stopped at the last column"
    );
    assert_eq!(b_row, GridPlacement::Line(2), "b moved one row, with a");
    assert_eq!(s.bounds(a).y0, 100.0, "a drawn from the second row");
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

/// **Outlining a grid item keeps its cell** (§15 D930, the release review's
/// `[X1-L1-01]`). Three 100 px columns, `a` set to `grid-column: 3`, `b` auto —
/// so `b` takes the first cell and `a` the third. `replace_with_path` dropped
/// the layer's item properties, so the outline of `a` came back auto-placed in
/// the first cell and pushed `b` into the second: two layers moved by an edit
/// that changes only how one of them is held.
///
/// **Flip run**, the `SetLayoutItem` carry deleted: fails on *"the path keeps
/// a's cell"*, x0 0 against 200, the predicted site.
#[test]
fn outlining_a_grid_item_keeps_its_cell() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let b = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![px(100.0), px(100.0), px(100.0)], vec![px(50.0)])
        })),
    );
    s.item(a, |i| i.grid_column.start = GridPlacement::Line(3));
    let (was_a, was_b) = (s.bounds(a), s.bounds(b));
    assert_eq!((was_a.x0, was_b.x0), (200.0, 0.0), "the fixture");
    let (tx, path) = ondin_core::build::outline(&s.doc, &s.res, &mut s.ids, a).unwrap();
    s.commit(tx.0);
    assert_eq!(s.bounds(path), was_a, "the path keeps a's cell");
    assert_eq!(s.bounds(b), was_b, "and b keeps its own");
}

/// **A file's refused layout value can be edited away, undone back to, and
/// carried forward by an edit to something else** (§15 D937, the release
/// review's `[R3-L1-01]`). A grid item at `grid-column: 0` (CSS refuses line 0;
/// D914 opens it and keeps it): set to line 3, then undone. The undo's inverse
/// wrote line 0 back, `SetLayoutItem` refused it with `BadLayout`, and
/// `History::undo`, having popped the step, dropped it — the file's value could
/// never be returned to. And a change to the item's `grow`, which writes the
/// whole record back with the line in it, was refused whole. (A gap edit on a
/// grid whose template a file carries refused was too; `Display::is_valid_over`
/// is the same rule for it, read and not tested here.)
///
/// **Flip runs**: `History::undo` back on plain `apply` fails on *"the undo
/// lands"*, `Err(BadLayout)`; `op_set_layout_item` checking `is_valid` rather
/// than `is_valid_over` fails on *"carried forward"* — both predicted.
#[test]
fn a_files_refused_value_can_be_undone_back_to_and_carried_forward() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(grid(
            vec![px(100.0), px(100.0), px(100.0)],
            vec![px(50.0)],
        ))),
    );
    s.item(a, |i| i.grid_column.start = GridPlacement::Line(2));
    let (mut doc, _) = reopened(&s, r#""Line": 2"#, r#""Line": 0"#);
    assert_eq!(
        doc.get(a).unwrap().item().grid_column.start,
        GridPlacement::Line(0),
        "the fixture: the file carries line 0"
    );
    let mut history = History::new();
    let at = |doc: &Document, f: &dyn Fn(&mut LayoutItem)| {
        let mut item = *doc.get(a).unwrap().item();
        f(&mut item);
        Transaction(vec![Operation::SetLayoutItem { id: a, item }])
    };
    let tx = at(&doc, &|i| i.grid_column.start = GridPlacement::Line(3));
    history.commit(&mut doc, tx).expect("a valid line commits");
    assert!(
        matches!(history.undo(&mut doc), Ok(Some(_))),
        "the undo lands"
    );
    assert_eq!(
        doc.get(a).unwrap().item().grid_column.start,
        GridPlacement::Line(0),
        "and puts the file's value back"
    );
    assert!(
        matches!(history.redo(&mut doc), Ok(Some(_))),
        "and the redo"
    );
    history.undo(&mut doc).unwrap();
    let tx = at(&doc, &|i| i.grow = 1.0);
    assert!(
        history.commit(&mut doc, tx).is_ok(),
        "an edit to another property carried forward"
    );
    let tx = at(&doc, &|i| i.grid_row.start = GridPlacement::Line(0));
    assert!(
        matches!(history.commit(&mut doc, tx), Err(OpError::BadLayout)),
        "while a refused value an edit writes is still refused"
    );
}

/// **A negative padding, gap, growth or size is refused, and read as zero from
/// a file** (§15 D937, `[X3.1-L2-01]`) — CSS refuses all of them, and a row with
/// `column-gap: -20` laid its second item over its first. The cards clamp at
/// zero; the operations did not.
///
/// **Flip run**, `Display::is_valid_over` without the padding and gap rule:
/// fails on *"a negative gap is refused"*, `Ok`, the predicted site.
#[test]
fn a_negative_layout_length_is_refused_and_read_as_zero() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(50.0, 50.0), (0.0, 0.0));
    let b = s.add(f, rect(50.0, 50.0), (0.0, 0.0));
    let flex = |gap: f64| {
        Some(Display::Flex(Flex {
            column_gap: gap,
            align_items: AlignItems::Start,
            ..Default::default()
        }))
    };
    s.display(f, flex(10.0));
    assert!(
        matches!(
            s.try_commit(vec![Operation::SetDisplay {
                id: f,
                display: flex(-20.0)
            }]),
            Err(OpError::BadLayout)
        ),
        "a negative gap is refused"
    );
    for item in [
        LayoutItem {
            grow: -1.0,
            ..Default::default()
        },
        LayoutItem {
            width: ondin_core::container::Dimension::Px(-40.0),
            ..Default::default()
        },
    ] {
        assert!(
            matches!(
                s.try_commit(vec![Operation::SetLayoutItem { id: a, item }]),
                Err(OpError::BadLayout)
            ),
            "{item:?} is refused"
        );
    }
    let (_, res) = reopened(&s, r#""column_gap": 10.0"#, r#""column_gap": -20.0"#);
    assert_eq!(
        res.world_bounds(b).unwrap().x0,
        50.0,
        "a file's negative gap is laid as zero"
    );
}

/// **`baseline` lines up text's first baselines, in a grid and in a flex row**
/// (§15 D939, the release review's `[X3.3-L1-01]`). Two labels, 12 and 24 px
/// Inter at a 1.2em line, side by side with `align-items: baseline`: a text leaf
/// reported no baseline to the engine, which then aligned the boxes' bottom
/// edges — the small label placed at y 14.40625, its letters about 3 px below
/// the big one's. Asserted on what the user sees: each label's first baseline in
/// world y, read off its own shaped layout, equal across the two. A shape keeps
/// no baseline and is still aligned by its bottom, CSS's synthesized one.
///
/// **Flip run**, the leaf's `baselines.first` left unset: fails on the grid's
/// *"letters on one line"*, the bottoms aligned instead — the predicted site.
#[test]
fn baseline_aligns_text_by_its_first_baseline() {
    let label = |size: f64| NodeKind::Text {
        content: "Label".into(),
        style: Box::new(ondin_core::TextStyle {
            font_family: "Inter".into(),
            font_size: size,
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
    };
    let baseline = |s: &Scene, id: NodeId| {
        let layout = s.res.text_layout(id).expect("shaped");
        s.res.world_transform(id).unwrap().translation().y + layout.baselines[0]
    };
    for grid_layout in [true, false] {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
        let small = s.add(f, label(12.0), (0.0, 0.0));
        let big = s.add(f, label(24.0), (0.0, 0.0));
        s.display(
            f,
            Some(if grid_layout {
                Display::Grid(Grid {
                    align_items: Some(AlignItems::Baseline),
                    justify_items: Some(AlignItems::Start),
                    ..grid(vec![px(200.0), px(200.0)], vec![px(100.0)])
                })
            } else {
                Display::Flex(Flex {
                    align_items: AlignItems::Baseline,
                    ..Default::default()
                })
            }),
        );
        let (a, b) = (baseline(&s, small), baseline(&s, big));
        assert!(
            (a - b).abs() < 1.0 / 64.0,
            "letters on one line ({}): {a} against {b}",
            if grid_layout { "grid" } else { "flex" }
        );
    }
}

/// **A drop into a grid pinned across a stretched frame picks the cell it is
/// drawn over** (§15 D942, D933's residue): an 800-wide page holds a frame stored
/// 500 wide and pinned `left: 0; right: 0`, so drawn 800; in it, four `1fr`
/// columns stored 200 wide and pinned the same way, so drawn 800 — 200 apiece.
/// `a`, in the first, dragged 260 right lands in the second. The drop laid its
/// grid through the parent's *stored* box — the trait's default, 500 — at 125
/// apiece, and wrote the third.
///
/// **Flip run**, `grid_drop_many` back on `DocView`: fails on *"the second
/// column"*, line 3 against 2, the predicted site. (A first draft pinned the grid
/// in an unstretched frame, whose stored size *is* its drawn one, and the flip
/// passed.)
#[test]
fn a_drop_into_a_pinned_grid_picks_the_cell_it_is_drawn_over() {
    let mut s = Scene::new();
    let both = ondin_core::Insets {
        left: Some(ondin_core::LengthPct::Px(0.0)),
        right: Some(ondin_core::LengthPct::Px(0.0)),
        ..Default::default()
    };
    let page = s.add(s.root, frame(800.0, 300.0), (0.0, 0.0));
    let outer = s.add(page, frame(500.0, 200.0), (0.0, 0.0));
    s.commit(vec![Operation::SetInsets {
        id: outer,
        insets: both,
    }]);
    let g = s.add(outer, frame(200.0, 100.0), (0.0, 0.0));
    let a = s.add(g, rect(10.0, 10.0), (0.0, 0.0));
    s.display(
        g,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::Start),
            ..grid(vec![fr(1.0), fr(1.0), fr(1.0), fr(1.0)], vec![px(100.0)])
        })),
    );
    s.commit(vec![Operation::SetInsets {
        id: g,
        insets: both,
    }]);
    assert_eq!(s.bounds(g).width(), 800.0, "the fixture: stretched twice");
    let Some(Operation::SetLayoutItem { item, .. }) =
        ondin_core::build::grid_drop(&s.doc, &s.res, a, ondin_core::kurbo::Vec2::new(260.0, 0.0))
    else {
        panic!("a drop");
    };
    assert_eq!(
        item.grid_column.start,
        GridPlacement::Line(2),
        "the second column"
    );
}
