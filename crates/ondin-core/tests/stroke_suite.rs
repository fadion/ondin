//! *Outline stroke* (`build::stroke_ink`) over the catalogue of closed shapes and
//! open paths, at several widths, joins, caps and alignments, each result checked
//! against the oracle in `common`.
//!
//! The ink is what a stroke renders as: each segment's slab, each join (a mitre
//! bevelled past the stroke's limit, as SVG draws it) and each open end's cap. An
//! inside stroke is that band at twice the width cut to the shape, an outside one
//! the same band less the shape.

mod common;

use common::{Band, Corners, Fill};
use ondin_core::build::stroke_ink;
use ondin_core::kurbo::{self, BezPath, Shape};
use ondin_core::node::StrokeAlign;
use ondin_core::{NodeKind, Stroke};

/// The most segments one stroke's ink may come back with.
const MAX_SEGMENTS: usize = 200;
/// A dashed stroke's: some two dozen dashes, a round-capped one ten segments.
const MAX_DASHED_SEGMENTS: usize = 300;

const WIDTHS: [f64; 3] = [3.0, 24.0, 110.0];

/// One stroke of `path`, checked.
fn stroke_case(path: &BezPath, s: &Stroke) -> Vec<String> {
    let kind = NodeKind::Path {
        path: path.clone(),
        corner_radii: Vec::new(),
    };
    let align = match ondin_core::geometry::stroke_align_applies(&kind) {
        true => s.align,
        false => StrokeAlign::Center,
    };
    let laid = match align {
        StrokeAlign::Center => s.width,
        _ => 2.0 * s.width,
    };
    let dashes = ondin_core::geometry::resolved_dashes(s, s.width, path);
    let inked = match dashes.is_empty() {
        true => path.clone(),
        false => BezPath::from_iter(kurbo::dash(path.iter(), s.dash_offset, &dashes)),
    };
    let band = Band::new(
        &inked,
        laid / 2.0,
        Corners::Svg(s.join, s.miter_limit),
        s.cap,
    );
    let fill = Fill::new(path);
    let expected = |p| {
        let b = band.contains(p);
        match align {
            StrokeAlign::Center => b,
            StrokeAlign::Inside => common::and(b, fill.contains(p)),
            StrokeAlign::Outside => common::and(b, fill.contains(p).map(|f| !f)),
        }
    };
    let reach = laid * 1.5 + 5.0;
    let area = path.bounding_box().inflate(reach, reach);
    let result = stroke_ink(&kind, path, s);
    let max = match s.dashes.is_empty() {
        true => MAX_SEGMENTS,
        false => MAX_DASHED_SEGMENTS,
    };
    common::check(result.as_ref(), area, max, expected)
}

fn stroke(width: f64, join: kurbo::Join, cap: kurbo::Cap, align: StrokeAlign) -> Stroke {
    Stroke {
        width,
        join,
        cap,
        align,
        ..Default::default()
    }
}

/// Every closed shape, stroked centred with each join and inside and outside with
/// mitres, matches the oracle and is well formed.
#[test]
fn outline_stroke_matches_the_oracle_on_closed_shapes() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (BezPath, Stroke))> = Vec::new();
    for (name, path) in common::closed_shapes() {
        for w in WIDTHS {
            for (join, align) in [
                (Join::Miter, StrokeAlign::Center),
                (Join::Round, StrokeAlign::Center),
                (Join::Bevel, StrokeAlign::Center),
                (Join::Miter, StrokeAlign::Inside),
                (Join::Miter, StrokeAlign::Outside),
            ] {
                let s = stroke(w, join, Cap::Butt, align);
                cases.push((format!("{name} {w} {join:?} {align:?}"), (path.clone(), s)));
            }
        }
    }
    common::run_all(&cases, |(path, s)| stroke_case(path, s));
}

/// Every open path, with each cap and with mitre and round joins, matches the
/// oracle and is well formed.
#[test]
fn outline_stroke_matches_the_oracle_on_open_paths() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (BezPath, Stroke))> = Vec::new();
    for (name, path) in common::open_paths() {
        for w in WIDTHS {
            for cap in [Cap::Butt, Cap::Round, Cap::Square] {
                for join in [Join::Miter, Join::Round] {
                    let s = stroke(w, join, cap, StrokeAlign::Center);
                    cases.push((format!("{name} {w} {cap:?} {join:?}"), (path.clone(), s)));
                }
            }
        }
    }
    common::run_all(&cases, |(path, s)| stroke_case(path, s));
}

/// Dashed strokes, butt and round, on a circle, a square and a curve.
#[test]
fn outline_stroke_matches_the_oracle_when_dashed() {
    use kurbo::{Cap, Join};
    let cases: Vec<(String, (BezPath, Stroke))> = [
        ("circle", common::circle((0.0, 0.0), 80.0)),
        (
            "square",
            kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1),
        ),
        (
            "s-curve",
            BezPath::from_svg("M0,0 C60,-90 100,90 160,0").unwrap(),
        ),
    ]
    .into_iter()
    .flat_map(|(n, p)| {
        [Cap::Butt, Cap::Round].map(|cap| {
            let s = Stroke {
                dashes: vec![14.0, 9.0],
                ..stroke(6.0, Join::Miter, cap, StrokeAlign::Center)
            };
            (format!("{n} {cap:?}"), (p.clone(), s))
        })
    })
    .collect();
    common::run_all(&cases, |(path, s)| stroke_case(path, s));
}
