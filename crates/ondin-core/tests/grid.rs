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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
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

/// **An item with lines takes its area, and a stretched one fills it** — CSS's
/// `normal`, which this model spells `Stretch` (§15 D914). `grid-column: 2 /
/// span 2` over the tracks above is 80 + 10 + 160 = 250 wide from x 130, and
/// `grid-row: 2` is the 40 row from y 80; the auto-placed item skips nothing it
/// does not have to and takes the first cell, stretched to 100 × 50.
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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
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
        justify_items: AlignItems::Start,
        align_items: AlignItems::Start,
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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
            ..grid(vec![px(50.0), px(50.0)], vec![])
        })),
    );
    assert_eq!(s.bounds(a).x0, 50.0, "the second column");
}

/// **What CSS refuses, the operations refuse** (`OpError::BadLayout`, §15 D914):
/// a negative track, an `fr` minimum, `repeat(0, …)`, an empty repeat, line 0
/// and `span 0` — and nothing is changed by any of them.
#[test]
fn values_css_refuses_are_refused() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let bad_tracks = [
        px(-1.0),
        Track::Size(TrackSize::MinMax {
            min: TrackBreadth::Fr(1.0),
            max: TrackBreadth::Fr(2.0),
        }),
        Track::Repeat {
            repeat: 0,
            tracks: vec![TrackSize::Breadth(TrackBreadth::Auto)],
        },
        Track::Repeat {
            repeat: 2,
            tracks: vec![],
        },
    ];
    for t in bad_tracks {
        let r = s.try_commit(vec![Operation::SetDisplay {
            id: f,
            display: Some(Display::Grid(grid(vec![t.clone()], vec![]))),
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
            justify_items: AlignItems::Start,
            align_items: AlignItems::Start,
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
