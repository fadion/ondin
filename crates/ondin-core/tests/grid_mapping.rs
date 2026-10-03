//! Grid's model → taffy mapping and its validity rules, one value at a time,
//! against boxes worked by hand from CSS (the release review's `[X3.3-L6-01]`).
//!
//! `tests/grid.rs` pins grid at row and column flow, the `minmax()` order and
//! negative lines; every fixture there uses uniform padding, sets
//! `justify-items` and `align-items` to one value, and lays no percentage or
//! content-keyword track — so a swapped axis or a wrong arm passed the whole
//! workspace. `update` against `rebuild` cannot see one either: both go through
//! `style_of`.
//!
//! The harness is `tests/grid.rs`' `Scene`, and its `reopened` for the values
//! only a file can carry.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::{
    Document, History, IdSource, NodeId, NodeKind, OpError, Operation, Resolved, Transaction,
    container::{
        AlignContent, AlignItems, Dimension, Display, Flex, Grid, LayoutItem, Track, TrackBreadth,
        TrackSize,
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

fn breadth(b: TrackBreadth) -> Track {
    Track::Size(TrackSize::Breadth(b))
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

    fn bounds(&self, id: NodeId) -> Rect {
        self.res.world_bounds(id).expect("measured")
    }
}

/// `s`'s document saved, `from` swapped for `to` in the file — exactly once —
/// and opened again: how a test gets a value the operations refuse into a
/// document, as another tool or a hand edit would. `tests/grid.rs`' helper.
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

/// **Padding, the item alignments and the content alignments each read their
/// own side and axis** — a 400 × 200 frame with padding `[10, 20, 30, 40]`
/// (top, right, bottom, left: CSS's order), columns `100px 100px`, one `50px`
/// row, `justify-items: start`, `align-items: end`, `justify-content: start`
/// and `align-content: center`, holding a 20 × 20 rect in the first cell.
///
/// By hand: the content box is x 40..380 and y 10..170, 160 tall; the row is
/// centred in it, at 10 + (160 − 50) / 2 = 65..115; the first column is 40..140.
/// The rect sits at the start across — x 40..60 — and at the end down —
/// y 115 − 20 = 95..115.
///
/// Every value differs from its neighbour on purpose: each fixture in
/// `tests/grid.rs` gives the four padding sides one value and the two item
/// alignments one value, so a swapped side or axis is invisible there.
///
/// **Flip runs**, each one alone in `style_of`'s grid arm, all failing on the one
/// assertion as predicted: padding destructured `[left, top, right, bottom]`,
/// the rect at (10, 95, 30, 115) — wrong across only, since the swapped top and
/// bottom (20 and 40 for 10 and 30) centre the row at the same 65; `align_content`
/// fed `g.justify_content`, (40, 40, 60, 60), the row at the top; `justify_items`
/// and `align_items` swapped, (120, 65, 140, 85).
#[test]
fn padding_and_alignments_each_read_their_own_side_and_axis() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    let a = s.add(f, rect(20.0, 20.0), (300.0, 150.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            padding: [10.0, 20.0, 30.0, 40.0],
            justify_items: Some(AlignItems::Start),
            align_items: Some(AlignItems::End),
            justify_content: AlignContent::Start,
            align_content: AlignContent::Center,
            ..grid(vec![px(100.0), px(100.0)], vec![px(50.0)])
        })),
    );
    assert_eq!(
        s.bounds(a),
        Rect::new(40.0, 95.0, 60.0, 115.0),
        "x at the left padding 40, start of column 1; y at the end of the centred row, 115 - 20"
    );
}

/// **A percentage track is a share of the frame**: `25% 50px` in a 400-wide
/// frame with no padding is a column 0..100 and one 100..150. CSS's percent is
/// stored as typed (`TrackBreadth::Percent(25)`) and handed over as a fraction.
///
/// **Flip run**, `min_breadth` and `max_breadth` handing `Percent(p)` over as `p`
/// rather than `p / 100`: fails on *"25% of 400 is 100"*, the columns at
/// 0..10000 and 10000..10050 — predicted.
#[test]
fn a_percent_track_is_a_share_of_the_frame() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_content: AlignContent::Start,
            ..grid(
                vec![breadth(TrackBreadth::Percent(25.0)), px(50.0)],
                vec![px(50.0)],
            )
        })),
    );
    let g = ondin_core::build::laid_grid(&s.doc, f).expect("a grid");
    assert_eq!(
        g.columns.spans,
        vec![(0.0, 100.0), (100.0, 150.0)],
        "25% of 400 is 100"
    );
}

/// A wrapping text item, `Inter` 12, wrapping at `width` — CSS's `max-content`
/// for it is `width`, its `min-content` its widest word
/// (`FlexTree::measure`'s `AutoHeight` arm).
fn wrapping(content: &str, width: f64) -> NodeKind {
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
        sizing: ondin_core::TextSizing::AutoHeight(width),
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// **A `min-content` track is as narrow as its widest word and a `max-content`
/// one as wide as its text's preferred width** — columns
/// `min-content max-content`, each holding the same wrapping label, 200 wide
/// when it does not wrap. The first column is the label's min-content width as
/// `text::content_widths` measures it, well under 200; the second is 200.
/// Asserted as much as possible on facts that need no font metrics: the second
/// is exactly the stored width, and the first is narrower.
///
/// **Flip run**, `MinContent` and `MaxContent` swapped in both `min_breadth` and
/// `max_breadth`: fails on *"max-content is the preferred width"*, 40.765625
/// against 200 — predicted. (40.765625 is the widest word, which no assertion
/// spells: it is a font measurement.)
#[test]
fn min_content_and_max_content_tracks_size_by_their_text() {
    let text = "several short words";
    let kind = wrapping(text, 200.0);
    let (min, _) = ondin_core::text::content_widths(ondin_core::TextRef::of(&kind).expect("text"));
    assert!(
        min > 0.0 && min < 150.0,
        "the fixture: a widest word well under 200, {min}"
    );
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
    s.add(f, wrapping(text, 200.0), (0.0, 0.0));
    s.add(f, wrapping(text, 200.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_content: AlignContent::Start,
            ..grid(
                vec![
                    breadth(TrackBreadth::MinContent),
                    breadth(TrackBreadth::MaxContent),
                ],
                vec![],
            )
        })),
    );
    let g = ondin_core::build::laid_grid(&s.doc, f).expect("a grid");
    let width = |(a, b): (f64, f64)| b - a;
    let (narrow, wide) = (width(g.columns.spans[0]), width(g.columns.spans[1]));
    assert_eq!(wide, 200.0, "max-content is the preferred width");
    assert!(
        narrow < wide,
        "min-content narrower than max-content: {narrow} against {wide}"
    );
    assert!(
        (narrow - min).abs() <= 1.0 / 64.0,
        "min-content is the widest word: {narrow} against {min}"
    );
}

/// **A `fit-content` item follows its own alignment over the container's
/// `stretch`** — `justify-self` and `align-self` are `auto` only when unset, so
/// an item told `center` is centred whatever `justify-items` says (§15 D915's
/// `grid_held`: only a stretched `fit-content` is held at the start). A laid
/// group hugging a 10 × 10 rect, `fit-content` on both axes, `center` on both,
/// in one 100 × 50 cell of a grid told `stretch` on both: it is 10 × 10, at
/// (100 − 10) / 2 = 45 across and (50 − 10) / 2 = 20 down.
///
/// **Flip run**, `grid_held` reading `items.or(own)` for `own.or(items)`: fails on
/// *"centred in its cell on both axes"*, the group at (0, 0, 10, 10) — held at
/// the start — while the fixture's size passes — predicted.
#[test]
fn a_fit_content_item_follows_its_own_alignment_over_the_containers_stretch() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 50.0), (0.0, 0.0));
    let boxed = s.add(f, NodeKind::Group, (0.0, 0.0));
    let _inner = s.add(boxed, rect(10.0, 10.0), (0.0, 0.0));
    s.display(boxed, Some(Display::Flex(Flex::default())));
    s.item(boxed, |i| {
        i.width = Dimension::FitContent;
        i.height = Dimension::FitContent;
        i.justify_self = Some(AlignItems::Center);
        i.align_self = Some(AlignItems::Center);
    });
    s.display(
        f,
        Some(Display::Grid(Grid {
            justify_items: Some(AlignItems::Stretch),
            align_items: Some(AlignItems::Stretch),
            ..grid(vec![px(100.0), px(100.0)], vec![px(50.0)])
        })),
    );
    assert_eq!(
        s.res.used_frame(boxed),
        Some(Size::new(10.0, 10.0)),
        "the fixture: hugging, not stretched"
    );
    assert_eq!(
        s.bounds(boxed),
        Rect::new(45.0, 20.0, 55.0, 30.0),
        "centred in its cell on both axes"
    );
}

/// **What CSS refuses on columns it refuses on rows** (`OpError::BadLayout`,
/// §15 D914, D919) — `tests/grid.rs`' `values_css_refuses_are_refused` with every
/// bad template on `grid-template-rows`, and **`minmax(0px, -1fr)`** — a
/// negative *maximum* — on both axes. Each is refused by the operation, nothing
/// changes, and `Grid::is_valid`, the judge the Container card's CSS field asks,
/// says no as well.
///
/// The card validates a typed list as `Grid { columns: parsed, .. }` whichever
/// axis it is for, so for rows the operation is the only guard; until this test
/// nothing pinned it.
///
/// ⚠️ **The operation does not ask `Grid::is_valid`.** Since §15 D937 it asks
/// `Display::is_valid_over`, which checks each axis itself, so the two are
/// separate judges of the same rule and each is asserted here.
///
/// **Flip runs**, each predicted: `Grid::is_valid` without its rows half (no
/// `.chain(&self.rows)`, no rows cap) fails on *"Grid::is_valid on rows:
/// [Px(-1.0)]"*; `Display::is_valid_over` without its `axis(&g.rows, …)` fails on
/// *"rows [Px(-1.0)]: Ok(())"*, and nothing else in `ondin-core`'s suite fails
/// under it; `TrackSize::is_valid` without `max.is_valid()` fails on
/// *"Grid::is_valid on rows: [minmax(0px, -1fr)]"*.
#[test]
fn rows_are_refused_as_columns_are() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    s.add(f, rect(20.0, 20.0), (0.0, 0.0));
    let auto = TrackSize::Breadth(TrackBreadth::Auto);
    let negative_max = Track::Size(TrackSize::MinMax {
        min: TrackBreadth::Px(0.0),
        max: TrackBreadth::Fr(-1.0),
    });
    let bad_tracks = [
        vec![px(-1.0)],
        vec![Track::Size(TrackSize::MinMax {
            min: TrackBreadth::Fr(1.0),
            max: TrackBreadth::Fr(2.0),
        })],
        vec![negative_max.clone()],
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
        display: Some(Display::Grid(grid(vec![], vec![at_the_cap]))),
    }])
    .expect("exactly MAX_TEMPLATE_TRACKS rows is accepted");
    s.display(f, None);
    for t in bad_tracks {
        let rows = grid(vec![], t.clone());
        assert!(!rows.is_valid(), "Grid::is_valid on rows: {t:?}");
        let r = s.try_commit(vec![Operation::SetDisplay {
            id: f,
            display: Some(Display::Grid(rows)),
        }]);
        assert!(matches!(r, Err(OpError::BadLayout)), "rows {t:?}: {r:?}");
    }
    let columns = grid(vec![negative_max.clone()], vec![]);
    assert!(!columns.is_valid(), "Grid::is_valid on minmax(0px, -1fr)");
    let r = s.try_commit(vec![Operation::SetDisplay {
        id: f,
        display: Some(Display::Grid(columns)),
    }]);
    assert!(
        matches!(r, Err(OpError::BadLayout)),
        "columns minmax(0px, -1fr): {r:?}"
    );
    assert_eq!(s.doc.get(f).unwrap().display(), None, "nothing changed");
}

/// **A track after a capped repeat is not laid past the cap** (§15 D919,
/// `container::template`) — a file's `repeat(5000, 10px) 20px` lays exactly
/// `MAX_TEMPLATE_TRACKS` explicit columns: the repeat takes all the room and the
/// single track after it has none. `tests/grid.rs`' cap case ends on the repeat,
/// so the single-track arm's room check was reached by nothing.
///
/// **Flip runs** in `template`'s single-track arm: the `if room > 0` guard
/// dropped, as the review named it, panics in this debug build at its own
/// `room -= 1`, *"attempt to subtract with overflow"* — not the predicted 1001,
/// though a failure all the same; with the guard dropped and the decrement
/// saturating instead — the version that lays past the cap in any profile — it
/// fails on *"laid to the cap"*, 1001 against 1000, as predicted.
#[test]
fn a_track_after_a_capped_repeat_is_not_laid() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 100.0), (0.0, 0.0));
    s.add(f, rect(10.0, 10.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Grid(grid(
            vec![
                Track::Repeat {
                    repeat: 3,
                    tracks: vec![TrackSize::Breadth(TrackBreadth::Px(10.0))],
                },
                px(20.0),
            ],
            vec![],
        ))),
    );
    let g = ondin_core::build::laid_grid(&s.doc, f).expect("a grid");
    assert_eq!(g.columns.explicit, 4, "the fixture: three and one");
    let (doc, _res) = reopened(&s, r#""repeat": 3"#, r#""repeat": 5000"#);
    let g = ondin_core::build::laid_grid(&doc, f).expect("a grid");
    assert_eq!(
        usize::from(g.columns.explicit),
        ondin_core::container::MAX_TEMPLATE_TRACKS,
        "laid to the cap, the 20px track left out"
    );
}
