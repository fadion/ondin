//! *Offset path* over the catalogue of closed shapes at sixteen distances, each
//! result checked against the oracle in `common` rather than a stored answer.
//!
//! The band is the outline stroked `2·|d|` wide with mitres clipped at
//! `OFFSET_MITER_LIMIT` times the distance. A grow is the shape and the band; a
//! shrink is the shape less the band. A shape that misbehaves in the app belongs in
//! `common::closed_shapes`.

mod common;

use common::{Band, Corners, Fill};
use ondin_core::build::{OFFSET_MITER_LIMIT, offset_outline};
use ondin_core::kurbo::{self, BezPath, Shape};

/// The most segments an offset of any shape here may come back with.
const MAX_SEGMENTS: usize = 80;

const DISTANCES: [f64; 16] = [
    -60.0, -38.0, -21.0, -9.0, -4.0, -1.0, 1.0, 4.0, 9.0, 17.0, 26.0, 38.0, 55.0, 70.0, 90.0, 120.0,
];

/// Everything wrong with offsetting `shape` by `d`.
fn offset_case(shape: &BezPath, d: f64) -> Vec<String> {
    let fill = Fill::new(shape);
    let band = Band::new(
        shape,
        d.abs(),
        Corners::Clip(OFFSET_MITER_LIMIT),
        kurbo::Cap::Butt,
    );
    let area = shape
        .bounding_box()
        .inflate(d.abs() * 1.5 + 5.0, d.abs() * 1.5 + 5.0);
    let result = offset_outline(shape, d).ok();
    let expected = |p| match d > 0.0 {
        true => common::or(fill.contains(p), band.contains(p)),
        false => common::and(fill.contains(p), band.contains(p).map(|b| !b)),
    };
    common::check(result.as_ref(), area, MAX_SEGMENTS, expected)
}

/// Every closed shape at every distance matches the oracle and is well formed.
#[test]
fn offset_matches_the_oracle_on_every_shape() {
    let cases: Vec<(String, (BezPath, f64))> = common::closed_shapes()
        .into_iter()
        .flat_map(|(n, p)| DISTANCES.map(|d| (format!("{n} @ {d}"), (p.clone(), d))))
        .collect();
    common::run_all(&cases, |(shape, d)| offset_case(shape, *d));
}
