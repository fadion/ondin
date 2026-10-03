//! Flex's model → taffy mapping, one value at a time, against boxes worked by
//! hand from CSS (the release review's `[X3.2-L6-01]`).
//!
//! `tests/flex.rs` pins that the engine is wired; `container`'s `flex_tests` pin
//! what it computes. When the review looked, neither placed an item under
//! `column-reverse`, `wrap`, `justify-content` past `start`, `align-content`
//! past `start`/`stretch`, `align-items: center`, a limit or a basis (a basis
//! fixture has since arrived with §15 D935's resize test) — and
//! `incremental_update_equals_rebuild_over_random_layout_ops` cannot see a wrong
//! arm, because `update` and `rebuild` both go through `style_of` (§15 D898's
//! own caveat). §15 D909 is a mapping error of exactly that shape which lived
//! through two build steps with every gate green. So every expected box here is
//! arithmetic on CSS, not a reading of the engine, and each row says its sum.
//!
//! The harness is `tests/flex.rs`' `Scene`: incremental `update` checked against
//! a full `rebuild` after every commit.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size, Vec2};
use ondin_core::{
    Document, GeometryPatch, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    container::{
        AlignContent, AlignItems, Dimension, Display, Flex, FlexDirection, FlexWrap,
        JustifyContent, LayoutItem,
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

    fn item(&mut self, id: NodeId, f: impl FnOnce(&mut LayoutItem)) {
        let mut item = *self.doc.get(id).unwrap().item();
        f(&mut item);
        self.commit(vec![Operation::SetLayoutItem { id, item }]);
    }

    fn bounds(&self, id: NodeId) -> Rect {
        self.res.world_bounds(id).expect("measured")
    }
}

/// The table's container: padding 20 and both gaps 10 — so a 400 × 200 frame
/// has the content box 20..380 × 20..180, 360 by 160 — items at the start across
/// the line so no stretch muddies the numbers, and `f` for the row's own values.
fn flex(f: impl FnOnce(&mut Flex)) -> Flex {
    let mut flex = Flex {
        column_gap: 10.0,
        row_gap: 10.0,
        padding: [20.0; 4],
        align_items: AlignItems::Start,
        ..Default::default()
    };
    f(&mut flex);
    flex
}

/// One row: its name and sum, the container, each item's size and item
/// properties, and each item's box.
struct Row {
    what: &'static str,
    flex: Flex,
    items: Vec<(Size, LayoutItem)>,
    boxes: Vec<Rect>,
}

/// `n` items of the given sizes at CSS's defaults.
fn plain(sizes: &[(f64, f64)]) -> Vec<(Size, LayoutItem)> {
    sizes
        .iter()
        .map(|(w, h)| (Size::new(*w, *h), LayoutItem::default()))
        .collect()
}

/// **Every flex value lays its items where CSS puts them** — one row per arm of
/// `container::style_of`'s flex mapping and `content_alignment`, each in a fresh
/// 400 × 200 frame (content box 20..380 × 20..180, gaps 10), each expected box
/// worked by hand in the row's name. Every row is laid and compared before the
/// test fails, so a wrong arm names every row it moves, not just the first.
///
/// The main-axis rows hold `a` 40 × 30 and `b` 70 × 60: 40 + 10 + 70 = 120 of
/// 360, so 240 is free. The wrapping rows hold `a` 200 × 30 and `b` 200 × 60 —
/// 410 does not fit 360, so `b` takes a second line — and the two lines are
/// 30 + 10 + 60 = 100 of 160, so 60 is free across.
///
/// `Start` and `End` are `flex-start` and `flex-end` (§15 D909) and follow the
/// reversals: under `row-reverse` `End` packs left, under `column-reverse` it
/// packs to the top, and under `wrap-reverse` `align-content: end` puts the lines
/// at the top. A plain row or column cannot tell `flex-end` from CSS's physical
/// `end`, which is why the reversed rows are here.
///
/// **Flip runs**, each one alone in `container.rs`, reverted with the inverse
/// edit:
///
/// - `style_of`'s `ColumnReverse` sent as taffy's `Column`: fails on both
///   `column-reverse` rows — *"column-reverse, justify start"*, `a` at y 20..50
///   against 150..180 — the predicted site; the reorder test below fails too, on
///   its fixture.
/// - `JustifyContent::End` sent as taffy's `END` (§15 D909's mapping before its
///   fix): fails on *"row-reverse, justify end"*, `a` at x 340..380 against
///   100..140, and on *"column-reverse, justify end"*, `a` at y 150..180 against
///   90..120 — predicted. The plain row's and column's `end` rows pass, as
///   predicted: they cannot tell the two apart.
/// - `SpaceAround` sent as `SPACE_EVENLY`: fails on *"row, space-around"*, `a` at
///   x 100 against 80 — predicted.
/// - `content_alignment`'s `End` sent as `END`: fails on *"wrap-reverse,
///   align-content end"* alone, `a` at y 150..180 against 90..120 — predicted.
/// - `min_size` and `max_size` swapped: fails on *"max-width against grow"*, `a`
///   at x 20..185 against 20..120, and on *"min-width against shrink"*, `b` 0
///   wide at 280 against 280..380 — predicted. `a` is 250 there either way (its
///   max is then 250), which is why `b` carries an explicit `min-width: 0`.
/// - `flex_basis` sent as `auto()`: fails on *"flex-basis over the stored
///   size"*, `a` at x 20..60 against 20..120 — predicted.
#[test]
fn every_flex_value_lays_its_items_where_css_puts_them() {
    let main = || plain(&[(40.0, 30.0), (70.0, 60.0)]);
    let lines = || plain(&[(200.0, 30.0), (200.0, 60.0)]);
    let r = Rect::new;
    let rows = vec![
        Row {
            what: "row, justify start: a at 20, b at 20 + 40 + 10 = 70",
            flex: flex(|_| {}),
            items: main(),
            boxes: vec![r(20.0, 20.0, 60.0, 50.0), r(70.0, 20.0, 140.0, 80.0)],
        },
        Row {
            what: "row, justify end: b ends at 380, a at 380 - 70 - 10 - 40 = 260",
            flex: flex(|f| f.justify_content = JustifyContent::End),
            items: main(),
            boxes: vec![r(260.0, 20.0, 300.0, 50.0), r(310.0, 20.0, 380.0, 80.0)],
        },
        Row {
            what: "row, justify center: 240 / 2 = 120 before a, so a at 140, b at 190",
            flex: flex(|f| f.justify_content = JustifyContent::Center),
            items: main(),
            boxes: vec![r(140.0, 20.0, 180.0, 50.0), r(190.0, 20.0, 260.0, 80.0)],
        },
        Row {
            what: "row, space-between: a at 20, b against 380",
            flex: flex(|f| f.justify_content = JustifyContent::SpaceBetween),
            items: main(),
            boxes: vec![r(20.0, 20.0, 60.0, 50.0), r(310.0, 20.0, 380.0, 80.0)],
        },
        Row {
            what: "row, space-around: 240 / 4 = 60 each side, a at 80, b at 120 + 60 + 10 + 60 = 250",
            flex: flex(|f| f.justify_content = JustifyContent::SpaceAround),
            items: main(),
            boxes: vec![r(80.0, 20.0, 120.0, 50.0), r(250.0, 20.0, 320.0, 80.0)],
        },
        Row {
            what: "row, space-evenly: 240 / 3 = 80 each, a at 100, b at 140 + 80 + 10 = 230",
            flex: flex(|f| f.justify_content = JustifyContent::SpaceEvenly),
            items: main(),
            boxes: vec![r(100.0, 20.0, 140.0, 50.0), r(230.0, 20.0, 300.0, 80.0)],
        },
        Row {
            what: "row-reverse, justify end: flex-end is the left, b at 20, a at 20 + 70 + 10 = 100",
            flex: flex(|f| {
                f.direction = FlexDirection::RowReverse;
                f.justify_content = JustifyContent::End;
            }),
            items: main(),
            boxes: vec![r(100.0, 20.0, 140.0, 50.0), r(20.0, 20.0, 90.0, 80.0)],
        },
        Row {
            what: "column, justify end: 30 + 10 + 60 = 100 of 160, so a at 80, b at 120",
            flex: flex(|f| {
                f.direction = FlexDirection::Column;
                f.justify_content = JustifyContent::End;
            }),
            items: main(),
            boxes: vec![r(20.0, 80.0, 60.0, 110.0), r(20.0, 120.0, 90.0, 180.0)],
        },
        Row {
            what: "column-reverse, justify start: bottom-up, a ends at 180, b at 150 - 10 - 60 = 80",
            flex: flex(|f| f.direction = FlexDirection::ColumnReverse),
            items: main(),
            boxes: vec![r(20.0, 150.0, 60.0, 180.0), r(20.0, 80.0, 90.0, 140.0)],
        },
        Row {
            what: "column-reverse, justify end: flex-end is the top, b at 20, a at 20 + 60 + 10 = 90",
            flex: flex(|f| {
                f.direction = FlexDirection::ColumnReverse;
                f.justify_content = JustifyContent::End;
            }),
            items: main(),
            boxes: vec![r(20.0, 90.0, 60.0, 120.0), r(20.0, 20.0, 90.0, 80.0)],
        },
        Row {
            what: "row, align-items center: one line 160 tall, a at 20 + (160 - 30) / 2 = 85, b at 20 + 50 = 70",
            flex: flex(|f| f.align_items = AlignItems::Center),
            items: main(),
            boxes: vec![r(20.0, 85.0, 60.0, 115.0), r(70.0, 70.0, 140.0, 130.0)],
        },
        Row {
            what: "row, align-items end: a at 180 - 30 = 150, b at 120",
            flex: flex(|f| f.align_items = AlignItems::End),
            items: main(),
            boxes: vec![r(20.0, 150.0, 60.0, 180.0), r(70.0, 120.0, 140.0, 180.0)],
        },
        Row {
            what: "wrap, align-content start: b wraps under a, at 20 + 30 + 10 = 60",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::Start;
            }),
            items: lines(),
            boxes: vec![r(20.0, 20.0, 220.0, 50.0), r(20.0, 60.0, 220.0, 120.0)],
        },
        Row {
            what: "wrap, align-content end: 60 free above, a at 80, b at 120",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::End;
            }),
            items: lines(),
            boxes: vec![r(20.0, 80.0, 220.0, 110.0), r(20.0, 120.0, 220.0, 180.0)],
        },
        Row {
            what: "wrap, align-content center: 30 above, a at 50, b at 90",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::Center;
            }),
            items: lines(),
            boxes: vec![r(20.0, 50.0, 220.0, 80.0), r(20.0, 90.0, 220.0, 150.0)],
        },
        Row {
            what: "wrap, align-content space-between: a at 20, b against 180",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::SpaceBetween;
            }),
            items: lines(),
            boxes: vec![r(20.0, 20.0, 220.0, 50.0), r(20.0, 120.0, 220.0, 180.0)],
        },
        Row {
            what: "wrap, align-content space-around: 60 / 4 = 15 each side, a at 35, b at 65 + 15 + 10 + 15 = 105",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::SpaceAround;
            }),
            items: lines(),
            boxes: vec![r(20.0, 35.0, 220.0, 65.0), r(20.0, 105.0, 220.0, 165.0)],
        },
        Row {
            what: "wrap, align-content space-evenly: 60 / 3 = 20 each, a at 40, b at 70 + 20 + 10 = 100",
            flex: flex(|f| {
                f.wrap = FlexWrap::Wrap;
                f.align_content = AlignContent::SpaceEvenly;
            }),
            items: lines(),
            boxes: vec![r(20.0, 40.0, 220.0, 70.0), r(20.0, 100.0, 220.0, 160.0)],
        },
        Row {
            what: "wrap-reverse, align-content end: flex-end is the top, b's line at 20, a's at 20 + 60 + 10 = 90",
            flex: flex(|f| {
                f.wrap = FlexWrap::WrapReverse;
                f.align_content = AlignContent::End;
            }),
            items: lines(),
            boxes: vec![r(20.0, 90.0, 220.0, 120.0), r(20.0, 20.0, 220.0, 80.0)],
        },
        Row {
            what: "max-width against grow: 250 free, 125 each, a capped at 100, b takes 350 - 100 = 250",
            flex: flex(|_| {}),
            items: vec![
                (
                    Size::new(40.0, 30.0),
                    LayoutItem {
                        grow: 1.0,
                        max_width: Dimension::Px(100.0),
                        ..Default::default()
                    },
                ),
                (
                    Size::new(60.0, 30.0),
                    LayoutItem {
                        grow: 1.0,
                        ..Default::default()
                    },
                ),
            ],
            boxes: vec![r(20.0, 20.0, 120.0, 50.0), r(130.0, 20.0, 380.0, 50.0)],
        },
        Row {
            what: "min-width against shrink: 150 over, a would shrink to 210 and is held at 250, b takes 350 - 250 = 100",
            flex: flex(|_| {}),
            items: vec![
                (
                    Size::new(300.0, 30.0),
                    LayoutItem {
                        min_width: Dimension::Px(250.0),
                        ..Default::default()
                    },
                ),
                (
                    Size::new(200.0, 30.0),
                    LayoutItem {
                        min_width: Dimension::Px(0.0),
                        ..Default::default()
                    },
                ),
            ],
            boxes: vec![r(20.0, 20.0, 270.0, 50.0), r(280.0, 20.0, 380.0, 50.0)],
        },
        Row {
            what: "flex-basis over the stored size: a 100 wide from a 40-wide rect, b at 130",
            flex: flex(|_| {}),
            items: vec![
                (
                    Size::new(40.0, 30.0),
                    LayoutItem {
                        basis: Dimension::Px(100.0),
                        ..Default::default()
                    },
                ),
                (Size::new(60.0, 30.0), LayoutItem::default()),
            ],
            boxes: vec![r(20.0, 20.0, 120.0, 50.0), r(130.0, 20.0, 190.0, 50.0)],
        },
    ];
    let mut wrong = Vec::new();
    for row in rows {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
        let ids: Vec<NodeId> = row
            .items
            .iter()
            .map(|(size, item)| {
                let id = s.add(f, rect(size.width, size.height), (300.0, 150.0));
                if !item.is_default() {
                    s.item(id, |i| *i = *item);
                }
                id
            })
            .collect();
        s.display(f, Some(Display::Flex(row.flex)));
        for (k, (id, want)) in ids.iter().zip(&row.boxes).enumerate() {
            let got = s.bounds(*id);
            if got != *want {
                wrong.push(format!(
                    "{} — item {}: {got:?} against {want:?}",
                    row.what,
                    ["a", "b"][k]
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **A released stretch keeps the edge the user held under `row-reverse` and
/// `column-reverse` too** (§15 D905 under D909) — `tests/flex.rs`'
/// `a_released_stretch_keeps_the_held_edge_on_every_axis` with the *main*-axis
/// reversals it did not cross. A main-axis reversal does not move the cross axis:
/// `row-reverse`'s cross-start is still the top and `column-reverse`'s the left,
/// so a stretched item dragged by its top or left aligns to `flex-end` exactly
/// as it does in a plain row or column, and by its bottom or right to
/// `flex-start`. Only `wrap-reverse` swaps the two, which the other test pins.
///
/// **Flip run**, `build::resized_from_cross_start` swapping the edge under a
/// main-axis reversal as well as under `wrap-reverse`
/// (`(wrap == WrapReverse) != main_reversed`): fails on the first case,
/// *"row-reverse, top"*, `Start` against `End` — predicted.
#[test]
fn a_released_stretch_keeps_the_held_edge_under_a_main_axis_reversal() {
    // (what, direction, dragged the top/left edge, the keyword, the box's cross
    // extent afterwards)
    let cases = [
        (
            "row-reverse, top",
            FlexDirection::RowReverse,
            true,
            AlignItems::End,
            (110.0, 180.0),
        ),
        (
            "row-reverse, bottom",
            FlexDirection::RowReverse,
            false,
            AlignItems::Start,
            (20.0, 90.0),
        ),
        (
            "column-reverse, left",
            FlexDirection::ColumnReverse,
            true,
            AlignItems::End,
            (280.0, 380.0),
        ),
        (
            "column-reverse, right",
            FlexDirection::ColumnReverse,
            false,
            AlignItems::Start,
            (20.0, 120.0),
        ),
    ];
    for (what, direction, from_start, keyword, extent) in cases {
        let mut s = Scene::new();
        let f = s.add(s.root, frame(400.0, 200.0), (0.0, 0.0));
        let a = s.add(f, rect(40.0, 30.0), (0.0, 0.0));
        s.display(
            f,
            Some(Display::Flex(Flex {
                direction,
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

/// **`column-reverse` reorders a dragged item reading the column bottom-up**
/// (`build::flex_reorder`'s `flow_index`, §15 D877) — the column twin of
/// `tests/flex.rs`' `row-reverse` case. A 200 × 400 frame, padding 20, gap 10,
/// holding `a` 40 × 40, `b` 40 × 60 and `c` 40 × 20, laid from the bottom: `a`
/// at 340..380 (centre 360), `b` at 270..330 (300), `c` at 240..260 (250).
///
/// Dragged up by 130, `a`'s centre is at 230 — above both — so it goes to the
/// end of the flow; `c` dragged down by 130 is at 380, below both, and goes
/// first; `a` nudged up by 20 is at 340, short of `b`'s centre, and stays.
/// Committed, `a` is laid at the top: `b` 320..380, `c` 290..310, `a` 240..280.
///
/// **Flip runs**: `build::flow_index`'s `reversed` without `ColumnReverse` fails
/// on *"dragged above both"*, `None` against `Some(2)` — predicted; `style_of`'s
/// `ColumnReverse` sent as taffy's `Column` fails earlier, on the fixture's
/// *"laid bottom-up"*.
#[test]
fn column_reverse_reorders_by_where_the_centre_falls_bottom_up() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 400.0), (0.0, 0.0));
    let a = s.add(f, rect(40.0, 40.0), (0.0, 0.0));
    let b = s.add(f, rect(40.0, 60.0), (0.0, 0.0));
    let c = s.add(f, rect(40.0, 20.0), (0.0, 0.0));
    s.display(
        f,
        Some(Display::Flex(flex(|f| {
            f.direction = FlexDirection::ColumnReverse;
        }))),
    );
    assert_eq!(
        [a, b, c].map(|id| (s.bounds(id).y0, s.bounds(id).y1)),
        [(340.0, 380.0), (270.0, 330.0), (240.0, 260.0)],
        "the fixture: laid bottom-up"
    );

    let reorder = |s: &Scene, id, dy| match ondin_core::build::flex_reorder(
        &s.doc,
        &s.res,
        id,
        Vec2::new(0.0, dy),
    ) {
        Some(Operation::Reorder { index, .. }) => Some(index),
        None => None,
        Some(other) => panic!("not a reorder: {other:?}"),
    };
    assert_eq!(
        reorder(&s, a, -130.0),
        Some(2),
        "dragged above both: the end of the flow, which is the top"
    );
    assert_eq!(
        reorder(&s, c, 130.0),
        Some(0),
        "dragged below both: the start of the flow, which is the bottom"
    );
    assert_eq!(
        reorder(&s, a, -20.0),
        None,
        "short of b's centre: no change"
    );

    s.commit(vec![Operation::Reorder { id: a, index: 2 }]);
    assert_eq!(
        [b, c, a].map(|id| (s.bounds(id).y0, s.bounds(id).y1)),
        [(320.0, 380.0), (290.0, 310.0), (240.0, 280.0)],
        "committed: a laid at the top"
    );
}
