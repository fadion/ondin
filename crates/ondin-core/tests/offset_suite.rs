//! *Offset path* over the catalogue of closed shapes at sixteen distances, and over
//! the inputs the catalogue lacks — even-odd and self-crossing shapes, tiny shapes
//! and distances, open paths, dots — each result checked against the oracle in
//! `common` rather than a stored answer.
//!
//! The band is the outline stroked `2·|d|` wide with mitres clipped at
//! `OFFSET_MITER_LIMIT` times the distance and round caps. A grow is the shape and
//! the band; a shrink is the shape less the band. A shape that misbehaves in the app
//! belongs in `common::closed_shapes` or in `extra_cases` here.

mod common;

use common::{Band, Corners, Fill};
use ondin_core::FillRule;
use ondin_core::build::{OFFSET_MITER_LIMIT, offset_outline};
use ondin_core::kurbo::{self, BezPath, Shape};

/// The most segments an offset of any shape here may come back with.
const MAX_SEGMENTS: usize = 80;

const DISTANCES: [f64; 16] = [
    -60.0, -38.0, -21.0, -9.0, -4.0, -1.0, 1.0, 4.0, 9.0, 17.0, 26.0, 38.0, 55.0, 70.0, 90.0, 120.0,
];

/// One offset to check: the path, the rule it is read with, the distance, and the
/// outline the oracle draws its band along where that is not the path itself (a
/// shape whose subpaths overlap or cross, read non-zero).
struct Case {
    shape: BezPath,
    rule: FillRule,
    d: f64,
    outline: Option<BezPath>,
}

/// Everything wrong with offsetting `case.shape`, read with `case.rule`, by `case.d`.
///
/// The oracle's blind band narrows with the distance and the shape's size, so a
/// small offset is held to what it is: at `d = 1` a quarter of a unit, where the
/// default band would pass a result half a unit off.
fn offset_case(case: &Case) -> Vec<String> {
    let Case { shape, rule, d, .. } = case;
    let (d, rule) = (*d, *rule);
    let bounds = shape.bounding_box();
    let size = bounds.width().max(bounds.height());
    let tol = (0.25 + 0.04 * d.abs())
        .min(common::TOL)
        .min(size.max(2.0 * d) / 20.0);
    let outline = case.outline.as_ref().unwrap_or(shape);
    let closed: BezPath = ondin_core::geometry::subpaths(outline)
        .into_iter()
        .filter(|s| matches!(s.elements().last(), Some(kurbo::PathEl::ClosePath)))
        .flat_map(|s| s.into_iter())
        .collect();
    let fill = Fill::new(&closed)
        .even_odd(rule == FillRule::EvenOdd)
        .tol(tol);
    let band = Band::new(
        outline,
        d.abs(),
        Corners::Clip(OFFSET_MITER_LIMIT),
        kurbo::Cap::Round,
    )
    .tol(tol);
    let reach = d.abs() * 1.5 + (0.05 * size).min(5.0);
    let area = bounds.inflate(reach, reach);
    let result = offset_outline(shape, d, rule).ok();
    let expected = |p| match d > 0.0 {
        true => common::or(fill.contains(p), band.contains(p)),
        false => common::and(fill.contains(p), band.contains(p).map(|b| !b)),
    };
    let speck = (0.01 * size).powi(2).min(1.0);
    common::check_read(result.as_ref(), area, MAX_SEGMENTS, false, speck, expected)
}

/// Every closed shape at every distance matches the oracle and is well formed.
#[test]
fn offset_matches_the_oracle_on_every_shape() {
    let cases: Vec<(String, Case)> = common::closed_shapes()
        .into_iter()
        .flat_map(|(n, p)| {
            DISTANCES.map(|d| {
                let case = Case {
                    shape: p.clone(),
                    rule: FillRule::NonZero,
                    d,
                    outline: None,
                };
                (format!("{n} @ {d}"), case)
            })
        })
        .collect();
    common::run_all(&cases, offset_case);
}

/// The inputs the catalogue of closed shapes lacks: a name, the path, the rule it
/// is read with, the oracle's outline where it differs, and the distances.
type Extra = (&'static str, BezPath, FillRule, Option<BezPath>, Vec<f64>);

fn extra_cases() -> Vec<Extra> {
    use std::f64::consts::TAU;
    let parse = |d: &str| BezPath::from_svg(d).unwrap();
    // Two circles wound the same way: a ring read even-odd, a disc read non-zero.
    let mut ring = common::circle((0.0, 0.0), 100.0);
    ring.extend(common::circle((0.0, 0.0), 50.0).iter());
    // A five-point star drawn as one crossing subpath: hollow read even-odd, its
    // ten-sided outline filled read non-zero.
    let at = |r: f64, k: usize| {
        let a = k as f64 / 10.0 * TAU - TAU / 4.0;
        (r * a.cos(), r * a.sin())
    };
    let pentagram = common::polygon(&(0..5).map(|k| at(100.0, 4 * k)).collect::<Vec<_>>());
    let inner = 100.0 * (TAU / 5.0).cos() / (TAU / 10.0).cos();
    let star: Vec<(f64, f64)> = (0..10)
        .map(|k| at(if k % 2 == 0 { 100.0 } else { inner }, k))
        .collect();
    let mut dotted = common::circle((0.0, 0.0), 10.0);
    dotted.extend(common::circle((14.0, 0.0), 0.4).iter());
    let mut mixed = kurbo::Rect::new(0.0, 0.0, 60.0, 60.0).to_path(0.1);
    mixed.extend(parse("M80,30 L140,30 L140,90").iter());
    mixed.extend(parse("M30,100 L30,100").iter());
    let small = [-9.0, -1.0, 1.0, 4.0, 9.0, 26.0].to_vec();
    let grows = [1.0, 4.0, 9.0, 26.0].to_vec();
    let (nz, eo) = (FillRule::NonZero, FillRule::EvenOdd);
    let disc = Some(common::circle((0.0, 0.0), 100.0));
    let mut cases = vec![
        ("even-odd ring", ring.clone(), eo, None, small.clone()),
        ("same-wound circles", ring, nz, disc, small.clone()),
        ("pentagram", pentagram.clone(), eo, None, small.clone()),
        (
            "pentagram non-zero",
            pentagram,
            nz,
            Some(common::polygon(&star)),
            small,
        ),
        (
            "square 0.5",
            kurbo::Rect::new(0.0, 0.0, 0.5, 0.5).to_path(0.1),
            nz,
            None,
            vec![-0.1, 0.05, 0.2, 1.0],
        ),
        (
            "circle r0.4",
            common::circle((0.0, 0.0), 0.4),
            nz,
            None,
            vec![-0.1, 0.1, 0.5],
        ),
        (
            "circle and a dot",
            dotted,
            nz,
            None,
            vec![-0.1, 0.1, 0.3, 1.0],
        ),
        ("dot", parse("M10,10 L10,10"), nz, None, grows.clone()),
        ("mixed", mixed, nz, None, vec![-4.0, 1.0, 4.0, 9.0]),
    ];
    for (name, path) in common::open_paths() {
        cases.push((name, path, nz, None, grows.clone()));
    }
    cases
}

/// Even-odd and self-crossing shapes, tiny shapes and distances, open paths, dots
/// and a path mixing them match the oracle and are well formed.
#[test]
fn offset_matches_the_oracle_beyond_the_catalogue() {
    let cases: Vec<(String, Case)> = extra_cases()
        .into_iter()
        .flat_map(|(n, shape, rule, outline, ds)| {
            ds.into_iter()
                .map(|d| {
                    let case = Case {
                        shape: shape.clone(),
                        rule,
                        d,
                        outline: outline.clone(),
                    };
                    (format!("{n} @ {d}"), case)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    common::run_all(&cases, offset_case);
}

/// A square and a circle offset by every distance enclose the area they must: the
/// square's mitred corners keep it square, the circle stays a circle. The grid
/// cannot see a distance a fraction of a unit off or a round corner for a mitre at
/// a small distance; the area can.
/// flip: `offset_wedges` returning no wedges → fails at "square".
#[test]
fn offset_encloses_the_area_it_must() {
    use std::f64::consts::PI;
    let square = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
    let circle = common::circle((0.0, 0.0), 50.0);
    let mut failures = Vec::new();
    for d in DISTANCES {
        let area =
            |p: &BezPath| offset_outline(p, d, FillRule::NonZero).map_or(0.0, |r| r.area().abs());
        let want = (100.0 + 2.0 * d).max(0.0).powi(2);
        let got = area(&square);
        if (got - want).abs() > 1e-6 * want.max(1.0) {
            failures.push(format!("square @ {d}: {got:.4}, not {want:.4}"));
        }
        let want = PI * (50.0 + d).max(0.0).powi(2);
        let got = area(&circle);
        if (got - want).abs() > 3e-3 * want.max(1.0) {
            failures.push(format!("circle @ {d}: {got:.2}, not {want:.2}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A grow by a tiny distance stays outside the original all the way round, and
/// within the distance of it: the flattening and the refit are held to a fraction
/// of the distance, not to a fixed tenth of a unit.
#[test]
fn a_tiny_grow_stays_outside_the_original() {
    use kurbo::{ParamCurve, Point};
    for (r, d) in [(50.0, 0.01), (50.0, 0.05), (0.5, 0.1), (1.0, 0.25)] {
        // Four cubics stray a few hundredths from a radius-50 circle; this is true to
        // a millionth.
        let circle = kurbo::Circle::new((0.0, 0.0), r).to_path(1e-6);
        let grown = offset_outline(&circle, d, FillRule::NonZero).expect("a grow leaves something");
        for seg in grown.segments() {
            for k in 0..=20 {
                let q: Point = seg.eval(k as f64 / 20.0);
                let at = q.to_vec2().hypot();
                assert!(
                    at > r && at < r + 2.0 * d,
                    "r {r} grown {d}: a point at radius {at:.4}"
                );
            }
        }
    }
}

/// Two thousand bumps three units wide offset by forty, where in one step every
/// vertex's join overlaps thousands of others, take well under a second in a
/// release build; the bound is generous for a loaded machine. The deterministic
/// guard is `build`'s `an_offset_bounds_its_work`.
#[test]
#[cfg_attr(debug_assertions, ignore = "timed in a release build")]
fn a_dense_wave_offsets_in_a_moment() {
    let mut wave = BezPath::new();
    wave.move_to((0.0, 0.0));
    for i in 0..2000 {
        let (x, s) = (3.0 * i as f64, if i % 2 == 0 { 20.0 } else { -20.0 });
        wave.curve_to((x, s), (x + 3.0, s), (x + 3.0, 0.0));
    }
    wave.line_to((6000.0, -60.0));
    wave.line_to((0.0, -60.0));
    wave.close_path();
    for d in [5.0, 40.0, -5.0] {
        let start = std::time::Instant::now();
        let out = offset_outline(&wave, d, FillRule::NonZero);
        let took = start.elapsed();
        assert!(out.is_ok(), "{d}: {out:?}");
        assert!(took.as_secs_f64() < 10.0, "{d}: {took:?}");
    }
}
