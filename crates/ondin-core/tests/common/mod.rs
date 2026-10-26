//! The oracle the geometry suites check against, and their catalogue of shapes.
//!
//! An oracle answers where a point belongs from first principles, without the
//! arithmetic under test: a shape's fill by its winding number over a fine
//! flattening, a stroke's ink as each segment's slab plus each join and cap. It
//! stays silent within [`TOL`] of any edge it draws, so the arithmetic's own
//! tolerance never reads as a failure. [`check`] samples a grid against an oracle
//! and adds the structural checks a fill test cannot make: specks, edges with the
//! fill on both sides or neither (stray lines and points when the result is edited),
//! and runaway segment counts.

#![allow(dead_code)]

use ondin_core::kurbo::{
    self, BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathSeg,
    Point, Rect, Shape, Vec2,
};

/// The oracle's blind band either side of an edge it expects, in units.
pub const TOL: f64 = 0.6;
/// Grid points per side of the sampled box.
pub const GRID: usize = 56;

/// How a stroke band turns a corner.
#[derive(Clone, Copy, Debug)]
pub enum Corners {
    /// As a stroke renders (SVG): `join`, a mitre bevelled past `limit`.
    Svg(kurbo::Join, f64),
    /// As *Offset path* draws it: mitred, clipped square at `limit` times the
    /// half-width from the corner (SVG 2's `miter-clip`).
    Clip(f64),
}

/// A region of a join or cap.
enum Piece {
    Poly(Vec<Point>),
    Disk(Point),
}

/// The ink of `paths` stroked at half-width `h`: each segment's slab, each join,
/// and each open end's cap.
pub struct Band {
    segs: Vec<PathSeg>,
    /// Each segment's control-point box: a point further from it than a reach is
    /// further from the segment, and skips its root-find.
    boxes: Vec<Rect>,
    pieces: Vec<Piece>,
    h: f64,
}

impl Band {
    pub fn new(path: &BezPath, h: f64, corners: Corners, cap: kurbo::Cap) -> Self {
        let (mut all, mut pieces) = (Vec::new(), Vec::new());
        for sub in ondin_core::geometry::subpaths(path) {
            let closed = matches!(sub.elements().last(), Some(kurbo::PathEl::ClosePath));
            let segs: Vec<PathSeg> = sub.segments().filter(|s| !degenerate(s)).collect();
            let n = segs.len();
            if n == 0 {
                // A dot draws only its caps.
                if let Some(kurbo::PathEl::MoveTo(p)) = sub.elements().first() {
                    pieces.extend(cap_piece(*p, Vec2::new(1.0, 0.0), h, cap));
                    pieces.extend(cap_piece(*p, Vec2::new(-1.0, 0.0), h, cap));
                }
                continue;
            }
            let joints = if closed { n } else { n - 1 };
            for i in 0..joints {
                let (a, b) = (&segs[i], &segs[(i + 1) % n]);
                pieces.extend(join_piece(a.end(), end_dir(a), start_dir(b), h, corners));
            }
            if !closed {
                pieces.extend(cap_piece(segs[0].start(), -start_dir(&segs[0]), h, cap));
                pieces.extend(cap_piece(segs[n - 1].end(), end_dir(&segs[n - 1]), h, cap));
            }
            all.extend(segs);
        }
        let boxes = all.iter().map(hull).collect();
        Self {
            segs: all,
            boxes,
            pieces,
            h,
        }
    }

    /// Whether `p` is in the ink, or `None` within [`TOL`] of its edge.
    pub fn contains(&self, p: Point) -> Option<bool> {
        let mut inside = false;
        for (seg, b) in self.segs.iter().zip(&self.boxes) {
            if box_dist(b, p) > self.h + TOL {
                continue;
            }
            if seg.nearest(p, 1e-7).distance_sq.sqrt() > self.h + TOL {
                continue;
            }
            // The slab ends on the normals through the segment's ends.
            let a0 = (p - seg.start()).dot(start_dir(seg));
            let a1 = (p - seg.end()).dot(end_dir(seg));
            if a0.abs() < TOL || a1.abs() < TOL {
                return None;
            }
            // The slab is swept by the segment's normals: `p` is in it where some
            // normal passes through it within `h` — where the segment's distance to
            // `p` is stationary, not only at its nearest point. Where a curve turns
            // tighter than `h` those differ, and the stroke covers both.
            for dist in normal_feet(seg, p) {
                if (dist - self.h).abs() < TOL {
                    return None;
                }
                inside |= dist < self.h;
            }
        }
        for piece in &self.pieces {
            let s = match piece {
                Piece::Poly(poly) => poly_dist(poly, p),
                Piece::Disk(c) => (p - *c).hypot() - self.h,
            };
            if s.abs() < TOL {
                return None;
            }
            inside |= s < 0.0;
        }
        Some(inside)
    }
}

/// The distance from `p` to each point inside `seg` whose normal passes through
/// `p`: the roots of `(p - q(t))·q'(t)`, found by sampling and bisection.
fn normal_feet(seg: &PathSeg, p: Point) -> Vec<f64> {
    const SAMPLES: usize = 96;
    // A line has one foot, its projection.
    if let PathSeg::Line(l) = seg {
        let ab = l.p1 - l.p0;
        let t = (p - l.p0).dot(ab) / ab.hypot2();
        return match t > 1e-6 && t < 1.0 - 1e-6 {
            true => vec![(l.p0 + ab * t - p).hypot()],
            false => Vec::new(),
        };
    }
    let c = seg.to_cubic();
    let d = c.deriv();
    let f = |t: f64| (p - c.eval(t)).dot(d.eval(t).to_vec2());
    let mut out = Vec::new();
    let mut prev = (0.0, f(0.0));
    for k in 1..=SAMPLES {
        let t = k as f64 / SAMPLES as f64;
        let ft = f(t);
        if prev.1.signum() != ft.signum() {
            let (mut lo, mut hi) = (prev.0, t);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if f(mid).signum() == prev.1.signum() {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let root = 0.5 * (lo + hi);
            if root > 1e-6 && root < 1.0 - 1e-6 {
                out.push((p - c.eval(root)).hypot());
            }
        }
        prev = (t, ft);
    }
    out
}

/// The join a stroke of half-width `h` adds on the outside of a corner at `p`
/// turning from `t_in` to `t_out`; `None` where the path runs on smoothly.
fn join_piece(p: Point, t_in: Vec2, t_out: Vec2, h: f64, corners: Corners) -> Option<Piece> {
    let cross = t_in.cross(t_out);
    if cross.abs() < 1e-9 && t_in.dot(t_out) > 0.0 {
        return None;
    }
    if let Corners::Svg(kurbo::Join::Round, _) = corners {
        return Some(Piece::Disk(p));
    }
    if cross.abs() < 1e-9 {
        // A U-turn: no outside to pick, and its bevel is the segments' butt ends.
        return None;
    }
    let side = -cross.signum();
    let (o1, o2) = (t_in.turn_90() * side, t_out.turn_90() * side);
    let bisector = (o1 + o2).normalize();
    let cos_half = o1.dot(bisector);
    let bevel = Piece::Poly(vec![p, p + o1 * h, p + o2 * h]);
    let quad = [
        p,
        p + o1 * h,
        p + bisector * (h / cos_half.max(1e-9)),
        p + o2 * h,
    ];
    Some(match corners {
        Corners::Svg(kurbo::Join::Miter, limit) if cos_half > 1e-9 && 1.0 / cos_half <= limit => {
            Piece::Poly(quad.to_vec())
        }
        Corners::Svg(..) => bevel,
        Corners::Clip(_) if cos_half < 1e-6 => return None,
        Corners::Clip(limit) => Piece::Poly(clipped(&quad, p, bisector, limit * h)),
    })
}

/// The cap a stroke of half-width `h` ends in at `p`, heading out along `t`.
fn cap_piece(p: Point, t: Vec2, h: f64, cap: kurbo::Cap) -> Option<Piece> {
    let n = t.turn_90() * h;
    match cap {
        kurbo::Cap::Butt => None,
        kurbo::Cap::Round => Some(Piece::Disk(p)),
        kurbo::Cap::Square => Some(Piece::Poly(vec![
            p + n,
            p + n + t * h,
            p - n + t * h,
            p - n,
        ])),
    }
}

/// A closed shape's fill, read non-zero.
pub struct Fill {
    segs: Vec<PathSeg>,
    boxes: Vec<Rect>,
    edges: Vec<(Point, Point)>,
}

impl Fill {
    pub fn new(shape: &BezPath) -> Self {
        let segs: Vec<PathSeg> = shape.segments().filter(|s| !degenerate(s)).collect();
        Self {
            boxes: segs.iter().map(hull).collect(),
            segs,
            edges: flat(shape),
        }
    }

    /// Whether `p` is in the fill, or `None` within [`TOL`] of its outline.
    pub fn contains(&self, p: Point) -> Option<bool> {
        let near = self
            .segs
            .iter()
            .zip(&self.boxes)
            .any(|(s, b)| box_dist(b, p) < TOL && s.nearest(p, 1e-7).distance_sq < TOL * TOL);
        (!near).then(|| winding(&self.edges, p) != 0)
    }
}

/// `a` or `b`, unknown where either is unknown and the other does not decide it.
pub fn or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// `a` and `b`, unknown where either is unknown and the other does not decide it.
pub fn and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// Everything wrong with `result` against `expected` over the box `area`: points
/// on the wrong side, specks, stray edges, and more than `max_segments` segments.
/// `None` is an empty result. The result is read non-zero, and must read the same
/// even-odd.
pub fn check(
    result: Option<&BezPath>,
    area: Rect,
    max_segments: usize,
    expected: impl Fn(Point) -> Option<bool>,
) -> Vec<String> {
    check_read(result, area, max_segments, false, 1.0, expected)
}

/// [`check`], reading the result even-odd where `even_odd` says so, and taking a
/// subpath of less than `speck` square units for junk.
pub fn check_read(
    result: Option<&BezPath>,
    area: Rect,
    max_segments: usize,
    even_odd: bool,
    speck: f64,
    expected: impl Fn(Point) -> Option<bool>,
) -> Vec<String> {
    let filled = |w: i32| match even_odd {
        true => w % 2 != 0,
        false => w != 0,
    };
    let mut out = Vec::new();
    let empty = BezPath::new();
    let r = result.unwrap_or(&empty);
    let rf = flat(r);
    // A result is edited afterwards: a refit that gave up leaves a raw polygon,
    // hundreds of points.
    let segments = r.segments().count();
    if segments > max_segments {
        out.push(format!("{segments} segments, more than {max_segments}"));
    }
    let (mut wrong, mut parity, mut first) = (0, 0, None);
    for i in 0..=GRID {
        for j in 0..=GRID {
            let p = Point::new(
                area.x0 + area.width() * i as f64 / GRID as f64,
                area.y0 + area.height() * j as f64 / GRID as f64,
            );
            let w = winding(&rf, p);
            if !even_odd && (w != 0) != (w % 2 != 0) {
                parity += 1;
            }
            if let Some(want) = expected(p)
                && filled(w) != want
            {
                wrong += 1;
                first.get_or_insert((p, want));
            }
        }
    }
    if let Some((p, want)) = first {
        out.push(format!(
            "{wrong} points on the wrong side, first ({:.1}, {:.1}) should be {}",
            p.x,
            p.y,
            if want { "filled" } else { "empty" }
        ));
    }
    if parity > 0 {
        out.push(format!(
            "{parity} points where non-zero and even-odd disagree"
        ));
    }
    for (k, sub) in ondin_core::geometry::subpaths(r).iter().enumerate() {
        let area = sub.area().abs();
        if area < speck {
            out.push(format!("subpath {k} is a speck ({area:.2} units²)"));
        }
    }
    // Every edge bounds the fill: filled on exactly one side. Probed close in,
    // since a shape eroded past its width leaves legitimately thin slivers; a
    // subpath thinner on average than the probe's reach is not probed.
    const PROBE: f64 = 0.01;
    let (mut stray, mut first_stray) = (0, None);
    let probed = ondin_core::geometry::subpaths(r)
        .into_iter()
        .filter(|s| 2.0 * s.area().abs() / s.perimeter(1e-3).max(1e-12) > 4.0 * PROBE);
    for seg in probed.flat_map(|s| s.segments().collect::<Vec<_>>()) {
        if seg.arclen(1e-3) < 1.0 {
            continue;
        }
        let c = seg.to_cubic();
        let t = c.deriv().eval(0.5).to_vec2();
        if t.hypot() < 1e-9 {
            continue;
        }
        let n = t.normalize().turn_90() * PROBE;
        let m = c.eval(0.5);
        if filled(winding(&rf, m + n)) == filled(winding(&rf, m - n)) {
            stray += 1;
            first_stray.get_or_insert(m);
        }
    }
    if let Some(m) = first_stray {
        out.push(format!(
            "{stray} edges with the fill on both sides or neither, first at ({:.1}, {:.1})",
            m.x, m.y
        ));
    }
    out
}

/// Run `case` over every item on a pool of threads, the oracle and the arithmetic
/// both being slow in a debug build, and fail with every failure listed. Items are
/// handed out one at a time, so one slow case does not hold up those behind it.
pub fn run_all<T: Sync>(items: &[(String, T)], case: impl Fn(&T) -> Vec<String> + Sync) {
    run_all_known(items, &[], case)
}

/// [`run_all`], excusing the failures of `known`: a case's name and a phrase its
/// failure contains. Each must still fail, so one that is fixed is struck off the
/// list rather than left to excuse a regression later.
pub fn run_all_known<T: Sync>(
    items: &[(String, T)],
    known: &[(&str, &str)],
    case: impl Fn(&T) -> Vec<String> + Sync,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut failures: Vec<(usize, String)> = std::thread::scope(|s| {
        let pool: Vec<_> = (0..workers)
            .map(|_| {
                let (case, next) = (&case, &next);
                s.spawn(move || {
                    let mut found = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((name, item)) = items.get(i) else {
                            return found;
                        };
                        found.extend(case(item).into_iter().map(|f| (i, format!("{name}: {f}"))));
                    }
                })
            })
            .collect();
        pool.into_iter()
            .flat_map(|w| w.join().expect("a case's worker panicked"))
            .collect()
    });
    failures.sort_by_key(|(i, _)| *i);
    let excused = |f: &str| {
        known
            .iter()
            .any(|(name, phrase)| f.starts_with(&format!("{name}: ")) && f.contains(phrase))
    };
    let stale: Vec<String> = known
        .iter()
        .filter(|(name, phrase)| {
            !failures
                .iter()
                .any(|(_, f)| f.starts_with(&format!("{name}: ")) && f.contains(phrase))
        })
        .map(|(name, phrase)| format!("{name}: \"{phrase}\" no longer fails; strike it off"))
        .collect();
    let failures: Vec<String> = failures
        .into_iter()
        .map(|(_, f)| f)
        .filter(|f| !excused(f))
        .chain(stale)
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures over {} cases:\n{}",
        failures.len(),
        items.len(),
        failures.join("\n")
    );
}

fn degenerate(s: &PathSeg) -> bool {
    (s.end() - s.start()).hypot() <= 1e-9 && s.arclen(1e-6) <= 1e-9
}

fn hull(s: &PathSeg) -> Rect {
    let c = s.to_cubic();
    Rect::from_points(c.p0, c.p3).union_pt(c.p1).union_pt(c.p2)
}

/// The distance from `p` to the box `b`, zero inside it.
fn box_dist(b: &Rect, p: Point) -> f64 {
    let dx = (b.x0 - p.x).max(p.x - b.x1).max(0.0);
    let dy = (b.y0 - p.y).max(p.y - b.y1).max(0.0);
    dx.hypot(dy)
}

pub fn start_dir(s: &PathSeg) -> Vec2 {
    let c = s.to_cubic();
    [c.p1, c.p2, c.p3]
        .into_iter()
        .map(|p| p - c.p0)
        .find(|v| v.hypot() > 1e-9)
        .unwrap_or(Vec2::new(1.0, 0.0))
        .normalize()
}

pub fn end_dir(s: &PathSeg) -> Vec2 {
    let c = s.to_cubic();
    [c.p2, c.p1, c.p0]
        .into_iter()
        .map(|p| c.p3 - p)
        .find(|v| v.hypot() > 1e-9)
        .unwrap_or(Vec2::new(1.0, 0.0))
        .normalize()
}

/// `path` flattened finely, for [`winding`]: kurbo's own ray cast over cubics
/// miscounts some points near a curve's top, which a grid finds.
pub fn flat(path: &BezPath) -> Vec<(Point, Point)> {
    let mut edges = Vec::new();
    let (mut at, mut start) = (Point::ZERO, Point::ZERO);
    kurbo::flatten(path, 0.002, |el| match el {
        kurbo::PathEl::MoveTo(p) => (at, start) = (p, p),
        kurbo::PathEl::LineTo(p) => {
            edges.push((at, p));
            at = p;
        }
        kurbo::PathEl::ClosePath => {
            edges.push((at, start));
            at = start;
        }
        _ => {}
    });
    edges
}

/// The winding number of the flattened path `edges` at `p`.
pub fn winding(edges: &[(Point, Point)], p: Point) -> i32 {
    let mut w = 0;
    for (a, b) in edges {
        let side = (*b - *a).cross(p - *a);
        if a.y <= p.y && b.y > p.y && side > 0.0 {
            w += 1;
        } else if b.y <= p.y && a.y > p.y && side < 0.0 {
            w -= 1;
        }
    }
    w
}

/// The convex polygon `poly` less what lies more than `limit` from `at` along `dir`.
fn clipped(poly: &[Point], at: Point, dir: Vec2, limit: f64) -> Vec<Point> {
    let reach = |p: Point| (p - at).dot(dir) - limit;
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (ra, rb) = (reach(a), reach(b));
        if ra <= 0.0 {
            out.push(a);
        }
        if (ra <= 0.0) != (rb <= 0.0) {
            out.push(a.lerp(b, ra / (ra - rb)));
        }
    }
    out
}

/// The distance from `p` to the line segment `a`–`b`.
fn seg_dist(a: Point, b: Point, p: Point) -> f64 {
    let ab = b - a;
    let t = if ab.hypot2() > 0.0 {
        ((p - a).dot(ab) / ab.hypot2()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t - p).hypot()
}

/// Signed distance to a convex polygon: negative inside.
fn poly_dist(poly: &[Point], p: Point) -> f64 {
    let n = poly.len();
    let d = (0..n)
        .map(|i| seg_dist(poly[i], poly[(i + 1) % n], p))
        .fold(f64::INFINITY, f64::min);
    let area: f64 = (0..n)
        .map(|i| poly[i].to_vec2().cross(poly[(i + 1) % n].to_vec2()))
        .sum();
    let inside = (0..n).all(|i| {
        let e = poly[(i + 1) % n] - poly[i];
        e.cross(p - poly[i]) * area.signum() >= 0.0
    });
    if inside { -d } else { d }
}

pub fn polygon(points: &[(f64, f64)]) -> BezPath {
    let mut p = BezPath::new();
    for (i, q) in points.iter().enumerate() {
        if i == 0 { p.move_to(*q) } else { p.line_to(*q) }
    }
    p.close_path();
    p
}

pub fn circle(c: (f64, f64), r: f64) -> BezPath {
    let mut p = kurbo::Ellipse::new(c, (r, r), 0.0).to_path(0.1);
    p.close_path();
    p
}

/// The closed shapes: each once, some also wound the other way.
pub fn closed_shapes() -> Vec<(&'static str, BezPath)> {
    use std::f64::consts::TAU;
    let regular = |n: usize, r: f64, inner: Option<f64>| {
        let steps = if inner.is_some() { 2 * n } else { n };
        let pts: Vec<(f64, f64)> = (0..steps)
            .map(|k| {
                let a = k as f64 / steps as f64 * TAU - TAU / 4.0;
                let rr = if k % 2 == 1 { inner.unwrap_or(r) } else { r };
                (rr * a.cos(), rr * a.sin())
            })
            .collect();
        polygon(&pts)
    };
    // A straight-sided path drawn with the pen: cubics whose handles sit on their
    // chords.
    let mut pen_square = BezPath::new();
    let corners = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    pen_square.move_to(corners[0]);
    for i in 0..4 {
        let (a, b) = (Point::from(corners[i]), Point::from(corners[(i + 1) % 4]));
        pen_square.curve_to(a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b);
    }
    pen_square.close_path();
    let rounded = ondin_core::geometry::round_corners(
        &Rect::new(0.0, 0.0, 160.0, 100.0).to_path(0.1),
        &[20.0, 20.0, 20.0, 20.0],
    );
    let mut donut = circle((0.0, 0.0), 100.0);
    donut.extend(circle((0.0, 0.0), 45.0).reverse_subpaths());
    let mut pair = Rect::new(0.0, 0.0, 60.0, 60.0).to_path(0.1);
    pair.extend(Rect::new(75.0, 10.0, 135.0, 70.0).to_path(0.1));
    let blob = BezPath::from_svg(
        "M0,0 C100,-60 200,60 300,0 C360,100 240,200 150,120 C60,200 -60,100 0,0 Z",
    )
    .unwrap();
    // A thick C: two arcs joined by straight ends, its inside concave.
    let c_shape = {
        let mut p = BezPath::new();
        let outer = kurbo::Arc::new((0.0, 0.0), (100.0, 100.0), 0.6, TAU - 1.2, 0.0);
        let inner = kurbo::Arc::new((0.0, 0.0), (60.0, 60.0), TAU - 0.6, -(TAU - 1.2), 0.0);
        let at = |r: f64, a: f64| Point::new(r * a.cos(), r * a.sin());
        p.move_to(at(100.0, 0.6));
        outer.to_cubic_beziers(0.1, |a, b, c| p.curve_to(a, b, c));
        p.line_to(at(60.0, TAU - 0.6));
        inner.to_cubic_beziers(0.1, |a, b, c| p.curve_to(a, b, c));
        p.close_path();
        p
    };
    vec![
        ("circle r50", circle((0.0, 0.0), 50.0)),
        ("circle r120", circle((0.0, 0.0), 120.0)),
        (
            "circle reversed",
            circle((0.0, 0.0), 80.0).reverse_subpaths(),
        ),
        ("ellipse", {
            let mut e = kurbo::Ellipse::new((0.0, 0.0), (150.0, 40.0), 0.3).to_path(0.1);
            e.close_path();
            e
        }),
        ("square", Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1)),
        ("thin rect", Rect::new(0.0, 0.0, 200.0, 12.0).to_path(0.1)),
        ("pen square", pen_square),
        ("rounded rect", rounded),
        (
            "triangle",
            polygon(&[(0.0, 0.0), (160.0, 0.0), (60.0, 120.0)]),
        ),
        (
            "triangle reversed",
            polygon(&[(60.0, 120.0), (160.0, 0.0), (0.0, 0.0)]),
        ),
        (
            "triangle with extra points",
            // A collinear point, a near-collinear one, and a doubled corner.
            polygon(&[
                (0.0, 0.0),
                (80.0, 0.0),
                (160.0, 0.0),
                (110.0, 58.0),
                (60.0, 120.0),
                (60.0, 120.0),
                (31.0, 61.0),
            ]),
        ),
        (
            "sliver triangle",
            polygon(&[(0.0, 0.0), (200.0, 0.0), (0.0, 26.0)]),
        ),
        ("pentagon", regular(5, 90.0, None)),
        ("star", regular(5, 120.0, Some(50.0))),
        (
            "L",
            polygon(&[
                (0.0, 0.0),
                (40.0, 0.0),
                (40.0, 100.0),
                (120.0, 100.0),
                (120.0, 140.0),
                (0.0, 140.0),
            ]),
        ),
        (
            "plus",
            polygon(&[
                (40.0, 0.0),
                (80.0, 0.0),
                (80.0, 40.0),
                (120.0, 40.0),
                (120.0, 80.0),
                (80.0, 80.0),
                (80.0, 120.0),
                (40.0, 120.0),
                (40.0, 80.0),
                (0.0, 80.0),
                (0.0, 40.0),
                (40.0, 40.0),
            ]),
        ),
        (
            "comb",
            polygon(&[
                (0.0, 0.0),
                (20.0, 0.0),
                (20.0, 80.0),
                (40.0, 80.0),
                (40.0, 0.0),
                (60.0, 0.0),
                (60.0, 80.0),
                (80.0, 80.0),
                (80.0, 0.0),
                (100.0, 0.0),
                (100.0, 100.0),
                (0.0, 100.0),
            ]),
        ),
        ("blob", blob),
        ("C", c_shape),
        ("donut", donut),
        ("two squares", pair),
    ]
}

/// The open paths: lines, a zigzag, curves and a hairpin.
pub fn open_paths() -> Vec<(&'static str, BezPath)> {
    let parse = |d: &str| BezPath::from_svg(d).unwrap();
    vec![
        ("line", parse("M0,0 L200,0")),
        ("diagonal", parse("M0,0 L140,90")),
        ("zigzag", parse("M0,0 L40,60 L80,0 L120,60 L160,0")),
        ("hairpin", parse("M0,0 L200,0 L0,12")),
        ("arc", parse("M0,0 C0,-80 160,-80 160,0")),
        ("s-curve", parse("M0,0 C60,-90 100,90 160,0")),
        (
            "tight curl",
            parse("M0,0 C40,0 40,30 20,30 C0,30 0,10 30,10"),
        ),
    ]
}
