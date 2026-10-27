//! Point-in-path, as a click asks it (`FillRule::contains`), over the catalogue's
//! shapes, their offsets and simplifications, and a circle cut into arcs that turn
//! through horizontal mid-segment, each checked against the winding number counted
//! on a fine flattening: every point more than a twentieth of a unit off an outline
//! must read as that count says.

mod common;

use ondin_core::FillRule;
use ondin_core::kurbo::{self, BezPath, ParamCurveNearest, Point, Shape};

/// How far off an outline a point must be for its reading to count.
const CLEAR: f64 = 0.05;

/// A circle of radius 170 cut into cubic arcs at uneven angles, several of which
/// turn through horizontal mid-segment, where kurbo's own ray cast misreads points
/// a unit outside as inside.
fn arcs() -> BezPath {
    let cuts = [
        180.0f64, 204.7, 225.5, 243.8, 296.2, 317.0, 338.3, 360.0, 384.7, 405.5, 423.8, 476.2,
        497.0, 518.3, 540.0,
    ];
    let at = |deg: f64| {
        Point::new(
            170.0 * deg.to_radians().cos(),
            170.0 * deg.to_radians().sin(),
        )
    };
    let mut p = BezPath::new();
    p.move_to(at(cuts[0]));
    for w in cuts.windows(2) {
        let arc = kurbo::Arc::new(
            (0.0, 0.0),
            (170.0, 170.0),
            w[0].to_radians(),
            (w[1] - w[0]).to_radians(),
            0.0,
        );
        arc.to_cubic_beziers(1e9, |a, b, c| p.curve_to(a, b, c));
    }
    p.close_path();
    p
}

/// Everything `rule` misreads about `path`.
fn hit_case(path: &BezPath, rule: FillRule) -> Vec<String> {
    let edges = common::flat(path);
    let b = path.bounding_box().inflate(5.0, 5.0);
    let n = 100;
    let (mut wrong, mut first) = (0, None);
    for i in 0..=n {
        for j in 0..=n {
            let q = Point::new(
                b.x0 + b.width() * i as f64 / n as f64,
                b.y0 + b.height() * j as f64 / n as f64,
            );
            let near = path
                .segments()
                .map(|s| s.nearest(q, 1e-6).distance_sq)
                .fold(f64::INFINITY, f64::min)
                .sqrt();
            if near < CLEAR {
                continue;
            }
            let w = common::winding(&edges, q);
            let truth = match rule {
                FillRule::NonZero => w != 0,
                FillRule::EvenOdd => w % 2 != 0,
            };
            if rule.contains(path, q) != truth {
                wrong += 1;
                first.get_or_insert((q, truth));
            }
        }
    }
    match first {
        None => Vec::new(),
        Some((q, truth)) => vec![format!(
            "{wrong} points misread, first ({:.2}, {:.2}), truly {}",
            q.x,
            q.y,
            if truth { "inside" } else { "outside" }
        )],
    }
}

/// Every shape, its offsets and its simplification read right under both rules,
/// and the arcs at every radius just off them.
#[test]
fn point_in_path_matches_the_flattened_count() {
    let mut cases: Vec<(String, (BezPath, FillRule))> = Vec::new();
    let mut add = |name: String, p: BezPath| {
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            cases.push((format!("{name} {rule:?}"), (p.clone(), rule)));
        }
    };
    add("arcs".into(), arcs());
    for (name, p) in common::closed_shapes() {
        add(name.into(), p.clone());
        for d in [-9.0, 17.0, 70.0] {
            if let Ok(r) = ondin_core::build::offset_outline(&p, d, FillRule::NonZero) {
                add(format!("{name} offset {d}"), r);
            }
        }
        add(
            format!("{name} simplified"),
            ondin_core::build::simplify_outline(&p),
        );
    }
    common::run_all(&cases, |(p, rule)| hit_case(p, *rule));
}

/// The arcs read outside just past their outline all the way round, where kurbo's
/// own winding reads hundreds of points a tenth to a unit and a half out as inside.
/// flip: `FillRule::contains` back on `Shape::winding` → fails at "outside".
#[test]
fn a_point_just_off_an_arc_reads_outside() {
    let p = arcs();
    for r in [170.1, 170.5, 171.0, 171.5] {
        for i in 0..4000 {
            let a = i as f64 / 4000.0 * std::f64::consts::TAU;
            let q = Point::new(r * a.cos(), r * a.sin());
            assert!(
                !FillRule::NonZero.contains(&p, q),
                "outside: ({:.2}, {:.2}) at radius {r}",
                q.x,
                q.y
            );
        }
    }
}
