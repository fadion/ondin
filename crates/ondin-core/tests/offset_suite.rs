//! *Offset path* over a catalogue of shapes and distances, each result checked
//! against an oracle rather than a stored answer.
//!
//! Where a point belongs is known without the boolean arithmetic. The band is the
//! outline stroked `2·|d|` wide: each segment's slab (points whose nearest point on
//! it is not an end, within `|d|`) and each corner's join, mitred up to
//! `OFFSET_MITER_LIMIT` and bevelled past it. A grow is the shape and the band; a
//! shrink is the shape less the band. The oracle reads that point by point and stays
//! silent within `TOL` of any edge of it. Structural checks then catch what a fill
//! test cannot see: specks, spikes and edges with the fill on both sides or neither,
//! which show as stray points and lines when the result is edited.

use ondin_core::build::{OFFSET_MITER_LIMIT, offset_outline};
use ondin_core::kurbo::{
    self, BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, Point, Shape, Vec2,
};

/// The oracle's blind band either side of an expected edge, in units.
const TOL: f64 = 0.6;
/// Grid points per side of the sampled box.
const GRID: usize = 56;
/// The most segments an offset of any shape here may come back with.
const MAX_SEGMENTS: usize = 80;

/// The outline's segments, and the joins of its stroke at half-width `h`.
struct Oracle {
    segs: Vec<kurbo::PathSeg>,
    /// Each segment's control-point box: a point further from it than a reach is
    /// further from the segment, and skips its root-find.
    boxes: Vec<kurbo::Rect>,
    joins: Vec<Vec<Point>>,
    shape: Vec<(Point, Point)>,
    h: f64,
}

impl Oracle {
    fn new(shape: &BezPath, h: f64) -> Self {
        let (mut all, mut joins) = (Vec::new(), Vec::new());
        for sub in ondin_core::geometry::subpaths(shape) {
            let segs: Vec<_> = sub
                .segments()
                .filter(|s| (s.end() - s.start()).hypot() > 1e-9 || s.arclen(1e-6) > 1e-9)
                .collect();
            let n = segs.len();
            for i in 0..n {
                let (a, b) = (&segs[i], &segs[(i + 1) % n]);
                let (t_in, t_out) = (end_dir(a), start_dir(b));
                let cross = t_in.cross(t_out);
                if cross.abs() < 1e-9 {
                    continue;
                }
                // On the outside of the turn: a mitre, clipped square across its
                // bisector at `OFFSET_MITER_LIMIT · h` from the corner.
                let side = -cross.signum();
                let (o1, o2) = (t_in.turn_90() * side, t_out.turn_90() * side);
                let p = a.end();
                let bisector = (o1 + o2).normalize();
                let cos_half = o1.dot(bisector);
                if cos_half < 1e-6 {
                    continue;
                }
                let quad = [p, p + o1 * h, p + bisector * (h / cos_half), p + o2 * h];
                joins.push(clipped(&quad, p, bisector, OFFSET_MITER_LIMIT * h));
            }
            all.extend(segs);
        }
        let boxes = all
            .iter()
            .map(|s| {
                let c = s.to_cubic();
                kurbo::Rect::from_points(c.p0, c.p3)
                    .union_pt(c.p1)
                    .union_pt(c.p2)
            })
            .collect();
        Self {
            segs: all,
            boxes,
            joins,
            shape: flat(shape),
            h,
        }
    }

    /// Whether `p` is in the stroke, or `None` within `TOL` of its edge.
    fn in_band(&self, p: Point) -> Option<bool> {
        use kurbo::ParamCurveNearest;
        let mut inside = false;
        for (seg, b) in self.segs.iter().zip(&self.boxes) {
            if box_dist(b, p) > self.h + TOL {
                continue;
            }
            let near = seg.nearest(p, 1e-7);
            let dist = near.distance_sq.sqrt();
            if dist > self.h + TOL {
                continue;
            }
            // The slab ends on the normals through the segment's ends.
            let a0 = (p - seg.start()).dot(start_dir(seg));
            let a1 = (p - seg.end()).dot(end_dir(seg));
            if a0.abs() < TOL || a1.abs() < TOL {
                return None;
            }
            if near.t > 1e-6 && near.t < 1.0 - 1e-6 {
                if (dist - self.h).abs() < TOL {
                    return None;
                }
                inside |= dist < self.h;
            }
        }
        for j in &self.joins {
            let s = poly_dist(j, p);
            if s.abs() < TOL {
                return None;
            }
            inside |= s < 0.0;
        }
        Some(inside)
    }

    /// Whether `p` is in the shape, or `None` within `TOL` of its outline.
    fn in_shape(&self, p: Point) -> Option<bool> {
        use kurbo::ParamCurveNearest;
        let near = self
            .segs
            .iter()
            .zip(&self.boxes)
            .any(|(s, b)| box_dist(b, p) < TOL && s.nearest(p, 1e-7).distance_sq < TOL * TOL);
        (!near).then(|| winding(&self.shape, p) != 0)
    }
}

/// The distance from `p` to the box `b`, zero inside it.
fn box_dist(b: &kurbo::Rect, p: Point) -> f64 {
    let dx = (b.x0 - p.x).max(p.x - b.x1).max(0.0);
    let dy = (b.y0 - p.y).max(p.y - b.y1).max(0.0);
    dx.hypot(dy)
}

fn start_dir(s: &kurbo::PathSeg) -> Vec2 {
    let c = s.to_cubic();
    [c.p1, c.p2, c.p3]
        .into_iter()
        .map(|p| p - c.p0)
        .find(|v| v.hypot() > 1e-9)
        .unwrap()
        .normalize()
}

fn end_dir(s: &kurbo::PathSeg) -> Vec2 {
    let c = s.to_cubic();
    [c.p2, c.p1, c.p0]
        .into_iter()
        .map(|p| c.p3 - p)
        .find(|v| v.hypot() > 1e-9)
        .unwrap()
        .normalize()
}

/// `path` flattened finely, for [`winding`]: kurbo's own ray cast over cubics
/// miscounts points near a curve's top, which a grid finds.
fn flat(path: &BezPath) -> Vec<(Point, Point)> {
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
fn winding(edges: &[(Point, Point)], p: Point) -> i32 {
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

/// What the offset by `d` should hold at `p`, or `None` where the answer is within
/// `TOL` of changing.
fn expected(o: &Oracle, d: f64, p: Point) -> Option<bool> {
    let (shape, band) = (o.in_shape(p), o.in_band(p));
    match (d > 0.0, shape, band) {
        (true, Some(true), _) | (true, _, Some(true)) => Some(true),
        (true, Some(false), Some(false)) => Some(false),
        (false, Some(false), _) | (false, _, Some(true)) => Some(false),
        (false, Some(true), Some(false)) => Some(true),
        _ => None,
    }
}

/// Everything wrong with `result` as the offset of `shape` by `d`.
fn check(shape: &BezPath, d: f64, result: Option<&BezPath>) -> Vec<String> {
    let mut out = Vec::new();
    let o = Oracle::new(shape, d.abs());
    let b = shape
        .bounding_box()
        .inflate(d.abs() * 1.5 + 5.0, d.abs() * 1.5 + 5.0);
    let empty = BezPath::new();
    let r = result.unwrap_or(&empty);
    let rf = flat(r);
    // An offset is edited afterwards: a refit that gave up leaves the raw polygon,
    // hundreds of points.
    let segments = r.segments().count();
    if segments > MAX_SEGMENTS {
        out.push(format!("{segments} segments, more than {MAX_SEGMENTS}"));
    }
    let (mut wrong, mut parity, mut first) = (0, 0, None);
    for i in 0..=GRID {
        for j in 0..=GRID {
            let p = Point::new(
                b.x0 + b.width() * i as f64 / GRID as f64,
                b.y0 + b.height() * j as f64 / GRID as f64,
            );
            let w = winding(&rf, p);
            if (w != 0) != (w % 2 != 0) {
                parity += 1;
            }
            if let Some(want) = expected(&o, d, p)
                && (w != 0) != want
            {
                wrong += 1;
                first.get_or_insert((p, want));
            }
        }
    }
    if wrong > 0 {
        let (p, want) = first.unwrap();
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
        if area < 1.0 {
            out.push(format!("subpath {k} is a speck ({area:.2} units²)"));
        }
    }
    // Every edge bounds the fill: filled on exactly one side.
    let mut stray = 0;
    let mut first_stray = None;
    for seg in r.segments() {
        if seg.arclen(1e-3) < 1.0 {
            continue;
        }
        let c = seg.to_cubic();
        let t = c.deriv().eval(0.5).to_vec2();
        if t.hypot() < 1e-9 {
            continue;
        }
        // Close in: a shrink past a shape's width leaves legitimately thin slivers.
        let n = t.normalize().turn_90() * 0.01;
        let m = c.eval(0.5);
        if (winding(&rf, m + n) != 0) == (winding(&rf, m - n) != 0) {
            stray += 1;
            first_stray.get_or_insert(m);
        }
    }
    if stray > 0 {
        let m = first_stray.unwrap();
        out.push(format!(
            "{stray} edges with the fill on both sides or neither, first at ({:.1}, {:.1})",
            m.x, m.y
        ));
    }
    out
}

fn polygon(points: &[(f64, f64)]) -> BezPath {
    let mut p = BezPath::new();
    for (i, q) in points.iter().enumerate() {
        if i == 0 { p.move_to(*q) } else { p.line_to(*q) }
    }
    p.close_path();
    p
}

fn closed(mut p: BezPath) -> BezPath {
    p.close_path();
    p
}

/// The catalogue: each shape once, some also wound the other way.
fn shapes() -> Vec<(&'static str, BezPath)> {
    use std::f64::consts::TAU;
    let circle = |c: (f64, f64), r: f64| closed(kurbo::Ellipse::new(c, (r, r), 0.0).to_path(0.1));
    let regular = |n: usize, r: f64, inner: Option<f64>| {
        let pts: Vec<(f64, f64)> = (0..n * if inner.is_some() { 2 } else { 1 })
            .map(|k| {
                let steps = if inner.is_some() { 2 * n } else { n };
                let a = k as f64 / steps as f64 * TAU - TAU / 4.0;
                let rr = if k % 2 == 1 { inner.unwrap_or(r) } else { r };
                (rr * a.cos(), rr * a.sin())
            })
            .collect();
        polygon(&pts)
    };
    // A straight-sided path drawn with the pen: cubics whose handles sit on their
    // chords, which `offset_cubic` cannot take.
    let mut pen_square = BezPath::new();
    let corners = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    pen_square.move_to(corners[0]);
    for i in 0..4 {
        let (a, b) = (Point::from(corners[i]), Point::from(corners[(i + 1) % 4]));
        pen_square.curve_to(a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b);
    }
    pen_square.close_path();
    let rounded = ondin_core::geometry::round_corners(
        &kurbo::Rect::new(0.0, 0.0, 160.0, 100.0).to_path(0.1),
        &[20.0, 20.0, 20.0, 20.0],
    );
    let mut donut = circle((0.0, 0.0), 100.0);
    donut.extend(circle((0.0, 0.0), 45.0).reverse_subpaths());
    let mut pair = kurbo::Rect::new(0.0, 0.0, 60.0, 60.0).to_path(0.1);
    pair.extend(kurbo::Rect::new(75.0, 10.0, 135.0, 70.0).to_path(0.1));
    let blob = BezPath::from_svg(
        "M0,0 C100,-60 200,60 300,0 C360,100 240,200 150,120 C60,200 -60,100 0,0 Z",
    )
    .unwrap();
    // A thick C: two arcs and two round ends, its inside concave.
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
        (
            "ellipse",
            closed(kurbo::Ellipse::new((0.0, 0.0), (150.0, 40.0), 0.3).to_path(0.1)),
        ),
        (
            "square",
            kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1),
        ),
        (
            "thin rect",
            kurbo::Rect::new(0.0, 0.0, 200.0, 12.0).to_path(0.1),
        ),
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

const DISTANCES: [f64; 16] = [
    -60.0, -38.0, -21.0, -9.0, -4.0, -1.0, 1.0, 4.0, 9.0, 17.0, 26.0, 38.0, 55.0, 70.0, 90.0, 120.0,
];

/// Every shape at every distance matches the oracle and is well formed. A thread
/// per shape: the oracle is slow in a debug build.
#[test]
fn offset_matches_the_oracle_on_every_shape() {
    let shapes = shapes();
    let cases = shapes.len() * DISTANCES.len();
    let failures: Vec<String> = std::thread::scope(|s| {
        let workers: Vec<_> = shapes
            .iter()
            .map(|(name, shape)| {
                s.spawn(move || {
                    let mut failures = Vec::new();
                    for d in DISTANCES {
                        let result = offset_outline(shape, d).ok();
                        for f in check(shape, d, result.as_ref()) {
                            failures.push(format!("{name} @ {d}: {f}"));
                        }
                    }
                    failures
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("a shape's worker panicked"))
            .collect()
    });
    assert!(
        failures.is_empty(),
        "{} of {cases} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
