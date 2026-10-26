//! *Tidy up* (`build::tidy_moves`, `build::tidy_up`) over hand-drawn arrangements
//! and seeded random ones. Each result is checked against what tidying promises
//! rather than a stored answer: nothing overlaps, the top-left holds, order along a
//! row and down a column is kept, and tidying again moves nothing. The hand cases
//! then pin the grid each arrangement should come to.

use ondin_core::build::{tidy_moves, tidy_up};
use ondin_core::kurbo::{Affine, Rect, Size, Vec2};
use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

const EPS: f64 = 1e-6;

fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, x + w, y + h)
}

fn moved(boxes: &[Rect]) -> Vec<Rect> {
    boxes
        .iter()
        .zip(tidy_moves(boxes))
        .map(|(b, d)| *b + d)
        .collect()
}

/// A deterministic generator, so a failure names a seed that reproduces it.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

/// A `rows` × `cols` grid of `w` × `h` cards `gap` apart, each shaken by up to
/// `jitter`, with the cells `skip` names left empty.
fn grid(
    rows: usize,
    cols: usize,
    (w, h): (f64, f64),
    gap: f64,
    jitter: f64,
    rng: &mut Lcg,
    skip: &[(usize, usize)],
) -> Vec<Rect> {
    let mut out = Vec::new();
    for i in 0..rows {
        for j in 0..cols {
            if skip.contains(&(i, j)) {
                continue;
            }
            let x = 100.0 + j as f64 * (w + gap) + rng.range(-jitter, jitter);
            let y = 50.0 + i as f64 * (h + gap) + rng.range(-jitter, jitter);
            out.push(r(x, y, w, h));
        }
    }
    out
}

/// Every arrangement the properties run over: the hand cases' shapes, then
/// seeded random grids and scatters.
fn inputs() -> Vec<(String, Vec<Rect>)> {
    let mut rng = Lcg(7);
    let mut out = vec![
        (
            "one row".to_string(),
            vec![
                r(0.0, 0.0, 20.0, 20.0),
                r(30.0, 2.0, 20.0, 20.0),
                r(80.0, -1.0, 20.0, 20.0),
                r(120.0, 0.0, 20.0, 20.0),
            ],
        ),
        (
            "one column".to_string(),
            vec![
                r(0.0, 0.0, 40.0, 10.0),
                r(1.0, 25.0, 40.0, 10.0),
                r(-2.0, 37.0, 40.0, 10.0),
            ],
        ),
        (
            "mixed sizes".to_string(),
            vec![
                r(0.0, 0.0, 30.0, 20.0),
                r(40.0, -10.0, 10.0, 40.0),
                r(60.0, 5.0, 50.0, 10.0),
            ],
        ),
        (
            "overlapping".to_string(),
            vec![
                r(0.0, 0.0, 40.0, 40.0),
                r(25.0, 3.0, 40.0, 40.0),
                r(50.0, -2.0, 40.0, 40.0),
            ],
        ),
        (
            "bricks".to_string(),
            vec![
                r(0.0, 0.0, 40.0, 20.0),
                r(50.0, 0.0, 40.0, 20.0),
                r(25.0, 30.0, 40.0, 20.0),
                r(75.0, 30.0, 40.0, 20.0),
            ],
        ),
        (
            "stacked".to_string(),
            vec![
                r(0.0, 0.0, 30.0, 30.0),
                r(2.0, 2.0, 30.0, 30.0),
                r(4.0, 4.0, 30.0, 30.0),
            ],
        ),
        (
            "hairlines".to_string(),
            vec![
                r(0.0, 0.0, 0.0, 30.0),
                r(10.0, 0.0, 0.0, 30.0),
                r(25.0, 0.0, 0.0, 30.0),
            ],
        ),
    ];
    out.push((
        "tidy 3x4".into(),
        grid(3, 4, (60.0, 40.0), 16.0, 0.0, &mut rng, &[]),
    ));
    out.push((
        "holes".into(),
        grid(3, 3, (50.0, 50.0), 10.0, 3.0, &mut rng, &[(1, 1), (0, 2)]),
    ));
    for seed in 0..120u64 {
        let mut rng = Lcg(seed * 7919 + 1);
        let rows = 1 + (rng.next() * 5.0) as usize;
        let cols = 1 + (rng.next() * 6.0) as usize;
        if rows * cols < 2 {
            continue;
        }
        let w = rng.range(4.0, 120.0);
        let h = rng.range(4.0, 120.0);
        let gap = rng.range(0.0, 40.0);
        let jitter = rng.range(0.0, w.min(h) * 0.3);
        out.push((
            format!("grid seed {seed}"),
            grid(rows, cols, (w, h), gap, jitter, &mut rng, &[]),
        ));
    }
    for seed in 0..120u64 {
        let mut rng = Lcg(seed * 104_729 + 3);
        let n = 2 + (rng.next() * 18.0) as usize;
        let boxes = (0..n)
            .map(|_| {
                r(
                    rng.range(-300.0, 300.0),
                    rng.range(-300.0, 300.0),
                    rng.range(1.0, 90.0),
                    rng.range(1.0, 90.0),
                )
            })
            .collect();
        out.push((format!("scatter seed {seed}"), boxes));
    }
    out
}

/// Tidied, no two boxes overlap by more than float noise.
#[test]
fn a_tidied_set_has_no_overlap() {
    for (name, boxes) in inputs() {
        let after = moved(&boxes);
        for i in 0..after.len() {
            for j in i + 1..after.len() {
                let o = after[i].intersect(after[j]);
                assert!(
                    o.width() <= EPS || o.height() <= EPS,
                    "{name}: {i} {:?} and {j} {:?} overlap",
                    after[i],
                    after[j]
                );
            }
        }
    }
}

/// Tidying moves nothing a second time.
///
/// flip: `tidy_moves` measuring gaps box to box rather than band to band →
/// fails on *scatter seed 1*, whose narrow boxes sit inset in their columns.
#[test]
fn tidying_twice_is_tidying_once() {
    for (name, boxes) in inputs() {
        let once = moved(&boxes);
        for (k, d) in tidy_moves(&once).iter().enumerate() {
            assert!(
                d.hypot() < 1e-6,
                "{name}: box {k} moved {d:?} the second time"
            );
        }
    }
}

/// The union's top-left holds still.
#[test]
fn the_top_left_holds() {
    for (name, boxes) in inputs() {
        let corner = |bs: &[Rect]| {
            bs.iter().fold((f64::INFINITY, f64::INFINITY), |(x, y), b| {
                (x.min(b.x0), y.min(b.y0))
            })
        };
        let (before, after) = (corner(&boxes), corner(&moved(&boxes)));
        assert!(
            (before.0 - after.0).abs() < EPS && (before.1 - after.1).abs() < EPS,
            "{name}: {before:?} → {after:?}"
        );
    }
}

/// Two boxes that shared a row keep their left-to-right order, and two that shared
/// a column keep their top-to-bottom order. Shared means each centre lies inside
/// the other's band, which `tidy_bands` always groups.
#[test]
fn order_along_a_row_and_down_a_column_is_kept() {
    for (name, boxes) in inputs() {
        let after = moved(&boxes);
        for i in 0..boxes.len() {
            for j in 0..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                let same_row =
                    (a.center().y - b.center().y).abs() < a.height().min(b.height()) / 4.0;
                if same_row && b.center().x - a.center().x > a.width().max(b.width()) {
                    assert!(
                        after[j].center().x > after[i].center().x - EPS,
                        "{name}: {j} was right of {i} and is not"
                    );
                }
                let same_col = (a.center().x - b.center().x).abs() < a.width().min(b.width()) / 4.0;
                if same_col && b.center().y - a.center().y > a.height().max(b.height()) {
                    assert!(
                        after[j].center().y > after[i].center().y - EPS,
                        "{name}: {j} was below {i} and is not"
                    );
                }
            }
        }
    }
}

/// A shaken grid comes back as the exact grid: every gap the median, every card
/// in its own cell.
#[test]
fn a_shaken_grid_comes_back_square() {
    let mut rng = Lcg(42);
    let boxes = grid(3, 4, (60.0, 40.0), 16.0, 5.0, &mut rng, &[]);
    let after = moved(&boxes);
    let x0 = after.iter().map(|b| b.x0).fold(f64::INFINITY, f64::min);
    let y0 = after.iter().map(|b| b.y0).fold(f64::INFINITY, f64::min);
    let gx = after[1].x0 - after[0].x1;
    let gy = after[4].y0 - after[0].y1;
    for (k, b) in after.iter().enumerate() {
        let (i, j) = (k / 4, k % 4);
        let want = r(
            x0 + j as f64 * (60.0 + gx),
            y0 + i as f64 * (40.0 + gy),
            60.0,
            40.0,
        );
        assert!(
            (b.x0 - want.x0).abs() < EPS && (b.y0 - want.y0).abs() < EPS,
            "card {k}: {b:?}, want {want:?}"
        );
    }
    assert!(
        (gx - 16.0).abs() < 10.0 && (gy - 16.0).abs() < 10.0,
        "{gx} {gy}"
    );
}

/// A row with gaps of 10, 30 and 20 is evened to the median, 20, and its cards
/// share the top edge of the tallest.
///
/// flip: `median` answering the mean → fails on 10, 40, 20 (the mean is 23.3);
/// 10, 30, 20 has the same mean and median.
#[test]
fn a_row_is_evened_to_its_median_gap() {
    let row = |gaps: [f64; 3]| {
        let mut x = 0.0;
        let mut out = Vec::new();
        for g in std::iter::once(0.0).chain(gaps) {
            x += g;
            out.push(r(x, 0.0, 20.0, 20.0));
            x += 20.0;
        }
        out
    };
    for (gaps, want) in [([10.0, 30.0, 20.0], 20.0), ([10.0, 40.0, 20.0], 20.0)] {
        let after = moved(&row(gaps));
        for pair in after.windows(2) {
            let g = pair[1].x0 - pair[0].x1;
            assert!((g - want).abs() < EPS, "{gaps:?}: gap {g}, want {want}");
        }
    }
}

/// An empty cell stays empty: the grid's holes are not filled by reading order.
#[test]
fn a_hole_stays_a_hole() {
    let mut rng = Lcg(5);
    let boxes = grid(3, 3, (50.0, 50.0), 10.0, 3.0, &mut rng, &[(1, 1)]);
    let after = moved(&boxes);
    // Row 1 is boxes 3 and 4 (columns 0 and 2): the second stays in column 2.
    let col0 = after[0].x0;
    let pitch = after[1].x0 - after[0].x0;
    assert!(
        (after[4].x0 - (col0 + 2.0 * pitch)).abs() < EPS,
        "{:?} left column 2",
        after[4]
    );
}

/// Boxes of different sizes are centred in their row and column.
#[test]
fn a_smaller_box_is_centred_in_its_cell() {
    let after = moved(&[r(0.0, 0.0, 30.0, 40.0), r(40.0, 3.0, 30.0, 10.0)]);
    assert!(
        (after[1].center().y - after[0].center().y).abs() < EPS,
        "{after:?}"
    );
}

/// Overlapping cards in a row end up touching, in their order.
#[test]
fn overlapping_cards_are_pulled_apart_to_touch() {
    let after = moved(&[
        r(0.0, 0.0, 40.0, 40.0),
        r(25.0, 2.0, 40.0, 40.0),
        r(50.0, -1.0, 40.0, 40.0),
    ]);
    for pair in after.windows(2) {
        assert!((pair[1].x0 - pair[0].x1).abs() < EPS, "{after:?}");
    }
}

/// Bricks, whose second row sits between the first's columns, cannot keep their
/// cells, so each row is refilled from the left: two rows of two.
///
/// flip: `tidy_moves` keeping cells without the longest-row check → fails here,
/// as four columns with holes.
#[test]
fn bricks_are_refilled_in_reading_order() {
    let after = moved(&[
        r(0.0, 0.0, 40.0, 20.0),
        r(50.0, 0.0, 40.0, 20.0),
        r(25.0, 30.0, 40.0, 20.0),
        r(75.0, 30.0, 40.0, 20.0),
    ]);
    assert!((after[2].x0 - after[0].x0).abs() < EPS, "{after:?}");
    assert!((after[3].x0 - after[1].x0).abs() < EPS, "{after:?}");
}

/// One box, or none, never moves.
#[test]
fn fewer_than_two_boxes_move_nothing() {
    assert!(tidy_moves(&[]).is_empty());
    assert_eq!(tidy_moves(&[r(5.0, 5.0, 10.0, 10.0)]), vec![Vec2::ZERO]);
}

/// Through a document: layers under a moved and scaled group are placed by their
/// world boxes, and the transaction is translations only.
#[test]
fn tidy_up_moves_layers_by_their_world_boxes() {
    let mut ids = IdSource::new(0x7D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let group = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: group,
        parent: root,
        index: 0,
        kind: NodeKind::Group,
        transform: Some(Affine::translate((40.0, 10.0)) * Affine::scale(2.0)),
        name: None,
    }];
    let mut made = Vec::new();
    for (k, x) in [0.0, 17.0, 50.0].into_iter().enumerate() {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: group,
            index: k,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: None,
        });
        made.push(id);
    }
    doc.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&doc);
    let tx = tidy_up(&doc, &res, &made);
    assert!(
        tx.0.iter()
            .all(|op| matches!(op, Operation::SetTransform { .. }))
    );
    doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&doc);
    let b: Vec<Rect> = made
        .iter()
        .map(|id| res.world_bounds(*id).unwrap())
        .collect();
    // World gaps were 14 and 46 (scaled ×2); the median of two is 30.
    for pair in b.windows(2) {
        let g = pair[1].x0 - pair[0].x1;
        assert!((g - 30.0).abs() < 1e-6, "gap {g}: {b:?}");
    }
    assert!((b[0].width() - 20.0).abs() < 1e-6, "sizes are left alone");
}
