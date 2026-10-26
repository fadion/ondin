//! Curves fitted to a polygon: Schneider's algorithm ("An Algorithm for
//! Automatically Fitting Digitized Curves", Graphics Gems, 1990).
//!
//! The polygon is cut at its corners, and each run between two is fitted with as
//! few cubics as keep every one of its points within the accuracy: a cubic is
//! least-squares fitted along the run's end tangents, reparameterised a few times
//! when it is close, and split at its worst point when it is not. Unlike
//! `kurbo::simplify`, which wants a smooth source, this reads a polyline as the
//! samples it is.

use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveDeriv, Point, Vec2};

/// The closed polygon `points` as cubics within `accuracy` of each point, keeping
/// a corner wherever it turns by more than `corner_deg`.
pub fn fit_closed(points: &[Point], accuracy: f64, corner_deg: f64) -> BezPath {
    let n = points.len();
    let mut out = BezPath::new();
    if n < 3 {
        return out;
    }
    let at = |i: usize| points[i % n];
    let cos_corner = corner_deg.to_radians().cos();
    let corners: Vec<usize> = (0..n)
        .filter(|&i| {
            let (a, b) = (at(i) - at(i + n - 1), at(i + 1) - at(i));
            a.normalize().dot(b.normalize()) < cos_corner
        })
        .collect();
    // With no corner the loop runs on smoothly through where it is cut, so both
    // ends of the one run share a tangent.
    let (starts, smooth) = match corners.is_empty() {
        true => (vec![0], true),
        false => (corners, false),
    };
    out.move_to(at(starts[0]));
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(starts[0] + n);
        let run = densified(&(s..=e).map(at).collect::<Vec<_>>(), SPACING);
        let (t1, t2) = if smooth {
            let t = through(at(n - 1), at(0), at(1));
            (t, -t)
        } else {
            (
                lead(&run),
                lead(&run.iter().rev().copied().collect::<Vec<_>>()),
            )
        };
        fit_run(&run, t1, t2, accuracy, 0, &mut out);
    }
    out.close_path();
    out
}

/// The longest stretch of a run the fit checks no point along, in units.
const SPACING: f64 = 2.0;

/// `run` with points added along each edge longer than `spacing`, so a curve
/// fitted across a long straight stretch is checked along it rather than only at
/// its ends, where it could bulge unseen.
fn densified(run: &[Point], spacing: f64) -> Vec<Point> {
    let mut out = vec![run[0]];
    for w in run.windows(2) {
        let steps = ((w[1] - w[0]).hypot() / spacing).ceil().max(1.0) as usize;
        out.extend((1..=steps).map(|k| w[0].lerp(w[1], k as f64 / steps as f64)));
    }
    out
}

/// The direction of travel at `b` on the circle through `a`, `b` and `c`, or along
/// `a`–`c` where they are in line. Exact for points on an arc however unevenly
/// they are spaced, where the chord between the neighbours is not.
fn through(a: Point, b: Point, c: Point) -> Vec2 {
    let chord = (c - a).normalize();
    let (ab, cb) = (a - b, c - b);
    let d = 2.0 * ab.cross(cb);
    if d.abs() < 1e-9 * ab.hypot2().max(cb.hypot2()) {
        return chord;
    }
    // The circle's centre, relative to `b`.
    let o = Vec2::new(
        cb.y * ab.hypot2() - ab.y * cb.hypot2(),
        ab.x * cb.hypot2() - cb.x * ab.hypot2(),
    ) / d;
    let t = o.turn_90().normalize();
    if t.dot(chord) < 0.0 { -t } else { t }
}

/// The direction a run leaves its first point in: towards the first point a few
/// steps along, which averages out the polygon's jitter.
fn lead(run: &[Point]) -> Vec2 {
    let reach = run
        .iter()
        .skip(1)
        .find(|p| (**p - run[0]).hypot() >= 0.5)
        .unwrap_or(&run[run.len() - 1]);
    (*reach - run[0]).normalize()
}

/// Append cubics through `pts` to `out`, leaving along `t1` and arriving back
/// along `t2` (both pointing into the run).
fn fit_run(pts: &[Point], t1: Vec2, t2: Vec2, accuracy: f64, depth: u32, out: &mut BezPath) {
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let span = (last - first).hypot();
    // Two points, or a recursion that is not converging: the chord, as a cubic.
    if pts.len() <= 2 || depth > 24 {
        out.curve_to(first + t1 * span / 3.0, last + t2 * span / 3.0, last);
        return;
    }
    let mut u = chord_lengths(pts);
    let mut c = generate(pts, &u, t1, t2);
    let (mut worst, mut split) = max_error(pts, &u, &c);
    if worst < accuracy {
        out.curve_to(c.p1, c.p2, c.p3);
        return;
    }
    // Chord length is a poor parameter where the points are unevenly spaced, as a
    // polygon offset's are, so a fit within reach is reparameterised first.
    if worst < 50.0 * accuracy {
        for _ in 0..12 {
            u = reparameterised(pts, &u, &c);
            c = generate(pts, &u, t1, t2);
            (worst, split) = max_error(pts, &u, &c);
            if worst < accuracy {
                out.curve_to(c.p1, c.p2, c.p3);
                return;
            }
        }
    }
    let centre = -through(pts[split - 1], pts[split], pts[split + 1]);
    fit_run(&pts[..=split], t1, centre, accuracy, depth + 1, out);
    fit_run(&pts[split..], -centre, t2, accuracy, depth + 1, out);
}

/// Each point's parameter by cumulative chord length, from 0 to 1.
fn chord_lengths(pts: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    let mut total = 0.0;
    u.push(0.0);
    for w in pts.windows(2) {
        total += (w[1] - w[0]).hypot();
        u.push(total);
    }
    u.iter().map(|d| d / total.max(1e-12)).collect()
}

/// The cubic through the run's ends along `t1` and `t2` whose handle lengths fit
/// the points at parameters `u` best, by least squares.
fn generate(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> CubicBez {
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let s = 1.0 - t;
        let (b0, b1, b2, b3) = (s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t);
        let (a1, a2) = (t1 * b1, t2 * b2);
        c00 += a1.dot(a1);
        c01 += a1.dot(a2);
        c11 += a2.dot(a2);
        let tmp = p.to_vec2() - (first.to_vec2() * (b0 + b1) + last.to_vec2() * (b2 + b3));
        x0 += a1.dot(tmp);
        x1 += a2.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let span = (last - first).hypot();
    let (mut l, mut r) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (span / 3.0, span / 3.0)
    };
    // A handle of no length, or one pointing backwards, is the least-squares fit
    // going wrong; Schneider's fallback is a third of the chord.
    if l < 1e-6 * span || r < 1e-6 * span {
        (l, r) = (span / 3.0, span / 3.0);
    }
    CubicBez::new(first, first + t1 * l, last + t2 * r, last)
}

/// The largest distance from a point to where the cubic puts it, and the index of
/// that point (never an end, so a split always shortens the run).
fn max_error(pts: &[Point], u: &[f64], c: &CubicBez) -> (f64, usize) {
    let mut worst = (0.0, pts.len() / 2);
    for i in 1..pts.len() - 1 {
        let d = (c.eval(u[i]) - pts[i]).hypot();
        if d > worst.0 {
            worst = (d, i);
        }
    }
    worst
}

/// `u` moved one Newton step towards each point's nearest parameter on `c`.
fn reparameterised(pts: &[Point], u: &[f64], c: &CubicBez) -> Vec<f64> {
    let (d1, d2) = (c.deriv(), c.deriv().deriv());
    pts.iter()
        .zip(u)
        .map(|(p, &t)| {
            let diff = c.eval(t) - *p;
            let (q1, q2) = (d1.eval(t).to_vec2(), d2.eval(t).to_vec2());
            let den = q1.dot(q1) + diff.dot(q2);
            if den.abs() < 1e-12 {
                t
            } else {
                (t - diff.dot(q1) / den).clamp(0.0, 1.0)
            }
        })
        .collect()
}
