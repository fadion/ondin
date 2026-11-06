//! A solid stroke drawn as the union of its pieces, for a curve that bends tighter
//! than half the stroke is wide.
//!
//! There the stroke's inner edge folds back over itself, and both backends' outline
//! (vello's GPU stroker and `kurbo::stroke`, which `vello_cpu` fills) winds the fold
//! the other way, so it nets to zero and a notch of ground shows through the ink: an
//! Archimedean spiral starts with its tightest bend at the very middle, which is
//! where a thick stroke cut its first turn.
//!
//! The union is exact under non-zero because every piece is wound the same way: a
//! quad on each segment of the flattened centreline, a round wedge where two
//! segments of one curve meet, the stroke's own join where two segments of the path
//! meet, and its caps. Overlaps add up and never cancel.

use crate::scene::StrokePaint;
use kurbo::{Affine, BezPath, Cap, Join, ParamCurveCurvature, PathEl, PathSeg, Point, Vec2};

/// Whether `path`, stroked `width` wide, bends anywhere tighter than half the
/// width: where the backends' outline folds. Sampled along each curve; a corner
/// between segments is a join, which the backends draw right.
pub(crate) fn bends_too_tight(path: &BezPath, width: f64) -> bool {
    /// Samples a curve; a spiral's tightest bend is at a segment's end, which is
    /// always sampled.
    const SAMPLES: usize = 8;
    let half = width / 2.0;
    path.segments().any(|seg| match seg {
        PathSeg::Line(_) => false,
        seg => (0..=SAMPLES).any(|i| {
            let k = curvature(&seg, i as f64 / SAMPLES as f64);
            k.is_finite() && k.abs() * half > 1.0
        }),
    })
}

/// The curvature of `seg` at `t`, signed; infinite at a cusp.
fn curvature(seg: &PathSeg, t: f64) -> f64 {
    match seg {
        PathSeg::Line(_) => 0.0,
        PathSeg::Quad(q) => q.curvature(t),
        PathSeg::Cubic(c) => c.curvature(t),
    }
}

/// `path` stroked as `paint` asks, as one path of positively wound pieces to fill
/// under non-zero. Flattened within `tolerance`, in `path`'s units. Solid strokes
/// only: a dash pattern is the caller's to refuse.
pub(crate) fn union_outline(path: &BezPath, paint: &StrokePaint<'_>, tolerance: f64) -> BezPath {
    let half = paint.width / 2.0;
    let mut out = BezPath::new();
    if half <= 0.0 {
        return out;
    }
    for sub in subpaths(path, tolerance) {
        lay_subpath(&mut out, &sub, half, paint, tolerance);
    }
    out
}

/// One subpath's centreline: its points, whether each point after the first ends
/// one of the path's own segments (a join) rather than a step of flattening, and
/// whether it closes.
struct Centreline {
    points: Vec<Point>,
    joins: Vec<bool>,
    closed: bool,
}

/// `path`'s subpaths, flattened segment by segment so the path's own vertices are
/// known; points closer than a hair to the one before are dropped.
fn subpaths(path: &BezPath, tolerance: f64) -> Vec<Centreline> {
    let mut out: Vec<Centreline> = Vec::new();
    let mut start = Point::ZERO;
    let push = |c: &mut Centreline, p: Point, join: bool| match c.points.last() {
        Some(last) if last.distance(p) < 1e-9 => {
            if join && let Some(j) = c.joins.last_mut() {
                *j = true;
            }
        }
        _ => {
            c.points.push(p);
            c.joins.push(join);
        }
    };
    let mut last = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                start = p;
                last = p;
                out.push(Centreline {
                    points: vec![p],
                    joins: vec![false],
                    closed: false,
                });
            }
            PathEl::LineTo(p) => {
                if let Some(c) = out.last_mut() {
                    push(c, p, true);
                }
                last = p;
            }
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                let mut one = BezPath::new();
                one.move_to(last);
                one.push(*el);
                let mut steps = Vec::new();
                kurbo::flatten(&one, tolerance, |e| {
                    if let PathEl::LineTo(p) = e {
                        steps.push(p);
                    }
                });
                if let Some(c) = out.last_mut() {
                    let n = steps.len();
                    for (i, p) in steps.into_iter().enumerate() {
                        push(c, p, i + 1 == n);
                    }
                }
                last = el.end_point().unwrap_or(last);
            }
            PathEl::ClosePath => {
                if let Some(c) = out.last_mut() {
                    push(c, start, true);
                    if c.points.len() > 2
                        && c.points.last().is_some_and(|p| p.distance(start) < 1e-9)
                    {
                        c.points.pop();
                        c.joins.pop();
                    }
                    c.closed = true;
                }
                last = start;
            }
        }
    }
    out
}

/// Push `pts` as one closed piece, wound so its signed area is positive.
fn piece(out: &mut BezPath, pts: &[Point]) {
    let area: f64 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    if area.abs() < 1e-12 {
        return;
    }
    let mut it: Box<dyn Iterator<Item = &Point>> = match area > 0.0 {
        true => Box::new(pts.iter()),
        false => Box::new(pts.iter().rev()),
    };
    if let Some(p) = it.next() {
        out.move_to(*p);
    }
    for p in it {
        out.line_to(*p);
    }
    out.close_path();
}

/// The unit left normal of `d`.
fn normal(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x) / d.hypot()
}

/// The arc of radius `half` about `at` from direction `from` to `to` the short way
/// round, as a fan of points within `tolerance` of the circle.
fn fan(at: Point, from: Vec2, to: Vec2, half: f64, tolerance: f64) -> Vec<Point> {
    let (a0, a1) = (from.atan2(), to.atan2());
    let mut sweep = a1 - a0;
    while sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    while sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    let step = 2.0 * (1.0 - (tolerance / half).min(1.0)).acos().max(1e-3);
    let n = (sweep.abs() / step).ceil().max(1.0) as usize;
    let mut pts = vec![at];
    for i in 0..=n {
        let a = a0 + sweep * i as f64 / n as f64;
        pts.push(at + Vec2::new(a.cos(), a.sin()) * half);
    }
    pts
}

/// One centreline's pieces into `out`.
fn lay_subpath(
    out: &mut BezPath,
    c: &Centreline,
    half: f64,
    paint: &StrokePaint<'_>,
    tolerance: f64,
) {
    let p = &c.points;
    let n = p.len();
    if n < 2 {
        // A lone point draws only its caps, as the backends draw it.
        if let Some(at) = p.first() {
            match paint.cap {
                Cap::Round => piece(
                    out,
                    &fan(
                        *at,
                        Vec2::new(1.0, 0.0),
                        Vec2::new(-1.0, 0.0),
                        half,
                        tolerance,
                    )[1..],
                ),
                Cap::Square | Cap::Butt => {}
            }
        }
        return;
    }
    let segs = if c.closed { n } else { n - 1 };
    let dir = |i: usize| p[(i + 1) % n] - p[i];
    for i in 0..segs {
        let (a, b) = (p[i], p[(i + 1) % n]);
        let nv = normal(b - a) * half;
        piece(out, &[a + nv, b + nv, b - nv, a - nv]);
    }
    // Every vertex between two segments: round where flattening put it, the
    // stroke's join where the path has a vertex of its own.
    let inner = match c.closed {
        true => 0..n,
        false => 1..n - 1,
    };
    for v in inner {
        let (d0, d1) = (dir((v + n - 1) % n), dir(v));
        let turn = d0.cross(d1);
        if turn.abs() < 1e-12 && d0.dot(d1) > 0.0 {
            continue;
        }
        // The outside of the turn is where the two quads leave a gap.
        let side = if turn > 0.0 { -1.0 } else { 1.0 };
        let (n0, n1) = (normal(d0) * side, normal(d1) * side);
        let at = p[v];
        let join = if c.joins[v] { paint.join } else { Join::Round };
        match join {
            Join::Round => piece(out, &fan(at, n0, n1, half, tolerance)),
            Join::Bevel => piece(out, &[at, at + n0 * half, at + n1 * half]),
            Join::Miter => {
                let mid = n0 + n1;
                let cos_half = mid.hypot() / 2.0;
                let ratio = if cos_half > 1e-9 {
                    1.0 / cos_half
                } else {
                    f64::INFINITY
                };
                if ratio <= paint.miter_limit {
                    let tip = at + mid.normalize() * (half * ratio);
                    piece(out, &[at, at + n0 * half, tip, at + n1 * half]);
                } else {
                    piece(out, &[at, at + n0 * half, at + n1 * half]);
                }
            }
        }
    }
    if c.closed {
        return;
    }
    for (at, d) in [(p[0], p[0] - p[1]), (p[n - 1], p[n - 1] - p[n - 2])] {
        let out_dir = d / d.hypot();
        let nv = normal(d) * half;
        match paint.cap {
            Cap::Butt => {}
            Cap::Square => {
                let ahead = out_dir * half;
                piece(out, &[at + nv, at + nv + ahead, at - nv + ahead, at - nv]);
            }
            Cap::Round => {
                let mut half_disc = fan(at, nv, out_dir, half, tolerance);
                let rest = fan(at, out_dir, -nv, half, tolerance);
                half_disc.extend(rest.into_iter().skip(2));
                piece(out, &half_disc);
            }
        }
    }
}

/// `path`'s flattening tolerance in its own units, so the facets stay under a
/// tenth of a device pixel through `world`.
pub(crate) fn tolerance_through(world: Affine, device: f64) -> f64 {
    let scale = world.determinant().abs().sqrt();
    match scale > 1e-12 {
        true => device / scale,
        false => device,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn paint(width: f64, cap: Cap, join: Join) -> StrokePaint<'static> {
        static BRUSH: std::sync::LazyLock<ondin_core::Brush> =
            std::sync::LazyLock::new(|| ondin_core::Brush::Solid(ondin_core::peniko::Color::BLACK));
        StrokePaint {
            brush: &BRUSH,
            framing: None,
            width,
            join,
            cap,
            miter_limit: 4.0,
            dashes: Vec::new(),
            dash_offset: 0.0,
        }
    }

    /// The winding of `path` at `p`, summed over its pieces.
    fn winding(path: &BezPath, p: Point) -> i32 {
        path.winding(p)
    }

    /// A straight butt-capped stroke is its rectangle: inside it the pieces wind
    /// positively, past its ends and sides nothing.
    #[test]
    fn a_straight_stroke_is_its_rectangle() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((100.0, 0.0));
        let u = union_outline(&line, &paint(10.0, Cap::Butt, Join::Miter), 0.1);
        assert!(winding(&u, Point::new(50.0, 4.0)) > 0);
        assert_eq!(winding(&u, Point::new(50.0, 6.0)), 0);
        assert_eq!(winding(&u, Point::new(-1.0, 0.0)), 0);
        assert!((u.area().abs() - 1000.0).abs() < 1e-6);
    }

    /// A curve far tighter than the stroke winds positively everywhere inside the
    /// stroke and never negatively, so a non-zero fill has no hole; a gentle one
    /// does not ask for the union at all.
    /// flip: `piece` keeping each piece's own winding → fails at "never negative".
    #[test]
    fn a_tight_curl_has_no_hole() {
        let mut curl = BezPath::new();
        curl.move_to((0.0, 0.0));
        curl.curve_to((6.0, 0.0), (6.0, 6.0), (0.0, 6.0));
        curl.curve_to((-8.0, 6.0), (-8.0, -4.0), (4.0, -6.0));
        let p = paint(30.0, Cap::Butt, Join::Miter);
        assert!(bends_too_tight(&curl, p.width));
        let u = union_outline(&curl, &p, 0.1);
        for y in -10..=16 {
            for x in -14..=12 {
                let w = winding(&u, Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5));
                assert!(w >= 0, "never negative at {x},{y}: {w}");
            }
        }
        let mut gentle = BezPath::new();
        gentle.move_to((0.0, 0.0));
        gentle.curve_to((100.0, 0.0), (200.0, 50.0), (300.0, 50.0));
        assert!(!bends_too_tight(&gentle, 10.0));
    }

    /// Round caps reach half the width past each end, square caps square it off.
    #[test]
    fn caps_reach_past_the_ends() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((100.0, 0.0));
        for cap in [Cap::Round, Cap::Square] {
            let u = union_outline(&line, &paint(10.0, cap, Join::Miter), 0.05);
            assert!(winding(&u, Point::new(-4.0, 0.0)) > 0, "{cap:?}");
            assert!(winding(&u, Point::new(104.0, 0.0)) > 0, "{cap:?}");
            assert_eq!(winding(&u, Point::new(-6.0, 0.0)), 0, "{cap:?}");
        }
    }
}
