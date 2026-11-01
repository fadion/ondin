//! `build::arrangement` and its two writes, `respaced` and `reordered`: a loose
//! selection read as a row, a column or a grid with even gaps, re-gapped around a
//! held band and reordered through its slots.

use ondin_core::build::{self, Axis};
use ondin_core::kurbo::{Rect, Vec2};

/// Four 78 × 80 cards in a row at y 60, `gaps` apart, the first at x 40.
fn row(gaps: [f64; 3]) -> Vec<Rect> {
    let mut x = 40.0;
    let mut out = vec![Rect::new(x, 60.0, x + 78.0, 140.0)];
    for gap in gaps {
        x += 78.0 + gap;
        out.push(Rect::new(x, 60.0, x + 78.0, 140.0));
    }
    out
}

/// Six 76 × 70 tiles in two rows of three, column gaps 12 and row gaps 20.
fn grid() -> Vec<Rect> {
    let mut out = Vec::new();
    for r in 0..2 {
        for c in 0..3 {
            let (x, y) = (60.0 + c as f64 * 88.0, 40.0 + r as f64 * 90.0);
            out.push(Rect::new(x, y, x + 76.0, y + 70.0));
        }
    }
    out
}

/// An even row is one row of four columns with its gap; uneven gaps, or a lone
/// box, are no arrangement.
/// flip: `arrangement` without its `hi - lo <= EVEN_GAP` test → fails at "uneven".
#[test]
fn an_even_row_reads_and_an_uneven_one_does_not() {
    let arr = build::arrangement(&row([16.0, 16.0, 16.0])).expect("an even row");
    assert_eq!(arr.rows.len(), 1);
    assert_eq!(arr.cols.len(), 4);
    assert_eq!(arr.col_gap, Some(16.0));
    assert_eq!(arr.row_gap, None);
    assert!(
        build::arrangement(&row([16.0, 16.3, 16.0])).is_some(),
        "within half a unit"
    );
    assert!(
        build::arrangement(&row([10.0, 26.0, 14.0])).is_none(),
        "uneven"
    );
    assert!(build::arrangement(&row([16.0; 3])[..1]).is_none());
}

/// A tidy grid reads both gaps; overlapping boxes, sharing a cell, do not read.
#[test]
fn a_grid_reads_both_gaps() {
    let arr = build::arrangement(&grid()).expect("a grid");
    assert_eq!((arr.rows.len(), arr.cols.len()), (2, 3));
    assert_eq!(arr.col_gap, Some(12.0));
    assert_eq!(arr.row_gap, Some(20.0));
    assert_eq!(arr.reading_order(), vec![0, 1, 2, 3, 4, 5]);

    let stacked = [
        Rect::new(0.0, 0.0, 50.0, 50.0),
        Rect::new(5.0, 5.0, 55.0, 55.0),
    ];
    assert!(build::arrangement(&stacked).is_none());
}

/// Every gap of the axis takes the new value around the held box's band: the
/// third card stays, the cards either side move away from it.
/// flip: `respaced` packing forwards from the first band only → fails at "held".
#[test]
fn respacing_holds_the_anchor_and_moves_the_rest() {
    let boxes = row([16.0; 3]);
    let arr = build::arrangement(&boxes).unwrap();
    let moves = build::respaced(&arr, Axis::X, 2, 28.0);
    assert_eq!(moves[2], Vec2::ZERO, "held");
    assert_eq!(moves[3], Vec2::new(12.0, 0.0));
    assert_eq!(moves[1], Vec2::new(-12.0, 0.0));
    assert_eq!(moves[0], Vec2::new(-24.0, 0.0));

    // A grid's row gaps move whole rows; its columns stay.
    let arr = build::arrangement(&grid()).unwrap();
    let moves = build::respaced(&arr, Axis::Y, 0, 30.0);
    assert!(moves[..3].iter().all(|m| *m == Vec2::ZERO));
    assert!(moves[3..].iter().all(|m| *m == Vec2::new(0.0, 10.0)));
}

/// The first card taken past the second swaps with it, and the run keeps its
/// gaps however wide the cards; in a grid a box lands on its new slot's centre.
/// flip: `reordered` packing a row in its old order → fails at "slides into the first slot".
#[test]
fn reordering_slides_the_others_over() {
    let mut boxes = row([16.0; 3]);
    // A wider second card, so a swap has to repack rather than trade places.
    boxes[1].x1 += 20.0;
    for b in &mut boxes[2..] {
        *b = *b + Vec2::new(20.0, 0.0);
    }
    let arr = build::arrangement(&boxes).unwrap();
    let moves = build::reordered(&boxes, &arr, 0, 1);
    let after: Vec<Rect> = boxes.iter().zip(&moves).map(|(b, m)| *b + *m).collect();
    assert_eq!(
        after[1].x0, 40.0,
        "the second card slides into the first slot"
    );
    assert_eq!(
        after[0].x0,
        after[1].x1 + 16.0,
        "and the first follows it, 16 on"
    );
    assert_eq!(moves[2], Vec2::ZERO);
    assert_eq!(moves[3], Vec2::ZERO);

    let boxes = grid();
    let arr = build::arrangement(&boxes).unwrap();
    let moves = build::reordered(&boxes, &arr, 0, 4);
    assert_eq!(boxes[0].center() + moves[0], boxes[4].center());
    assert_eq!(
        boxes[1].center() + moves[1],
        boxes[0].center(),
        "into the first slot"
    );
    assert_eq!(moves[5], Vec2::ZERO);
}

/// On near-even gaps, a card let go in its own slot moves nothing, and a swap
/// keeps each slot's own gap, so whole units stay whole.
/// flip: `reordered` re-packing at the mean gap → fails at "whole units".
#[test]
fn a_reorder_on_near_even_gaps_keeps_whole_units() {
    let boxes = row([10.0, 10.5, 10.0]);
    let arr = build::arrangement(&boxes).expect("near even reads as a row");
    assert!(
        build::reordered(&boxes, &arr, 1, 1)
            .iter()
            .all(|m| *m == Vec2::ZERO),
        "its own slot"
    );
    // The first two cards sit on whole units; the mean gap is 10.17.
    let moves = build::reordered(&boxes, &arr, 0, 1);
    for (b, m) in boxes.iter().zip(&moves).take(2) {
        let x = (*b + *m).x0;
        assert_eq!(x, x.round(), "whole units: {x}");
    }
    assert_eq!(moves[2..], [Vec2::ZERO; 2], "the rest stay put");
}
