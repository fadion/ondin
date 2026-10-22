//! Per-kind local geometry: bounds and exact hit-testing (§5.10).
//!
//! Owns the math the queries in `query.rs` rely on. Local bounds are in the
//! node's own coordinate space; the caller transforms them to world space.
//! Containment tests also run in local space (the caller maps the world point
//! through the inverse world transform first).

use crate::node::{Node, NodeKind, Paint, Pivot, Side, Stroke, StrokeSides};
use crate::text::TextLayout;
use kurbo::{
    Affine, Arc, BezPath, Cap, CubicBez, Ellipse, Line, ParamCurve, ParamCurveArclen,
    ParamCurveDeriv, ParamCurveNearest, PathEl, PathSeg, Point, QuadBez, Rect, RoundedRect,
    RoundedRectRadii, Shape, Size, Vec2,
};

/// Flattening tolerance for the curve→path conversions below.
const TOLERANCE: f64 = 0.1;

/// Below this two points are the same point, and a vector is no direction.
const EPS: f64 = 1e-9;

/// One subpath of a `BezPath`, as anchors *and the segments between them*.
///
/// This is the canonical anchor numbering: a `Path` carries a radius per anchor
/// (`NodeKind::Path::corner_radii`), so the order is part of the saved model.
/// `tools::pen_anchors` produces the same order, and
/// `the_app_and_core_number_a_paths_anchors_alike` keeps the two from drifting.
///
/// The real `PathSeg`s are carried, not just endpoints, so rounding re-emits an
/// untouched segment as itself instead of flattening curves to lines.
#[derive(Clone, Debug, PartialEq)]
pub struct AnchorRun {
    /// The anchor positions, in order.
    pub points: Vec<Point>,
    /// The segment **leaving** anchor `i`. A closed run has one per anchor, the
    /// last closing back to anchor 0; an open run has one fewer.
    pub segs: Vec<PathSeg>,
    pub closed: bool,
}

/// Split `path` into the anchor runs a per-anchor value is indexed by.
///
/// Mirrors `tools::pen_anchors`' rule exactly, including the one subtlety: a
/// `ClosePath` whose last point is already the first **folds**, because that is
/// what the pen's own writer emits and counting it would give every closed path
/// one anchor more than it has. A `ClosePath` that does *not* land on the start
/// gains the closing segment SVG implies.
pub fn anchor_runs(path: &BezPath) -> Vec<AnchorRun> {
    let mut out: Vec<AnchorRun> = Vec::new();
    let mut cur = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.push(AnchorRun {
                    points: vec![p],
                    segs: Vec::new(),
                    closed: false,
                });
                cur = p;
            }
            PathEl::LineTo(p) => {
                if let Some(run) = out.last_mut() {
                    run.segs.push(PathSeg::Line(Line::new(cur, p)));
                    run.points.push(p);
                    cur = p;
                }
            }
            PathEl::QuadTo(c, p) => {
                if let Some(run) = out.last_mut() {
                    run.segs.push(PathSeg::Quad(QuadBez::new(cur, c, p)));
                    run.points.push(p);
                    cur = p;
                }
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(run) = out.last_mut() {
                    run.segs.push(PathSeg::Cubic(CubicBez::new(cur, c1, c2, p)));
                    run.points.push(p);
                    cur = p;
                }
            }
            PathEl::ClosePath => {
                if let Some(run) = out.last_mut() {
                    run.closed = true;
                    let start = run.points[0];
                    if run.points.len() >= 2 {
                        if (run.points[run.points.len() - 1] - start).hypot() < EPS {
                            // The repeated point folds; the segment that reached
                            // it *is* the closing one.
                            run.points.pop();
                        } else {
                            run.segs.push(PathSeg::Line(Line::new(cur, start)));
                        }
                    }
                    cur = start;
                }
            }
        }
    }
    out
}

/// The number of anchors in `path` — the length a `corner_radii` list is
/// measured against.
pub fn anchor_count(path: &BezPath) -> usize {
    anchor_runs(path).iter().map(|r| r.points.len()).sum()
}

/// Where a pointer landed on a path, named in [`anchor_runs`]' numbering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentHit {
    /// Which anchor run — the subpath index.
    pub subpath: usize,
    /// The anchor the segment **leaves** — the same index a corner radius is
    /// stored at.
    pub anchor: usize,
    /// Parameter along the segment, 0..=1.
    pub t: f64,
    /// The point on the segment itself, and how far the query point was from it.
    pub at: Point,
    pub distance: f64,
}

/// The segment of `path` nearest `p`.
///
/// Scans [`AnchorRun`] rather than `BezPath::segments`, which has no subpath
/// boundaries or anchor numbers, so the result is already in the canonical numbering.
///
/// `None` for a path with no segments at all. A closed run includes its closing
/// segment; an open one has none leaving its last anchor, hence the answer names
/// the anchor a segment *leaves*.
pub fn nearest_segment(path: &BezPath, p: Point) -> Option<SegmentHit> {
    let mut best: Option<SegmentHit> = None;
    for (subpath, run) in anchor_runs(path).iter().enumerate() {
        for (anchor, seg) in run.segs.iter().enumerate() {
            let near = seg.nearest(p, NEAREST_ACC);
            let hit = SegmentHit {
                subpath,
                anchor,
                t: near.t,
                at: seg.eval(near.t),
                distance: near.distance_sq.max(0.0).sqrt(),
            };
            if best.is_none_or(|b| hit.distance < b.distance) {
                best = Some(hit);
            }
        }
    }
    best
}

/// Accuracy for the nearest-point solves [`nearest_segment`] runs; well under a
/// device pixel at any sane zoom.
const NEAREST_ACC: f64 = 1e-3;

/// Accuracy for the arc-length solves that trim a segment back from a corner.
const ARCLEN_ACC: f64 = 1e-4;

/// `path` with a fillet cut into each corner that has a radius.
///
/// Every corner is rounded whatever its neighbours are; the legs are trimmed back
/// **by arc length**, so a curved leg is shortened rather than replaced.
///
/// Between two straight legs the fillet is an exact circular arc (agreeing with
/// `RoundedRect`). Against a curve there is generally no tangent circle, so the
/// join is a cubic meeting each side along its own tangent.
///
/// An endpoint of an open subpath is never rounded, and a radius on a **smooth**
/// anchor does nothing, because the tangents are collinear.
///
/// **Each radius is clamped to half of each adjoining segment**, so two rounded
/// corners sharing a short edge cannot eat past each other and invert the outline.
pub fn round_corners(path: &BezPath, radii: &[f64]) -> BezPath {
    // `any(> 0)` rather than `all(<= 0)`: they are not each other's negation
    // with `NaN` (see the guard in `round_run`).
    if !radii.iter().any(|r| *r > 0.0) {
        return path.clone();
    }
    let mut out = BezPath::new();
    let mut base = 0usize;
    for run in anchor_runs(path) {
        round_run(&mut out, &run, &radii[base.min(radii.len())..]);
        base += run.points.len();
    }
    out
}

/// One anchor's fillet: where it leaves the incoming segment, where it rejoins
/// the outgoing one, and how to bridge the two.
struct Fillet {
    /// Parameter on the incoming segment to cut at, and on the outgoing one to
    /// resume from.
    u_in: f64,
    u_out: f64,
    from: Point,
    to: Point,
    /// Unit tangents at `from` and `to`, pointing the way the path travels.
    d_from: Vec2,
    d_to: Vec2,
    /// Both legs straight — then the bridge is an exact circular arc about this
    /// corner.
    straight: Option<Point>,
}

fn round_run(out: &mut BezPath, run: &AnchorRun, radii: &[f64]) {
    let (n, ns) = (run.points.len(), run.segs.len());
    if ns == 0 {
        if n == 1 {
            out.move_to(run.points[0]);
        }
        return;
    }
    let mut fillets: Vec<Option<Fillet>> = (0..n).map(|_| None).collect();
    let mut trim_start = vec![0.0f64; ns];
    let mut trim_end = vec![1.0f64; ns];
    // Each segment's arc length is computed at most once: an interior segment
    // borders two corners and a cubic's `arclen` is a numeric integration, on a
    // per-frame path. Lazy rather than a pre-pass, so one rounded corner on a long
    // path does not cost what rounding every corner costs.
    let mut lens: Vec<Option<f64>> = vec![None; ns];
    if n >= 3 {
        for i in 0..n {
            // The two segments meeting at anchor `i`: the one arriving and the
            // one leaving. An open run's endpoints have only one and are skipped.
            let (ia, ib) = if run.closed {
                ((i + n - 1) % n, i)
            } else if i == 0 || i + 1 == n {
                continue;
            } else {
                (i - 1, i)
            };
            let (Some(sa), Some(sb)) = (run.segs.get(ia), run.segs.get(ib)) else {
                continue;
            };
            // Hoisted out of `fillet` so the memo below is only filled for a
            // corner that is going to be cut.
            let radius = radii.get(i).copied().unwrap_or(0.0);
            // `NaN` must be rejected explicitly: `radius <= 0.0` is false for it,
            // and `fillet`'s `f64::min` clamp returns the non-`NaN` operand, so a
            // `NaN` radius would draw the maximum fillet. `inf` stays accepted
            // (it clamps like any oversized radius). `is_nan()` rather than
            // `!(radius > 0.0)`, which clippy rejects.
            if radius.is_nan() || radius <= 0.0 {
                continue;
            }
            let la = *lens[ia].get_or_insert_with(|| run.segs[ia].arclen(ARCLEN_ACC));
            let lb = *lens[ib].get_or_insert_with(|| run.segs[ib].arclen(ARCLEN_ACC));
            let Some(f) = fillet(sa, sb, radius, la, lb) else {
                continue;
            };
            trim_end[ia] = f.u_in;
            trim_start[ib] = f.u_out;
            // Indexed, not pushed: the loop skips anchors, so position matters.
            if let Some(slot) = fillets.get_mut(i) {
                *slot = Some(f);
            }
        }
    }

    let start = fillets[0].as_ref().map_or(run.points[0], |f| f.to);
    out.move_to(start);
    for k in 0..ns {
        // A segment cut at both ends by its two corners. The clamp to half its
        // length is what guarantees the range is still ascending.
        if trim_start[k] < trim_end[k] {
            push_seg(out, run.segs[k].subsegment(trim_start[k]..trim_end[k]));
        }
        let j = if run.closed { (k + 1) % n } else { k + 1 };
        if let Some(f) = fillets.get(j).and_then(|f| f.as_ref()) {
            bridge(out, f);
        }
    }
    if run.closed {
        out.close_path();
    }
}

/// The fillet at the corner where `sa` arrives and `sb` leaves, or `None` when
/// there is no corner there to cut.
///
/// `la` and `lb` are the two legs' arc lengths, passed in because `round_run`
/// memoizes them; the caller has already established `radius > 0.0`.
fn fillet(sa: &PathSeg, sb: &PathSeg, radius: f64, la: f64, lb: f64) -> Option<Fillet> {
    let corner = sa.eval(1.0);
    // The directions the two legs actually leave the corner in — tangents, so a
    // curved leg is measured by where it is going rather than by where its far
    // end happens to be.
    let a = -unit_tangent(sa, 1.0)?;
    let b = unit_tangent(sb, 0.0)?;
    let theta = a.dot(b).clamp(-1.0, 1.0).acos();
    // Collinear either way: a straight-through vertex, or a **smooth** anchor,
    // which is exactly why a radius on one draws nothing.
    if theta < 1e-6 || (std::f64::consts::PI - theta).abs() < 1e-6 {
        return None;
    }
    let t = (radius / (theta * 0.5).tan()).min(la * 0.5).min(lb * 0.5);
    if t < EPS {
        return None;
    }
    let (u_in, u_out) = (
        sa.inv_arclen(la - t, ARCLEN_ACC),
        sb.inv_arclen(t, ARCLEN_ACC),
    );
    let straight = matches!((sa, sb), (PathSeg::Line(_), PathSeg::Line(_))).then_some(corner);
    Some(Fillet {
        u_in,
        u_out,
        from: sa.eval(u_in),
        to: sb.eval(u_out),
        d_from: unit_tangent(sa, u_in)?,
        d_to: unit_tangent(sb, u_out)?,
        straight,
    })
}

/// The unit tangent of `seg` at `t`, pointing along the direction of travel.
fn unit_tangent(seg: &PathSeg, t: f64) -> Option<Vec2> {
    let d = match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
    };
    let len = d.hypot();
    (len > EPS).then(|| d / len)
}

fn push_seg(out: &mut BezPath, seg: PathSeg) {
    match seg {
        PathSeg::Line(l) => out.line_to(l.p1),
        PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
        PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
    }
}

/// Bridge the gap a fillet cut.
fn bridge(out: &mut BezPath, f: &Fillet) {
    if let Some(corner) = f.straight {
        arc_to(out, f.from, f.to, corner);
        return;
    }
    // Against a curve there is generally no circle tangent to both legs at the
    // trim points, so the bridge is the cubic that leaves each side along its own
    // tangent. Its control points sit on the tangent intersection at the fraction
    // a circular arc's would — the straight case comes out the familiar 0.5523,
    // so this *is* that construction rather than a second idea beside it.
    let cross = f.d_from.x * f.d_to.y - f.d_from.y * f.d_to.x;
    let d = f.to - f.from;
    if cross.abs() < EPS {
        out.line_to(f.to);
        return;
    }
    let s = (d.x * f.d_to.y - d.y * f.d_to.x) / cross;
    let meet = f.from + f.d_from * s;
    let phi = f.d_from.dot(f.d_to).clamp(-1.0, 1.0).acos();
    let half = (phi * 0.5).tan();
    if phi < 1e-6 || half.abs() < EPS {
        out.line_to(f.to);
        return;
    }
    let k = (4.0 / 3.0) * (phi * 0.25).tan() / half;
    out.curve_to(f.from + (meet - f.from) * k, f.to + (meet - f.to) * k, f.to);
}

/// A circular arc from `from` to `to`, turning about the corner at `corner`.
fn arc_to(out: &mut BezPath, from: Point, to: Point, corner: Point) {
    // The centre lies along the corner's bisector, at the distance that puts it
    // `radius` from both tangent points. Derived from the tangent points rather
    // than carried, so the arc cannot disagree with the trim.
    let (a, b) = (from - corner, to - corner);
    let bis = a / a.hypot() + b / b.hypot();
    let bl = bis.hypot();
    if bl < EPS {
        out.line_to(to);
        return;
    }
    let bis = bis / bl;
    let half = (a.dot(b) / (a.hypot() * b.hypot())).clamp(-1.0, 1.0).acos() * 0.5;
    let t = a.hypot();
    let radius = t * half.tan();
    let centre = corner + bis * (t / half.cos());
    let start = (from - centre).atan2();
    let end = (to - centre).atan2();
    // The short way round: a fillet never turns more than half a circle.
    let mut sweep = end - start;
    while sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    while sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    let arc = Arc::new(centre, (radius, radius), start, sweep, 0.0);
    // `append_iter` emits curves only — the `MoveTo` is already down.
    arc.append_iter(TOLERANCE).for_each(|el| match el {
        PathEl::MoveTo(_) => {}
        el => out.push(el),
    });
}

/// The node's outline in its own local space. `None` for the pure containers, for
/// text and for a boolean.
///
/// One definition: the scene walk strokes and fills it and the SVG writer uses it
/// verbatim as the `clipPath` for aligned strokes, so two copies would drift.
///
/// A `Boolean` answers `None` because its outline is a function of its subtree,
/// not its kind; read [`crate::Resolved::local_path`], which returns the cached
/// result for a boolean and defers to this otherwise. A `Text` node's outline is
/// its glyphs', built by [`crate::text::outline`] from the cached `TextLayout`.
pub fn local_path(kind: &NodeKind) -> Option<BezPath> {
    match kind {
        // A frame's outline is its own box, square-cornered through the same
        // `Rect::to_path` a square rect uses; its stroke, alignment clip and
        // sides are built from it.
        NodeKind::Artboard { size, .. } => {
            Some(Rect::new(0.0, 0.0, size.width, size.height).to_path(TOLERANCE))
        }
        NodeKind::Rect { size, corner_radii } => {
            let rect = Rect::new(0.0, 0.0, size.width, size.height);
            // `from_rect` clamps each radius to half the shorter side, so an
            // over-large corner rounds as far as it can rather than turning the
            // outline inside out.
            Some(if is_square_cornered(corner_radii) {
                rect.to_path(TOLERANCE)
            } else {
                RoundedRect::from_rect(rect, *corner_radii).to_path(TOLERANCE)
            })
        }
        // Closed by hand, because `Ellipse::to_path` emits four cubics and no
        // `ClosePath`. A fill cannot tell, but an unclosed stroke is an open path:
        // its ends are capped rather than joined, and a wide open expansion differs
        // from a closed one. `scene::lay_stroke` fixes the wide-stroke hole itself.
        NodeKind::Ellipse { size } => {
            let mut path = Ellipse::new(
                (size.width * 0.5, size.height * 0.5),
                (size.width * 0.5, size.height * 0.5),
                0.0,
            )
            .to_path(TOLERANCE);
            path.close_path();
            Some(path)
        }
        NodeKind::Polygon { size, sides } => Some(star_path(*size, *sides, None)),
        NodeKind::Star {
            size,
            points,
            inner_ratio,
        } => Some(star_path(*size, *points, Some(*inner_ratio))),
        NodeKind::Line { end } => Some(BezPath::from_vec(vec![
            PathEl::MoveTo(Point::ZERO),
            PathEl::LineTo(*end),
        ])),
        // The one place a path's corner radii become geometry; every consumer
        // reaches it through here or `Resolved::local_path`.
        NodeKind::Path { path, corner_radii } => Some(round_corners(path, corner_radii)),
        // Both have an outline this argument cannot build (see above).
        NodeKind::Root | NodeKind::Group | NodeKind::Text { .. } | NodeKind::Boolean { .. } => None,
    }
}

/// Fewest sides that still enclose an area.
pub const MIN_SIDES: u32 = 3;
/// Most sides the UI offers. Past this a polygon is a circle with a slow
/// tessellation, and the field becomes a way to make the app crawl.
pub const MAX_SIDES: u32 = 64;
/// Narrowest a star's inner radius may get. At zero the spokes are hairlines
/// with no interior, which is a shape no fill can show.
pub const MIN_INNER_RATIO: f64 = 0.05;

/// The outline shared by [`NodeKind::Polygon`] and [`NodeKind::Star`]: `n`
/// vertices evenly around a circle, first one at the top, **stretched to fill
/// `size` exactly**.
///
/// `inner` turns it into a star — every other vertex is pulled in to that
/// fraction of the radius, so a polygon is the case with no inner ring.
///
/// **Filling the box is not cosmetic.** `local_bounds` reports `0,0..w,h` for
/// these kinds and the selection rect, snap targets and resize handles come from
/// it; a merely inscribed triangle spans three quarters of its box's height.
///
/// Counts are clamped here rather than on the way into the model, so dragging the
/// sides field down through 2 and back does not lose the number the user wanted.
fn star_path(size: Size, n: u32, inner: Option<f64>) -> BezPath {
    let n = n.clamp(MIN_SIDES, MAX_SIDES);
    let vertices = match inner {
        Some(_) => n * 2,
        None => n,
    };
    let step = std::f64::consts::TAU / f64::from(vertices);
    let start = -std::f64::consts::FRAC_PI_2;

    let unit: Vec<Point> = (0..vertices)
        .map(|i| {
            let t = start + step * f64::from(i);
            let r = match inner {
                Some(ratio) if i % 2 == 1 => ratio.clamp(MIN_INNER_RATIO, 1.0),
                _ => 1.0,
            };
            Point::new(r * t.cos(), r * t.sin())
        })
        .collect();

    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &unit {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let sx = if max_x > min_x {
        size.width / (max_x - min_x)
    } else {
        0.0
    };
    let sy = if max_y > min_y {
        size.height / (max_y - min_y)
    } else {
        0.0
    };

    let mut path = BezPath::new();
    for (i, p) in unit.iter().enumerate() {
        let at = Point::new((p.x - min_x) * sx, (p.y - min_y) * sy);
        if i == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    path.close_path();
    path
}

/// Whether a rect's radii round nothing — every corner at or below zero.
pub fn is_square_cornered(radii: &RoundedRectRadii) -> bool {
    let RoundedRectRadii {
        top_left,
        top_right,
        bottom_right,
        bottom_left,
    } = *radii;
    top_left <= 0.0 && top_right <= 0.0 && bottom_right <= 0.0 && bottom_left <= 0.0
}

/// The single radius all four corners share, or `None` when they differ.
///
/// The SVG writer asks this: `<rect rx>` can only say one number, so a rect
/// with unequal corners has to be emitted as a path instead.
pub fn uniform_radius(radii: &RoundedRectRadii) -> Option<f64> {
    let RoundedRectRadii {
        top_left,
        top_right,
        bottom_right,
        bottom_left,
    } = *radii;
    (top_left == top_right && top_right == bottom_right && bottom_right == bottom_left)
        .then_some(top_left)
}

/// Whether every subpath of `path` closes — i.e. whether it has an interior for
/// an inside/outside stroke to sit in.
///
/// A subpath counts as closed if it ends in `ClosePath` **or** its last point
/// lands back on its first; testing only the marker would say ellipses have no
/// inside.
pub fn is_closed(path: &BezPath) -> bool {
    /// Slack for "lands back on its first point", in local units.
    const EPS: f64 = 1e-6;

    let mut subpaths = 0usize;
    let mut closed = 0usize;
    let mut start = Point::ZERO;
    let mut last = Point::ZERO;
    // A subpath has begun and has not yet been settled as closed.
    let mut pending = false;

    for el in path.iter() {
        match el {
            PathEl::MoveTo(p) => {
                if pending && (last - start).hypot() <= EPS {
                    closed += 1;
                }
                subpaths += 1;
                pending = true;
                start = p;
                last = p;
            }
            PathEl::LineTo(p) => last = p,
            PathEl::QuadTo(_, p) => last = p,
            PathEl::CurveTo(_, _, p) => last = p,
            PathEl::ClosePath => {
                if pending {
                    closed += 1;
                    pending = false;
                }
                last = start;
            }
        }
    }
    if pending && (last - start).hypot() <= EPS {
        closed += 1;
    }
    subpaths > 0 && subpaths == closed
}

/// Whether a node's stroke alignment can be honoured at all: it needs a closed
/// outline to have an inside.
///
/// **A `Boolean` answers yes without being measured**, because [`local_path`]
/// cannot build its outline and the test below would read "not closed". Every
/// subpath `boolean::from_flo` emits is closed, with winding normalized against
/// nesting depth, so an inside is well defined even with holes.
///
/// A `Text` node answers yes for the same reason: its outline is its glyphs'
/// ([`crate::text::outline`]) and every font contour is closed.
pub fn stroke_align_applies(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Boolean { .. } | NodeKind::Text { .. } => true,
        NodeKind::Line { .. } => false,
        kind => local_path(kind).is_some_and(|p| is_closed(&p)),
    }
}

/// Whether a node's outline has corners for a join to shape.
///
/// An **ellipse** is four tangent cubics, so every "corner" is already smooth; a
/// **line** has no interior vertex. Miter, Round and Bevel draw the same on either.
///
/// **Gated on the kind, not on the geometry**, so the control does not appear and
/// disappear while a rect's radius field is being scrubbed.
pub fn stroke_join_applies(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Rect { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Path { .. }
        // Its outline is arbitrary: wherever two operands' edges cross is a
        // vertex, and it is usually the sharpest one on the page.
        | NodeKind::Boolean { .. }
        // A frame is a box, so it has the four corners a rect has.
        | NodeKind::Artboard { .. }
        // Glyph outlines are full of corners.
        | NodeKind::Text { .. } => true,
        // No corners: smooth throughout, and no interior vertex respectively.
        NodeKind::Ellipse { .. } | NodeKind::Line { .. } => false,
        // Nothing here strokes at all (§6.3), so there is no join to shape.
        NodeKind::Root | NodeKind::Group => false,
    }
}

/// Whether a node's stroke can be put on one named side: it needs four of them.
///
/// **The two boxes only.** An ellipse, polygon, star, path and text have no four
/// edges, and a `Line` has no sides at all.
pub fn stroke_sides_apply(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Rect { .. } | NodeKind::Artboard { .. })
}

/// The sides a stroke actually occupies on this kind — [`StrokeSides::All`]
/// wherever the field cannot be honoured.
///
/// The stored value stays in the model untouched; every consumer asks this rather
/// than reading `sides`, so a rect that becomes an ellipse draws its whole outline.
pub fn effective_sides(kind: &NodeKind, stroke: &Stroke) -> StrokeSides {
    if stroke_sides_apply(kind) {
        stroke.sides
    } else {
        StrokeSides::All
    }
}

/// The **open** outline of one side of a rect, with each corner arc split at its
/// midpoint so the top edge owns half of each top corner.
///
/// A per-side stroke is drawn along this. Splitting at the midpoint matches CSS
/// `border-top` against `border-radius`: a rounded corner belongs half to each side
/// sharing it, where cutting at the straight edge would leave corners bare.
///
/// The radii come back off [`RoundedRect::from_rect`] rather than being clamped
/// here, so the arc is the same curve the fill uses. A corner with no radius
/// contributes its sharp vertex.
///
/// `None` for any kind [`stroke_sides_apply`] rejects.
pub fn side_path(kind: &NodeKind, side: Side) -> Option<BezPath> {
    let (size, corner_radii) = match kind {
        NodeKind::Rect { size, corner_radii } => (*size, *corner_radii),
        // A frame is the same box with no radii, so its four sides are four
        // straight edges meeting at sharp vertices — the `radius == 0` branch
        // below, which a square-cornered rect already takes.
        NodeKind::Artboard { size, .. } => (*size, RoundedRectRadii::from_single_radius(0.0)),
        _ => return None,
    };
    let rect = Rect::new(0.0, 0.0, size.width, size.height);
    let radii = if is_square_cornered(&corner_radii) {
        RoundedRectRadii::from_single_radius(0.0)
    } else {
        RoundedRect::from_rect(rect, corner_radii).radii()
    };
    let corners = corners(size, radii);
    // The corner this side starts at — taking its *second* half — and the one it
    // runs into, taking that one's first half.
    let (from, to) = match side {
        Side::Top => (0, 1),
        Side::Right => (1, 2),
        Side::Bottom => (2, 3),
        Side::Left => (3, 0),
    };
    const HALF_QUARTER: f64 = std::f64::consts::FRAC_PI_4;

    let mut path = BezPath::new();
    let a = &corners[from];
    if a.radius > 0.0 {
        append_arc(
            &mut path,
            &Arc::new(
                a.center,
                (a.radius, a.radius),
                a.start + HALF_QUARTER,
                HALF_QUARTER,
                0.0,
            ),
        );
    } else {
        path.move_to(a.vertex);
    }
    let b = &corners[to];
    if b.radius > 0.0 {
        // `append_arc`'s leading `line_to` **is** this side's straight edge: it
        // runs from where the first arc left off to where the second begins.
        append_arc(
            &mut path,
            &Arc::new(b.center, (b.radius, b.radius), b.start, HALF_QUARTER, 0.0),
        );
    } else if path.elements().is_empty() {
        path.move_to(b.vertex);
    } else {
        path.line_to(b.vertex);
    }
    Some(path)
}

/// One of a rect's corners as the arc that rounds it, plus the sharp vertex it
/// falls back to at zero radius.
struct Corner {
    center: Point,
    radius: f64,
    /// Where the arc begins, sweeping a quarter turn clockwise **on screen** —
    /// so `+y` is down and an angle of 180° is the point left of the centre.
    start: f64,
    vertex: Point,
}

/// The four corners, top-left first and clockwise from there.
fn corners(size: Size, radii: RoundedRectRadii) -> [Corner; 4] {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    let (w, h) = (size.width, size.height);
    let RoundedRectRadii {
        top_left,
        top_right,
        bottom_right,
        bottom_left,
    } = radii;
    [
        Corner {
            center: Point::new(top_left, top_left),
            radius: top_left,
            start: PI,
            vertex: Point::new(0.0, 0.0),
        },
        Corner {
            center: Point::new(w - top_right, top_right),
            radius: top_right,
            start: PI + FRAC_PI_2,
            vertex: Point::new(w, 0.0),
        },
        Corner {
            center: Point::new(w - bottom_right, h - bottom_right),
            radius: bottom_right,
            start: TAU,
            vertex: Point::new(w, h),
        },
        Corner {
            center: Point::new(bottom_left, h - bottom_left),
            radius: bottom_left,
            start: FRAC_PI_2,
            vertex: Point::new(0.0, h),
        },
    ]
}

/// Append `arc` to `path`, opening a subpath if there is none yet and drawing a
/// straight line to the arc's start if there is.
fn append_arc(path: &mut BezPath, arc: &Arc) {
    for (i, el) in arc.path_elements(TOLERANCE).enumerate() {
        match el {
            // An `Arc` leads with a `MoveTo`; mid-path that start must be
            // *reached*, not jumped to. `elements()`, not `BezPath::is_empty`,
            // which counts segments: a lone `MoveTo` is empty by it and would get
            // a second `MoveTo`, leaving a side with no ink.
            PathEl::MoveTo(p) if i == 0 => {
                if path.elements().is_empty() {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            el => path.push(el),
        }
    }
}

/// The dash pattern a backend should draw `stroke` with along `path`, at the
/// `width` actually being laid down (which is the stroke's own except on a
/// `Custom` side).
///
/// Two things the model leaves until here, because both need numbers it does not
/// carry (§5.3):
///
/// - **A dot is a zero-length dash**, which draws nothing under `Cap::Butt` or
///   `Cap::Square`. Those get a square dot one width long, with the gap shortened
///   so the **period is preserved**.
/// - **Fit to corners** scales the pattern so a whole number of periods spans the
///   outline, which needs the path's length.
pub fn resolved_dashes(stroke: &Stroke, width: f64, path: &BezPath) -> Vec<f64> {
    if stroke.dashes.is_empty() {
        return Vec::new();
    }
    let mut pattern = stroke.dashes.clone();
    if let [length, gap] = pattern.as_mut_slice()
        && *length <= 0.0
        && stroke.cap != Cap::Round
    {
        let period = *gap;
        *length = width.min(period);
        *gap = (period - width).max(0.0);
    }
    if stroke.dash_fit {
        pattern = fitted_dashes(&pattern, path.perimeter(TOLERANCE));
    }
    pattern
}

/// The pieces a stroke has to be laid along for its dash pattern to be right, each
/// with the pattern resolved for it — or `None` when the whole path is one piece and
/// [`resolved_dashes`] answers on its own.
///
/// **Fitting is per subpath**: subpaths of different lengths need different scales
/// to land on their own corners, and fitting the total drifts by up to half a
/// period on each.
///
/// `None` for a solid stroke, an unfitted pattern or a single subpath, so the
/// common case clones no path and measures no perimeter twice. Each subpath is
/// fitted against its own `perimeter`, restarting phase there as `dash_offset`
/// already does.
pub fn dash_fit_pieces(
    stroke: &Stroke,
    width: f64,
    path: &BezPath,
) -> Option<Vec<(BezPath, Vec<f64>)>> {
    if !stroke.dash_fit || stroke.dashes.is_empty() {
        return None;
    }
    let pieces = subpaths(path);
    if pieces.len() < 2 {
        return None;
    }
    Some(
        pieces
            .into_iter()
            .map(|sub| {
                let dashes = resolved_dashes(stroke, width, &sub);
                (sub, dashes)
            })
            .collect(),
    )
}

/// `path`'s subpaths, one per `MoveTo`.
///
/// Shared by the scene walk, the SVG writer and [`dash_fit_pieces`], which all need
/// the same split.
pub fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) {
            out.push(BezPath::new());
        }
        if let Some(last) = out.last_mut() {
            last.push(*el);
        }
    }
    out
}

/// `dashes` scaled so a whole number of periods spans `length` exactly.
///
/// Rounds to the *nearest* whole number of periods rather than the floor, so each
/// dash moves by at most half a period. `length` is one subpath's perimeter (see
/// [`dash_fit_pieces`]).
pub fn fitted_dashes(dashes: &[f64], length: f64) -> Vec<f64> {
    let period: f64 = dashes.iter().sum();
    // `is_finite` covers the NaN a bad pattern could sum to; the comparison then
    // only has to reject zero and negative periods, which have no scale.
    if !period.is_finite() || period <= 0.0 || !length.is_finite() || length <= 0.0 {
        return dashes.to_vec();
    }
    let n = (length / period).round().max(1.0);
    let scale = length / (n * period);
    dashes.iter().map(|d| d * scale).collect()
}

/// Local (untransformed) bounding box of a node's geometry. `None` for pure
/// containers (Group/Root), whose bounds are the union of their children.
///
/// `text` is the node's cached layout from [`crate::resolve::Resolved`] and is
/// required for `Text` nodes to be measured without re-shaping. Passing `None`
/// for a `Text` node falls back to shaping on the spot — correct, but the slow
/// path; every in-tree caller has a `Resolved` and supplies the cached layout.
pub fn local_bounds(kind: &NodeKind, text: Option<&TextLayout>) -> Option<Rect> {
    match kind {
        NodeKind::Rect { size, .. }
        | NodeKind::Ellipse { size }
        | NodeKind::Polygon { size, .. }
        | NodeKind::Star { size, .. }
        | NodeKind::Artboard { size, .. } => Some(Rect::new(0.0, 0.0, size.width, size.height)),
        NodeKind::Line { end } => Some(Rect::new(
            0.0_f64.min(end.x),
            0.0_f64.min(end.y),
            0.0_f64.max(end.x),
            0.0_f64.max(end.y),
        )),
        // The box of the rounded outline: a rect's box is unchanged by a radius,
        // but a rounded path's can shrink. `round_corners` returns the path
        // unchanged when there are no radii.
        NodeKind::Path { path, corner_radii } => {
            Some(round_corners(path, corner_radii).bounding_box())
        }
        // The layout's box, not `0,0..w,h`: a trimmed text node's box starts
        // below its local origin ([`TextLayout::origin`]), so this is the one kind
        // whose local bounds need not begin at the origin.
        NodeKind::Text { .. } => {
            let parts = crate::node::TextRef::of(kind)?;
            Some(match text {
                Some(layout) => layout.bounds(),
                None => crate::text::measure(parts),
            })
        }
        // A boolean's box is its derived outline's, which this function cannot
        // build (see [`local_path`]) — `Resolved` measures it from the cache, the
        // same way it measures a text node from a cached layout.
        NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. } => None,
    }
}

/// The point a node's transforms pivot about, in the node's own local space.
///
/// `local` is the node's geometry box — [`local_bounds`] for a shape, the union
/// of its children for a container ([`crate::local_box`]). An unset pivot is that
/// box's centre.
pub fn pivot_point(pivot: Option<Pivot>, local: Rect) -> Point {
    match pivot {
        None => local.center(),
        Some(Pivot::Normalized(f)) => Point::new(
            local.x0 + local.width() * f.x,
            local.y0 + local.height() * f.y,
        ),
        Some(Pivot::Local(p)) => p,
    }
}

/// The [`Pivot`] that puts a node of this kind at the local point `at`, given its
/// box.
///
/// The one place [`Pivot`]'s variants are chosen between: authored box → fraction,
/// emergent box → absolute point. The test matches the resize gesture's
/// (`tools::resizable_size` is its app-side twin): is the extent a typed field or a
/// consequence of what is inside? Auto-sized text lands in the emergent half, so a
/// content edit cannot drag the pivot around with the wrapping.
pub fn pivot_for(kind: &NodeKind, local: Rect, at: Point) -> Pivot {
    let authored = match kind {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Artboard { .. } => true,
        NodeKind::Text { sizing, .. } => matches!(sizing, crate::node::TextSizing::Fixed(_)),
        NodeKind::Root
        | NodeKind::Group
        | NodeKind::Line { .. }
        | NodeKind::Path { .. }
        // Emergent, like a group's: editing an operand would move a fraction of
        // its box.
        | NodeKind::Boolean { .. } => false,
    };
    if authored && local.width() > MIN_EXTENT && local.height() > MIN_EXTENT {
        Pivot::Normalized(Vec2::new(
            (at.x - local.x0) / local.width(),
            (at.y - local.y0) / local.height(),
        ))
    } else {
        Pivot::Local(at)
    }
}

/// Below this a box has no extent for a fraction to be measured against, so even
/// an authored one falls back to an absolute pivot rather than dividing by it.
const MIN_EXTENT: f64 = 1e-9;

/// The [`Pivot`] a placement at local point `at` should store, or `None` when
/// `at` **is** the box's centre — which is the absence of a pivot rather than a
/// pivot in the middle of it.
///
/// **One rule for both the canvas marker and the inspector's origin fields**, so
/// dragging back to the centre and typing 50/50 leave the same state: `None` is
/// what [`crate::build::flip`] reads and the canvas draws no affordance for.
///
/// A degenerate axis is always centred (its one position is its minimum), so an
/// axis-aligned line's origin clears on the *other* axis alone.
///
/// The epsilon is [`MIN_EXTENT`]: both callers arrive at the centre by the
/// expression that defines it, so the comparison is against rounding, not tolerance.
pub fn pivot_placed_at(kind: &NodeKind, local: Rect, at: Point) -> Option<Pivot> {
    let centre = local.center();
    if (at.x - centre.x).abs() <= MIN_EXTENT && (at.y - centre.y).abs() <= MIN_EXTENT {
        return None;
    }
    Some(pivot_for(kind, local, at))
}

/// How far the widest visible stroke reaches *outside* the geometry. Used to
/// expand bounds, and as the hit-test tolerance for open shapes.
///
/// Alignment-aware: an inside stroke adds nothing, an outside one its full width.
///
/// **Takes the kind** so it reads [`effective_sides`] rather than the stored
/// `sides`: on a kind with no nameable sides, `Custom { 0, 0, 0, 0 }` would report
/// no reach while the walk strokes the whole outline. Pinned by
/// `bounds_allow_for_a_stroke_the_walk_will_actually_draw`.
pub fn stroke_expansion(kind: &NodeKind, paint: &Paint) -> f64 {
    paint
        .strokes
        .iter()
        .filter(|s| s.visible)
        // The widest side, not the nominal width; see [`StrokeSides::max_width`]
        // for why generous is the safe direction.
        .map(|s| effective_sides(kind, s).max_width(s.width) * s.align.outward_fraction())
        .fold(0.0, f64::max)
}

/// World-space, stroke-expanded bounds of a non-container node, from loose parts
/// rather than a `Node`. `None` for Group/Root (handled by union at the
/// `Resolved` level). See [`local_bounds`] for the `text` argument.
///
/// **Parts, because no caller has the kind a `Node` would hand it.** Render
/// previews measure under a pending edit's override, and `Resolved` measures at the
/// **used** kind, which is the node's own only until container layout resizes it.
pub fn world_bounds_of_parts(
    kind: &NodeKind,
    paint: &Paint,
    world: Affine,
    text: Option<&TextLayout>,
) -> Option<Rect> {
    let local = local_bounds(kind, text)?;
    let expansion = stroke_expansion(kind, paint);
    measurable(transform_rect(world, local.inflate(expansion, expansion)))
}

/// `r` if it is a box anything can be measured against, `None` otherwise.
///
/// **Two tests, because either alone lets bad boxes through:**
///
/// - **Ordered**, as in [`crate::resolve`]'s `clipped_to` guard. A `NaN` in the
///   transform gives `Rect { x0: inf, x1: -inf }`, because `f64::min`/`max` return
///   the non-`NaN` operand and leave both seeds where [`transform_rect`] put them.
/// - **Finite**, which ordering does not imply: `Affine::new([1e308, 0, 0, 1e308,
///   0, 0])` has finite coefficients but overflows in the corner multiply to an
///   ordered, infinite rect.
///
/// **`None` rather than a clamp**: a node with no bounds is out of the spatial
/// index, the cull and *zoom to selection*, where a clamped box would put a click
/// target on the artwork.
pub fn measurable(r: Rect) -> Option<Rect> {
    (r.x1 >= r.x0
        && r.y1 >= r.y0
        && r.x0.is_finite()
        && r.y0.is_finite()
        && r.x1.is_finite()
        && r.y1.is_finite())
    .then_some(r)
}

/// World bounds of a node whose outline is *derived* rather than declared — a
/// [`NodeKind::Boolean`], measured from the outline `Resolved` has cached for it.
///
/// Separate from [`world_bounds_of_parts`] because only `Resolved`, which owns the
/// cache, can measure this.
pub fn world_bounds_of_path(
    path: &BezPath,
    paint: &Paint,
    kind: &NodeKind,
    world: Affine,
) -> Option<Rect> {
    if path.is_empty() {
        return None;
    }
    let expansion = stroke_expansion(kind, paint);
    let local = path.bounding_box();
    measurable(transform_rect(world, local.inflate(expansion, expansion)))
}

/// Axis-aligned bounding box of `rect` after `affine`.
///
/// Transforms all four corners, so it is correct for any affine (rotation, flip,
/// skew).
///
/// It can return a rect nothing can use: four `NaN` corners leave the seeds at
/// `Rect { x0: inf, y0: inf, x1: -inf, y1: -inf }`, and an overflowing product gives
/// an infinite one. Callers answering `Option<Rect>` pass the result through
/// [`measurable`]; this stays total for callers with nowhere to put the absence.
pub fn transform_rect(affine: Affine, rect: Rect) -> Rect {
    let corners = [
        affine * Point::new(rect.x0, rect.y0),
        affine * Point::new(rect.x1, rect.y0),
        affine * Point::new(rect.x1, rect.y1),
        affine * Point::new(rect.x0, rect.y1),
    ];
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in corners {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    Rect::new(min_x, min_y, max_x, max_y)
}

/// Whether `p` (already in the node's LOCAL space) hits the node's painted area.
/// Closed shapes test filled area; lines test distance to the segment. See
/// [`local_bounds`] for the `text` argument.
///
/// `slop` is an extra picking allowance in local units that **every kind** uses, as
/// a band reaching *outward* from the ink: with no fill the outline is what the
/// user aims at. The allowance is the ink's own reach plus `slop`, so a heavy stroke
/// is grabbable across its width and keeps the same margin beyond it.
///
/// **`kind` is the node's *used* kind, `paint` its paint as drawn, and `node`
/// supplies the rest** — the fill rule a path is tested under. The kind is the
/// node's own until container layout resizes it, and then the click target has to
/// be the box that is drawn; the paint is `Document::resolved_paint`'s, a bound
/// stroke width its number.
pub fn contains_local(
    node: &Node,
    paint: &Paint,
    kind: &NodeKind,
    p: Point,
    text: Option<&TextLayout>,
    slop: f64,
) -> bool {
    let tol = stroke_expansion(kind, paint) + slop;
    match kind {
        NodeKind::Rect { size, .. } | NodeKind::Artboard { size, .. } => {
            p.x >= -tol && p.y >= -tol && p.x <= size.width + tol && p.y <= size.height + tol
        }
        NodeKind::Ellipse { size } => {
            // Grown radii rather than a distance to the true ellipse, which has no
            // closed form; the error is a fraction of `tol`.
            let (rx, ry) = (size.width * 0.5 + tol, size.height * 0.5 + tol);
            if rx <= 0.0 || ry <= 0.0 {
                return false;
            }
            let nx = (p.x - size.width * 0.5) / rx;
            let ny = (p.y - size.height * 0.5) / ry;
            nx * nx + ny * ny <= 1.0
        }
        NodeKind::Line { end } => {
            // A line has no interior, so its whole target is its stroke's width
            // plus `slop`, which the canvas derives from a screen distance and the
            // zoom. Added to the reach rather than maxed with it.
            distance_point_segment(p, Point::ZERO, *end) <= tol
        }
        // Star spokes and a polygon's corners leave a lot of the box empty, so
        // these test the outline rather than the box — clicking the gap between
        // two points of a star must fall through to whatever is behind it.
        NodeKind::Polygon { .. } | NodeKind::Star { .. } => {
            local_path(kind).is_some_and(|path| path.contains(p) || near_outline(&path, p, tol))
        }
        // An open path is grabbed by its stroke: `BezPath::contains` closes it
        // implicitly, which would make the empty space inside the closure
        // selectable. The rounded outline is tested, not the authored one, so a
        // cut-off corner is not clickable. `node.fill_rule()` rather than the
        // non-zero `contains`, so even-odd holes are not hit targets;
        // `near_outline` applies under both rules.
        NodeKind::Path { .. } => local_path(kind)
            .is_some_and(|path| node.fill_rule().contains(&path, p) || near_outline(&path, p, tol)),
        // `tol` must apply here too: the world bounds include an outside stroke,
        // so the click region has to match them.
        NodeKind::Text { .. } => match local_bounds(kind, text) {
            Some(b) => b.inflate(tol, tol).contains(p),
            None => false,
        },
        // A boolean is hit-testable, but its outline is not this function's to
        // build; the test lives beside the cache in `query::hit_test`.
        NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. } => false,
    }
}

/// Whether `p` is within `tol` of a path's outline, curves included.
///
/// Flattened rather than solved, since a cubic has no closed-form point distance;
/// the flattening tolerance is tied to `tol` so a generous allowance does not pay
/// for precision it cannot use.
fn near_outline(path: &BezPath, p: Point, tol: f64) -> bool {
    if tol <= 0.0 {
        return false;
    }
    let flat = (tol * 0.25).clamp(0.01, 1.0);
    let (mut cursor, mut start) = (Point::ZERO, Point::ZERO);
    let mut hit = false;
    kurbo::flatten(path.iter(), flat, |el| {
        // Keep walking after a hit: the callback cannot bail, and the arithmetic
        // is a handful of dot products per segment.
        match el {
            PathEl::MoveTo(a) => {
                cursor = a;
                start = a;
            }
            PathEl::LineTo(a) => {
                hit |= distance_point_segment(p, cursor, a) <= tol;
                cursor = a;
            }
            PathEl::ClosePath => {
                hit |= distance_point_segment(p, cursor, start) <= tol;
                cursor = start;
            }
            // `flatten` emits only the three above.
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
        }
    });
    hit
}

fn distance_point_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p - a).dot(ab)) / len2).clamp(0.0, 1.0)
    };
    let proj = a + ab * t;
    (p - proj).hypot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Size;

    fn rect(radii: RoundedRectRadii) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(100.0, 60.0),
            corner_radii: radii,
        }
    }

    /// A node of `kind` with no paint, so `stroke_expansion` contributes zero.
    fn bare(kind: NodeKind) -> Node {
        Node {
            id: crate::IdSource::new(1).mint(),
            parent: None,
            children: Vec::new(),
            kind,
            transform: Affine::IDENTITY,
            name: String::new(),
            visible: true,
            locked: false,
            proportions_locked: false,
            opacity: 1.0,
            clip: false,
            mask: false,
            mask_mode: Default::default(),
            fill_rule: Default::default(),
            paint: Paint::default(),
            pivot: None,
            exports: Vec::new(),
            effects: Vec::new(),
            grids: Vec::new(),
            insets: Default::default(),
            display: None,
            item: Default::default(),
            component: false,
            link: None,
            swap: None,
            set: None,
            variant: Vec::new(),
            props: Vec::new(),
            origin: None,
            mode: None,
            corner_vars: Default::default(),
            spacing_vars: Default::default(),
            visible_var: None,
            size_vars: Default::default(),
            text_vars: Default::default(),
            opacity_var: None,
            styles: Default::default(),
        }
    }

    /// Four radii stay four: `local_path` is the only outline, so collapsing them
    /// would misround both canvas and export.
    #[test]
    fn each_corner_rounds_independently() {
        let square = local_path(&rect(RoundedRectRadii::default())).unwrap();
        let one = local_path(&rect(RoundedRectRadii::new(12.0, 0.0, 0.0, 0.0))).unwrap();
        let all = local_path(&rect(RoundedRectRadii::from_single_radius(12.0))).unwrap();

        assert_ne!(
            square.to_svg(),
            one.to_svg(),
            "one rounded corner is not square"
        );
        assert_ne!(one.to_svg(), all.to_svg(), "one corner is not four");

        // Only the rounded corner is cut away, so the missing area is a quarter
        // of what rounding all four removes.
        let cut = |p: &BezPath| 100.0 * 60.0 - p.area().abs();
        assert!(
            (cut(&all) / cut(&one) - 4.0).abs() < 1e-6,
            "one corner cut {}, four cut {}",
            cut(&one),
            cut(&all)
        );
    }

    /// A radius larger than the shape is clamped where the outline is built, so
    /// the stored number survives a round trip through a smaller size.
    #[test]
    fn an_oversized_radius_is_clamped_not_inverted() {
        let path = local_path(&rect(RoundedRectRadii::from_single_radius(400.0))).unwrap();
        let b = path.bounding_box();
        assert!(
            (b.width() - 100.0).abs() < 1e-6 && (b.height() - 60.0).abs() < 1e-6,
            "outline escaped its box: {b:?}"
        );
        // Clamped to half the shorter side: a 100×60 stadium, not a point.
        assert!(path.area().abs() > 0.5 * 100.0 * 60.0, "{}", path.area());
    }

    fn corners_of(path: &BezPath) -> Vec<Point> {
        path.elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    /// A polygon fills its box, has the vertices it says it has, and points up.
    #[test]
    fn a_polygon_has_n_vertices_and_points_up() {
        for sides in [3u32, 5, 8] {
            let kind = NodeKind::Polygon {
                size: Size::new(100.0, 100.0),
                sides,
            };
            let path = local_path(&kind).unwrap();
            let pts = corners_of(&path);
            assert_eq!(pts.len(), sides as usize, "{sides} sides");

            let b = path.bounding_box();
            assert!(
                (b.x0).abs() < 1e-6
                    && (b.y0).abs() < 1e-6
                    && (b.width() - 100.0).abs() < 1e-6
                    && (b.height() - 100.0).abs() < 1e-6,
                "{sides}: spans {b:?}, not 0,0..100,100"
            );
            // First vertex at top centre — a triangle points up.
            assert!(
                (pts[0].x - 50.0).abs() < 1e-6 && pts[0].y.abs() < 1e-6,
                "{sides}: starts at {:?}, not the top",
                pts[0]
            );
        }
    }

    /// A star fills its box at every inner ratio.
    #[test]
    fn a_star_fills_its_box_at_every_ratio() {
        for ratio in [0.05, 0.382, 0.8, 1.0] {
            let path = local_path(&NodeKind::Star {
                size: Size::new(120.0, 80.0),
                points: 5,
                inner_ratio: ratio,
            })
            .unwrap();
            let b = path.bounding_box();
            assert!(
                b.x0.abs() < 1e-6
                    && b.y0.abs() < 1e-6
                    && (b.width() - 120.0).abs() < 1e-6
                    && (b.height() - 80.0).abs() < 1e-6,
                "ratio {ratio}: spans {b:?}"
            );
        }
    }

    /// A star has twice the points it is asked for, every other one pulled in, and
    /// is leaner than its polygon. Radii are compared relatively, since filling the
    /// box stretches x and y unevenly.
    #[test]
    fn a_star_alternates_between_two_radii() {
        let size = Size::new(100.0, 100.0);
        let star_at = |ratio: f64| {
            local_path(&NodeKind::Star {
                size,
                points: 5,
                inner_ratio: ratio,
            })
            .unwrap()
        };
        let star = star_at(0.4);
        let pts = corners_of(&star);
        assert_eq!(pts.len(), 10, "five points is ten vertices");

        let centre = Point::new(50.0, 50.0);
        let radii: Vec<f64> = pts.iter().map(|p| (*p - centre).hypot()).collect();
        for i in 0..radii.len() {
            let (out, inn) = (radii[i - i % 2], radii[(i / 2) * 2 + 1]);
            assert!(
                inn < out,
                "vertex {i}: inner {inn} is not inside outer {out}"
            );
        }

        // The ratio is the control it claims to be.
        assert!(star_at(0.2).area().abs() < star_at(0.7).area().abs());

        let pentagon = local_path(&NodeKind::Polygon { size, sides: 5 }).unwrap();
        assert!(
            star.area().abs() < pentagon.area().abs(),
            "a star should not be fatter than its pentagon"
        );
    }

    /// Two sides is not a shape: the count is clamped to `MIN_SIDES..=MAX_SIDES`
    /// where the outline is built.
    #[test]
    fn a_degenerate_side_count_still_builds_a_shape() {
        for sides in [0u32, 1, 2] {
            let path = local_path(&NodeKind::Polygon {
                size: Size::new(80.0, 80.0),
                sides,
            })
            .unwrap();
            assert_eq!(corners_of(&path).len(), MIN_SIDES as usize, "{sides} sides");
            assert!(path.area().abs() > 0.0, "{sides} sides enclosed nothing");
        }
        // And the cap holds.
        let path = local_path(&NodeKind::Polygon {
            size: Size::new(80.0, 80.0),
            sides: 10_000,
        })
        .unwrap();
        assert_eq!(corners_of(&path).len(), MAX_SIDES as usize);
    }

    /// The picking allowance reaches a few units outward from the edge, not far
    /// outside it; three distances are asserted so a canvas-wide allowance fails.
    #[test]
    fn a_shape_is_grabbable_just_outside_its_edge_not_far_outside() {
        let node = bare(NodeKind::Rect {
            size: Size::new(100.0, 50.0),
            corner_radii: RoundedRectRadii::default(),
        });
        let slop = 4.0;

        assert!(
            contains_local(
                &node,
                node.paint(),
                node.kind(),
                Point::new(50.0, 25.0),
                None,
                slop
            ),
            "the interior still hits"
        );
        assert!(
            contains_local(
                &node,
                node.paint(),
                node.kind(),
                Point::new(-2.0, 25.0),
                None,
                slop
            ),
            "2 units outside the left edge is inside the allowance"
        );
        assert!(
            !contains_local(
                &node,
                node.paint(),
                node.kind(),
                Point::new(-9.0, 25.0),
                None,
                slop
            ),
            "9 units outside is not: the band is a few units, not a halo"
        );
        // With no allowance asked for, the test is exact.
        assert!(!contains_local(
            &node,
            node.paint(),
            node.kind(),
            Point::new(-2.0, 25.0),
            None,
            0.0
        ));
    }

    /// An open path is grabbed by its stroke, not by the region its implied
    /// closure encloses.
    #[test]
    fn an_open_path_is_grabbed_by_its_ink() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        let node = bare(NodeKind::Path {
            path,
            corner_radii: Vec::new(),
        });

        assert!(
            contains_local(
                &node,
                node.paint(),
                node.kind(),
                Point::new(50.0, 2.0),
                None,
                4.0
            ),
            "2 units off a straight open path is on its ink"
        );
        assert!(
            !contains_local(
                &node,
                node.paint(),
                node.kind(),
                Point::new(50.0, 40.0),
                None,
                4.0
            ),
            "40 units away is not"
        );
    }

    /// Rounding all four corners of a square agrees with kurbo's `RoundedRect`,
    /// which is built by a different route.
    #[test]
    fn rounding_every_corner_of_a_square_gives_a_rounded_rectangle() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((100.0, 0.0));
        square.line_to((100.0, 100.0));
        square.line_to((0.0, 100.0));
        square.close_path();

        let rounded = round_corners(&square, &[10.0; 4]);
        let want = RoundedRect::from_rect(Rect::new(0.0, 0.0, 100.0, 100.0), 10.0).to_path(0.01);
        assert!(
            (rounded.area().abs() - want.area().abs()).abs() < 0.5,
            "area {} vs kurbo's {}",
            rounded.area().abs(),
            want.area().abs()
        );
        // The bounding box is untouched: a fillet only cuts inward.
        let (a, b) = (rounded.bounding_box(), square.bounding_box());
        assert!((a.x0 - b.x0).abs() < 1e-9 && (a.x1 - b.x1).abs() < 1e-9);
        assert!((a.y0 - b.y0).abs() < 1e-9 && (a.y1 - b.y1).abs() < 1e-9);
        // The sharp vertex is gone, not an arc drawn beside it.
        assert!(
            !rounded.elements().iter().any(|el| matches!(
                el,
                PathEl::LineTo(p) | PathEl::MoveTo(p) if (*p - Point::new(100.0, 0.0)).hypot() < 1e-6
            )),
            "the corner should have been trimmed away: {:?}",
            rounded.elements()
        );

        // Zero radii are the identity.
        assert_eq!(round_corners(&square, &[]).elements(), square.elements());
        assert_eq!(
            round_corners(&square, &[0.0; 4]).elements(),
            square.elements()
        );
    }

    /// A radius past what the edges can give rounds as far as it can rather than
    /// inverting the outline.
    #[test]
    fn an_over_large_radius_clamps_instead_of_inverting() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((100.0, 0.0));
        square.line_to((100.0, 100.0));
        square.line_to((0.0, 100.0));
        square.close_path();

        let huge = round_corners(&square, &[500.0; 4]);
        // Clamped to half of each 100-unit edge, so the four fillets meet at the
        // midpoints: the square becomes its inscribed circle.
        let circle = std::f64::consts::PI * 50.0 * 50.0;
        assert!(
            (huge.area().abs() - circle).abs() < 5.0,
            "expected about a {circle:.0}-unit disc, got {:.0}",
            huge.area().abs()
        );
        assert!(
            huge.area().abs() < square.area().abs(),
            "a clamped fillet still cuts inward"
        );
    }

    /// A `NaN` radius is no radius, while `inf` is a magnitude that clamps to the
    /// inscribed circle (the control).
    ///
    /// Two guards cover `NaN`, and the test pins each:
    ///
    /// - `round_corners`' fast path reverted to `all(|r| *r <= 0.0)`: red at the
    ///   first assertion, which pins the route `[NaN; 4]` takes.
    /// - `round_run`'s per-corner guard reverted to `radius <= 0.0`: the first
    ///   assertion stays green and `the NaN corner was cut` fails.
    #[test]
    fn a_nan_radius_is_no_radius_where_an_infinite_one_is_the_largest() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((100.0, 0.0));
        square.line_to((100.0, 100.0));
        square.line_to((0.0, 100.0));
        square.close_path();

        // Untouched, element for element.
        assert_eq!(
            round_corners(&square, &[f64::NAN; 4]).elements(),
            square.elements(),
            "a NaN radius rounded the square"
        );

        // The per-corner guard needs a real radius beside the `NaN`:
        // `[NaN, 0, 0, 0]` would take the early return and never reach `round_run`.
        let mixed = round_corners(&square, &[f64::NAN, 20.0, 0.0, 0.0]);
        let vertex = |p: Point| {
            mixed.elements().iter().any(|el| {
                matches!(
                    el, PathEl::LineTo(q) | PathEl::MoveTo(q) if (*q - p).hypot() < 1e-6
                )
            })
        };
        assert!(
            vertex(Point::new(0.0, 0.0)),
            "the NaN corner was cut: {:?}",
            mixed.elements()
        );
        assert!(
            !vertex(Point::new(100.0, 0.0)),
            "the 20-unit corner beside it should still have rounded"
        );

        // The control: an infinite radius clamps to half each edge.
        let circle = std::f64::consts::PI * 50.0 * 50.0;
        let inf = round_corners(&square, &[f64::INFINITY; 4]);
        assert!(
            (inf.area().abs() - circle).abs() < 5.0,
            "an infinite radius should clamp to the inscribed circle, got {:.0}",
            inf.area().abs()
        );
    }

    /// A corner rounds even when a neighbour curves, and the curve survives as a
    /// curve. The segment kinds are asserted, since a flattened bulge also shrinks
    /// the area.
    #[test]
    fn a_corner_rounds_beside_a_curve_and_leaves_the_curve_a_curve() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        // A bulging side: its two anchors each have a curve on one side and a
        // line on the other.
        path.curve_to((160.0, 30.0), (160.0, 70.0), (100.0, 100.0));
        path.line_to((0.0, 100.0));
        path.close_path();

        let out = round_corners(&path, &[20.0, 20.0, 20.0, 20.0]);
        // Every original vertex is gone: all four corners took the radius,
        // including the two the curve adjoins.
        for v in [
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 100.0),
            Point::new(0.0, 100.0),
        ] {
            assert!(
                !out.elements().iter().any(|el| matches!(
                    el,
                    PathEl::LineTo(p) | PathEl::MoveTo(p) if (*p - v).hypot() < 1e-6
                )),
                "{v:?} should have been rounded away: {:?}",
                out.elements()
            );
        }
        // The trimmed bulge still bows out past the chord between its ends.
        let far = out
            .segments()
            .filter_map(|s| match s {
                PathSeg::Cubic(c) => Some(c.eval(0.5).x),
                _ => None,
            })
            .fold(0.0_f64, f64::max);
        assert!(
            far > 120.0,
            "the curved side flattened: nothing reaches past x=120, max was {far:.1}"
        );
    }

    /// An endpoint of an open subpath has one adjoining segment, so no corner.
    #[test]
    fn only_interior_anchors_of_an_open_path_round() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        path.line_to((100.0, 100.0));

        let out = round_corners(&path, &[20.0, 20.0, 20.0]);
        let els = out.elements();
        assert!(
            matches!(els[0], PathEl::MoveTo(p) if (p - Point::new(0.0, 0.0)).hypot() < 1e-9),
            "the first anchor stays put: {els:?}"
        );
        assert!(
            matches!(els[els.len() - 1], PathEl::LineTo(p) if (p - Point::new(100.0, 100.0)).hypot() < 1e-9),
            "and so does the last: {els:?}"
        );
        // The middle one rounded, so there is an arc in between.
        assert!(
            els.iter().any(|el| matches!(el, PathEl::CurveTo(..))),
            "the interior corner should have rounded: {els:?}"
        );
    }

    /// The indexing a radius is stored against; a closed path written by the pen
    /// repeats its first point, which must fold.
    #[test]
    fn anchors_are_numbered_across_subpaths_with_the_closing_point_folded() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.line_to((0.0, 0.0)); // back onto the start, as `pen_outline` writes it
        path.close_path();
        path.move_to((50.0, 50.0));
        path.curve_to((60.0, 50.0), (70.0, 50.0), (80.0, 50.0));

        let runs = anchor_runs(&path);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].points.len(), 3, "the repeated closing point folded");
        assert!(runs[0].closed);
        assert_eq!(
            runs[0].segs.len(),
            3,
            "a closed run has one segment per anchor"
        );
        assert!(runs[0].segs.iter().all(|s| matches!(s, PathSeg::Line(_))));
        assert_eq!(runs[1].points.len(), 2);
        assert!(!runs[1].closed);
        assert!(
            matches!(runs[1].segs[0], PathSeg::Cubic(_)),
            "the segment leaving anchor 0 curves"
        );
        assert_eq!(runs[1].segs.len(), 1, "an open run has one fewer");
        assert_eq!(anchor_count(&path), 5, "numbering runs across subpaths");
    }

    /// A rounded path reports the box of what is drawn, while a rounded `Rect`'s
    /// box is unchanged.
    #[test]
    fn a_rounded_path_reports_the_box_it_draws_and_a_rounded_rect_is_unchanged() {
        // A triangle with its apex at the top; rounding it cuts the top away.
        let mut path = BezPath::new();
        path.move_to((50.0, 0.0));
        path.line_to((100.0, 100.0));
        path.line_to((0.0, 100.0));
        path.line_to((50.0, 0.0));
        path.close_path();

        let square = NodeKind::Path {
            path: path.clone(),
            corner_radii: Vec::new(),
        };
        let rounded = NodeKind::Path {
            path,
            corner_radii: vec![30.0],
        };
        let plain = local_bounds(&square, None).unwrap();
        let cut = local_bounds(&rounded, None).unwrap();
        assert_eq!(plain.min_y(), 0.0);
        assert!(
            cut.min_y() > plain.min_y() + 1.0,
            "the rounded apex should pull the top down: {cut:?}"
        );
        assert_eq!(cut.max_y(), plain.max_y(), "the flat side is untouched");

        // A `Rect`'s fillets cut in from corners, not edges, so its box is the same.
        let size = Size::new(100.0, 60.0);
        let sharp = NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
        };
        let soft = NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::from_single_radius(20.0),
        };
        assert_eq!(local_bounds(&sharp, None), local_bounds(&soft, None));
    }

    /// A segment is named by the anchor it **leaves**, in the numbering a radius is
    /// stored against.
    #[test]
    fn a_segment_is_named_by_the_anchor_it_leaves_across_subpaths() {
        // Subpath 0: a closed triangle written the way `pen_outline` writes one.
        // Subpath 1: an open pair well away from it.
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        path.line_to((100.0, 100.0));
        path.line_to((0.0, 0.0));
        path.close_path();
        path.move_to((0.0, 500.0));
        path.line_to((100.0, 500.0));

        // Halfway along the top edge, a couple of units above it.
        let hit = nearest_segment(&path, Point::new(50.0, -2.0)).expect("a hit");
        assert_eq!((hit.subpath, hit.anchor), (0, 0));
        assert!((hit.t - 0.5).abs() < 1e-6, "{hit:?}");
        assert!((hit.distance - 2.0).abs() < 1e-6, "{hit:?}");
        assert!((hit.at - Point::new(50.0, 0.0)).hypot() < 1e-6, "{hit:?}");

        // The closing segment of the closed run leaves its last anchor.
        let closing = nearest_segment(&path, Point::new(48.0, 52.0)).expect("a hit");
        assert_eq!((closing.subpath, closing.anchor), (0, 2), "{closing:?}");

        // The second subpath is numbered from its own anchor 0.
        let other = nearest_segment(&path, Point::new(20.0, 501.0)).expect("a hit");
        assert_eq!((other.subpath, other.anchor), (1, 0), "{other:?}");

        // Nothing with no segment to hit.
        let mut lone = BezPath::new();
        lone.move_to((0.0, 0.0));
        assert!(nearest_segment(&lone, Point::ZERO).is_none());
        assert!(nearest_segment(&BezPath::new(), Point::ZERO).is_none());
    }

    /// Two rounded corners sharing a short edge each take half of it: pins
    /// `t = (radius / tan(θ/2)).min(la/2).min(lb/2)`, the one arithmetic that reads
    /// the memoized leg lengths.
    ///
    /// Measured as how far the outline stays from the original corner: at a right
    /// angle that is `t(√2 − 1)`, so `2.071` clamped and `0.828` for the unclamped
    /// control.
    ///
    /// flip: one leg's memoized length quadrupled in `round_run` → fails at
    /// 4.142 against 2.071. The fixture is 10 × 40, not square, because on a square
    /// the same flip stays green.
    #[test]
    fn two_rounded_corners_on_a_short_edge_take_half_of_it_each() {
        let mut square = BezPath::new();
        square.move_to(Point::new(0.0, 0.0));
        square.line_to(Point::new(10.0, 0.0));
        square.line_to(Point::new(10.0, 40.0));
        square.line_to(Point::new(0.0, 40.0));
        square.close_path();

        let rounded = round_corners(&square, &[10.0; 4]);
        let b = rounded.bounding_box();
        assert!(
            (b.x0 - 0.0).abs() < 1e-6
                && (b.y0 - 0.0).abs() < 1e-6
                && (b.x1 - 10.0).abs() < 1e-6
                && (b.y1 - 40.0).abs() < 1e-6,
            "the rounded rectangle still spans its own box: {b:?}"
        );
        let clearance = |p: &BezPath| {
            p.segments()
                .map(|s| s.nearest(Point::ZERO, 1e-9).distance_sq.sqrt())
                .fold(f64::INFINITY, f64::min)
        };
        let want = |t: f64| t * (2f64.sqrt() - 1.0);
        assert!(
            (clearance(&rounded) - want(5.0)).abs() < 0.02,
            "the trim is clamped to half the 10-unit side, so t = 5: {} against {}",
            clearance(&rounded),
            want(5.0)
        );
        // The control: a radius small enough that the clamp does not bind, so the
        // answer above came from `la`/`lb` rather than the radius.
        let gentle = round_corners(&square, &[2.0; 4]);
        assert!(
            (clearance(&gentle) - want(2.0)).abs() < 0.02,
            "an unclamped radius trims by itself, so t = 2: {} against {}",
            clearance(&gentle),
            want(2.0)
        );
    }

    /// A box that is not a number is no box at all. The last two rows are correctly
    /// ordered and only finiteness rejects them; the identity row is the control.
    ///
    /// flip: `measurable` reduced to the ordering test alone → fails at "an
    /// infinite translation", `Rect { x0: inf, x1: inf }`.
    #[test]
    fn a_transform_that_cannot_be_measured_gives_no_bounds() {
        let node = bare(rect(RoundedRectRadii::from_single_radius(0.0)));
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();

        // A transform a guard on `SetTransform`'s coefficients would accept.
        let overflowing = Affine::new([1e308, 0.0, 0.0, 1e308, 0.0, 0.0]);
        assert!(
            overflowing.as_coeffs().iter().all(|c| c.is_finite()),
            "every coefficient of scale(1e308) is finite, which is the point of it"
        );

        for (name, world) in [
            (
                "a NaN scale",
                Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
            ),
            (
                "a NaN translation",
                Affine::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0]),
            ),
            (
                "an infinite translation",
                Affine::new([1.0, 0.0, 0.0, 1.0, f64::INFINITY, 0.0]),
            ),
            ("a finite scale whose product overflows", overflowing),
        ] {
            assert_eq!(
                world_bounds_of_parts(node.kind(), node.paint(), world, None),
                None,
                "{name}: a node with no measurable extent has no bounds"
            );
            assert_eq!(
                world_bounds_of_path(&path, node.paint(), node.kind(), world),
                None,
                "{name}: and neither does a derived outline under it"
            );
        }

        assert!(
            world_bounds_of_parts(node.kind(), node.paint(), Affine::IDENTITY, None).is_some(),
            "control: an ordinary transform still measures"
        );
        assert!(
            world_bounds_of_path(&path, node.paint(), node.kind(), Affine::IDENTITY).is_some(),
            "control: and so does an ordinary outline"
        );
    }
}
