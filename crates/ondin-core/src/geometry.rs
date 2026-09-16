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
/// **The canonical anchor numbering, and it lives in core because the model
/// does.** A `Path` carries a radius per anchor (`NodeKind::Path::corner_radii`),
/// so "which anchor is number three" stopped being the app's private business
/// the moment it had to survive being saved. `tools::pen_anchors` produces the
/// same order — with handles attached, which this does not need — and
/// `the_app_and_core_number_a_paths_anchors_alike` keeps the two from drifting.
/// If they drift, a radius set on one corner appears on another.
///
/// **The segments are carried, not just their endpoints, and that is not
/// convenience.** The first version kept `points` and a `straight` flag, so
/// rounding rebuilt the path from its vertices — and every curve in it was
/// flattened to a line the moment any corner had a radius. Reported as "the
/// smooth point became a corner". Holding the real `PathSeg` makes that
/// unsayable: an untouched segment is re-emitted as itself.
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
    /// The anchor the segment **leaves**, which is what makes this a name an
    /// editor can act on: it is the same index a corner radius is stored at, so
    /// "the segment after point 3" needs no second numbering.
    pub anchor: usize,
    /// Parameter along the segment, 0..=1.
    pub t: f64,
    /// The point on the segment itself, and how far the query point was from it.
    pub at: Point,
    pub distance: f64,
}

/// The segment of `path` nearest `p`.
///
/// **The mapping back to `(subpath, anchor)` is the whole reason this is here
/// rather than over `BezPath::segments`.** `segments()` yields one flat run with
/// no subpath boundaries and no anchor numbers in it, so every caller would have
/// to re-derive the numbering — a third walk beside this module's and
/// `tools::pen_anchors`', and the one that drifts is the one that puts an
/// inserted point on the wrong side of a corner. [`AnchorRun`] already carries
/// the real `PathSeg`s indexed by the anchor each one leaves, so the query is a
/// scan over that.
///
/// `None` for a path with no segments at all — a single `MoveTo`, or nothing.
/// A closed run includes its closing segment (`segs` has one per anchor); an open
/// one has no segment leaving its last anchor, which is why the answer is the
/// anchor a segment *leaves* rather than the one it arrives at.
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

/// Accuracy for the nearest-point solves [`nearest_segment`] runs. Well under a
/// device pixel at any sane zoom, and the query is answering a pointer.
const NEAREST_ACC: f64 = 1e-3;

/// Accuracy for the arc-length solves that trim a segment back from a corner.
/// Well under a device pixel at any sane zoom.
const ARCLEN_ACC: f64 = 1e-4;

/// `path` with a fillet cut into each corner that has a radius.
///
/// **Every corner, whatever its neighbours are.** An earlier cut rounded only
/// where both adjoining segments were straight, and that was wrong in use rather
/// than merely limited: a quadrilateral with one curved side rounded exactly one
/// of its three corners, because the other two each had the curve on one side.
/// A corner is a corner — what the neighbours do is not the question the radius
/// is asking. The legs are trimmed back **by arc length**, so a curved one is
/// shortened rather than replaced.
///
/// **What the fillet *is* depends on the legs, and it has to.** Between two
/// straight legs it is an exact circular arc, so a path rounded this way and a
/// `Rect` rounded by `RoundedRect` agree. Against a curve there is generally no
/// circle tangent to both at the trim points, so the join is a cubic that meets
/// each side along its own tangent — smooth where it matters, and the very same
/// construction a circular arc is when the legs happen to be straight.
///
/// An endpoint of an open subpath is never rounded: one adjoining segment is no
/// corner. A radius on a **smooth** anchor does nothing, because the two tangents
/// are collinear and there is no turn to cut — the behaviour asked for, falling
/// out of the geometry rather than needing a rule of its own.
///
/// **Each radius is clamped to half of each adjoining segment**, which stops two
/// rounded corners sharing a short edge from eating past each other and turning
/// the outline inside out. `RoundedRect::from_rect` clamps for the same reason.
pub fn round_corners(path: &BezPath, radii: &[f64]) -> BezPath {
    // `any(> 0)` rather than `all(<= 0)`, which are not each other's negation in
    // the presence of `NaN` — see the guard in `round_run` (§15 D640). A list of
    // nothing but `NaN`s takes this fast path now, which is the same answer the
    // per-corner guard gives one at a time; before, it fell through to a walk
    // that rounded every corner to its maximum.
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
    // **Each segment's arc length, computed at most once** (§15 D507's neighbour,
    // §15 D510, `[S4.2-L4-04]`). Every interior segment borders two corners, so
    // `fillet` used to measure it **twice** — once as the arriving leg of corner
    // *i*, once as the leaving leg of corner *i+1* — and a cubic's `arclen` is a
    // numeric integration, with `inv_arclen` a root-find that calls it again per
    // iteration. `local_path` runs on every painted node on **every frame**, so
    // this is per-frame work.
    //
    // **Lazy rather than a pre-pass**, which is the half that matters for the
    // ordinary document: `round_corners` takes this route as soon as *one* radius
    // is non-zero, and a length is only ever needed beside a corner that is
    // actually rounded. Filling all `ns` up front would make one rounded corner on
    // a long path cost what rounding every corner costs.
    //
    // ⚠️ **It is worth about 7%, not the half `[S4.2-L4-04]` estimated, and only
    // measuring said so.** Release, a ring of cubics, every corner rounded:
    // 11.68 → 10.64 ms at 10,000 anchors, 0.84 → 0.79 at 1,000. The redundant
    // `arclen` is real and is now gone, but `inv_arclen` is a **root-find that
    // calls `arclen` again per iteration** and there are two of those per corner,
    // so removing one of two direct calls is a small share of the work. The
    // finding's larger fix — caching the rounded outline on `Resolved` beside
    // `boolean`, so the walk is *handed* it like every other derived geometry —
    // is untouched and is where the rest of it is.
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
            // Hoisted out of `fillet` so the memo below is only ever filled for a
            // corner that is going to be cut. `ia` and `ib` are distinct here —
            // they coincide only at `n == 1`, which this arm excludes.
            let radius = radii.get(i).copied().unwrap_or(0.0);
            // ⚠️ **Written to reject `NaN` by construction, not by `<= 0.0`**
            // (§15 D640, `[S4.2-L1-06]`). Every comparison against `NaN` is
            // false, so `radius <= 0.0` *lets it through*, and what it reaches is
            // `fillet`'s `(radius / …).min(la * 0.5).min(lb * 0.5)` — where
            // `f64::min` returns the **non-`NaN`** operand. The corner is then
            // cut by the largest trim the clamp allows, so a square carrying a
            // `NaN` radius draws as its inscribed circle: not a refusal, not a
            // `NaN` path, the maximum fillet. The guard reads as doing the
            // opposite of what it does.
            //
            // `inf` is deliberately still accepted and is not the same case: it
            // is a magnitude, and *"each radius is clamped to half of each
            // adjoining segment"* is this function's documented answer to one
            // that is too big — the same answer radius 500 gets.
            //
            // ⚠️ Spelled with `is_nan()` rather than as `!(radius > 0.0)`, which
            // is the same predicate and is what `clippy::neg_cmp_op_on_partial_ord`
            // objects to — for precisely the reason this bug happened: a negated
            // comparison hides the incomparable case, and hiding it is what the
            // old guard did.
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
/// `la` and `lb` are the two legs' arc lengths. **Passed in rather than measured
/// here** (§15 D510): every interior segment borders two corners, so measuring
/// them at the point of use did each one twice, and a cubic's `arclen` is a
/// numeric integration. `round_run` memoizes them; the caller has also already
/// established `radius > 0.0`, which is why there is no check for it here.
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
/// text and for a boolean — see below for the last two, which have an outline that
/// is not a function of this argument.
///
/// One definition, because more than one consumer needs the *same* outline: the
/// scene walk strokes and fills it, and the SVG writer needs it verbatim as a
/// `clipPath` for aligned strokes. Two copies would drift, and the symptom
/// would be an export that clips a stroke slightly differently from the canvas.
///
/// **A `Boolean` answers `None` here and it is not a shape without one.** Its
/// outline is a function of its whole subtree, not of its kind, so it cannot be
/// built from the argument this function takes — read
/// [`crate::Resolved::local_path`] instead, which returns the cached result for a
/// boolean and defers to this for everything else. Every caller that draws or
/// measures has a `Resolved` to hand.
///
/// **A `Text` node answers `None` too, and for the same shape of reason.** Its
/// outline is the outline of its *glyphs*, which needs the shaped layout rather
/// than the kind — [`crate::text::outline`] builds it from the cached
/// `TextLayout`, and [`stroke_align_applies`] answers yes for text on that basis.
pub fn local_path(kind: &NodeKind) -> Option<BezPath> {
    match kind {
        // **A frame has an outline, and it is its own frame.** This is what a
        // frame's stroke is drawn along and what its alignment clip and its four
        // sides are built from — square-cornered, because a frame carries no
        // radii, so it comes out of the same `Rect::to_path` a square-cornered
        // rect does and the two cannot drift (§15 D144).
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
        // **Closed by hand**, because `Ellipse::to_path` does not close it — four
        // cubics whose last point is the first one, and no `ClosePath`. It is the
        // only shape here that needed saying: `Rect`, `RoundedRect` and `star_path`
        // all close their own outlines.
        //
        // A fill cannot tell the difference, which is why this survived. A *stroke*
        // can: an unclosed outline is an **open** path, so the backend caps its two
        // ends instead of joining them, and at large widths an open expansion is a
        // different shape from a closed one. Measured either side of this line: the
        // expansion's winding at the ellipse's centre is 0 unclosed and 4 closed.
        //
        // **One half of "a very wide stroke leaves a hole in the middle of its own
        // ink", not the whole of it** (§15 D131). Closing the outline was necessary
        // and is not sufficient — a *rect*, closed all along, holes from 330pt on the
        // same 200×120 box — so `scene::lay_stroke` is what actually fixes the hole
        // and this is what stops the ellipse being a second, differently-shaped case
        // of it.
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
        // **The one place a path's corner radii become geometry.** Every consumer
        // that draws, measures, exports or hit-tests a path reaches it through
        // here or through `Resolved::local_path` above it, so the rounding is
        // resolved once and cannot be forgotten by one of them (§15 D119).
        NodeKind::Path { path, corner_radii } => Some(round_corners(path, corner_radii)),
        // `Boolean` and `Text` included, and see the notes above: both have an
        // outline, neither has one this argument can build.
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
/// fraction of the radius, so a polygon is simply the case with no inner ring.
/// One function because they are one construction, and two copies would drift
/// on the details that matter: both start at the top, so a triangle points up
/// and a five-pointed star sits the way one is drawn.
///
/// **Filling the box is not cosmetic.** `local_bounds` reports `0,0..w,h` for
/// these kinds, and the selection rect, the snap targets and the resize handles
/// all come from it. A triangle merely *inscribed* in the circle spans three
/// quarters of its box's height, so every one of those would sit away from the
/// shape. Normalising the vertices to the box is what keeps the reported bounds
/// honest — and it is also what a designer means by dragging out a shape.
///
/// Counts are clamped here rather than on the way into the model, like a rect's
/// corner radii: dragging the sides field down through 2 and back must not lose
/// the number the user was heading for.
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
/// lands back on its first. Both are needed: `Rect::to_path` emits the explicit
/// `ClosePath`, while `Ellipse::to_path` is four cubics that simply arrive back
/// where they started. Testing only for the marker would decide that ellipses
/// have no inside.
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
/// cannot build its outline (see the note there) and the test below would
/// therefore read "not closed" — the one answer a boolean's outline is never. Every
/// subpath `boolean::from_flo` emits is explicitly closed, and each one's winding is
/// normalized against its nesting depth, which is precisely what makes an inside
/// well defined even where the shape has holes.
///
/// It was measured, and the two symptoms were a matched pair: the alignment
/// dropdown was absent from a boolean's Stroke panel, and `Resolved` sized the node
/// as though an outside stroke were there while both the canvas and the SVG drew it
/// centred.
/// **A `Text` node answers yes without being measured, for the same reason.** Its
/// outline is its glyphs' ([`crate::text::outline`]), which [`local_path`] cannot
/// build either — and every contour a font emits is closed, which is precisely what
/// makes outlined type expressible: an outside stroke on text is the whole point of
/// the alignment control being offered there at all.
pub fn stroke_align_applies(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Boolean { .. } | NodeKind::Text { .. } => true,
        NodeKind::Line { .. } => false,
        kind => local_path(kind).is_some_and(|p| is_closed(&p)),
    }
}

/// Whether a node's outline has corners for a join to shape.
///
/// The sibling [`stroke_align_applies`] and [`stroke_sides_apply`] were both
/// precedents for: a control that cannot change anything is offered as one that
/// can, and the user is left looking for the difference. An **ellipse** is four
/// cubics that meet tangentially, so every "corner" is already smooth; a **line**
/// has two ends and no interior vertex at all. Miter, Round and Bevel are the same
/// drawing on either.
///
/// **Gated on the kind, not on the geometry.** A rect whose four corners are fully
/// rounded has no sharp vertex either, and gating on that would make the control
/// appear and disappear while the radius field is being scrubbed — a flicker in
/// exchange for a distinction nobody is looking for. A rect is a thing you put a
/// mitre on; that is the honest answer at the granularity the panel works in.
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
        // Glyph outlines are full of corners — a stem meeting a serif, the apex of
        // an A — and a stroked one shapes every last of them. It reads at the
        // weights outlined type is set at, which is the only reason to offer it.
        | NodeKind::Text { .. } => true,
        // No corners: smooth throughout, and no interior vertex respectively.
        NodeKind::Ellipse { .. } | NodeKind::Line { .. } => false,
        // Nothing here strokes at all (§6.3), so there is no join to shape.
        NodeKind::Root | NodeKind::Group => false,
    }
}

/// Whether a node's stroke can be put on one named side: it needs four of them.
///
/// **The two boxes only**, and narrower than it might look. An ellipse, a polygon,
/// a star and a path have no top edge to speak of, and a `Line` has no sides at
/// all — so the answer there is no, exactly as [`stroke_align_applies`] says no to
/// an open path. Text is no for the same reason: a paragraph's outline is its
/// glyphs, which have no four edges between them.
///
/// **`Artboard` used to be excluded for a second reason and no longer is.** The
/// scene walk painted a frame's one background and returned, so a frame drew no strokes
/// of any kind and offering it a per-side one would have promised ink nothing paints
/// — the widening §15 D68 held open until frames stroked. They do (§15 D144), and a
/// frame is exactly the shape a per-side border is for.
pub fn stroke_sides_apply(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Rect { .. } | NodeKind::Artboard { .. })
}

/// The sides a stroke actually occupies on this kind — [`StrokeSides::All`]
/// wherever the field cannot be honoured.
///
/// The sibling of `stroke_align_applies`'s use in the walk: the value stays in
/// the model untouched, and every consumer asks this rather than reading `sides`
/// directly, so a rect that becomes an ellipse draws its whole outline instead of
/// a stray edge.
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
/// This is what a per-side stroke is drawn along, and splitting at the midpoint
/// is what CSS does with `border-top` against `border-radius`: two adjacent sides
/// of different widths meet on the diagonal, and a rounded corner belongs half to
/// each of the sides that share it. Cutting at the *straight* edge instead would
/// leave a rounded rect's corners bare, which reads as four gaps rather than a
/// border.
///
/// The radii come back off [`RoundedRect::from_rect`] rather than being clamped
/// here, so a side's arc is the same curve the fill and the closed outline use —
/// two clamping rules would show up as a stroke sitting a fraction off its own
/// shape. A corner with no radius contributes its sharp vertex instead of an arc.
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
            // An `Arc` leads with a `MoveTo` to its start point. Mid-path that
            // start has to be *reached*, not jumped to — dropping the element
            // would leave the following curve's control points describing an arc
            // from somewhere the pen is not.
            // **`elements()`, not `BezPath::is_empty`.** kurbo reads that as
            // "contains no *segments*", so a path holding a lone `MoveTo` is
            // empty by it — and this branch would then emit a second `MoveTo`,
            // leaving a side made of two jumps and no ink at all. What is being
            // asked here is whether the pen has been put down yet.
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
/// Two things the model deliberately leaves until here, because both need
/// numbers the model does not carry (§5.3):
///
/// - **A dot is a zero-length dash**, which is what makes its spacing
///   unambiguously centre-to-centre — but a zero-length dash draws *nothing*
///   under `Cap::Butt` or `Cap::Square`, so the stroke would vanish. Those two
///   get a square dot one width long instead, with the gap shortened to match so
///   the **period is preserved** and the spacing field keeps meaning the same
///   thing under all three caps.
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
/// **Fitting is per subpath, and that is why this exists.** `dash_fit` scales the
/// period so a whole number of them spans the outline, and a scale is a property of
/// *one* closed run of the path: two subpaths of different lengths need two
/// different scales to each land on their own corners, and one pattern cannot say
/// two things. Fitting the total instead — which is what this replaced — is exact
/// for a single-subpath shape and drifts by up to half a period on every subpath of
/// a path that has several, worst where the subpaths differ most in length.
///
/// `None` for everything else, and that is the common case doing no work: a solid
/// stroke, an unfitted pattern, or a path with one subpath all go down the
/// single-pattern route with no path cloned and no perimeter measured twice.
///
/// A subpath's own `perimeter` is what each is fitted against, so a stroke crossing
/// from one to the next restarts its phase there — which it already did, since
/// `dash_offset` is measured from each subpath's start.
///
/// §15 D147; it closes the approximation D69 recorded.
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
/// Shared rather than spelled per consumer: the scene walk, the SVG writer and
/// [`dash_fit_pieces`] all need the same split, and the walk's own copy of it is
/// load-bearing for the wide-stroke construction (`scene::outermost_subpaths`).
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
/// Rounding to the *nearest* whole number of periods rather than the floor: the
/// point of fitting is that the pattern lands where the shape turns, and the
/// nearest count moves each dash by at most half a period where flooring can
/// stretch every one of them by a full one.
///
/// `length` is one subpath's perimeter, not necessarily the whole path's — see
/// [`dash_fit_pieces`], which is what splits a multi-subpath path so that each
/// piece is fitted against its own corners.
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
        // **The box of what is actually drawn, radii and all**, which is what
        // `Rect` reports too — and the correction is that the two were never
        // disagreeing in the first place. The old rule kept the *unrounded* path
        // here "as `Rect` does", but a `Rect`'s rounded corners cut inward from
        // corners that are not on any extreme: its box is unchanged by a radius, so
        // the unrounded box was exact for that kind and merely a superset for this
        // one. The symmetry was the wrong way round, and what it cost was the
        // inspector reading a few units generous on a heavily rounded path (§15).
        //
        // `local_path` hands the path straight back when there are no radii, so the
        // ordinary case pays nothing.
        NodeKind::Path { path, corner_radii } => {
            Some(round_corners(path, corner_radii).bounding_box())
        }
        // **The box, not `0,0..w,h`.** A trimmed text node's box starts below
        // its local origin ([`TextLayout::origin`]), which is what lets trimming
        // tighten the box without moving the ink — so this is the one kind whose
        // local bounds need not begin at the origin, and every consumer of this
        // function gets the trim for free.
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
/// box's centre, which is the fixed point every gesture used before the field
/// existed, so a document that never touched a pivot behaves exactly as it did.
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
/// The one place [`Pivot`]'s two variants are chosen between, so that "authored
/// box → fraction, emergent box → absolute point" is a single statement rather
/// than a rule every caller has to remember. The test is the same one the resize
/// gesture uses to decide whether a node has a box of its own to edit
/// (`tools::resizable_size` is its app-side twin), because that is precisely the
/// question being asked: is this extent a field someone typed, or a consequence
/// of what is inside?
///
/// Auto-sized text is the interesting case and it comes out right: its box is a
/// field once it has been dragged, and before that it has no authored extent at
/// all — so it lands in the emergent half, where a later content edit cannot drag
/// the pivot around with the wrapping.
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
        // Emergent, like a group's: a boolean has no authored box, only whatever
        // its operands add up to, so a *fraction* of that box is not a stable
        // place to pin a pivot — editing an operand would move it.
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
/// **One rule, two doors** (§15 D247). The canvas marker and the inspector's origin
/// fields both have to agree that the middle clears the field, or dragging the marker
/// back to the centre and typing 50/50 would leave the document in two different
/// states — one with `pivot: None`, one holding a stored half that resolves to the
/// same point. They are not the same state: `None` is what [`crate::build::flip`]
/// and the pre-field gestures read, and it is the one the canvas stops drawing an
/// affordance for, so it is worth being able to get back to exactly.
///
/// **A degenerate axis is always centred**, and that falls out rather than being
/// special-cased: a box with no extent on an axis has one position on it, its
/// minimum, which is also its centre. So an axis-aligned line's origin clears on
/// the *other* axis alone — where the marker's own snap test insisted on a middle
/// third that a zero extent can never report, and a horizontal line's pivot could
/// not be dragged back to nothing at all.
///
/// The epsilon is [`MIN_EXTENT`] because it is the same question in the same
/// units: is a distance in local space zero? Both callers arrive at the centre by
/// evaluating the same expression the centre is defined by — a snap to the middle
/// third, or a fraction of exactly `0.5` — so the comparison is against rounding,
/// not against tolerance.
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
/// Alignment-aware: an inside stroke adds nothing to the bounds, an outside one
/// adds its full width. Getting this wrong shows up as an outside stroke being
/// clipped by its own artboard, or culled a frame early when scrolled to the
/// edge of the viewport.
///
/// **Takes the kind, because it has to answer the same question the walk does.**
/// This is the one bounds consumer that reads `sides`, and reading the *stored*
/// field rather than [`effective_sides`] is a way to disagree with what gets
/// painted: on a kind with no nameable sides, `Custom { 0, 0, 0, 0 }` reports no
/// reach at all while the walk strokes the whole outline at full width — ink
/// outside its own reported bounds, which is exactly the clipped-and-culled
/// failure above. The panel gates the control so only a hand-edited file can set
/// that up, but the two functions must be unable to disagree rather than merely
/// unlikely to; passing the kind is what makes it structural. Pinned by
/// `bounds_allow_for_a_stroke_the_walk_will_actually_draw`.
pub fn stroke_expansion(kind: &NodeKind, paint: &Paint) -> f64 {
    paint
        .strokes
        .iter()
        .filter(|s| s.visible)
        // The widest side, not the stroke's nominal width: a `Custom` set of
        // per-side widths reaches only as far as its thickest one, and one that
        // is 0 on every side reaches nowhere at all. Still a single number for
        // all four sides — see [`StrokeSides::max_width`] for why generous is
        // the safe direction here.
        .map(|s| effective_sides(kind, s).max_width(s.width) * s.align.outward_fraction())
        .fold(0.0, f64::max)
}

/// World-space, stroke-expanded bounds of a non-container node. `None` for
/// Group/Root (handled by union at the `Resolved` level). See [`local_bounds`]
/// for the `text` argument.
pub fn world_bounds_of(node: &Node, world: Affine, text: Option<&TextLayout>) -> Option<Rect> {
    world_bounds_of_parts(node.kind(), node.paint(), world, text)
}

/// [`world_bounds_of`] from loose parts rather than a `Node`.
///
/// Render previews need the bounds a node *would* have under a pending edit,
/// where the kind and paint come from an override rather than the document.
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

/// `r` if it is a box anything can be measured against, `None` otherwise
/// (§15 D495).
///
/// **Two tests, because either one alone lets half the bad boxes through**, and
/// that asymmetry is the whole of this function:
///
/// - **Ordered**, which is [`crate::resolve`]'s `clipped_to` guard — the one
///   place in the model that already rejected an inverted rect. A `NaN` in the
///   transform gives `Rect { x0: inf, x1: -inf }`, because `f64::min` and
///   `f64::max` return the operand that is *not* `NaN` and so leave both seeds
///   where [`transform_rect`] put them.
/// - **Finite**, which ordering does not imply. `Affine::new([1e308, 0, 0,
///   1e308, 0, 0])` has every coefficient finite — so a guard on the *operands*
///   of `SetTransform` accepts it — and the overflow happens in the corner
///   multiply, giving `Rect { x0: 0, y0: 0, x1: inf, y1: inf }`, correctly
///   ordered and useless. `transform="scale(1e308)"` in an imported SVG is the
///   short way to one.
///
/// **`None` rather than a clamp**, because both callers already answer
/// `Option<Rect>` and every consumer of theirs already handles the absence: a
/// node with no bounds is out of the spatial index, out of the cull and out of
/// *zoom to selection*, which is the honest description of a layer whose extent
/// is not a number. A clamped box would put a click target on the artwork
/// instead.
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
/// Separate from [`world_bounds_of`] rather than a fifth argument to it, because
/// the two are asked by different callers: everything measures a shape from its
/// kind, and only `Resolved` — which owns the cache — can measure this.
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
/// Transforms all four corners rather than the two opposite ones, which is what
/// makes it correct for *any* affine — a rotation, a flip, or a skew, where two
/// corners no longer bound the box the other two are in.
///
/// ⚠️ **It can return a rect nothing can use, and it says so rather than
/// refusing** (§15 D495). `f64::min` and `f64::max` return the operand that is
/// *not* `NaN`, so four `NaN` corners leave the seeds where they started and the
/// answer is `Rect { x0: inf, y0: inf, x1: -inf, y1: -inf }` — inverted rather
/// than empty. A finite affine whose product overflows gives an ordered but
/// infinite one. Callers that answer `Option<Rect>` pass the result through
/// [`measurable`]; this stays total because several callers have a `Rect` to
/// return and nowhere to put the absence.
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
/// `slop` is an extra picking allowance in local units, and **every kind uses it
/// now** — as a band reaching *outward* from the ink, on top of whatever interior
/// the shape has.
///
/// **This used to be open shapes only, and that was wrong on the report.** The
/// argument for withholding it was that a filled shape has an interior to aim at,
/// so "a click a few units outside a rectangle selecting it would be wrong". What
/// that missed is the shape with a stroke and *no fill*, which is most of what a
/// pen draws and plenty of what a rectangle is: the ink you can see is the
/// outline, so the outline is where you aim — and landing a pixel outside it hit
/// nothing at all. Reported as "selecting a shape is a bit tricky, I need to move
/// the mouse within the stroke". The band is strictly a superset of the old test,
/// so nothing that used to be selectable stopped being so; what it costs is that a
/// shape now shadows the few units just outside its edge, which is the trade every
/// editor makes.
///
/// The allowance is the ink's own reach plus `slop`, exactly as a `Line` has
/// always computed it — so a heavy stroke is grabbable across its whole width
/// *and* keeps the same comfort margin beyond it.
pub fn contains_local(node: &Node, p: Point, text: Option<&TextLayout>, slop: f64) -> bool {
    let tol = stroke_expansion(node.kind(), node.paint()) + slop;
    match node.kind() {
        NodeKind::Rect { size, .. } | NodeKind::Artboard { size, .. } => {
            p.x >= -tol && p.y >= -tol && p.x <= size.width + tol && p.y <= size.height + tol
        }
        NodeKind::Ellipse { size } => {
            // Grown radii rather than a distance to the true ellipse, which has no
            // closed form. The error is a fraction of `tol` at the diagonals and
            // nobody can feel it; an exact solve here would be arithmetic nothing
            // asked for.
            let (rx, ry) = (size.width * 0.5 + tol, size.height * 0.5 + tol);
            if rx <= 0.0 || ry <= 0.0 {
                return false;
            }
            let nx = (p.x - size.width * 0.5) / rx;
            let ny = (p.y - size.height * 0.5) / ry;
            nx * nx + ny * ny <= 1.0
        }
        NodeKind::Line { end } => {
            // **The ink's own reach, plus the caller's picking allowance.** A line is the
            // one kind with no interior to aim at: its whole target is the width of its
            // stroke, so a hairline was a one-unit band and reported as "a very narrow
            // window to click and select a line with a thin stroke". `slop` is what the
            // caller thinks a comfortable grab is, in *world* units — the canvas derives
            // it from a screen distance and the zoom, so the band is the same size under
            // the pointer at any magnification, which a constant here could not be.
            //
            // Added to the reach rather than maxed with it, so a heavy stroke is still
            // grabbable across its whole width *and* keeps the same comfort margin.
            distance_point_segment(p, Point::ZERO, *end) <= tol
        }
        // Star spokes and a polygon's corners leave a lot of the box empty, so
        // these test the outline rather than the box — clicking the gap between
        // two points of a star must fall through to whatever is behind it.
        NodeKind::Polygon { .. } | NodeKind::Star { .. } => local_path(node.kind())
            .is_some_and(|path| path.contains(p) || near_outline(&path, p, tol)),
        // **An open path is grabbed by its stroke, not only by the region it
        // happens to enclose.** `BezPath::contains` closes the path implicitly and
        // asks for a winding number, so a pen squiggle was selectable by the empty
        // space inside its imaginary closure and hardly at all by the ink.
        // **The rounded outline, not the authored one.** A fillet cuts the corner
        // away, so testing the raw path would keep the cut-off triangle clickable
        // — a few units of shape that is no longer drawn. `local_path` is free
        // when there are no radii: it returns the path unchanged.
        // **`node.fill_rule()` and not `contains`, which is non-zero** (§15 D239): a
        // path that fills even-odd has holes where its subpaths overlap, and testing
        // it with a winding number makes those holes clickable — a hit target over
        // artwork that is not there. `near_outline` stays either way, because the
        // ink of the outline is grabbable under both rules.
        NodeKind::Path { .. } => local_path(node.kind())
            .is_some_and(|path| node.fill_rule().contains(&path, p) || near_outline(&path, p, tol)),
        // ⚠️ **This arm spent nothing until §15 D483** (`[S4.2-L2-03]`), and it is
        // the kind users click most. D115 rewrote every arm to add `tol` and its
        // own enumeration of what it had rewritten — `Rect`/`Artboard`, `Ellipse`,
        // `Polygon`/`Star`/`Path` — never says `Text`, so the sweep skipped it and
        // neither the entry nor §5.10's prose noticed. Two consequences, and the
        // second is the sharper one because it is a *disagreement* rather than a
        // missing comfort margin: the picking allowance did nothing here, and a
        // text node with a 40pt **outside** stroke reported world bounds 40 units
        // wider on every side than the region that could be clicked — a band of the
        // layer's own stated extent selecting nothing.
        NodeKind::Text { .. } => match local_bounds(node.kind(), text) {
            Some(b) => b.inflate(tol, tol).contains(p),
            None => false,
        },
        // A boolean *is* hit-testable — it has one filled outline, and clicking it
        // must select it rather than falling through to the page. But the outline
        // is not this function's to build, so the test lives beside the cache in
        // `query::hit_test`, which has the `Resolved` this needs.
        NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. } => false,
    }
}

/// Whether `p` is within `tol` of a path's outline, curves included.
///
/// Flattened rather than solved: a cubic has no closed-form point distance, and
/// the tolerance here is a picking allowance measured in a few units, so a
/// polyline accurate to a fraction of it answers the same question. The flattening
/// tolerance is tied to `tol` so a generous allowance does not pay for precision
/// it cannot use.
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

    /// A node of `kind` with nothing else on it — no paint, so `stroke_expansion`
    /// contributes zero and a hit-test assertion is about the *allowance* rather
    /// than about some stroke width chosen in the fixture.
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
        }
    }

    /// `local_path` is the *only* outline: the scene walk fills and strokes it,
    /// and the SVG writer clips with it. If it collapsed four radii back to one
    /// the canvas and the export would round a shape the document never said.
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
    /// The last is what a user notices instantly and no type can enforce: a
    /// triangle drawn from angle 0 rather than −90° comes out lying on its side.
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

            // Fills its box exactly — see `star_path`: the reported bounds, the
            // selection rect and the resize handles all assume it does.
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

    /// A star fills its box too, and keeps doing so as the inner ring moves —
    /// the ratio must not shrink the shape away from the selection rect.
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

    /// A star alternates outer and inner vertices, so it has twice the points it
    /// is asked for, every other one is pulled in, and it is strictly leaner
    /// than the polygon around it.
    ///
    /// Radii are compared *relatively*, not against absolute numbers: filling
    /// the box stretches x and y by different factors whenever the vertex ring
    /// is not square, so the exact distances are not the ones the ratio names.
    /// What has to hold is the alternation, and that the ratio drives it.
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

    /// Two sides is not a shape. The count is clamped where the outline is built
    /// rather than on the way into the model, so dragging the field down through
    /// 2 and back up does not lose the number the user was heading for.
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
        // And the cap holds, so a fat number cannot tessellate the app to death.
        let path = local_path(&NodeKind::Polygon {
            size: Size::new(80.0, 80.0),
            sides: 10_000,
        })
        .unwrap();
        assert_eq!(corners_of(&path).len(), MAX_SIDES as usize);
    }

    /// **The reported bug: a shape you can only select by landing on its stroke.**
    ///
    /// The ink of an unfilled shape *is* its outline, so that is where the pointer
    /// goes — and a pixel outside it used to hit nothing, because the test was the
    /// exact interior with no allowance. The allowance now reaches outward from
    /// the edge, so the band the user is aiming at is grabbable from both sides.
    ///
    /// Asserted at three radii rather than one, because "outside now hits" would
    /// also pass for an allowance that had swallowed the whole canvas.
    #[test]
    fn a_shape_is_grabbable_just_outside_its_edge_not_far_outside() {
        let node = bare(NodeKind::Rect {
            size: Size::new(100.0, 50.0),
            corner_radii: RoundedRectRadii::default(),
        });
        let slop = 4.0;

        assert!(
            contains_local(&node, Point::new(50.0, 25.0), None, slop),
            "the interior still hits"
        );
        assert!(
            contains_local(&node, Point::new(-2.0, 25.0), None, slop),
            "2 units outside the left edge is inside the allowance"
        );
        assert!(
            !contains_local(&node, Point::new(-9.0, 25.0), None, slop),
            "9 units outside is not: the band is a few units, not a halo"
        );
        // And with no allowance asked for, the old exact behaviour is unchanged —
        // which is what keeps a marquee or a snapshot hit test from widening.
        assert!(!contains_local(&node, Point::new(-2.0, 25.0), None, 0.0));
    }

    /// An open path is grabbed by its **stroke**, where it used to be grabbed by
    /// the region its implied closure encloses.
    ///
    /// `BezPath::contains` closes a path implicitly and asks for a winding number,
    /// so a shallow pen squiggle was selectable in the empty air under its arc and
    /// barely at all on the ink. The two assertions are the whole reversal: on the
    /// line hits, and the enclosed emptiness a long way from any segment does not
    /// have to.
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
            contains_local(&node, Point::new(50.0, 2.0), None, 4.0),
            "2 units off a straight open path is on its ink"
        );
        assert!(
            !contains_local(&node, Point::new(50.0, 40.0), None, 4.0),
            "40 units away is not"
        );
    }

    /// **All four corners of a square rounded is a rounded rectangle**, which is
    /// the strongest thing this can be checked against: `RoundedRect` is kurbo's
    /// own, built by a different route, so agreeing with it to within a
    /// flattening tolerance says the fillet is right rather than merely
    /// self-consistent.
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
        // The bounding box is untouched: a fillet only ever cuts inward, which is
        // what lets `local_bounds` keep reporting the authored box.
        let (a, b) = (rounded.bounding_box(), square.bounding_box());
        assert!((a.x0 - b.x0).abs() < 1e-9 && (a.x1 - b.x1).abs() < 1e-9);
        assert!((a.y0 - b.y0).abs() < 1e-9 && (a.y1 - b.y1).abs() < 1e-9);
        // And the sharp vertex is actually gone, rather than an arc having been
        // drawn beside it.
        assert!(
            !rounded.elements().iter().any(|el| matches!(
                el,
                PathEl::LineTo(p) | PathEl::MoveTo(p) if (*p - Point::new(100.0, 0.0)).hypot() < 1e-6
            )),
            "the corner should have been trimmed away: {:?}",
            rounded.elements()
        );

        // Zero radii are the identity, and cost nothing: the common path.
        assert_eq!(round_corners(&square, &[]).elements(), square.elements());
        assert_eq!(
            round_corners(&square, &[0.0; 4]).elements(),
            square.elements()
        );
    }

    /// A radius past what the edges can give rounds **as far as it can** rather
    /// than turning the outline inside out — `RoundedRect::from_rect` clamps for
    /// the same reason, and an inverted contour is the failure being prevented.
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

    /// A `NaN` radius is **no** radius, and it used to be the largest one
    /// (§15 D640, `[S4.2-L1-06]`).
    ///
    /// This test is deliberately beside `an_over_large_radius_clamps_…` because
    /// that test asserts the answer this bug produced: the inscribed circle. Both
    /// guards were spelled `<= 0.0`, every comparison against `NaN` is false, so
    /// the value reached `fillet`'s `.min(la * 0.5).min(lb * 0.5)` — and
    /// `f64::min` returns the non-`NaN` operand, which is the *maximum* trim the
    /// clamp allows. A square carrying a `NaN` radius drew, exported and
    /// hit-tested as a circle, with nothing reporting it.
    ///
    /// ⚠️ **`inf` is the control and it is deliberately unchanged.** It is a
    /// magnitude, so the clamp is the documented answer to it, and it is asserted
    /// here as *still* rounding — otherwise the fix reads as "reject anything not
    /// finite" and the next reader tightens the guard onto a case the function
    /// has always answered on purpose. That the two used to be byte-identical is
    /// what identified `f64::min` as the mechanism in the first place.
    ///
    /// 🚨 **Three flips run, and the pair is the finding: neither guard alone
    /// produced the reported symptom.**
    ///
    /// - Fast path only reverted to `all(|r| *r <= 0.0)`: red at the first
    ///   assertion, and the left side is **the square**, not a circle — the
    ///   per-corner guard catches every `NaN` on the way through, and all the
    ///   walk changes is that the closing edge arrives as an explicit `LineTo`.
    ///   So the first assertion is pinning *which route* `[NaN; 4]` takes, which
    ///   is stronger than "was not rounded" and is deliberate.
    /// - Per-corner guard only reverted to `radius <= 0.0`: the first assertion
    ///   stays **green** — the fast path answers `[NaN; 4]` before the walk ever
    ///   runs — and it is red at *"the NaN corner was cut"*, with `(0,0)` filleted
    ///   50 units, i.e. the maximum.
    /// - Both reverted: red at the first assertion with four `CurveTo`s and no
    ///   straight edge left, which is `[S4.2-L1-06]`'s measured inscribed circle
    ///   reproduced exactly.
    ///
    /// ⚠️ **A fix at either site alone would have closed the reported symptom
    /// and left the other guard reading as doing the opposite of what it does.**
    /// That is why both are here and why the mixed case exists at all.
    #[test]
    fn a_nan_radius_is_no_radius_where_an_infinite_one_is_the_largest() {
        let mut square = BezPath::new();
        square.move_to((0.0, 0.0));
        square.line_to((100.0, 0.0));
        square.line_to((100.0, 100.0));
        square.line_to((0.0, 100.0));
        square.close_path();

        // Untouched, element for element — not merely "about 10000 units", which
        // a path rounded by a hair also satisfies.
        assert_eq!(
            round_corners(&square, &[f64::NAN; 4]).elements(),
            square.elements(),
            "a NaN radius rounded the square"
        );

        // ⚠️ **And the per-corner guard, not only the fast path — which needs a
        // radius that is really a radius beside it.** `[NaN, 0, 0, 0]` would not
        // do: no element of it is `> 0`, so it takes `round_corners`' early
        // return and never reaches `round_run` at all. One live 20 is what forces
        // the walk, and then the `NaN` corner has to survive it on its own.
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

        // The control, and the contrast: an infinite radius still clamps to half
        // each edge, which is the inscribed circle.
        let circle = std::f64::consts::PI * 50.0 * 50.0;
        let inf = round_corners(&square, &[f64::INFINITY; 4]);
        assert!(
            (inf.area().abs() - circle).abs() < 5.0,
            "an infinite radius should clamp to the inscribed circle, got {:.0}",
            inf.area().abs()
        );
    }

    /// **A corner rounds even when a neighbour curves, and the curve survives.**
    ///
    /// Both halves were bugs, reported together from one drawing: a quadrilateral
    /// with three corners and one curved side rounded exactly *one* of its
    /// corners — the only one with a straight segment on both sides — and the
    /// smooth point turned into a corner, because rounding rebuilt the path from
    /// its vertices and flattened every curve in it.
    ///
    /// **The earlier version of this test passed while both bugs were live**,
    /// which is the thing to learn from: it asserted the vertex survived (it did
    /// — as a polyline vertex) and that the area shrank (it did — flattening a
    /// bulge shrinks it). Neither said the curve was *still a curve*, so neither
    /// could fail. Assert the segment kinds.
    #[test]
    fn a_corner_rounds_beside_a_curve_and_leaves_the_curve_a_curve() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        // A bulging side, so the two anchors bounding it each have a curve on one
        // side and a line on the other — the shape that rounded nothing.
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
        // And the bulge is still curved, not a chord across it. The trimmed
        // remains of it must still bow out past the straight line between its
        // ends, which flattening could not do.
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

    /// An endpoint of an **open** subpath has one adjoining segment, so there is
    /// no corner there to cut — and rounding it would need a neighbour that does
    /// not exist.
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

    /// The indexing a radius is stored against. The fold is the subtle half —
    /// a closed path written by the pen repeats its first point, and counting
    /// that would put every radius one corner out.
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

    /// **A rounded path reports the box of what is drawn**, and a rounded `Rect`
    /// always did — which is the correction. The old rule kept the *unrounded* box
    /// here "as `Rect` does", but a rectangle's fillets cut in from corners that
    /// are not on any extreme, so its box never changed in the first place: the
    /// symmetry it appealed to did not exist, and only the path paid for it.
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

        // And the kind whose symmetry was appealed to: a `Rect`'s box is the same
        // rounded or not, because its fillets cut in from corners, not from edges.
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

    /// A segment is named by the anchor it **leaves**, in the same numbering a
    /// radius is stored against — which is the whole point of answering from
    /// `anchor_runs` rather than from a flat `segments()` walk.
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

        // The **closing** segment of the closed run is the one leaving its last
        // anchor — the case an open run does not have, and the one a walk that
        // numbered by arrival would get wrong.
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

    /// **Two rounded corners sharing a short edge each take half of it** — the
    /// clamp `round_corners`' own doc names, and the one case where a leg's arc
    /// **length** is load-bearing rather than incidental (§15 D510,
    /// `[S4.2-L4-04]`).
    ///
    /// D510 stopped measuring each interior segment twice — once as the arriving
    /// leg of one corner, once as the leaving leg of the next — and memoized it
    /// instead. That is a pure cost change with no answer to get wrong **except
    /// through the memo**, so this pins the one arithmetic that reads the value:
    /// `t = (radius / tan(θ/2)).min(la/2).min(lb/2)`.
    ///
    /// ⚠️ **The fixture is 10 × 40 with a 10-unit radius, and both halves of that
    /// are load-bearing.** With a 100-unit box and a radius of 10,
    /// `radius / tan(π/4)` is 10 and every half-length is 50, so `la` and `lb`
    /// never enter the answer at all and a wrong leg length is invisible. And with
    /// a *square*, the two legs at a corner are equal, so the `min` has nothing to
    /// choose between and only a leg that came back too **small** could be seen —
    /// measured, not reasoned: on a 10 × 10 fixture, quadrupling one leg left this
    /// test green. The rectangle gives each corner a short leg and a long one.
    ///
    /// Asserted as **how far the outline stays from the original corner**, which
    /// is a function of the trim and of nothing else: at a right angle the fillet's
    /// radius *is* `t`, its centre is at `(t, t)`, and the nearest point of the arc
    /// to `(0, 0)` is `t(√2 − 1)`. So the clamped case reads `2.071` and the
    /// unclamped control `0.828`, and the two are far enough apart that a wrong
    /// leg length cannot land on the right answer.
    ///
    /// ⚠️ **The obvious assertion was wrong and the run said so.** The first
    /// spelling expected `5` — the trim distance — on the reasoning that the
    /// corner is cut back by half a side. It is, but the *arc* then bulges back
    /// toward the corner, and what the fixture measures is the arc.
    ///
    /// **Flip run**, one leg's length quadrupled where the memo hands it over:
    /// fails at **4.142 against 2.071**, `t` coming out 10 instead of 5 — the
    /// predicted site. On the square fixture the same flip was **green**, which
    /// is why the rectangle is there.
    #[test]
    fn two_rounded_corners_on_a_short_edge_take_half_of_it_each() {
        // 10 × 40, **not a square**: each corner then has a short leg and a long
        // one, so the `min` has something to choose between and a memo handing
        // back the wrong segment's length changes the answer in *both*
        // directions. On a square the two legs are equal and only a leg that came
        // back too small could ever be seen.
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
        // The control: a radius small enough that the clamp does not bind, where
        // `t` is the radius itself. Its answer must be the *other* number, which
        // is what says the one above came from `la`/`lb` rather than from the
        // radius.
        let gentle = round_corners(&square, &[2.0; 4]);
        assert!(
            (clearance(&gentle) - want(2.0)).abs() < 0.02,
            "an unclamped radius trims by itself, so t = 2: {} against {}",
            clearance(&gentle),
            want(2.0)
        );
    }

    /// **A box that is not a number is no box at all** (`[S4.2-L1-01]`, §15 D495).
    ///
    /// `world_bounds_of` used to have exactly one `None` route — an empty
    /// `local_bounds` — so a bad *transform* had none, and every one of the four
    /// cases below came back `Some` with a rectangle nothing can use.
    ///
    /// ⚠️ **The last two are the finding**, and they are why `measurable` tests
    /// two things rather than one. `scale(1e308)` and `translate(inf)` are
    /// correctly **ordered**; only finiteness rejects them. Copying
    /// `resolve::clipped_to`'s ordering guard — the guard that already existed
    /// next door — would have closed the `NaN` half and passed these, and a
    /// finiteness guard on `SetTransform`'s own coefficients passes `scale(1e308)`
    /// too, because every coefficient in it *is* finite and the overflow happens
    /// in the corner multiply.
    ///
    /// The identity row is the control: an ordinary transform still answers a box,
    /// and a predicate with one comparison backwards would take every layer in the
    /// app out of the spatial index with everything else green.
    ///
    /// **Two flips run, and the second is the one that pays for the predicate's
    /// shape.** `measurable` returning `Some(r)` unconditionally fails here on *"a
    /// NaN scale"*, at `Rect { x0: inf, x1: -inf }` — the predicted site.
    /// `measurable` reduced to the **ordering** test alone — i.e. the guard
    /// `resolve::clipped_to` already had, the one a reader would reach for — gets
    /// past both `NaN` rows and fails on *"an infinite translation"*, at
    /// `Rect { x0: inf, x1: inf }`. Ordered and useless.
    #[test]
    fn a_transform_that_cannot_be_measured_gives_no_bounds() {
        let node = bare(rect(RoundedRectRadii::from_single_radius(0.0)));
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();

        // The fixture's own claim, asserted rather than asserted-about-in-prose:
        // this is the transform a guard on `SetTransform`'s coefficients accepts.
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
                world_bounds_of(&node, world, None),
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
            world_bounds_of(&node, Affine::IDENTITY, None).is_some(),
            "control: an ordinary transform still measures"
        );
        assert!(
            world_bounds_of_path(&path, node.paint(), node.kind(), Affine::IDENTITY).is_some(),
            "control: and so does an ordinary outline"
        );
    }
}
