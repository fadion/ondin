//! The Alt-hover measure — the distances between what is selected and what the
//! pointer is over, in either direction.
//!
//! Hold `Alt` with something selected and hover another layer, or a ruler guide,
//! and the app quotes the distances between the two on both axes. It is the
//! answer to "how far apart are these", which until now could only be got by
//! reading two X fields and subtracting.
//!
//! **It reads both ways round, and nothing here knows which way.** A selected layer
//! hovering a guide and a selected *guide* hovering a layer are the same question,
//! so [`to_guide`] answers both and its parameter is called `box_` rather than
//! `selection` (§15 D201). The distance between two things does not depend on which
//! one was clicked first, and a module that took a side would be able to disagree
//! with itself about one number.
//!
//! **This is `snap.rs`'s sibling, not part of it.** The two are one-line
//! neighbours in what they compute — 1-D comparisons between the features of two
//! world rectangles — and completely different in what they are *for*. Snapping
//! nudges a live gesture and draws a line to explain the nudge; measuring changes
//! nothing at all and draws a *number*. Keeping them apart is what stops a
//! tolerance leaking into a measurement: nothing here has a tolerance, because a
//! measure is never "close enough".
//!
//! [`Measure`] therefore has the same four fields as [`crate::snap::Guide`], and
//! reuses [`Axis`] with the same meaning — the direction of the *line*, so a
//! distance along x is drawn as an [`Axis::Horizontal`] measure. That is not
//! duplication to be collapsed; it is the shape a drawn line in world space has,
//! and canvas.rs draws both with the same three lines of code as a result.
//!
//! Pure geometry over `Rect`s: no document, no egui, no view. The overlay in
//! canvas.rs decides *what* to compare and draws what comes back.

use ondin_core::kurbo::Rect;

/// Which way a drawn line runs.
///
/// **Named for the line, and a distance along x is therefore `Horizontal`.** The
/// same convention `ondin_core::GuideAxis` uses for a ruler guide, for the same
/// reason: a designer names the line, not the coordinate it pins.
///
/// It lives here rather than in `snap.rs`, where it started, because that is what
/// makes the two modules' dependency run one way. `snap.rs` produces [`Measure`]s
/// to explain a spacing snap, so it reads this module; if this module reached back
/// for `snap::Axis` the two would be mutually dependent over a two-variant enum.
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
    /// line; a horizontal guide pins a y and is quoted by a vertical one. Spelled
    /// once because [`to_guide`] and [`between_guides`] both need the flip and a
    /// second hand-written `match` is how the two come to disagree about which way
    /// round it goes — a mistake that draws a plausible line at ninety degrees to
    /// the truth.
    pub fn across(self) -> Axis {
        match self {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        }
    }
}

/// Below this, a measure is dropped rather than drawn.
///
/// **Half the last digit [`label`] shows**, which is the whole of the reason for
/// the number: at exactly this threshold the label would read `0`, and a red line
/// with a `0` on it is worse than no line — it claims two edges are flush when
/// they are a fraction apart, or states the obvious when they really are flush.
/// So the cut is placed where the label stops being able to tell the truth.
const EPS: f64 = 0.005;

/// One distance the overlay quotes, in world space.
///
/// `from` <= `to` always, so [`Self::distance`] needs no `abs` and the drawing
/// code cannot accidentally produce a backwards line. Which of the two ends
/// belongs to the selection is deliberately *not* recorded: a measure is a
/// statement about two edges and reads the same in either direction.
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
/// **Not `round()`, which is what the size badge does**, and the difference is
/// the point of the feature rather than an inconsistency. A size badge is a
/// summary of the thing you are looking at; a measure is the answer to "exactly
/// how far", asked by someone who has already decided the eye is not enough. A
/// gap of 7.5 reported as `8` would be this overlay's only possible failure.
///
/// Two decimals rather than every digit the `f64` holds: three is past what any
/// document means and the pill has to stay narrow enough to sit in a small gap.
/// [`EPS`] keeps the low end honest.
pub fn label(distance: f64) -> String {
    let s = format!("{distance:.2}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

/// The distances between the selection's box and the hovered layer's box.
///
/// **Per axis, and the two axes never consult each other about what to say** —
/// only about where to draw it. On each axis the two boxes are either apart or
/// they are not, and that is the whole rule:
///
/// - **Apart** — one measure, the *gap* between the two facing edges. This is
///   the number the feature exists for.
/// - **Overlapping** — up to two measures, one at each end: the distance between
///   the two near edges and between the two far edges. When one box contains the
///   other this is its padding, which is the other thing people open a measure
///   tool for; when they merely cross, it is still two true statements about two
///   named pairs of edges, and the line drawn between each pair says which.
///   Either can be dropped for being zero, which is what makes an aligned edge
///   quietly say nothing rather than say `0`.
///
/// So a layer inside a frame gets four numbers, a layer diagonally away gets two
/// — a horizontal and a vertical, which is what "on both x and y" means when
/// nothing overlaps — and a layer beside another gets the gap **plus whatever its
/// edges are offset by**, which is three when the two are not aligned and one when
/// they are. The axes are independent, so there is no case where the answer on one
/// suppresses the answer on the other.
///
/// **Flush counts as apart, not as overlapping.** The disjointness tests are
/// `>=`, so two touching boxes produce a gap of zero (dropped) rather than an
/// "overlap" whose near-edge distance is one box's entire width. That case is a
/// snap's normal outcome, so it is the one that had to be right.
pub fn between(selection: Rect, target: Rect) -> Vec<Measure> {
    let (a, b) = (selection, target);
    let (xs, x_apart) = spans((a.min_x(), a.max_x()), (b.min_x(), b.max_x()));
    let (ys, y_apart) = spans((a.min_y(), a.max_y()), (b.min_y(), b.max_y()));

    // Where each line sits on the *other* axis. Wherever the boxes share that
    // axis, the middle of what they share: the line then starts and ends on real
    // edges of both boxes, and for a nested layer it runs through the middle of
    // the inner one, which is the picture a padding measurement wants.
    //
    // Where they share nothing, the line has to leave one of the boxes, and the
    // two axes make **opposite** choices so that the pair meets: the horizontal
    // sits on the selection's near edge, the vertical on the target's. The
    // horizontal then starts on the selection's near corner, the vertical ends on
    // the target's, and they join at the third point of the L between them — no
    // dashed extension to draw, because each measure ends exactly where the other
    // begins.
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
/// The fourth and last pairing the overlay has (§15 D203), and the only one where
/// *both* subjects are infinite. That is what makes `at` a parameter here where
/// every other function in this module derives it: two boxes share a band and two
/// parallel lines share nothing, so there is no position the geometry can prefer and
/// the caller has to choose one. `canvas::draw_measures` chooses the middle of the
/// canvas, and the reason is worth keeping here because it is not obvious — the
/// pointer would be the natural choice and is the **worst** one available. To hover a
/// vertical guide you must be within a few points of it in x, so the coordinate you
/// are free to sweep, and necessarily *are* sweeping as you track along the line, is
/// exactly the y this needs. The label would slide under the hand while the number
/// never changed.
///
/// **`None` when the two are not parallel**, and that is a statement rather than a
/// guard: perpendicular guides *cross*, and the distance between two lines that meet
/// is zero everywhere along one axis and undefined along the other. There is nothing
/// to quote and no line to draw it on. Two guides at the same coordinate come back
/// `None` too, through [`EPS`], which is also what makes hovering a guide that is
/// itself selected say nothing.
pub fn between_guides(a: (Axis, f64), b: (Axis, f64), at: f64) -> Option<Measure> {
    if a.0 != b.0 {
        return None;
    }
    let m = Measure::new(a.0.across(), at, a.1, b.1);
    (m.distance() > EPS).then_some(m)
}

/// The distances between a **box** and a **ruler guide**.
///
/// **Which of the two is selected is not this function's business** (§15 D201), and
/// saying so is the whole reason the parameter is not called `selection`. It serves both
/// directions of the overlay: a selected layer hovering a guide, and a selected
/// *guide* hovering a layer. The distance between a box and a line does not depend
/// on which one the user clicked first, so one function answers both and the two can
/// never disagree about a number they both quote.
///
/// `guide` is the guide's own axis in the sense [`Axis`] uses everywhere — the
/// direction of its line — so a [`Axis::Vertical`] guide pins an x and is
/// measured to with horizontal lines.
///
/// One axis only, and that is not a simplification: a guide is infinite along its
/// own direction, so there is no second distance to quote. The rest is
/// [`between`]'s rule with the guide standing in for a box of no width — apart,
/// and you get the gap; across the box, and you get **both** sides, which is what
/// makes a guide down the middle of a layout worth hovering — and, the other way
/// round, what makes selecting that guide and hovering the layout worth doing.
///
/// **The boundary goes the other way from [`between`]'s**, and on purpose: a
/// guide lying exactly on an edge takes the *across* branch, so it reports the
/// distance to the far edge instead of a zero that gets dropped. The reason the
/// two differ is that a guide is a point rather than an interval. Two flush boxes
/// have a real gap of zero worth preferring over a pair of edge distances that are
/// whole box widths; a guide on an edge has no gap reading to protect, so the
/// branch that says something is the right one.
pub fn to_guide(box_: Rect, guide: Axis, at: f64) -> Vec<Measure> {
    // The extent the guide is compared against, and where the line sits on the axis
    // the guide does not pin. `across` owns the flip, so this and `between_guides`
    // cannot disagree about which way round it goes.
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
/// [`between`]'s single most useful answer, on its own and by name, because the
/// spacing snap needs exactly this and none of the rest: it has already decided
/// which two boxes and which axis, and what it wants back is the one distance to
/// draw. Going through `between` would hand it the perpendicular edge offsets as
/// well, which during a drag is three lines where the gesture means one.
///
/// Laid in the middle of the band the two boxes share, so both ends land on a real
/// edge — and `None` when they share none, because a "gap" between two boxes that
/// miss each other entirely is a number with no picture.
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
    // They have to miss each other along the axis to have a gap, and share the
    // perpendicular one for the gap to be drawable.
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
/// Returns an iterator of one or two `(from, to)` pairs — the gap when they are
/// apart, the two end-to-end distances when they are not. See [`between`] for the
/// rule and why flush counts as apart.
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
    // Only the pill-width probe touches egui; the geometry above is deliberately
    // free of it, which is why this is here rather than at the top of the module.
    use eframe::egui;

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    /// Everything on one axis, sorted, so a test can say what it means without
    /// caring which order the two axes were pushed in.
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

        // The gap line runs edge to edge, and sits inside the shared band of y —
        // so both of its ends land on a real edge of a real box.
        let gap = ms.iter().find(|m| m.axis == Axis::Horizontal).unwrap();
        assert_eq!((gap.from, gap.to), (100.0, 140.0));
        assert_eq!(gap.at, 55.0, "the middle of the shared 10..100");
    }

    /// A layer inside a frame reports its four insets, and nothing else: this is
    /// padding, the second thing a measure tool is opened for.
    #[test]
    fn a_layer_inside_another_reports_its_four_insets() {
        let frame = r(0.0, 0.0, 400.0, 300.0);
        let inner = r(40.0, 60.0, 340.0, 260.0);
        let ms = between(inner, frame);
        assert_eq!(along(&ms, Axis::Horizontal), vec![40.0, 60.0]);
        assert_eq!(along(&ms, Axis::Vertical), vec![40.0, 60.0]);

        // Each pair runs through the middle of the inner box, which is what makes
        // the four of them read as a cross rather than as four stray lines.
        for m in &ms {
            let want = match m.axis {
                Axis::Horizontal => inner.center().y,
                Axis::Vertical => inner.center().x,
            };
            assert_eq!(m.at, want, "{m:?}");
        }
    }

    /// Diagonally apart: one measure per axis, and the two **meet**, so the
    /// picture is an L from one box's corner to the other's rather than two lines
    /// floating near each other.
    ///
    /// The join is the whole reason `between` picks the selection's edge for one
    /// axis and the target's for the other. Flipping either choice leaves a
    /// gap-shaped hole in the L, which is invisible in the source and obvious on
    /// screen.
    #[test]
    fn a_diagonal_pair_meets_at_the_corner_of_the_l() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        let b = r(200.0, 300.0, 260.0, 360.0);
        let ms = between(a, b);
        assert_eq!(along(&ms, Axis::Horizontal), vec![100.0]);
        assert_eq!(along(&ms, Axis::Vertical), vec![200.0]);

        let h = ms.iter().find(|m| m.axis == Axis::Horizontal).unwrap();
        let v = ms.iter().find(|m| m.axis == Axis::Vertical).unwrap();
        // The horizontal sits on the selection's near edge and runs out to the
        // target's; the vertical sits on the target's near edge and runs back to
        // the selection's. So one of the horizontal's ends and one of the
        // vertical's are the same point.
        assert_eq!((h.at, h.from, h.to), (100.0, 100.0, 200.0));
        assert_eq!((v.at, v.from, v.to), (200.0, 100.0, 300.0));
        assert_eq!(
            (v.at, h.at),
            (h.to, v.from),
            "the two measures do not meet: {h:?} {v:?}"
        );
    }

    /// The same, up and to the left, because `facing` has two arms and only one
    /// of them is exercised by a box below and to the right.
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

    /// **Flush is a gap of zero, not an overlap.** The `>=` in `spans` is what
    /// makes this true, and it is the case a snap produces constantly: get it
    /// wrong and two touching boxes report the distance from one's left edge to
    /// the other's — i.e. a whole box width, quoted as if it were a gap.
    #[test]
    fn touching_boxes_report_no_horizontal_distance_at_all() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        let b = r(100.0, 0.0, 200.0, 100.0);
        let ms = between(a, b);
        assert!(
            along(&ms, Axis::Horizontal).is_empty(),
            "a flush edge quoted something: {ms:?}"
        );
        // Perfectly aligned on y as well, so both offsets there are zero too and
        // the whole answer is "nothing to say".
        assert!(ms.is_empty(), "{ms:?}");
    }

    /// A measure that would round to `0` is dropped instead. Without this the
    /// overlay's own label turns into a lie about a gap that exists.
    #[test]
    fn a_distance_that_would_be_labelled_zero_is_dropped() {
        let a = r(0.0, 0.0, 100.0, 100.0);
        // 0.004 apart on x — under half the last digit `label` shows.
        let hair = between(a, r(100.004, 0.0, 200.0, 100.0));
        assert!(along(&hair, Axis::Horizontal).is_empty(), "{hair:?}");
        // 0.006 is over it, and comes back rather than being rounded away.
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
        // Not `round()`: the one number this overlay must never report is a
        // rounded one.
        assert_eq!(label(7.5), "7.5");
    }

    /// A vertical guide beside the box: one horizontal measure to the near edge,
    /// laid down the middle of the box.
    ///
    /// "The box" rather than "the selection", because either of the two may be the
    /// selected one (§15 D201) — and the box in these fixtures is whichever one is
    /// not the guide.
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

    /// A guide **across** the box gives both sides — the reading that makes a centre
    /// guide worth hovering, and the one a single "distance to the nearest edge"
    /// would throw away. It is also the case to try by hand in the guide-selected
    /// direction, where nothing pins the routing (§15 D201).
    #[test]
    fn a_guide_across_the_box_gives_both_sides() {
        let a = r(100.0, 200.0, 200.0, 300.0);
        let ms = to_guide(a, Axis::Vertical, 130.0);
        assert_eq!(along(&ms, Axis::Horizontal), vec![30.0, 70.0]);
        assert!(along(&ms, Axis::Vertical).is_empty(), "one axis only");

        // A horizontal guide is the same statement turned a quarter: it pins a y,
        // so it is measured to with vertical lines.
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

    /// **A pair of parallel guides quotes the distance between them, drawn across
    /// them.** Two vertical guides pin xs, so the answer is a *horizontal* line —
    /// which is the whole of what `Axis::across` is for, and the flip that reads
    /// correctly in prose and inverts silently in code.
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

        // The other orientation is the mirror image, and the order of the pair does
        // not matter — `Measure` normalizes its ends.
        let h = between_guides((Axis::Horizontal, 340.0), (Axis::Horizontal, 100.0), 7.0).unwrap();
        assert_eq!(h.axis, Axis::Vertical);
        assert_eq!((h.from, h.to), (100.0, 340.0));
        assert_eq!(h.at, 7.0);
    }

    /// **Perpendicular guides cross, so there is nothing to measure** — a statement
    /// rather than a guard. The distance between two lines that meet is zero along
    /// one axis and undefined along the other, so there is no number and no line to
    /// draw it on.
    #[test]
    fn perpendicular_guides_have_no_distance_between_them() {
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Horizontal, 340.0), 0.0).is_none());
        assert!(between_guides((Axis::Horizontal, 100.0), (Axis::Vertical, 340.0), 0.0).is_none());
    }

    /// Two guides at the same coordinate say nothing rather than `0` — the same
    /// `EPS` rule as everywhere else, and here it is also what makes hovering a
    /// guide that is *itself* selected quietly draw nothing.
    #[test]
    fn coincident_guides_say_nothing_rather_than_zero() {
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.0), 0.0).is_none());
        // And a hair apart is still nothing, for the reason `label` cannot say it.
        assert!(between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.004), 0.0).is_none());
        // Just over, and it comes back.
        let m = between_guides((Axis::Vertical, 100.0), (Axis::Vertical, 100.006), 0.0).unwrap();
        assert_eq!(label(m.distance()), "0.01");
    }

    /// `across` is an involution, which is the property both its callers lean on:
    /// flipping twice is where you started, so a distance along x is drawn
    /// horizontally and a horizontal line measures along x.
    #[test]
    fn across_flips_and_flips_back() {
        for a in [Axis::Vertical, Axis::Horizontal] {
            assert_ne!(a.across(), a, "{a:?} did not flip");
            assert_eq!(a.across().across(), a);
        }
    }

    /// **How wide the pill actually is**, measured rather than guessed, because it
    /// decides whether centring the label on its own line is tolerable.
    ///
    /// `draw_measure` centres the pill on the middle of its line, as Figma does, so
    /// a measure shorter than the pill is covered by its own label and only the end
    /// ticks say where it starts and stops. That is a fine trade if "shorter than
    /// the pill" is a genuinely tight gap and a bad one if it is most of them, and
    /// the difference is a number rather than an opinion. Measured at 9.5pt with
    /// the app's own fonts installed, `PAD` included:
    ///
    /// | label    | pill  |
    /// |----------|-------|
    /// | `8`      | 13.9  |
    /// | `40`     | 20.1  |
    /// | `1200`   | 29.7  |
    /// | `40.25`  | 34.1  |
    ///
    /// So at 100% zoom a two-digit gap is about 20 units wide and its pill is about
    /// 20px: the crossover sits right at the point where a gap stops being an
    /// ordinary one, and every zoom above 100% pushes it further down. The guess
    /// this replaced was 26px — a third too wide, which would have put the
    /// crossover into gaps people measure all the time and argued for offsetting
    /// the label instead. That is why it is measured.
    ///
    /// Kept rather than deleted because the trade is what has to be re-checked if
    /// the badge font size or `PAD` ever moves.
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
        // The widest label a whole number can produce, and the widest anything can.
        let four = width(&label(1200.0));
        let frac = width(&label(40.25));
        assert!(four <= 32.0, "a four-digit pill is {four}px");
        assert!(frac <= 36.0, "a fractional pill is {frac}px");
        // One digit is the tight case, and it is the *ticks* that carry it: even
        // there the pill is under 15px, so a 10-unit gap at 100% zoom is covered by
        // roughly 2px either side rather than swamped.
        let one = width(&label(8.0));
        assert!(one <= 15.0, "a one-digit pill is {one}px");
    }
}
