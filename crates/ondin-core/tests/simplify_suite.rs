//! *Simplify* (`build::simplify_outline`) over drawn polylines and the catalogue's
//! curves, each result checked against what simplify promises rather than a stored
//! answer: it stays within reach of the outline both ways, keeps the corners, keeps
//! the subpaths, and never loops. A refit with no fewer segments is not written, as
//! `simplify_path` refuses it, so the drawn shape stands; [`simplify_reduces_drawn_polylines`]
//! pins that the polylines it exists for do come back smaller.

mod common;

use ondin_core::build::{SIMPLIFY_ACCURACY, SIMPLIFY_CORNER_DEG, simplify_outline};
use ondin_core::kurbo::{self, BezPath, ParamCurve, ParamCurveDeriv, PathEl, Point};

/// How far apart the outline and its refit may be: twice the accuracy simplify
/// fits to, which its measure of error does not bound point by point.
const REACH: f64 = 2.0 * SIMPLIFY_ACCURACY;

/// A corner this sharp, in degrees, must keep an anchor: well past
/// `SIMPLIFY_CORNER_DEG`, so a turn near the threshold is not a coin toss.
const SHARP_DEG: f64 = 3.0 * SIMPLIFY_CORNER_DEG;

/// `points` as a polyline, closed or not.
fn poly(points: &[Point], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    for (i, q) in points.iter().enumerate() {
        if i == 0 { p.move_to(*q) } else { p.line_to(*q) }
    }
    if closed {
        p.close_path();
    }
    p
}

/// A deterministic wobble in [-1, 1], for hand-drawn jitter.
fn wobble(i: usize) -> f64 {
    ((i as f64 * 12.9898).sin() * 43758.5453).rem_euclid(1.0) * 2.0 - 1.0
}

/// The inputs: drawn polylines, and the catalogue's shapes as they are.
fn inputs() -> Vec<(String, BezPath)> {
    use std::f64::consts::TAU;
    let ring = |n: usize, r: f64, jitter: f64| {
        let pts: Vec<Point> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * TAU;
                let rr = r + jitter * wobble(i);
                Point::new(rr * a.cos(), rr * a.sin())
            })
            .collect();
        poly(&pts, true)
    };
    // A square with every side cut into many collinear steps, slightly shaken.
    let dense_square = {
        let corners = [(0.0, 0.0), (120.0, 0.0), (120.0, 120.0), (0.0, 120.0)];
        let mut pts = Vec::new();
        for k in 0..4 {
            let (a, b) = (Point::from(corners[k]), Point::from(corners[(k + 1) % 4]));
            for s in 0..30 {
                let p = a.lerp(b, s as f64 / 30.0);
                let jitter = if s == 0 {
                    0.0
                } else {
                    0.15 * wobble(k * 30 + s)
                };
                pts.push(p + (b - a).normalize().turn_90() * jitter);
            }
        }
        poly(&pts, true)
    };
    // A star whose sides are subdivided: its tips and notches must survive.
    let dense_star = {
        let mut pts = Vec::new();
        for k in 0..10 {
            let r = |k: usize| if k.is_multiple_of(2) { 120.0 } else { 50.0 };
            let at = |k: usize| {
                let a = k as f64 / 10.0 * TAU - TAU / 4.0;
                Point::new(r(k) * a.cos(), r(k) * a.sin())
            };
            let (a, b) = (at(k), at((k + 1) % 10));
            for s in 0..12 {
                pts.push(a.lerp(b, s as f64 / 12.0));
            }
        }
        poly(&pts, true)
    };
    let sampled = |c: kurbo::CubicBez, n: usize| {
        let pts: Vec<Point> = (0..=n).map(|i| c.eval(i as f64 / n as f64)).collect();
        poly(&pts, false)
    };
    let zigzag = {
        let pts: Vec<Point> = (0..=200)
            .map(|i| {
                let x = i as f64;
                // Teeth 20 wide, so each tip and trough falls on a sample.
                let tooth = (x / 20.0).fract();
                Point::new(x, 40.0 * (1.0 - (2.0 * tooth - 1.0).abs()))
            })
            .collect();
        poly(&pts, false)
    };
    let flattened = |p: &BezPath| {
        let mut out = BezPath::new();
        kurbo::flatten(p, 0.05, |el| out.push(el));
        out
    };
    let shapes: std::collections::HashMap<_, _> = common::closed_shapes().into_iter().collect();
    let mut v = vec![
        ("circle, 64 lines".to_string(), ring(64, 50.0, 0.0)),
        ("big circle, 200 lines".to_string(), ring(200, 200.0, 0.0)),
        ("hand-drawn circle".to_string(), ring(160, 80.0, 0.3)),
        ("tiny circle".to_string(), ring(32, 2.0, 0.0)),
        ("dense square".to_string(), dense_square),
        ("dense star".to_string(), dense_star),
        (
            "s-curve, 100 points".to_string(),
            sampled(
                kurbo::CubicBez::new((0.0, 0.0), (60.0, -90.0), (100.0, 90.0), (160.0, 0.0)),
                100,
            ),
        ),
        ("zigzag, 200 points".to_string(), zigzag),
        (
            "rounded rect, flattened".to_string(),
            flattened(&shapes["rounded rect"]),
        ),
        ("blob, flattened".to_string(), flattened(&shapes["blob"])),
    ];
    for (name, path) in common::closed_shapes() {
        v.push((format!("{name}, as drawn"), path));
    }
    v
}

/// `path`'s points, sampled finely, and its edges, for the distances below.
fn samples(path: &BezPath) -> (Vec<Point>, Vec<(Point, Point)>) {
    let edges = common::flat(path);
    let mut pts: Vec<Point> = Vec::new();
    for (a, b) in &edges {
        let n = ((*b - *a).hypot() / 0.5).ceil().max(1.0) as usize;
        pts.extend((0..n).map(|k| a.lerp(*b, k as f64 / n as f64)));
    }
    (pts, edges)
}

/// The furthest any of `pts` is from the polyline `edges`, and where.
fn furthest(pts: &[Point], edges: &[(Point, Point)]) -> (f64, Point) {
    let dist = |p: Point| {
        edges
            .iter()
            .map(|(a, b)| {
                let ab = *b - *a;
                let t = ((p - *a).dot(ab) / ab.hypot2().max(1e-18)).clamp(0.0, 1.0);
                (*a + ab * t - p).hypot()
            })
            .fold(f64::INFINITY, f64::min)
    };
    pts.iter()
        .map(|p| (dist(*p), *p))
        .fold((0.0, Point::ZERO), |m, x| if x.0 > m.0 { x } else { m })
}

/// The corners of `path` sharper than [`SHARP_DEG`]: where one segment ends and the
/// next leaves at that much of a turn.
fn sharp_corners(path: &BezPath) -> Vec<Point> {
    let mut out = Vec::new();
    for sub in ondin_core::geometry::subpaths(path) {
        let closed = matches!(sub.elements().last(), Some(PathEl::ClosePath));
        let segs: Vec<_> = sub
            .segments()
            .filter(|s| (s.end() - s.start()).hypot() > 1e-9)
            .collect();
        let n = segs.len();
        let joints = if closed { n } else { n.saturating_sub(1) };
        for i in 0..joints {
            let (a, b) = (&segs[i], &segs[(i + 1) % n]);
            let (t_in, t_out) = (common::end_dir(a), common::start_dir(b));
            if t_in.dot(t_out) < SHARP_DEG.to_radians().cos() {
                out.push(a.end());
            }
        }
    }
    out
}

/// Everything simplify promises and `path`'s refit breaks.
fn simplify_case(path: &BezPath) -> Vec<String> {
    let mut out = Vec::new();
    let count = |p: &BezPath| p.segments().count();
    // As `simplify_path` writes it: a refit with no fewer segments is not written.
    let fitted = match simplify_outline(path) {
        f if count(&f) < count(path) => f,
        _ => path.clone(),
    };
    let (from, from_edges) = samples(path);
    let (to, to_edges) = samples(&fitted);
    let (d, at) = furthest(&from, &to_edges);
    if d > REACH {
        out.push(format!(
            "the outline strays {d:.2} from the refit at ({:.1}, {:.1})",
            at.x, at.y
        ));
    }
    let (d, at) = furthest(&to, &from_edges);
    if d > REACH {
        out.push(format!(
            "the refit strays {d:.2} from the outline at ({:.1}, {:.1})",
            at.x, at.y
        ));
    }
    let anchors: Vec<Point> = fitted
        .elements()
        .iter()
        .filter_map(|e| e.end_point())
        .collect();
    for c in sharp_corners(path) {
        if !anchors.iter().any(|a| (*a - c).hypot() <= REACH) {
            out.push(format!("the corner at ({:.1}, {:.1}) is lost", c.x, c.y));
        }
    }
    let subs = |p: &BezPath| ondin_core::geometry::subpaths(p).len();
    if subs(&fitted) != subs(path) {
        out.push(format!("{} subpaths from {}", subs(&fitted), subs(path)));
    }
    // A cubic that heads back against its own chord has looped.
    for seg in fitted.segments() {
        let c = seg.to_cubic();
        let chord = c.p3 - c.p0;
        let d = c.deriv();
        let loops = (1..16).any(|k| {
            let v = d.eval(k as f64 / 16.0).to_vec2();
            v.dot(chord) < -0.1 * v.hypot() * chord.hypot()
        });
        if loops {
            out.push(format!("a curve loops at ({:.1}, {:.1})", c.p0.x, c.p0.y));
            break;
        }
    }
    out
}

/// Every input's refit keeps simplify's promises.
#[test]
fn simplify_keeps_its_promises() {
    common::run_all(&inputs(), simplify_case);
}

/// The drawn polylines come back as few segments as their shape needs: a square's
/// four sides and a star's ten as lines, a traced circle as a handful of curves.
#[test]
fn simplify_reduces_drawn_polylines() {
    let inputs: std::collections::HashMap<String, BezPath> = inputs().into_iter().collect();
    let lines = |p: &BezPath| {
        p.segments()
            .filter(|s| matches!(s, kurbo::PathSeg::Line(_)))
            .count()
    };
    for (name, at_most, all_lines) in [
        ("circle, 64 lines", 8, false),
        ("big circle, 200 lines", 8, false),
        ("hand-drawn circle", 12, false),
        ("dense square", 4, true),
        ("dense star", 10, true),
        ("zigzag, 200 points", 20, true),
        ("s-curve, 100 points", 4, false),
    ] {
        let fitted = simplify_outline(&inputs[name]);
        let n = fitted.segments().count();
        assert!(
            n <= at_most,
            "{name}: {n} segments, wanted at most {at_most}"
        );
        if all_lines {
            assert_eq!(lines(&fitted), n, "{name}: every side a line");
        }
    }
}
