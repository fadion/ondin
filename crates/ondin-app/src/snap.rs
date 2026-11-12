//! Snapping, and the lines and numbers that explain it.
//!
//! While a move or draw gesture is live, the dragged box's edges and centres
//! are compared against the same features of nearby nodes, and against the
//! ruler guides on the page; if one lands within a few screen pixels, the
//! gesture is nudged onto it and a line is drawn.
//!
//! An *alignment* puts a feature on a coordinate and is explained by a bare
//! [`Guide`]; a *spacing* snap ([`spacing_candidates`]) equalizes the gaps either
//! side of the box and is explained by [`Measure`]s. Both arrive in [`Marks`]. They
//! compete on one scale: the nearer claims the axis, alignment taking ties.
//!
//! [`Guide`] here is a transient line drawn for one frame; `ondin_core::Guide` is
//! a persistent ruler guide. This module snaps to the latter and draws the former.
//!
//! The tolerance is in screen pixels, so snapping is equally sticky at every zoom.
//! Pure geometry over `Resolved` bounds: no document mutation, no egui.

use crate::measure::{self, Axis, Measure};
use ondin_core::kurbo::{
    ParamCurve, ParamCurveDeriv, ParamCurveNearest, PathSeg, Point, Rect, Vec2,
};
use ondin_core::{Document, NodeId, Resolved};

/// How close (in screen pixels) an edge must come before it snaps.
pub(crate) const TOLERANCE_PX: f64 = 6.0;

/// Round to the pixel grid, whose pitch is **one world unit**.
///
/// A world unit is a pixel at 100% zoom; this function and the grid in `grid.rs`
/// must round and draw the same lattice, or the app shows one grid and honours
/// another.
fn to_pixel(value: f64) -> f64 {
    value.round()
}

/// The delta that puts `value` on the pixel grid.
///
/// The fallback under every snap, never a competitor: an alignment or guide match
/// on an axis wins outright, because rounding afterwards would break it. Skipped
/// on an axis a Shift-constrained gesture has locked.
fn pixel_delta(value: f64) -> f64 {
    to_pixel(value) - value
}

/// What a gesture is allowed to snap to — the top bar's **Snap** menu toggles,
/// resolved into the sources they admit.
///
/// One struct so no call site honours some toggles and forgets one. A source that
/// is switched off arrives **empty**, so it cannot win and needs no `if`.
pub struct Snapping {
    /// Node bounds. Empty when *Snap to shapes* is off.
    pub targets: Vec<Rect>,
    /// Ruler guides. Empty when *Snap to guides* is off.
    pub guides: GuideLines,
    /// Other outlines' anchors and segments, for a dragged **point** to land on
    /// ([`snap_point`]). Empty when *Snap to shapes* is off, and for every gesture
    /// that moves a box.
    pub outlines: Outlines,
    /// Text baselines in world space, as bare ys. Empty when *Snap to baselines*
    /// is off, or when nothing in view is a text node.
    ///
    /// Its own source rather than more `guides.y`: a ruler guide is already a
    /// drawn line and a baseline is not, so only a baseline match wants a mark
    /// (see [`Matched`]).
    pub baselines: Vec<f64>,
    /// Whether an axis nothing else claimed falls to the whole-pixel lattice —
    /// *Snap to grid*. A flag rather than an empty list, because the lattice is
    /// everywhere.
    pub pixel: bool,
    /// Logical points per world unit (the pointer is measured in points), which
    /// turns the screen-space tolerance into world units. A zero zoom would make
    /// the tolerance infinite, so there is no `Default`.
    pub zoom: f64,
}

impl Snapping {
    /// Snapping that consults nothing — the base a test adds one source to.
    /// `#[cfg(test)]` so it does not pose as production API.
    #[cfg(test)]
    pub fn none(zoom: f64) -> Self {
        Self {
            targets: Vec::new(),
            outlines: Outlines::default(),
            guides: GuideLines::default(),
            baselines: Vec::new(),
            pixel: false,
            zoom,
        }
    }
}

/// The geometry a dragged point can land on, in world space.
#[derive(Clone, Debug, Default)]
pub struct Outlines {
    /// Every anchor in view: a target, and an x and a y a point can line up with.
    pub anchors: Vec<Point>,
    /// The segments near the pointer, whose crossings, midpoints and nearest points
    /// are targets. Only those near it: everything else is out of reach.
    pub segments: Vec<PathSeg>,
    /// The centres of the shapes in view.
    pub centres: Vec<Point>,
    /// The anchors the dragged point is joined to by a segment — the pen run's
    /// previous anchor, a line's other end, a dragged anchor's neighbours — which
    /// tangent and perpendicular snaps are drawn from. Empty for a first anchor.
    pub from: Vec<Point>,
}

/// What a point snap landed on, in [`PointKind::label`]'s order of priority: a
/// target of an earlier kind in reach wins over a nearer one of a later kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    Anchor,
    Intersection,
    /// Where a line from one of [`Outlines::from`] touches a curve.
    Tangent,
    /// The foot of a line from one of [`Outlines::from`] that meets a segment at a
    /// right angle.
    Perpendicular,
    Midpoint,
    Centre,
    OnPath,
}

impl PointKind {
    /// The word on the pill beside the ring: a point is too small for a line to say
    /// what it matched.
    pub fn label(self) -> &'static str {
        match self {
            Self::Anchor => "Anchor",
            Self::Intersection => "Intersection",
            Self::Tangent => "Tangent",
            Self::Perpendicular => "Perpendicular",
            Self::Midpoint => "Midpoint",
            Self::Centre => "Centre",
            Self::OnPath => "On path",
        }
    }
}

/// A point snap: where the point landed and what it landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointMark {
    pub at: Point,
    pub kind: PointKind,
}

/// The ruler guides worth snapping to, split by the axis each one pins.
///
/// Coordinates rather than `ondin_core::Guide`s, so this module knows nothing
/// about the document and the comparison against a `Rect`'s features is one loop.
#[derive(Clone, Debug, Default)]
pub struct GuideLines {
    /// Xs — from *vertical* guides, which pin an x.
    pub x: Vec<f64>,
    /// Ys — from *horizontal* guides, which pin a y.
    pub y: Vec<f64>,
}

/// A transient line drawn while a snap is active, in world space. Not a ruler
/// guide — see the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    /// The world coordinate of the line (x for vertical, y for horizontal).
    pub at: f64,
    /// The extent to draw along the other axis — the union of the moving box
    /// and the thing it snapped to, so the guide visibly connects them.
    pub from: f64,
    pub to: f64,
}

/// What a snap drew, handed to the overlay as one thing.
///
/// A [`Guide`] is an alignment (a bare line); a [`Measure`] is a gap drawn with its
/// number, the same type the Alt-hover measure produces, via `canvas::draw_measure`.
#[derive(Default, Debug)]
pub struct Marks {
    pub guides: Vec<Guide>,
    /// The gaps a spacing snap equalized, labelled. Empty for every other snap.
    pub gaps: Vec<Measure>,
    /// The geometry a dragged point landed on, drawn as a ring and its name.
    pub point: Option<PointMark>,
}

impl From<Vec<Guide>> for Marks {
    fn from(guides: Vec<Guide>) -> Self {
        Marks {
            guides,
            ..Marks::default()
        }
    }
}

impl From<PointSnap> for Marks {
    fn from(s: PointSnap) -> Self {
        Marks {
            guides: s.guides,
            gaps: Vec::new(),
            point: s.mark,
        }
    }
}

/// The result of a snap: how much further to move, and what to draw.
#[derive(Default, Debug)]
pub struct Snap {
    pub adjust: Vec2,
    pub marks: Marks,
}

/// What the best candidate on an axis turned out to be.
///
/// The distinction is only about what to *draw*. A node match needs a line
/// spanning both boxes; a ruler guide is already a drawn line, so a second one
/// along it would say nothing.
#[derive(Clone, Copy)]
enum Matched {
    Node(Rect),
    RulerGuide,
    /// A text baseline. Draws a mark, unlike `RulerGuide`: the overlay shows
    /// baselines only for a selected text node, never the one being snapped to.
    /// Carries the matched y, since a baseline is the coordinate and `Node` needs
    /// `nearest_of` to recover its winning feature.
    Baseline(f64),
    Spacing(Spacing),
    /// An anchor of another outline a dragged point lines up with: its line runs
    /// from the anchor to the point.
    Anchor(Point),
}

/// What a **spacing** match was, kept so the gaps can be measured once the delta
/// has been applied.
///
/// The neighbours rather than finished measures: a gap is only true at the
/// position the candidate moves the box to, so measuring at candidate time would
/// quote the unsnapped gaps.
#[derive(Clone, Copy)]
struct Spacing {
    /// The neighbour below the moving box on this axis, when the gap to it is part
    /// of the match.
    low: Option<Rect>,
    /// The neighbour above it.
    high: Option<Rect>,
    /// The existing gap this match copied, drawn beside the new one. `None` for a
    /// centring, where both drawn gaps are new.
    rhythm: Option<Measure>,
}

/// How near two boxes have to come, in world units, before the gap between them
/// counts as a gap at all.
///
/// A spacing snap over a zero gap is two boxes touching, which alignment already
/// handles; the floor stops flush rows offering no-op candidates labelled `0`.
const MIN_GAP: f64 = 0.5;

/// The **spacing** candidates on one axis: centre the moving box between its two
/// nearest neighbours, or match the gap between the two nearest on one side (so a
/// row spaces itself as it is built).
///
/// Only boxes that share a band with the moving one count (a strict overlap on the
/// other axis), and nothing is offered for a box that overlaps a neighbour along
/// this axis, where there is no gap to equalize.
fn spacing_candidates(moving: Rect, targets: &[Rect], axis: Axis) -> Vec<(f64, Spacing)> {
    let along = |r: &Rect| match axis {
        Axis::Horizontal => (r.min_x(), r.max_x()),
        Axis::Vertical => (r.min_y(), r.max_y()),
    };
    let band = |r: &Rect| match axis {
        Axis::Horizontal => (r.min_y(), r.max_y()),
        Axis::Vertical => (r.min_x(), r.max_x()),
    };
    let (m_lo, m_hi) = along(&moving);
    let m_band = band(&moving);

    // The row, split by side; a target overlapping along the axis is in neither list.
    let mut before: Vec<Rect> = Vec::new();
    let mut after: Vec<Rect> = Vec::new();
    for t in targets {
        let t_band = band(t);
        if t_band.0.max(m_band.0) >= t_band.1.min(m_band.1) {
            continue; // not in the same row
        }
        let (t_lo, t_hi) = along(t);
        if t_hi <= m_lo {
            before.push(*t);
        } else if t_lo >= m_hi {
            after.push(*t);
        }
    }
    // Nearest first on each side.
    before.sort_by(|a, b| along(b).1.total_cmp(&along(a).1));
    after.sort_by(|a, b| along(a).0.total_cmp(&along(b).0));

    let mut out = Vec::new();

    // Centre between the two nearest: offered only when the average gap is real.
    if let (Some(low), Some(high)) = (before.first(), after.first()) {
        let gap_low = m_lo - along(low).1;
        let gap_high = along(high).0 - m_hi;
        if (gap_low + gap_high) / 2.0 > MIN_GAP {
            out.push((
                (gap_high - gap_low) / 2.0,
                Spacing {
                    low: Some(*low),
                    high: Some(*high),
                    rhythm: None,
                },
            ));
        }
    }

    // Match the rhythm: the gap between the two nearest on a side is what the gap
    // to the nearer of them wants to become.
    for (side, sign) in [(&before, 1.0), (&after, -1.0)] {
        let (Some(near), Some(next)) = (side.first(), side.get(1)) else {
            continue;
        };
        let Some(rhythm) = measure::gap(*near, *next, axis) else {
            continue;
        };
        if rhythm.distance() <= MIN_GAP {
            continue;
        }
        let mine = match sign > 0.0 {
            true => m_lo - along(near).1,
            false => along(near).0 - m_hi,
        };
        out.push((
            sign * (rhythm.distance() - mine),
            Spacing {
                low: (sign > 0.0).then_some(*near),
                high: (sign < 0.0).then_some(*near),
                rhythm: Some(rhythm),
            },
        ));
    }
    out
}

/// The gaps to draw for a spacing match, measured at the position the snap moved
/// the box to.
fn spacing_marks(moved: Rect, sp: Spacing, axis: Axis) -> Vec<Measure> {
    let mut out = Vec::new();
    // The copied gap first, so it reads as the thing being matched.
    out.extend(sp.rhythm);
    for neighbour in [sp.low, sp.high].into_iter().flatten() {
        out.extend(measure::gap(moved, neighbour, axis));
    }
    out
}

/// Snap `moving` (the dragged box, already at its un-snapped position) against
/// the world bounds of everything in `targets` and the ruler guides in
/// `guides`.
///
/// `axes` says which world axes the gesture is free to move along, as for
/// [`snap_point`]. A locked axis must come back with an adjustment of exactly zero
/// and no guide, so it is gated here rather than zeroed afterwards.
///
/// A ruler guide wins a tie with a node: the user placed it on purpose, and
/// preferring it stops a snap flickering between the two.
pub fn snap_rect(moving: Rect, axes: (bool, bool), s: &Snapping) -> Snap {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    let mut snap = Snap::default();

    // Candidate features on each axis: near edge, centre, far edge.
    let moving_x = [moving.min_x(), moving.center().x, moving.max_x()];
    let moving_y = [moving.min_y(), moving.center().y, moving.max_y()];

    let mut best_x: Option<(f64, f64, Matched)> = None; // (distance, delta, what)
    let mut best_y: Option<(f64, f64, Matched)> = None;

    for target in &s.targets {
        if axes.0 {
            for m in moving_x {
                for t in [target.min_x(), target.center().x, target.max_x()] {
                    consider(&mut best_x, m, t, tolerance, Matched::Node(*target), false);
                }
            }
        }
        if axes.1 {
            for m in moving_y {
                for t in [target.min_y(), target.center().y, target.max_y()] {
                    consider(&mut best_y, m, t, tolerance, Matched::Node(*target), false);
                }
            }
        }
    }
    // Baselines must *beat* the best so far, so a ruler guide still wins a dead
    // heat. Only y: a baseline has no x. Guides tie and baselines do not, which
    // settles the heat in either block order, so neither mechanism is pinned alone;
    // read the flag as the rule and this position as convention.
    if axes.1 {
        for m in moving_y {
            for t in &s.baselines {
                consider(&mut best_y, m, *t, tolerance, Matched::Baseline(*t), false);
            }
        }
    }
    // Guides last, allowed to equal the best so far: that is their tie.
    if axes.0 {
        for m in moving_x {
            for t in &s.guides.x {
                consider(&mut best_x, m, *t, tolerance, Matched::RulerGuide, true);
            }
        }
    }
    if axes.1 {
        for m in moving_y {
            for t in &s.guides.y {
                consider(&mut best_y, m, *t, tolerance, Matched::RulerGuide, true);
            }
        }
    }

    // Spacing last, and it must be strictly closer than an alignment to win: an
    // alignment names a visible edge, a spacing snap invents a position. Empty
    // `targets` (Snap to shapes off) switches this off too.
    for (axis, best, free) in [
        (Axis::Horizontal, &mut best_x, axes.0),
        (Axis::Vertical, &mut best_y, axes.1),
    ] {
        if !free {
            continue;
        }
        for (delta, spacing) in spacing_candidates(moving, &s.targets, axis) {
            consider_delta(best, delta, tolerance, Matched::Spacing(spacing));
        }
    }

    if let Some((_, delta, what)) = best_x {
        snap.adjust.x = delta;
        let snapped = moving + Vec2::new(delta, 0.0);
        match what {
            Matched::Node(target) => {
                // The feature that matched, so the line sits on it.
                let at = nearest_of(
                    [snapped.min_x(), snapped.center().x, snapped.max_x()],
                    [target.min_x(), target.center().x, target.max_x()],
                );
                snap.marks.guides.push(Guide {
                    axis: Axis::Vertical,
                    at,
                    from: snapped.min_y().min(target.min_y()),
                    to: snapped.max_y().max(target.max_y()),
                });
            }
            // Measured at `snapped`: the gaps reported are the ones just created.
            Matched::Spacing(sp) => {
                snap.marks
                    .gaps
                    .extend(spacing_marks(snapped, sp, Axis::Horizontal))
            }
            Matched::RulerGuide => {}
            // Unreachable (a baseline is only offered to `best_y`); spelled out so
            // an x-bearing source added later has to decide here.
            Matched::Baseline(_) => {}
            // Unreachable: anchors are offered to a dragged point only.
            Matched::Anchor(_) => {}
        }
    } else {
        // Nothing claimed this axis, so the pixel grid takes it. The leading edge
        // is rounded: it is the node's position, and the width stays whole.
        if s.pixel && axes.0 {
            snap.adjust.x = pixel_delta(moving.min_x());
        }
    }
    if let Some((_, delta, what)) = best_y {
        snap.adjust.y = delta;
        let snapped = moving + Vec2::new(0.0, delta);
        match what {
            Matched::Node(target) => {
                let at = nearest_of(
                    [snapped.min_y(), snapped.center().y, snapped.max_y()],
                    [target.min_y(), target.center().y, target.max_y()],
                );
                snap.marks.guides.push(Guide {
                    axis: Axis::Horizontal,
                    at,
                    from: snapped.min_x().min(target.min_x()),
                    to: snapped.max_x().max(target.max_x()),
                });
            }
            Matched::Spacing(sp) => {
                snap.marks
                    .gaps
                    .extend(spacing_marks(snapped, sp, Axis::Vertical))
            }
            Matched::RulerGuide => {}
            // A baseline gets a mark spanning the moving box only: what matched is
            // a line inside a text node, not that node's box.
            Matched::Baseline(at) => snap.marks.guides.push(Guide {
                axis: Axis::Horizontal,
                at,
                from: snapped.min_x(),
                to: snapped.max_x(),
            }),
            Matched::Anchor(_) => {}
        }
    } else if s.pixel && axes.1 {
        snap.adjust.y = pixel_delta(moving.min_y());
    }

    snap
}

/// Which axes a move may travel along: both, or — while Shift is held — only
/// the one the drag has gone furthest along.
///
/// Shift constrains rather than switching snapping off (the Snap menu does that).
/// The axis is re-read from the live delta every frame, so a drag that turns
/// follows the pointer; the tie goes to x.
pub fn move_axes(raw: Vec2, shift: bool) -> (bool, bool) {
    if !shift {
        return (true, true);
    }
    let horizontal = raw.x.abs() >= raw.y.abs();
    (horizontal, !horizontal)
}

/// `raw` with the axes `move_axes` forbids zeroed out.
pub fn constrain(raw: Vec2, axes: (bool, bool)) -> Vec2 {
    Vec2::new(
        if axes.0 { raw.x } else { 0.0 },
        if axes.1 { raw.y } else { 0.0 },
    )
}

/// The result of snapping a single dragged point.
#[derive(Default, Debug)]
pub struct PointSnap {
    pub at: Point,
    pub guides: Vec<Guide>,
    /// The geometry the point landed on, when it landed on [`Snapping::outlines`].
    pub mark: Option<PointMark>,
}

/// Snap a dragged **point** — a resize handle, a pen anchor, a node-tool point —
/// onto nearby features, on the axes it is free to move.
///
/// The resize counterpart of [`snap_rect`]: a resize moves one corner or edge and
/// must leave the opposite one in place, which snapping the whole box would not.
///
/// `axes` says which world axes the handle travels along (`OndinApp::resize_axes`);
/// snapping any other would move the anchored edge. With both free, the point
/// first tries to land on [`Snapping::outlines`] ([`point_target`]), exactly and on
/// both axes at once; failing that, an anchor's x or y lines it up as a box's edge does.
pub fn snap_point(at: Point, axes: (bool, bool), s: &Snapping) -> PointSnap {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    if axes == (true, true)
        && let Some(mark) = point_target(at, s, tolerance)
    {
        return PointSnap {
            at: mark.at,
            guides: Vec::new(),
            mark: Some(mark),
        };
    }
    let mut out = PointSnap {
        at,
        guides: Vec::new(),
        mark: None,
    };

    let mut best_x: Option<(f64, f64, Matched)> = None;
    let mut best_y: Option<(f64, f64, Matched)> = None;
    for a in &s.outlines.anchors {
        if axes.0 {
            consider(
                &mut best_x,
                at.x,
                a.x,
                tolerance,
                Matched::Anchor(*a),
                false,
            );
        }
        if axes.1 {
            consider(
                &mut best_y,
                at.y,
                a.y,
                tolerance,
                Matched::Anchor(*a),
                false,
            );
        }
    }
    for target in &s.targets {
        if axes.0 {
            for t in [target.min_x(), target.center().x, target.max_x()] {
                consider(
                    &mut best_x,
                    at.x,
                    t,
                    tolerance,
                    Matched::Node(*target),
                    false,
                );
            }
        }
        if axes.1 {
            for t in [target.min_y(), target.center().y, target.max_y()] {
                consider(
                    &mut best_y,
                    at.y,
                    t,
                    tolerance,
                    Matched::Node(*target),
                    false,
                );
            }
        }
    }
    if axes.0 {
        for t in &s.guides.x {
            consider(&mut best_x, at.x, *t, tolerance, Matched::RulerGuide, true);
        }
    }
    if axes.1 {
        for t in &s.guides.y {
            consider(&mut best_y, at.y, *t, tolerance, Matched::RulerGuide, true);
        }
    }

    if let Some((_, delta, what)) = best_x {
        out.at.x += delta;
        // A line from the target to the handle; none for a ruler guide.
        let span = match what {
            Matched::Node(t) => Some((t.min_y(), t.max_y())),
            Matched::Anchor(a) => Some((a.y, a.y)),
            _ => None,
        };
        if let Some((lo, hi)) = span {
            out.guides.push(Guide {
                axis: Axis::Vertical,
                at: out.at.x,
                from: lo.min(out.at.y),
                to: hi.max(out.at.y),
            });
        }
    } else if s.pixel && axes.0 {
        // The dragged edge lands on a whole pixel, so a resize against a whole
        // opposite edge gives a whole width. Gated on `s.pixel` like every other
        // pixel fallback: with Snap to grid off, nothing may round.
        out.at.x += pixel_delta(out.at.x);
    }
    if let Some((_, delta, what)) = best_y {
        out.at.y += delta;
        let span = match what {
            Matched::Node(t) => Some((t.min_x(), t.max_x())),
            Matched::Anchor(a) => Some((a.x, a.x)),
            _ => None,
        };
        if let Some((lo, hi)) = span {
            out.guides.push(Guide {
                axis: Axis::Horizontal,
                at: out.at.y,
                from: lo.min(out.at.x),
                to: hi.max(out.at.x),
            });
        }
    } else if s.pixel && axes.1 {
        out.at.y += pixel_delta(out.at.y);
    }
    out
}

/// The geometry within `tolerance` of `at` a dragged point lands on, the first
/// [`PointKind`] in reach winning: an anchor, a crossing of two segments, a
/// tangent or perpendicular from [`Outlines::from`], a segment's midpoint, a
/// shape's centre, and last the nearest point on a segment.
pub fn point_target(at: Point, s: &Snapping, tolerance: f64) -> Option<PointMark> {
    let nearest = |points: &mut dyn Iterator<Item = Point>| {
        points
            .map(|p| (p.distance(at), p))
            .filter(|(d, _)| *d <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    };
    let mark = |at, kind| PointMark { at, kind };
    if let Some(p) = nearest(&mut s.outlines.anchors.iter().copied()) {
        return Some(mark(p, PointKind::Anchor));
    }
    let reach = Rect::from_center_size(at, (2.0 * tolerance, 2.0 * tolerance));
    let near: Vec<PathSeg> = s
        .outlines
        .segments
        .iter()
        .copied()
        .filter(|seg| reaches(*seg, reach))
        .collect();
    let mut crossed = Vec::new();
    for (i, a) in near.iter().enumerate() {
        for b in &near[i + 1..] {
            crossings(*a, *b, &mut crossed);
        }
    }
    if let Some(p) = nearest(&mut crossed.into_iter()) {
        return Some(mark(p, PointKind::Intersection));
    }
    let from = &s.outlines.from;
    let mut touching = Vec::new();
    let mut feet = Vec::new();
    for seg in &near {
        for f in from {
            tangents(*seg, *f, &mut touching);
            perpendiculars(*seg, *f, &mut feet);
        }
    }
    if let Some(p) = nearest(&mut touching.into_iter()) {
        return Some(mark(p, PointKind::Tangent));
    }
    if let Some(p) = nearest(&mut feet.into_iter()) {
        return Some(mark(p, PointKind::Perpendicular));
    }
    if let Some(p) = nearest(&mut near.iter().map(|seg| seg.eval(0.5))) {
        return Some(mark(p, PointKind::Midpoint));
    }
    if let Some(p) = nearest(&mut s.outlines.centres.iter().copied()) {
        return Some(mark(p, PointKind::Centre));
    }
    let on = near.iter().map(|seg| seg.eval(seg.nearest(at, 1e-9).t));
    nearest(&mut on.into_iter()).map(|p| mark(p, PointKind::OnPath))
}

/// Whether `seg` could pass through `area`: its control box meets it.
pub(crate) fn reaches(seg: PathSeg, area: Rect) -> bool {
    overlaps(seg_bounds(seg), area)
}

/// A segment's box, control points included, which is all a reach test needs.
fn seg_bounds(seg: PathSeg) -> Rect {
    match seg {
        PathSeg::Line(l) => Rect::from_points(l.p0, l.p1),
        PathSeg::Quad(q) => Rect::from_points(q.p0, q.p1).union_pt(q.p2),
        PathSeg::Cubic(c) => Rect::from_points(c.p0, c.p1).union_pt(c.p2).union_pt(c.p3),
    }
}

/// Whether two closed boxes share a point; a flat box (a straight segment along an
/// axis) still overlaps, where `Rect::intersect` would call it empty.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// The points on a curved `seg` where a line from `from` touches it: the roots of
/// `(B(t) − from) × B′(t)`. A straight segment has none: a line from a point on
/// its extension runs along it rather than touching it.
fn tangents(seg: PathSeg, from: Point, out: &mut Vec<Point>) {
    if !matches!(seg, PathSeg::Line(_)) {
        roots_on(seg, from, |d, v| d.cross(v), out);
    }
}

/// The feet on `seg` of the lines from `from` that meet it at a right angle: the
/// roots of `(B(t) − from) · B′(t)`. A line's is its projection, solved exactly
/// and kept only when it falls on the segment.
fn perpendiculars(seg: PathSeg, from: Point, out: &mut Vec<Point>) {
    let PathSeg::Line(l) = seg else {
        roots_on(seg, from, |d, v| d.dot(v), out);
        return;
    };
    let d = l.p1 - l.p0;
    if d.hypot2() < ROOT_EPS * ROOT_EPS {
        return;
    }
    let t = (from - l.p0).dot(d) / d.hypot2();
    let p = l.eval(t);
    if (0.0..=1.0).contains(&t) && p.distance(from) > ROOT_EPS {
        out.push(p);
    }
}

/// The points of `seg` where `f(B(t) − from, B′(t))` changes sign, found by
/// sampling [`ROOT_SAMPLES`] spans and halving each that brackets one. A root at
/// `from` itself or where the curve stalls names no direction and is dropped.
fn roots_on(seg: PathSeg, from: Point, f: impl Fn(Vec2, Vec2) -> f64, out: &mut Vec<Point>) {
    let deriv = |t: f64| match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
    };
    let g = |t: f64| f(seg.eval(t) - from, deriv(t));
    let mut keep = |t: f64| {
        let p = seg.eval(t);
        if p.distance(from) > ROOT_EPS && deriv(t).hypot() > ROOT_EPS {
            out.push(p);
        }
    };
    let (mut t0, mut g0) = (0.0, g(0.0));
    if g0 == 0.0 {
        keep(0.0);
    }
    for i in 1..=ROOT_SAMPLES {
        let t1 = i as f64 / ROOT_SAMPLES as f64;
        let g1 = g(t1);
        if g1 == 0.0 {
            keep(t1);
        } else if g0 * g1 < 0.0 {
            let (mut lo, mut hi, mut g_lo) = (t0, t1, g0);
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                let g_mid = g(mid);
                if g_mid * g_lo <= 0.0 {
                    hi = mid;
                } else {
                    (lo, g_lo) = (mid, g_mid);
                }
            }
            keep((lo + hi) / 2.0);
        }
        (t0, g0) = (t1, g1);
    }
}

/// How many spans [`roots_on`] samples a segment in. A cubic's tangent and normal
/// conditions are of degree five, so at most five roots, and two closer than a
/// span apart would be a near-degenerate touch no pointer could tell apart.
const ROOT_SAMPLES: usize = 64;
/// Distances and speeds under this, in world units, count as zero for a root.
const ROOT_EPS: f64 = 1e-9;

/// Where two segments cross, appended to `out`. Two lines are solved exactly; a
/// curve is halved alongside the other while their boxes overlap, down to a box
/// [`CROSSING_EPS`] across. Two segments lying along each other give nothing
/// useful, so the work is capped rather than run out.
fn crossings(a: PathSeg, b: PathSeg, out: &mut Vec<Point>) {
    if let (PathSeg::Line(l), PathSeg::Line(m)) = (a, b) {
        let (r, s) = (l.p1 - l.p0, m.p1 - m.p0);
        let denom = r.cross(s);
        if denom.abs() < 1e-12 {
            return;
        }
        let q = m.p0 - l.p0;
        let (t, u) = (q.cross(s) / denom, q.cross(r) / denom);
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            out.push(l.eval(t));
        }
        return;
    }
    let mut budget = CROSSING_BUDGET;
    let mut found = Vec::new();
    let mut stack = vec![(0.0, 1.0, 0.0, 1.0)];
    while let Some((a0, a1, b0, b1)) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let (sa, sb) = (a.subsegment(a0..a1), b.subsegment(b0..b1));
        let (ba, bb) = (seg_bounds(sa), seg_bounds(sb));
        if !overlaps(ba, bb) {
            continue;
        }
        let small = |r: Rect| r.width().max(r.height()) < CROSSING_EPS;
        if small(ba) && small(bb) {
            let p = ba.center();
            if !found
                .iter()
                .any(|q: &Point| q.distance(p) < CROSSING_EPS * 10.0)
            {
                found.push(p);
            }
            continue;
        }
        let (am, bm) = ((a0 + a1) / 2.0, (b0 + b1) / 2.0);
        stack.extend([
            (a0, am, b0, bm),
            (a0, am, bm, b1),
            (am, a1, b0, bm),
            (am, a1, bm, b1),
        ]);
    }
    out.extend(found);
}

/// How small, in world units, the boxes [`crossings`] halves down to get before a
/// crossing is read off them: well under anything a pointer or a pixel can tell.
const CROSSING_EPS: f64 = 1e-6;
/// The most box pairs [`crossings`] looks at for one pair of segments.
const CROSSING_BUDGET: usize = 20_000;

/// Snap a ruler guide being dragged onto the edges and centres of nearby nodes.
///
/// Supports the block-out workflow: rectangles, guides dropped onto their edges,
/// rectangles deleted. Only nodes, not other guides: two guides at one coordinate
/// are a mistake, not a goal.
pub fn snap_guide(position: f64, axis: Axis, s: &Snapping) -> f64 {
    let tolerance = TOLERANCE_PX / s.zoom.max(f64::EPSILON);
    let mut best: Option<(f64, f64)> = None;
    for t in &s.targets {
        let features = match axis {
            Axis::Vertical => [t.min_x(), t.center().x, t.max_x()],
            Axis::Horizontal => [t.min_y(), t.center().y, t.max_y()],
        };
        for f in features {
            let d = (f - position).abs();
            if d <= tolerance && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, f));
            }
        }
    }
    // A guide that caught nothing still lands on a whole pixel when the grid is
    // on: everything aligned to it inherits any error.
    match best {
        Some((_, at)) => at,
        None if s.pixel => to_pixel(position),
        None => position,
    }
}

/// Keep `best` if it is closer than the pair `(m, t)`; otherwise take the new
/// one. `ties` lets an equally-close candidate displace the incumbent.
fn consider(
    best: &mut Option<(f64, f64, Matched)>,
    m: f64,
    t: f64,
    tolerance: f64,
    what: Matched,
    ties: bool,
) {
    consider_delta_with(best, t - m, tolerance, what, ties);
}

/// [`consider`] for a spacing candidate, whose delta is not the distance between
/// two coordinates. Never takes ties.
fn consider_delta(
    best: &mut Option<(f64, f64, Matched)>,
    delta: f64,
    tolerance: f64,
    what: Matched,
) {
    consider_delta_with(best, delta, tolerance, what, false);
}

fn consider_delta_with(
    best: &mut Option<(f64, f64, Matched)>,
    delta: f64,
    tolerance: f64,
    what: Matched,
    ties: bool,
) {
    let d = delta.abs();
    if d > tolerance {
        return;
    }
    let better = match *best {
        None => true,
        Some((bd, _, _)) if ties => d <= bd,
        Some((bd, _, _)) => d < bd,
    };
    if better {
        *best = Some((d, delta, what));
    }
}

/// The coordinate where a moving feature and a target feature coincide.
fn nearest_of(moving: [f64; 3], target: [f64; 3]) -> f64 {
    let mut best = (f64::INFINITY, target[0]);
    for m in moving {
        for t in target {
            let d = (m - t).abs();
            if d < best.0 {
                best = (d, t);
            }
        }
    }
    best.1
}

/// World bounds on `page` worth snapping to: everything visible that the gesture
/// is not itself carrying.
///
/// Only the gesture's own subtree is excluded: its bounds move with the pointer,
/// so a match is guaranteed and means nothing. The container chain stays, so a
/// shape snaps to the edges of its own frame (groups are not in the spatial index,
/// see [`ondin_core::nodes_in_view`]; an artboard's rect is its declared size).
pub fn targets(
    doc: &Document,
    res: &Resolved,
    page: NodeId,
    moving: &[NodeId],
    view: Rect,
    limit: usize,
) -> Vec<Rect> {
    let mut out = Vec::new();
    for id in ondin_core::nodes_in_view(res, page, view) {
        if moving.contains(&id) || is_inside_any(doc, id, moving) {
            continue;
        }
        if doc.get(id).is_some() && !doc.shown(id) {
            continue;
        }
        if let Some(b) = res.world_bounds(id) {
            out.push(b);
        }
        // Snapping is a comfort feature: on a very busy viewport, stop at `limit`.
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn ancestors(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    while let Some(p) = cursor {
        out.push(p);
        cursor = doc.get(p).and_then(|n| n.parent());
    }
    out
}

/// Whether `id` sits under any of `moving` — the ancestry half of excluding
/// everything being transformed from the snap targets. `pub(crate)` because
/// `canvas::baseline_lines` asks the same question.
pub(crate) fn is_inside_any(doc: &Document, id: NodeId, moving: &[NodeId]) -> bool {
    ancestors(doc, id).iter().any(|a| moving.contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Rect;

    /// An unconstrained move: both axes free.
    const FREE: (bool, bool) = (true, true);

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    #[test]
    fn a_near_edge_snaps_flush() {
        // Left edge 2 units from the target's left edge.
        let moving = r(102.0, 300.0, 152.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 50.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert_eq!(snap.marks.guides.len(), 1);
        assert_eq!(snap.marks.guides[0].axis, Axis::Vertical);
        assert!((snap.marks.guides[0].at - 100.0).abs() < 1e-9);
    }

    #[test]
    fn distant_edges_do_not_snap() {
        // On whole pixels, so the grid fallback has nothing to do either.
        let moving = r(400.0, 400.0, 450.0, 450.0);
        let target = r(0.0, 0.0, 50.0, 50.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert_eq!(snap.adjust, Vec2::ZERO);
        assert!(snap.marks.guides.is_empty());
    }

    /// With nothing in reach, an axis falls to the pixel grid: the box's leading
    /// edge lands on a whole unit and its size is untouched.
    #[test]
    fn an_unclaimed_axis_falls_to_the_pixel_grid() {
        let moving = r(400.3, 400.8, 450.3, 450.8);
        let snap = snap_rect(moving, FREE, &grid(1.0));
        let landed = moving + snap.adjust;
        assert!((landed.min_x() - 400.0).abs() < 1e-9, "{landed:?}");
        assert!((landed.min_y() - 401.0).abs() < 1e-9, "{landed:?}");
        assert!((landed.width() - 50.0).abs() < 1e-9, "size changed");
        // The grid is not a relationship, so it draws nothing.
        assert!(snap.marks.guides.is_empty());
    }

    /// An alignment match beats the grid on its own axis: an edge at 100.5 is not
    /// rounded away.
    #[test]
    fn an_alignment_match_is_not_then_rounded_away() {
        let moving = r(102.0, 300.4, 152.0, 350.4);
        let target = r(100.5, 0.0, 200.5, 50.0);
        // Both sources on: the grid must not undo the shapes.
        let snap = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![target],
                pixel: true,
                ..Snapping::none(1.0)
            },
        );
        let landed = moving + snap.adjust;
        assert!(
            (landed.min_x() - 100.5).abs() < 1e-9,
            "x was rounded off its alignment: {landed:?}"
        );
        // y caught nothing, so y alone went to the grid.
        assert!((landed.min_y() - 300.0).abs() < 1e-9, "{landed:?}");

        // The same for a guide at a fractional coordinate.
        let g = snap_rect(
            moving,
            FREE,
            &Snapping {
                guides: lines(&[100.5], &[]),
                pixel: true,
                ..Snapping::none(1.0)
            },
        );
        assert!(((moving + g.adjust).min_x() - 100.5).abs() < 1e-9);
    }

    /// Each source can be switched off on its own, and off means off.
    #[test]
    fn each_snap_source_can_be_switched_off_independently() {
        let moving = r(102.4, 203.4, 152.4, 253.4);
        let target = r(100.0, 200.0, 200.0, 260.0);

        // Everything off: the box goes exactly where it was put.
        assert_eq!(
            snap_rect(moving, FREE, &Snapping::none(1.0)).adjust,
            Vec2::ZERO
        );

        // Shapes only — lands on the shape, not on the grid.
        let s = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!(((moving + s.adjust).min_x() - 100.0).abs() < 1e-9);

        // Grid only — rounds, and ignores the shape 2.4 away.
        let g = snap_rect(moving, FREE, &grid(1.0));
        assert!(
            ((moving + g.adjust).min_x() - 102.0).abs() < 1e-9,
            "{:?}",
            g.adjust
        );

        // Guides only: the shape's edge at 100 is nearer than the guide at 105
        // and must still be ignored.
        let gl = snap_rect(moving, FREE, &guides(&[105.0], &[], 1.0));
        assert!(
            ((moving + gl.adjust).min_x() - 105.0).abs() < 1e-9,
            "{:?}",
            gl.adjust
        );
        assert_eq!(gl.adjust.y, 0.0, "y had no guide and no grid");
    }

    /// A resize rounds the dragged edge, only on the axes it moves.
    #[test]
    fn a_resize_rounds_the_dragged_edge_only() {
        let at = Point::new(102.4, 203.6);
        let side = snap_point(at, (true, false), &grid(1.0));
        assert!((side.at.x - 102.0).abs() < 1e-9, "{:?}", side.at);
        assert_eq!(side.at.y, 203.6, "the anchored axis was rounded");

        let corner = snap_point(at, (true, true), &grid(1.0));
        assert!((corner.at.x - 102.0).abs() < 1e-9);
        assert!((corner.at.y - 204.0).abs() < 1e-9);

        // Frozen on both: untouched, grid included.
        assert_eq!(snap_point(at, (false, false), &grid(1.0)).at, at);
    }

    /// A guide that catches nothing still lands on a whole pixel.
    #[test]
    fn a_guide_that_catches_nothing_lands_on_a_whole_pixel() {
        assert_eq!(snap_guide(400.4, Axis::Vertical, &grid(1.0)), 400.0);
        assert_eq!(snap_guide(400.6, Axis::Horizontal, &grid(1.0)), 401.0);
        // A shape edge at a fraction still wins.
        let target = r(100.5, 40.0, 200.0, 240.0);
        let got = snap_guide(102.0, Axis::Vertical, &shapes(&[target], 1.0));
        assert!((got - 100.5).abs() < 1e-9, "{got}");
    }

    #[test]
    fn centres_snap_to_centres() {
        // Target centre x = 150; moving centre x = 153.
        let moving = r(128.0, 300.0, 178.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 40.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!((snap.marks.guides[0].at - 150.0).abs() < 1e-9);
    }

    #[test]
    fn tolerance_follows_zoom_not_world_units() {
        // 20 world units apart: out of reach at 1x, 4 screen px at 0.2x.
        let moving = r(70.0, 0.0, 120.0, 50.0);
        let target = r(0.0, 0.0, 50.0, 50.0);
        assert_eq!(
            snap_rect(moving, FREE, &shapes(&[target], 1.0)).adjust,
            Vec2::ZERO
        );
        assert!(snap_rect(moving, FREE, &shapes(&[target], 0.2)).adjust.x != 0.0);
    }

    #[test]
    fn both_axes_can_snap_at_once() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        let target = r(100.0, 200.0, 200.0, 260.0);
        let snap = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!(snap.adjust.x != 0.0 && snap.adjust.y != 0.0);
        assert_eq!(snap.marks.guides.len(), 2);
    }

    #[test]
    fn the_closest_candidate_wins() {
        let moving = r(103.0, 500.0, 153.0, 550.0);
        // Left edge is 3 from 100 but 1 from 104.
        let near = r(104.0, 0.0, 154.0, 20.0);
        let far = r(100.0, 0.0, 150.0, 20.0);
        let snap = snap_rect(moving, FREE, &shapes(&[far, near], 1.0));
        assert!((snap.adjust.x - 1.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    fn lines(x: &[f64], y: &[f64]) -> GuideLines {
        GuideLines {
            x: x.to_vec(),
            y: y.to_vec(),
        }
    }

    /// Snapping to shape targets only.
    fn shapes(targets: &[Rect], zoom: f64) -> Snapping {
        Snapping {
            targets: targets.to_vec(),
            ..Snapping::none(zoom)
        }
    }

    /// Snapping to ruler guides only.
    fn guides(x: &[f64], y: &[f64], zoom: f64) -> Snapping {
        Snapping {
            guides: lines(x, y),
            ..Snapping::none(zoom)
        }
    }

    /// Snapping to the pixel grid only.
    fn grid(zoom: f64) -> Snapping {
        Snapping {
            pixel: true,
            ..Snapping::none(zoom)
        }
    }

    /// A ruler guide snaps on either axis, with the same screen-pixel tolerance.
    #[test]
    fn a_box_snaps_to_a_ruler_guide() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        let snap = snap_rect(moving, FREE, &guides(&[100.0], &[200.0], 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!((snap.adjust.y + 3.0).abs() < 1e-9, "{:?}", snap.adjust);

        // Out of reach on both axes, and nothing moves.
        let far = snap_rect(moving, FREE, &guides(&[10.0], &[10.0], 1.0));
        assert_eq!(far.adjust, Vec2::ZERO);
    }

    /// Snapping to a ruler guide draws no second line; a node snap on the other
    /// axis still draws its own.
    #[test]
    fn snapping_to_a_ruler_guide_draws_no_extra_line() {
        let moving = r(102.0, 203.0, 152.0, 253.0);
        let guide_only = snap_rect(moving, FREE, &guides(&[100.0], &[], 1.0));
        assert!(guide_only.adjust.x != 0.0);
        assert!(
            guide_only.marks.guides.is_empty(),
            "{:?}",
            guide_only.marks.guides
        );

        // x from the guide, y from a node: one line, for the node.
        let target = r(300.0, 200.0, 400.0, 260.0);
        let both = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![target],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!(both.adjust.x != 0.0 && both.adjust.y != 0.0);
        assert_eq!(both.marks.guides.len(), 1);
        assert_eq!(both.marks.guides[0].axis, Axis::Horizontal);
    }

    /// A nearer node still wins; only a tie goes to the guide.
    #[test]
    fn a_ruler_guide_takes_ties_but_not_a_closer_node() {
        let moving = r(103.0, 500.0, 153.0, 550.0);
        // Node edge 1 away, guide 3 away: the node wins and draws its line.
        let near_node = r(104.0, 0.0, 154.0, 20.0);
        let node_wins = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![near_node],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!(
            (node_wins.adjust.x - 1.0).abs() < 1e-9,
            "{:?}",
            node_wins.adjust
        );
        assert_eq!(node_wins.marks.guides.len(), 1);

        // Both 3 away: the guide takes it, so no line.
        let tied_node = r(106.0, 0.0, 156.0, 20.0);
        let tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                targets: vec![tied_node],
                guides: lines(&[100.0], &[]),
                ..Snapping::none(1.0)
            },
        );
        assert!((tie.adjust.x + 3.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(
            tie.marks.guides.is_empty(),
            "the guide won, so it draws nothing"
        );
    }

    /// Snapping to text baselines only.
    fn baselines(ys: &[f64], zoom: f64) -> Snapping {
        Snapping {
            baselines: ys.to_vec(),
            ..Snapping::none(zoom)
        }
    }

    /// A baseline pulls an edge or a centre and draws a mark on the baseline itself.
    /// The second case snaps a centre, where the baseline and the box edge differ,
    /// so a mark drawn at `snapped.min_y()` fails there.
    #[test]
    fn a_baseline_pulls_a_box_and_marks_the_line_it_pulled_to() {
        // Top edge 2 above a baseline at 300.
        let moving = r(10.0, 298.0, 60.0, 348.0);
        let snap = snap_rect(moving, FREE, &baselines(&[300.0], 1.0));
        assert!((snap.adjust.y - 2.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert_eq!(snap.marks.guides.len(), 1, "a baseline snap draws its line");
        let mark = snap.marks.guides[0];
        assert_eq!(mark.axis, Axis::Horizontal, "a baseline pins a y");
        assert!(
            (mark.at - 300.0).abs() < 1e-9,
            "the mark is on the baseline"
        );

        // Centre 1 below a baseline at 500.
        let by_centre = r(10.0, 476.0, 60.0, 526.0);
        let snap = snap_rect(by_centre, FREE, &baselines(&[500.0], 1.0));
        assert!((snap.adjust.y + 1.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!(
            (snap.marks.guides[0].at - 500.0).abs() < 1e-9,
            "the mark followed the box's edge instead of the baseline: {:?}",
            snap.marks.guides[0]
        );
    }

    /// A baseline offers no x: a vertical edge near one in value is not a
    /// candidate. The source is a bare `Vec<f64>`; only `axes.1` makes it a y.
    #[test]
    fn a_baseline_never_claims_the_horizontal_axis() {
        // Left edge two from the value 300.
        let moving = r(302.0, 20.0, 352.0, 70.0);
        let snap = snap_rect(moving, FREE, &baselines(&[300.0], 1.0));
        assert_eq!(
            snap.adjust.x, 0.0,
            "a baseline pulled an x, so it is being consulted on both axes"
        );
        assert!(snap.marks.guides.is_empty());
    }

    /// A ruler guide takes a tie against a baseline, and a nearer baseline beats a
    /// further guide.
    ///
    /// Pins the behaviour, not either mechanism: moving the baseline block after
    /// the guide block, or giving baselines the tie flag, changes nothing alone;
    /// only both together fail the first assertion (`y: 3.0` where `-3.0` is due).
    #[test]
    fn a_ruler_guide_takes_a_tie_against_a_baseline() {
        let moving = r(10.0, 303.0, 60.0, 353.0);
        // Guide 3 above, baseline 3 below: a dead heat.
        let tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![306.0],
                guides: lines(&[], &[300.0]),
                ..Snapping::none(1.0)
            },
        );
        assert!((tie.adjust.y + 3.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(
            tie.marks.guides.is_empty(),
            "the ruler guide won the tie, so nothing is drawn — a mark here means \
             the baseline took it"
        );

        // Baseline 1 away, guide 4 away: the baseline wins outright.
        let nearer = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![304.0],
                guides: lines(&[], &[299.0]),
                ..Snapping::none(1.0)
            },
        );
        assert!((nearer.adjust.y - 1.0).abs() < 1e-9, "{:?}", nearer.adjust);
        assert_eq!(nearer.marks.guides.len(), 1, "and it draws its line");

        // A node edge keeps a tie too; this pins the tie flag alone. flip: give
        // baselines `true` → fails here at 3.0 against −3.0.
        let node_tie = snap_rect(
            moving,
            FREE,
            &Snapping {
                baselines: vec![306.0],
                targets: vec![r(200.0, 300.0, 260.0, 320.0)],
                ..Snapping::none(1.0)
            },
        );
        assert!(
            (node_tie.adjust.y + 3.0).abs() < 1e-9,
            "a baseline took a dead heat off a node edge: {:?}",
            node_tie.adjust
        );
    }

    /// Centres snap to guides too, not just edges.
    #[test]
    fn a_box_centres_on_a_ruler_guide() {
        // Centre x = 153; guide at 150.
        let moving = r(128.0, 300.0, 178.0, 350.0);
        let snap = snap_rect(moving, FREE, &guides(&[150.0], &[], 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    /// A resize snaps only on the axes the handle travels along.
    #[test]
    fn a_resize_handle_snaps_only_on_the_axes_it_moves() {
        let at = Point::new(102.0, 203.0);
        let both_axes = || guides(&[100.0], &[200.0], 1.0);

        // A corner travels along both.
        let corner = snap_point(at, (true, true), &both_axes());
        assert!((corner.at.x - 100.0).abs() < 1e-9, "{:?}", corner.at);
        assert!((corner.at.y - 200.0).abs() < 1e-9, "{:?}", corner.at);

        // A vertical side travels along x only, though a horizontal guide is in reach.
        let side = snap_point(at, (true, false), &both_axes());
        assert!((side.at.x - 100.0).abs() < 1e-9, "{:?}", side.at);
        assert_eq!(side.at.y, 203.0, "the anchored axis moved");

        // Frozen on both.
        assert_eq!(snap_point(at, (false, false), &both_axes()).at, at);
    }

    /// A handle snaps to shape edges and guides, drawing a line only for a shape.
    #[test]
    fn a_resize_handle_snaps_to_shapes_and_explains_only_those() {
        let at = Point::new(102.0, 500.0);
        let target = r(100.0, 0.0, 200.0, 60.0);
        let node = snap_point(at, (true, false), &shapes(&[target], 1.0));
        assert!((node.at.x - 100.0).abs() < 1e-9, "{:?}", node.at);
        assert_eq!(node.guides.len(), 1);
        // The line reaches from the shape to the handle.
        let g = node.guides[0];
        assert_eq!(g.axis, Axis::Vertical);
        assert!(g.from <= 0.0 && g.to >= 500.0, "{g:?}");

        let guide = snap_point(at, (true, false), &guides(&[100.0], &[], 1.0));
        assert!((guide.at.x - 100.0).abs() < 1e-9);
        assert!(guide.guides.is_empty(), "a guide is already its own line");
    }

    /// A guide being dragged catches shape edges and centres.
    #[test]
    fn a_dragged_guide_catches_shape_edges_and_centres() {
        let target = r(100.0, 40.0, 200.0, 240.0);
        let boxes = [target];
        // Left edge, centre, right edge on x.
        for (from, want) in [(102.0, 100.0), (148.0, 150.0), (203.0, 200.0)] {
            let got = snap_guide(from, Axis::Vertical, &shapes(&boxes, 1.0));
            assert!((got - want).abs() < 1e-9, "{from} -> {got}, want {want}");
        }
        // And on y.
        for (from, want) in [(43.0, 40.0), (142.0, 140.0), (238.0, 240.0)] {
            let got = snap_guide(from, Axis::Horizontal, &shapes(&boxes, 1.0));
            assert!((got - want).abs() < 1e-9, "{from} -> {got}, want {want}");
        }
        // Out of reach, and the tolerance is in screen pixels here too.
        assert_eq!(
            snap_guide(400.0, Axis::Vertical, &shapes(&boxes, 1.0)),
            400.0
        );
        assert_eq!(
            snap_guide(120.0, Axis::Vertical, &shapes(&boxes, 1.0)),
            120.0
        );
        assert!((snap_guide(120.0, Axis::Vertical, &shapes(&boxes, 0.2)) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn guides_span_both_boxes_so_the_relationship_is_visible() {
        let moving = r(102.0, 300.0, 152.0, 350.0);
        let target = r(100.0, 0.0, 200.0, 50.0);
        let g = snap_rect(moving, FREE, &shapes(&[target], 1.0))
            .marks
            .guides[0];
        assert!(g.from <= 0.0 && g.to >= 350.0, "{g:?}");
    }

    // --- spacing: equal gaps -----------------------------------------------

    /// Two neighbours with bands tall enough that no alignment candidate is in
    /// reach. A 40-wide moving box has equal gaps at `180..220`.
    fn a_row() -> [Rect; 2] {
        [r(0.0, 0.0, 100.0, 1000.0), r(300.0, 0.0, 400.0, 1000.0)]
    }

    fn gaps_of(snap: &Snap) -> Vec<f64> {
        let mut out: Vec<f64> = snap.marks.gaps.iter().map(|m| m.distance()).collect();
        out.sort_by(f64::total_cmp);
        out
    }

    /// A box dropped between two others lands halfway, and the labels quote the
    /// gaps it made, not the 84/76 under the pointer.
    #[test]
    fn a_box_between_two_others_centres_itself_and_says_by_how_much() {
        // 4 right of centre.
        let moving = r(184.0, 17.0, 224.0, 73.0);
        let snap = snap_rect(moving, FREE, &shapes(&a_row(), 1.0));
        assert!((snap.adjust.x + 4.0).abs() < 1e-9, "{:?}", snap.adjust);
        let landed = moving + snap.adjust;
        assert!((landed.min_x() - 180.0).abs() < 1e-9, "{landed:?}");

        assert_eq!(
            gaps_of(&snap),
            vec![80.0, 80.0],
            "the labels must quote the gaps the snap made, not the 84/76 it found"
        );
        // Gaps draw horizontally, and an equalized axis draws no alignment line.
        assert!(snap.marks.gaps.iter().all(|m| m.axis == Axis::Horizontal));
        assert!(snap.marks.guides.is_empty(), "{:?}", snap.marks.guides);
    }

    /// Past the end of a run, the run's existing gap is the gap the next box takes.
    #[test]
    fn a_third_box_takes_the_rhythm_the_first_two_set() {
        // An existing gap of 100.
        let row = [r(0.0, 0.0, 100.0, 1000.0), r(200.0, 0.0, 300.0, 1000.0)];
        // 3 right of the rhythm position, 400.
        let moving = r(403.0, 17.0, 453.0, 73.0);
        let snap = snap_rect(moving, FREE, &shapes(&row, 1.0));
        assert!((snap.adjust.x + 3.0).abs() < 1e-9, "{:?}", snap.adjust);
        assert!(((moving + snap.adjust).min_x() - 400.0).abs() < 1e-9);
        // Both the gap it copied and the gap it made.
        assert_eq!(gaps_of(&snap), vec![100.0, 100.0]);
    }

    /// An alignment wins unless spacing is strictly nearer.
    #[test]
    fn an_alignment_beats_spacing_unless_spacing_is_strictly_nearer() {
        let moving = r(184.0, 17.0, 224.0, 73.0); // spacing wants -4

        // A box in a band of its own, so it can align but never be a spacing
        // neighbour; its right edge is its only feature in reach (a narrower box
        // would put its centre in reach and compare alignment with alignment).
        let elsewhere = |right: f64| r(100.0, 2000.0, right, 2100.0);

        // Tie at 4: the alignment takes it, drawing a line and no numbers.
        let tie = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(180.0)], 1.0),
        );
        assert!((tie.adjust.x + 4.0).abs() < 1e-9, "{:?}", tie.adjust);
        assert!(tie.marks.gaps.is_empty(), "spacing took a tie");
        assert_eq!(tie.marks.guides.len(), 1, "{:?}", tie.marks.guides);

        // Alignment nearer at 2.
        let nearer = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(182.0)], 1.0),
        );
        assert!((nearer.adjust.x + 2.0).abs() < 1e-9, "{:?}", nearer.adjust);
        assert!(nearer.marks.gaps.is_empty());

        // Alignment further at 6: spacing wins.
        let spacing = snap_rect(
            moving,
            FREE,
            &shapes(&[a_row()[0], a_row()[1], elsewhere(178.0)], 1.0),
        );
        assert!(
            (spacing.adjust.x + 4.0).abs() < 1e-9,
            "{:?}",
            spacing.adjust
        );
        assert_eq!(gaps_of(&spacing), vec![80.0, 80.0]);
    }

    /// Boxes that do not share a band are not a row.
    #[test]
    fn spacing_ignores_boxes_that_are_not_in_the_same_row() {
        let moving = r(184.0, 17.0, 224.0, 73.0);
        // The same row, moved down past the moving box.
        let away = [r(0.0, 500.0, 100.0, 600.0), r(300.0, 500.0, 400.0, 600.0)];
        let snap = snap_rect(moving, FREE, &shapes(&away, 1.0));
        assert!(snap.marks.gaps.is_empty(), "{:?}", snap.marks.gaps);
        assert_eq!(snap.adjust.x, 0.0, "{:?}", snap.adjust);

        // Touching bands are not a row either.
        let flush = [r(0.0, 73.0, 100.0, 600.0), r(300.0, 73.0, 400.0, 600.0)];
        assert!(
            snap_rect(moving, FREE, &shapes(&flush, 1.0))
                .marks
                .gaps
                .is_empty()
        );
    }

    /// A gap too small to be spacing is left to alignment (`MIN_GAP`).
    ///
    /// Tested just above zero, not at it: `measure::gap`'s epsilon already drops a
    /// zero gap, so a flush row passes without the floor. Gaps of 0.4 and 0.5 put
    /// the spacing delta at 0.05, nearer than either alignment, so only the floor
    /// keeps spacing out.
    #[test]
    fn a_gap_too_small_to_be_spacing_is_left_to_alignment() {
        let row = [r(0.0, 0.0, 100.0, 1000.0), r(140.9, 0.0, 240.9, 1000.0)];
        let moving = r(100.4, 17.0, 140.4, 73.0); // gaps of 0.4 and 0.5
        let snap = snap_rect(moving, FREE, &shapes(&row, 1.0));
        assert!(
            snap.marks.gaps.is_empty(),
            "0.45 is not spacing: {:?}",
            snap.marks.gaps
        );
        assert!(
            (snap.adjust.x + 0.4).abs() < 1e-9,
            "the left edge should land flush on the neighbour, not 0.05 off it: {:?}",
            snap.adjust
        );

        // Control: widen the row and spacing fires.
        let wide = [r(0.0, 0.0, 100.0, 1000.0), r(300.0, 0.0, 400.0, 1000.0)];
        let between = r(184.0, 17.0, 224.0, 73.0);
        assert_eq!(
            gaps_of(&snap_rect(between, FREE, &shapes(&wide, 1.0))),
            vec![80.0, 80.0]
        );
    }

    /// A Shift-locked axis gets no spacing, as it gets no alignment.
    #[test]
    fn a_locked_axis_gets_no_spacing_either() {
        let moving = r(184.0, 17.0, 224.0, 73.0);
        let locked = snap_rect(moving, (false, true), &shapes(&a_row(), 1.0));
        assert_eq!(locked.adjust.x, 0.0, "{:?}", locked.adjust);
        assert!(locked.marks.gaps.is_empty(), "{:?}", locked.marks.gaps);
        // Free, it centres.
        assert!((snap_rect(moving, FREE, &shapes(&a_row(), 1.0)).adjust.x + 4.0).abs() < 1e-9);
    }

    /// Spacing is a move behaviour: a resize may not touch the anchored edge (see
    /// `snap_point`).
    #[test]
    fn a_resize_handle_gets_no_spacing() {
        let at = Point::new(184.0, 45.0);
        let out = snap_point(at, (true, true), &shapes(&a_row(), 1.0));
        assert_eq!(out.at, at);
        assert!(out.guides.is_empty());
    }

    // --- targets: which nodes a gesture may snap to ------------------------

    /// A frame, a shape inside it, and a sibling shape beside the frame.
    fn frame_and_shapes() -> (Document, Resolved, NodeId, NodeId, NodeId) {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::{IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, inside, outside) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |size: f64| NodeKind::Rect {
            size: Size::new(size, size),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((0.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: inside,
                parent: frame,
                index: 0,
                kind: rect(50.0),
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
            Operation::CreateNode {
                id: outside,
                parent: root,
                index: 1,
                kind: rect(50.0),
                transform: Some(Affine::translate((600.0, 600.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, inside, outside)
    }

    /// A shape inside a frame can snap to that frame's edges.
    #[test]
    fn a_shape_snaps_to_the_frame_it_lives_in() {
        let (doc, res, frame, inside, _) = frame_and_shapes();
        let view = r(-1000.0, -1000.0, 1000.0, 1000.0);
        let targets = targets(&doc, &res, doc.first_page(), &[inside], view, 200);
        let frame_box = res.world_bounds(frame).unwrap();
        assert!(
            targets.contains(&frame_box),
            "the parent frame {frame_box:?} is not a target: {targets:?}"
        );

        // Left edge 2 units off the frame's own.
        let moving = r(2.0, 200.0, 52.0, 250.0);
        let snap = snap_rect(moving, FREE, &shapes(&targets, 1.0));
        assert!((snap.adjust.x + 2.0).abs() < 1e-9, "{:?}", snap.adjust);
    }

    /// The gesture's own subtree stays out of the targets.
    #[test]
    fn a_gesture_never_snaps_to_what_it_is_carrying() {
        let (doc, res, frame, inside, outside) = frame_and_shapes();
        let view = r(-1000.0, -1000.0, 1000.0, 1000.0);

        // Dragging the frame: neither it nor its shape may appear.
        let targets = targets(&doc, &res, doc.first_page(), &[frame], view, 200);
        for id in [frame, inside] {
            let b = res.world_bounds(id).unwrap();
            assert!(!targets.contains(&b), "{id:?} {b:?} is in {targets:?}");
        }
        // The layer beside the frame is what is left.
        assert_eq!(targets, vec![res.world_bounds(outside).unwrap()]);
    }

    // --- the axis lock -----------------------------------------------------

    /// Shift picks the axis the drag has gone furthest along, and re-picks it as
    /// the drag turns.
    #[test]
    fn shift_locks_the_axis_the_drag_is_mostly_travelling_along() {
        let v = Vec2::new;
        assert_eq!(move_axes(v(40.0, 3.0), true), (true, false));
        assert_eq!(move_axes(v(-40.0, 3.0), true), (true, false), "leftward");
        assert_eq!(move_axes(v(3.0, 40.0), true), (false, true));
        assert_eq!(move_axes(v(3.0, -40.0), true), (false, true), "upward");
        // The tie, and the first frame of every drag, go to x.
        assert_eq!(move_axes(v(0.0, 0.0), true), (true, false));
        assert_eq!(move_axes(v(9.0, 9.0), true), (true, false));
        // Without Shift nothing is constrained.
        assert_eq!(move_axes(v(3.0, 40.0), false), (true, true));

        // The constraint zeroes the other axis.
        assert_eq!(
            constrain(v(40.0, 3.0), move_axes(v(40.0, 3.0), true)),
            v(40.0, 0.0)
        );
    }

    /// A locked axis comes back with no adjustment and no line. The box is 2 from
    /// an alignment on both axes, so a leak in either direction shows up.
    #[test]
    fn a_locked_axis_neither_snaps_nor_explains_itself() {
        let moving = r(102.0, 202.0, 152.0, 252.0);
        let target = r(100.0, 200.0, 300.0, 400.0);

        // Free: both axes pull, two lines.
        let free = snap_rect(moving, FREE, &shapes(&[target], 1.0));
        assert!((free.adjust.x + 2.0).abs() < 1e-9, "{:?}", free.adjust);
        assert!((free.adjust.y + 2.0).abs() < 1e-9, "{:?}", free.adjust);
        assert_eq!(free.marks.guides.len(), 2);

        // Locked to x: y is untouched and unexplained.
        let locked = snap_rect(moving, (true, false), &shapes(&[target], 1.0));
        assert!((locked.adjust.x + 2.0).abs() < 1e-9, "{:?}", locked.adjust);
        assert_eq!(locked.adjust.y, 0.0, "y moved on a locked axis");
        assert_eq!(locked.marks.guides.len(), 1);
        assert_eq!(locked.marks.guides[0].axis, Axis::Vertical);

        // The grid does not sneak in behind it: nothing is in reach on y, so the
        // pixel fallback would otherwise claim it.
        let off_grid = r(102.0, 202.7, 152.0, 252.7);
        let locked = snap_rect(off_grid, (true, false), &grid(1.0));
        assert_eq!(locked.adjust.y, 0.0, "the grid rounded a locked axis");
        // Free, the same box rounds its leading edge up to 203.
        let free = snap_rect(off_grid, FREE, &grid(1.0));
        assert!((free.adjust.y - 0.3).abs() < 1e-9, "{:?}", free.adjust);
    }

    /// `snap_point` rounds to the lattice only when *Snap to grid* is on. Both
    /// axes are checked, since the two arms are separate `else if`s; the grid-on
    /// case is the control.
    ///
    /// flip: drop `s.pixel` from the y arm of `snap_point` → fails at the first
    /// assertion, `(102.4, 204.0)` against `(102.4, 203.6)`.
    #[test]
    fn a_dragged_point_falls_to_the_grid_only_when_the_grid_is_on() {
        let at = Point::new(102.4, 203.6);

        let off = snap_point(at, FREE, &Snapping::none(1.0));
        assert_eq!(
            (off.at.x, off.at.y),
            (102.4, 203.6),
            "every source is off, so nothing may move the handle"
        );

        let on = snap_point(at, FREE, &grid(1.0));
        assert_eq!(
            (on.at.x, on.at.y),
            (102.0, 204.0),
            "and with the grid on it lands on the lattice, both axes"
        );

        // One axis at a time.
        let x_only = snap_point(at, (true, false), &Snapping::none(1.0));
        assert_eq!((x_only.at.x, x_only.at.y), (102.4, 203.6));
        let y_only = snap_point(at, (false, true), &grid(1.0));
        assert_eq!(
            (y_only.at.x, y_only.at.y),
            (102.4, 204.0),
            "a locked axis is untouched even with the grid on"
        );
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> PathSeg {
        PathSeg::Line(ondin_core::kurbo::Line::new(a, b))
    }

    /// Two crossing lines, an X through (50, 50), and one corner anchor at (0, 0),
    /// with the pixel grid on.
    fn an_x() -> Snapping {
        Snapping {
            outlines: Outlines {
                anchors: vec![Point::new(0.0, 0.0)],
                segments: vec![
                    line((0.0, 0.0), (100.0, 100.0)),
                    line((0.0, 100.3), (100.0, 0.3)),
                ],
                centres: vec![Point::new(30.0, 70.0)],
                from: Vec::new(),
            },
            pixel: true,
            ..Snapping::none(1.0)
        }
    }

    /// In reach of several, the first kind wins, not the nearest one: an anchor over
    /// the path it sits on, a crossing over both lines, then a midpoint, a centre,
    /// and last the nearest point on a segment.
    ///
    /// flip: `point_target` asking for midpoints before crossings → fails at
    /// "the crossing beats the midpoints".
    #[test]
    fn a_point_lands_on_the_first_kind_of_target_in_reach() {
        let s = an_x();
        let hit = |x, y| point_target(Point::new(x, y), &s, 6.0).map(|m| m.kind);
        assert_eq!(
            hit(1.0, 1.0),
            Some(PointKind::Anchor),
            "over the line it ends"
        );
        // Both lines' midpoints sit within a pixel of the crossing.
        assert_eq!(
            hit(52.0, 49.0),
            Some(PointKind::Intersection),
            "the crossing beats the midpoints"
        );
        assert_eq!(hit(31.0, 69.0), Some(PointKind::Centre));
        assert_eq!(hit(80.0, 83.0), Some(PointKind::OnPath));
        assert_eq!(hit(80.0, 50.0), None, "nothing in reach");
    }

    /// A point that lands on geometry lands exactly: the crossing at (50.15, 50.15)
    /// is not rounded to the pixel grid, and nothing else is drawn for it.
    #[test]
    fn a_point_on_geometry_is_not_rounded_to_the_grid() {
        let out = snap_point(Point::new(52.0, 49.0), FREE, &an_x());
        let mark = out.mark.expect("landed on the crossing");
        assert!(
            (out.at.x - 50.15).abs() < 1e-9 && (out.at.y - 50.15).abs() < 1e-9,
            "{:?}",
            out.at
        );
        assert_eq!(mark.at, out.at);
        assert!(out.guides.is_empty());

        // Constrained to one axis, no point snap: it would move the locked one.
        let locked = snap_point(Point::new(52.0, 49.0), (true, false), &an_x());
        assert!(locked.mark.is_none());
        assert_eq!(locked.at.y, 49.0);
    }

    /// Out of reach of every target, a point still lines up with an anchor's x or y,
    /// with a line drawn from the anchor to it.
    #[test]
    fn a_point_lines_up_with_an_anchor() {
        let s = Snapping {
            outlines: Outlines {
                anchors: vec![Point::new(10.0, 10.0)],
                ..Outlines::default()
            },
            ..Snapping::none(1.0)
        };
        let out = snap_point(Point::new(12.0, 200.0), FREE, &s);
        assert_eq!(out.at, Point::new(10.0, 200.0));
        assert!(out.mark.is_none());
        let [g] = out.guides.as_slice() else {
            panic!("one line: {:?}", out.guides)
        };
        assert_eq!(
            (g.axis, g.at, g.from, g.to),
            (Axis::Vertical, 10.0, 10.0, 200.0)
        );
    }

    /// A circle of radius 50 about the origin, as kurbo's four cubics, which are
    /// near enough round that a tangent lands within 0.1 of the true circle's.
    /// Four, so no joint between them lies near a probe.
    fn a_circle() -> Vec<PathSeg> {
        use ondin_core::kurbo::{Circle, Shape};
        Circle::new((0.0, 0.0), 50.0)
            .to_path(0.1)
            .segments()
            .collect()
    }

    /// Drawn from (100, 0), a point near the circle lands where the line from there
    /// touches it — (25, ±43.30), where the radius meets the line at a right angle
    /// — and only with a `from`: a first anchor has no line to touch with.
    ///
    /// flip: `tangents` returning nothing → fails at the first kind assertion.
    #[test]
    fn a_point_lands_where_a_line_from_the_last_anchor_touches_a_curve() {
        let from = Point::new(100.0, 0.0);
        let mut s = Snapping {
            outlines: Outlines {
                segments: a_circle(),
                from: vec![from],
                ..Outlines::default()
            },
            ..Snapping::none(1.0)
        };
        let mark = point_target(Point::new(27.0, 41.0), &s, 6.0).expect("lands on the tangent");
        assert_eq!(mark.kind, PointKind::Tangent);
        let touch = mark.at;
        assert!(touch.distance(Point::new(25.0, 43.301)) < 0.1, "{touch:?}");
        // Against the cubics themselves, exact: the line runs along the curve there.
        let (seg, t) = a_circle()
            .into_iter()
            .map(|seg| (seg, seg.nearest(touch, 1e-12)))
            .min_by(|a, b| a.1.distance_sq.total_cmp(&b.1.distance_sq))
            .map(|(seg, n)| (seg, n.t))
            .unwrap();
        let PathSeg::Cubic(c) = seg else { panic!() };
        let along = c.deriv().eval(t).to_vec2().normalize();
        assert!(along.cross((from - touch).normalize()).abs() < 1e-9);

        s.outlines.from.clear();
        let bare = point_target(Point::new(27.0, 41.0), &s, 6.0).map(|m| m.kind);
        assert_eq!(bare, Some(PointKind::OnPath), "no tangent without a from");
    }

    /// Drawn from (30, 80), a point near the line along y = 0 lands on its foot at
    /// (30, 0), over the nearer point on the path; a foot off the segment's ends is
    /// no target, and a curve's foot is where the line meets its normal.
    ///
    /// flip: `point_target` asking for points on path before feet → fails at
    /// "the foot beats the point on path".
    #[test]
    fn a_point_lands_on_the_foot_of_a_perpendicular() {
        let mut s = Snapping {
            outlines: Outlines {
                segments: vec![line((0.0, 0.0), (100.0, 0.0))],
                from: vec![Point::new(30.0, 80.0)],
                ..Outlines::default()
            },
            ..Snapping::none(1.0)
        };
        let mark = point_target(Point::new(34.0, 1.0), &s, 6.0);
        assert_eq!(
            mark,
            Some(PointMark {
                at: Point::new(30.0, 0.0),
                kind: PointKind::Perpendicular
            }),
            "the foot beats the point on path"
        );

        s.outlines.from = vec![Point::new(130.0, 80.0)];
        let off = point_target(Point::new(98.0, 1.0), &s, 6.0).map(|m| m.kind);
        assert_eq!(off, Some(PointKind::OnPath), "a foot past the end");

        // From 100 out at 30°, the normal meets the circle 50 out at 30°.
        let (cos, sin) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        s.outlines.segments = a_circle();
        s.outlines.from = vec![Point::new(100.0 * cos, 100.0 * sin)];
        let foot = point_target(Point::new(45.0, 27.0), &s, 6.0).expect("a foot");
        assert_eq!(foot.kind, PointKind::Perpendicular);
        let want = Point::new(50.0 * cos, 50.0 * sin);
        assert!(foot.at.distance(want) < 0.05, "{:?}", foot.at);
    }

    /// A curve and a line cross where both are: every crossing found lies on both,
    /// to well under a pixel, and a curve crossed twice gives two.
    #[test]
    fn a_curve_and_a_line_cross_where_both_are() {
        use ondin_core::kurbo::{CubicBez, ParamCurveNearest};
        let arch = PathSeg::Cubic(CubicBez::new(
            (0.0, 100.0),
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
        ));
        let across = line((-10.0, 40.0), (110.0, 40.0));
        let mut found = Vec::new();
        crossings(arch, across, &mut found);
        assert_eq!(found.len(), 2, "{found:?}");
        for p in found {
            assert!((p.y - 40.0).abs() < 1e-5, "{p:?}");
            assert!(arch.nearest(p, 1e-12).distance_sq < 1e-10, "{p:?}");
        }
    }
}
