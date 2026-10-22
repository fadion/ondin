//! The Alt-hover measure — the distances between what is selected and what the
//! pointer is over, in either direction.
//!
//! Hold `Alt` with something selected and hover another layer, or a ruler guide,
//! and the app quotes the distances between the two on both axes.
//!
//! It reads both ways round: a selected layer hovering a guide and a selected
//! guide hovering a layer are the same question, so [`to_guide`] answers both and
//! its parameter is `box_`, not `selection`.
//!
//! This is `snap.rs`'s sibling, not part of it: snapping nudges a live gesture,
//! measuring changes nothing and draws a number. Nothing here has a tolerance,
//! because a measure is never "close enough". [`Measure`] has the same four
//! fields as [`crate::snap::Guide`] and reuses [`Axis`] with the same meaning —
//! the direction of the *line*, so a distance along x is an [`Axis::Horizontal`]
//! measure — and canvas.rs draws both alike.
//!
//! Pure geometry over `Rect`s: no document, no egui, no view. The overlay in
//! canvas.rs decides what to compare and draws what comes back.

use ondin_core::kurbo::Rect;

/// Which way a drawn line runs.
///
/// Named for the line, so a distance along x is `Horizontal` — the convention
/// `ondin_core::GuideAxis` uses: a designer names the line, not the coordinate
/// it pins.
///
/// It lives here, not in `snap.rs`, so the dependency runs one way: `snap.rs`
/// produces [`Measure`]s and reads this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    /// The axis a line of this one runs *across* — what a distance along it is
    /// drawn as, and back again.
    ///
    /// A vertical guide pins an x, so the distance to it is quoted by a horizontal
    /// line. Spelled once because [`to_guide`] and [`between_guides`] both need the
    /// flip, and a second hand-written `match` could disagree and draw a plausible
    /// line at ninety degrees to the truth.
    pub fn across(self) -> Axis {
        match self {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        }
    }
}

/// Below this, a measure is dropped rather than drawn.
///
/// Half the last digit [`label`] shows: below it the label would read `0`, and a
/// line labelled `0` is worse than no line.
const EPS: f64 = 0.005;

/// One distance the overlay quotes, in world space.
///
/// `from` <= `to` always, so [`Self::distance`] needs no `abs`. Which end belongs
/// to the selection is deliberately not recorded: a measure reads the same in
/// either direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// Which way the line runs — [`Axis::Horizontal`] for a distance along x.
    pub axis: Axis,
    /// The perpendicular coordinate the line sits on: its y when horizontal.
    pub at: f64,
    pub from: f64,
    pub to: f64,
}

impl Measure {
    fn new(axis: Axis, at: f64, a: f64, b: f64) -> Self {
        Self {
            axis,
            at,
            from: a.min(b),
            to: a.max(b),
        }
    }

    /// The number the label quotes. Non-negative by construction.
    pub fn distance(self) -> f64 {
        self.to - self.from
    }
}

/// A distance as the app quotes it: whole where it is whole, otherwise to two
/// decimals with trailing zeros trimmed.
///
/// Not `round()` like the size badge: a measure answers "exactly how far", and a
/// gap of 7.5 reported as `8` would be its only possible failure. Two decimals,
/// because more is past what any document means and the pill must stay narrow
/// enough for a small gap. [`EPS`] keeps the low end honest.
pub fn label(distance: f64) -> String {
    let s = format!("{distance:.2}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

/// The distances between the selection's box and the hovered layer's box.
///
/// Per axis, the two axes consulting each other only about where to draw:
///
/// - **Apart** — one measure, the gap between the two facing edges.
/// - **Overlapping** — up to two measures, one at each end: between the two near
///   edges and between the two far edges (a containing box's padding). A zero is
///   dropped, so an aligned edge says nothing rather than `0`.
///
/// So a layer inside a frame gets four numbers, a layer diagonally away two, and
/// a layer beside another the gap plus its edge offsets.
///
/// Flush counts as apart: the disjointness tests are `>=`, so touching boxes
/// (a snap's normal outcome) give a dropped zero gap, not an "overlap" whose
/// near-edge distance is a whole box width.
pub fn between(selection: Rect, target: Rect) -> Vec<Measure> {
    let (a, b) = (selection, target);
    let (xs, x_apart) = spans((a.min_x(), a.max_x()), (b.min_x(), b.max_x()));
    let (ys, y_apart) = spans((a.min_y(), a.max_y()), (b.min_y(), b.max_y()));

    // Where each line sits on the other axis. Where the boxes share that axis,
    // the middle of what they share, so the line meets real edges of both (and a
    // nested layer's runs through the inner one's middle).
    //
    // Where they share nothing, the two axes make opposite choices so the pair
    // meets in an L with no extension to draw: the horizontal sits on the
    // selection's near edge, the vertical on the target's.
    let line_y = if y_apart {
        facing(a.min_y(), a.max_y(), b.min_y())
    } else {
        shared_middle((a.min_y(), a.max_y()), (b.min_y(), b.max_y()))
    };
    let line_x = if x_apart {
        facing(b.min_x(), b.max_x(), a.min_x())
    } else {
        shared_middle((a.min_x(), a.max_x()), (b.min_x(), b.max_x()))
    };

    let mut out = Vec::new();
    out.extend(
        xs.into_iter()
            .map(|(f, t)| Measure::new(Axis::Horizontal, line_y, f, t)),
    );
    out.extend(
        ys.into_iter()
            .map(|(f, t)| Measure::new(Axis::Vertical, line_x, f, t)),
    );
    out.retain(|m| m.distance() > EPS);
    out
}

/// The distance between two **parallel** ruler guides, drawn at `at` on the axis
/// neither of them pins.
///
/// The only pairing where both subjects are infinite, so the geometry prefers no
/// position and the caller chooses `at`. `canvas::draw_measures` chooses the middle
/// of the canvas, not the pointer: hovering a vertical guide pins the pointer's x
/// but leaves its y sweeping, so the label would slide under the hand while the
/// number never changed.
///
/// `None` when the two are not parallel: perpendicular guides cross, so there is
/// no number and no line to draw it on. Two guides at the same coordinate come
/// back `None` through [`EPS`], so hovering a guide that is itself selected says
/// nothing.
pub fn between_guides(a: (Axis, f64), b: (Axis, f64), at: f64) -> Option<Measure> {
    if a.0 != b.0 {
        return None;
    }
    let m = Measure::new(a.0.across(), at, a.1, b.1);
    (m.distance() > EPS).then_some(m)
}

/// The distances between a **box** and a **ruler guide**.
///
/// Which of the two is selected is not this function's business: it serves a
/// selected layer hovering a guide and a selected guide hovering a layer, so the
/// two never disagree about a number.
///
/// `guide` is the guide's own axis — the direction of its line — so an
/// [`Axis::Vertical`] guide pins an x and is measured to with horizontal lines.
///
/// One axis only, since a guide is infinite along its own direction. Otherwise
/// [`between`]'s rule with the guide as a box of no width: apart gives the gap,
/// across the box gives both sides.
///
/// The boundary goes the other way from [`between`]'s, on purpose: a guide
/// exactly on an edge takes the across branch and reports the far edge's
/// distance, since a guide is a point and has no zero gap worth preferring.
pub fn to_guide(box_: Rect, guide: Axis, at: f64) -> Vec<Measure> {
    // The extent the guide is compared against, and where the line sits on the
    // axis the guide does not pin.
    let axis = guide.across();
    let (lo, hi, line_at) = match guide {
        Axis::Vertical => (box_.min_x(), box_.max_x(), box_.center().y),
        Axis::Horizontal => (box_.min_y(), box_.max_y(), box_.center().x),
    };
    let spans = if at > hi {
        vec![(hi, at)]
    } else if at < lo {
        vec![(at, lo)]
    } else {
        vec![(lo, at), (at, hi)]
    };
    let mut out: Vec<Measure> = spans
        .into_iter()
        .map(|(f, t)| Measure::new(axis, line_at, f, t))
        .collect();
    out.retain(|m| m.distance() > EPS);
    out
}

/// The **gap** between two boxes along one axis, or `None` if they overlap on it.
///
/// [`between`]'s gap alone, for the spacing snap, which has decided the boxes and
/// axis and wants one distance; `between` would add the perpendicular offsets.
///
/// Laid in the middle of the band the boxes share, so both ends land on a real
/// edge; `None` when they share none, a gap with no picture.
pub fn gap(a: Rect, b: Rect, axis: Axis) -> Option<Measure> {
    let (a_lo, a_hi, b_lo, b_hi, band_a, band_b) = match axis {
        Axis::Horizontal => (
            a.min_x(),
            a.max_x(),
            b.min_x(),
            b.max_x(),
            (a.min_y(), a.max_y()),
            (b.min_y(), b.max_y()),
        ),
        Axis::Vertical => (
            a.min_y(),
            a.max_y(),
            b.min_y(),
            b.max_y(),
            (a.min_x(), a.max_x()),
            (b.min_x(), b.max_x()),
        ),
    };
    // They must miss each other along the axis and share the perpendicular one.
    let span = if b_lo >= a_hi {
        (a_hi, b_lo)
    } else if a_lo >= b_hi {
        (b_hi, a_lo)
    } else {
        return None;
    };
    if band_a.0.max(band_b.0) >= band_a.1.min(band_b.1) {
        return None;
    }
    let m = Measure::new(axis, shared_middle(band_a, band_b), span.0, span.1);
    (m.distance() > EPS).then_some(m)
}

/// The spans worth quoting on one axis, and whether the two intervals are apart.
///
/// One or two `(from, to)` pairs — the gap when apart, the two end distances
/// when not. See [`between`] for why flush counts as apart.
fn spans(a: (f64, f64), b: (f64, f64)) -> (Vec<(f64, f64)>, bool) {
    if b.0 >= a.1 {
        (vec![(a.1, b.0)], true)
    } else if a.0 >= b.1 {
        (vec![(b.1, a.0)], true)
    } else {
        (vec![(b.0, a.0), (a.1, b.1)], false)
    }
}

/// The edge of `[lo, hi]` nearer to an interval that starts at `other_lo` and
/// does not overlap it.
fn facing(lo: f64, hi: f64, other_lo: f64) -> f64 {
    if other_lo >= hi { hi } else { lo }
}

/// The middle of what two overlapping intervals share.
fn shared_middle(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0.max(b.0) + a.1.min(b.1)) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the pill-width probe touches egui.
    use eframe::egui;

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    /// Every distance on one axis, sorted.
    fn along(ms: &[Measure], axis: Axis) -> Vec<f64> {
        let mut out: Vec<f64> = ms
            .iter()
            .filter(|m| m.axis == axis)
            .map(|m| m.distance())
            .collect();
        out.sort_by(f64::total_cmp);
        out
    }

    /// Two boxes side by side with their vertical ranges overlapping: the gap on
    /// x, and on y the two edge offsets — the tops are 10 apart, the bottoms 30.
    #[test]
    fn boxes_beside_each_other_report_the_gap_and_the_edge_offsets() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        let b = r(140.0, 10.0, 240.0, 130.0);
        let ms = between(a, b);
        assert_eq!(along(&ms, Axis::Horizontal), vec![40.0], "the x gap");
        assert_eq!(along(&ms, Axis::Vertical), vec![10.0, 30.0]);

        // The gap line runs edge to edge inside the shared band of y.
        let gap = ms.iter().find(|m| m.axis == Axis::Horizontal).unwrap();
        assert_eq!((gap.from, gap.to), (100.0, 140.0));
        assert_eq!(gap.at, 55.0, "the middle of the shared 10..100");
    }

    /// A layer inside a frame reports its four insets and nothing else.
    #[test]
    fn a_layer_inside_another_reports_its_four_insets() {
        let frame = r(0.0, 0.0, 400.0, 300.0);
        let inner = r(40.0, 60.0, 340.0, 260.0);
        let ms = between(inner, frame);
        assert_eq!(along(&ms, Axis::Horizontal), vec![40.0, 60.0]);
        assert_eq!(along(&ms, Axis::Vertical), vec![40.0, 60.0]);

        // Each pair runs through the middle of the inner box.
        for m in &ms {
            let want = match m.axis {
                Axis::Horizontal => inner.center().y,
                Axis::Vertical => inner.center().x,
            };
            assert_eq!(m.at, want, "{m:?}");
        }
    }

    /// Diagonally apart: one measure per axis, and the two meet in an L from one
    /// box's corner to the other's. Flipping either of `between`'s edge choices
    /// leaves a hole in the L.
    #[test]
    fn a_diagonal_pair_meets_at_the_corner_of_the_l() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        let b = r(200.0, 300.0, 260.0, 360.0);
        let ms = between(a, b);
        assert_eq!(along(&ms, Axis::Horizontal), vec![100.0]);
        assert_eq!(along(&ms, Axis::Vertical), vec![200.0]);

        let h = ms.iter().find(|m| m.axis == Axis::Horizontal).unwrap();
        let v = ms.iter().find(|m| m.axis == Axis::Vertical).unwrap();
        // One of the horizontal's ends and one of the vertical's are the same point.
        assert_eq!((h.at, h.from, h.to), (100.0, 100.0, 200.0));
        assert_eq!((v.at, v.from, v.to), (200.0, 100.0, 300.0));
        assert_eq!(
            (v.at, h.at),
            (h.to, v.from),
            "the two measures do not meet: {h:?} {v:?}"
        );
    }

    /// The same, up and to the left, exercising `facing`'s other arm.
    #[test]
    fn the_l_also_meets_going_the_other_way() {
        let a = r(200.0, 300.0, 260.0, 360.0);
        let b = r(0.0, 0.0, 100.0, 100.0);
        let ms = between(a, b);
        let h = ms.iter().find(|m| m.axis == Axis::Horizontal).unwrap();
        let v = ms.iter().find(|m| m.axis == Axis::Vertical).unwrap();
        assert_eq!((h.at, h.from, h.to), (300.0, 100.0, 200.0));
        assert_eq!((v.at, v.from, v.to), (100.0, 100.0, 300.0));
        assert_eq!((v.at, h.at), (h.from, v.to));
    }

    /// Flush is a gap of zero, not an overlap: the `>=` in `spans` keeps touching
    /// boxes from quoting a whole box width.
    #[test]
    fn touching_boxes_report_no_horizontal_distance_at_all() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        let b = r(100.0, 0.0, 200.0, 100.0);
        let ms = between(a, b);
        assert!(
            along(&ms, Axis::Horizontal).is_empty(),
            "a flush edge quoted something: {ms:?}"
        );
        // Aligned on y too, so the whole answer is nothing.
        assert!(ms.is_empty(), "{ms:?}");
    }

    /// A measure that would be labelled `0` is dropped instead.
    #[test]
    fn a_distance_that_would_be_labelled_zero_is_dropped() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        // 0.004 is under half the last digit `label` shows.
        let hair = between(a, r(100.004, 0.0, 200.0, 100.0));
        assert!(along(&hair, Axis::Horizontal).is_empty(), "{hair:?}");
        // 0.006 is over it and comes back.
        let real = between(a, r(100.006, 0.0, 200.0, 100.0));
        assert_eq!(along(&real, Axis::Horizontal).len(), 1, "{real:?}");
        assert_eq!(label(real[0].distance()), "0.01");
    }

    #[test]
    fn a_label_is_whole_where_it_is_whole() {
        assert_eq!(label(40.0), "40");
        assert_eq!(label(40.5), "40.5");
        assert_eq!(label(40.25), "40.25");
        assert_eq!(label(40.253), "40.25");
        assert_eq!(label(1200.0), "1200");
        // Not `round()`.
        assert_eq!(label(7.5), "7.5");
    }

    /// A vertical guide beside the box: one horizontal measure to the near edge,
    /// laid down the middle of the box.
    #[test]
    fn a_guide_beside_the_box_gives_one_distance() {
        let a = r(100.0, 200.0, 200.0, 300.0);
        let ms = to_guide(a, Axis::Vertical, 260.0);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].axis, Axis::Horizontal);
        assert_eq!((ms[0].from, ms[0].to), (200.0, 260.0));
        assert_eq!(ms[0].at, 250.0, "down the middle of the box");

        // And on the other side, from the other edge.
        let left = to_guide(a, Axis::Vertical, 60.0);
        assert_eq!((left[0].from, left[0].to), (60.0, 100.0));
    }

    /// A guide across the box gives both sides, not just the nearest edge.
    #[test]
    fn a_guide_across_the_box_gives_both_sides() {
        let a = r(100.0, 200.0, 200.0, 300.0);
        let ms = to_guide(a, Axis::Vertical, 130.0);
        assert_eq!(along(&ms, Axis::Horizontal), vec![30.0, 70.0]);
        assert!(along(&ms, Axis::Vertical).is_empty(), "one axis only");

        // A horizontal guide pins a y, so it is measured to with vertical lines.
        let h = to_guide(a, Axis::Horizontal, 220.0);
        assert_eq!(along(&h, Axis::Vertical), vec![20.0, 80.0]);
        assert_eq!(h[0].at, a.center().x);
    }

    /// A guide exactly on an edge says nothing on that side rather than `0`.
    #[test]
    fn a_guide_on_an_edge_quotes_only_the_far_side() {
        let a = r(100.0, 200.0, 200.0, 300.0);
        let ms = to_guide(a, Axis::Vertical, 100.0);
        assert_eq!(along(&ms, Axis::Horizontal), vec![100.0], "{ms:?}");
    }

    // --- two guides ---------------------------------------------------------

    /// Two parallel guides quote the distance between them, drawn across them:
    /// two vertical guides give a horizontal line (`Axis::across`).
    #[test]
    fn two_parallel_guides_are_measured_across_their_own_direction() {
        let m = between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 340.0), 42.0).unwrap();
        assert_eq!(
            m.axis,
            Axis::Horizontal,
            "a vertical pair needs a horizontal line"
        );
        assert_eq!((m.from, m.to), (100.0, 340.0));
        assert_eq!(m.distance(), 240.0);
        assert_eq!(m.at, 42.0, "the caller chooses where it sits");

        // The other orientation mirrors, and pair order is irrelevant.
        let h = between_guides((Axis::Horizontal, 340.0), (Axis::Horizontal, 100.0), 7.0).unwrap();
        assert_eq!(h.axis, Axis::Vertical);
        assert_eq!((h.from, h.to), (100.0, 340.0));
        assert_eq!(h.at, 7.0);
    }

    /// Perpendicular guides cross, so there is nothing to measure.
    #[test]
    fn perpendicular_guides_have_no_distance_between_them() {
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Horizontal, 340.0), 0.0).is_none());
        assert!(between_guides((Axis::Horizontal, 100.0), (Axis::Vertical, 340.0), 0.0).is_none());
    }

    /// Two guides at the same coordinate say nothing rather than `0` (`EPS`),
    /// which also silences hovering the selected guide itself.
    #[test]
    fn coincident_guides_say_nothing_rather_than_zero() {
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.0), 0.0).is_none());
        // A hair apart is still nothing; just over comes back.
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.004), 0.0).is_none());
        let m = between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.006), 0.0).unwrap();
        assert_eq!(label(m.distance()), "0.01");
    }

    /// `across` is an involution: flipping twice returns to the start.
    #[test]
    fn across_flips_and_flips_back() {
        for a in [Axis::Vertical, Axis::Horizontal] {
            assert_ne!(a.across(), a, "{a:?} did not flip");
            assert_eq!(a.across().across(), a);
        }
    }

    /// The label pill stays narrow enough to centre on its own line: a measure
    /// shorter than the pill is covered by its label and only the end ticks show,
    /// a fine trade only while that is a tight gap. At 9.5pt with `PAD`, `8` is
    /// about 14px, `40` about 20 (the table the two-digit assert refers to),
    /// `1200` about 30 and `40.25` about 34. Re-check if the badge font size or
    /// `PAD` moves.
    #[test]
    fn the_label_pill_is_narrow_enough_to_centre_on_its_own_line() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay out with.
        let _ = ctx.run_ui(Default::default(), |_| {});

        let width = |text: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(9.5),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            }) + 8.0 // `draw_measure`'s PAD, both sides
        };

        let two = width(&label(40.0));
        assert!(
            (18.0..=23.0).contains(&two),
            "a two-digit pill is {two}px, not the ~20 the table above records"
        );
        // The widest labels a whole and a fractional number produce.
        let four = width(&label(1200.0));
        let frac = width(&label(40.25));
        assert!(four <= 32.0, "a four-digit pill is {four}px");
        assert!(frac <= 36.0, "a fractional pill is {frac}px");
        // One digit is the tight case, carried by the ticks.
        let one = width(&label(8.0));
        assert!(one <= 15.0, "a one-digit pill is {one}px");
    }
}
