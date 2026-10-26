//! `boolean::evaluate` over a catalogue of operand sets and every operation, each
//! result checked against the oracle in `common`.
//!
//! The oracle is the definition: a point is in a union when it is in any operand,
//! an intersection when in all, a subtraction when in the first and none of the
//! rest, an exclusion when in an odd number — each operand read non-zero, the
//! exclusion's result even-odd as its node is. The sets lean on what curve booleans
//! get wrong: shared and nearly shared edges, touching corners, containment and
//! identical operands.

mod common;

use common::{Fill, circle, polygon};
use ondin_core::BoolOp;
use ondin_core::kurbo::{Affine, BezPath, Rect, Shape};

/// The most segments a result here may come back with.
const MAX_SEGMENTS: usize = 200;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    Rect::new(x0, y0, x1, y1).to_path(0.1)
}

fn moved(p: &BezPath, dx: f64, dy: f64) -> BezPath {
    Affine::translate((dx, dy)) * p.clone()
}

/// The operand sets.
fn sets() -> Vec<(&'static str, Vec<BezPath>)> {
    let shapes: std::collections::HashMap<_, _> = common::closed_shapes().into_iter().collect();
    let shape = |n: &str| shapes[n].clone();
    vec![
        (
            "overlapping circles",
            vec![circle((0.0, 0.0), 60.0), circle((70.0, 10.0), 50.0)],
        ),
        (
            "circle over square",
            vec![rect(0.0, 0.0, 100.0, 100.0), circle((100.0, 50.0), 40.0)],
        ),
        (
            "side by side",
            vec![rect(0.0, 0.0, 60.0, 60.0), rect(60.0, 0.0, 120.0, 60.0)],
        ),
        (
            "part of a side",
            vec![rect(0.0, 0.0, 60.0, 60.0), rect(60.0, 20.0, 120.0, 90.0)],
        ),
        (
            "inside, sharing a side",
            vec![rect(0.0, 0.0, 100.0, 100.0), rect(0.0, 30.0, 40.0, 70.0)],
        ),
        (
            "circle inside square",
            vec![rect(0.0, 0.0, 100.0, 100.0), circle((50.0, 50.0), 30.0)],
        ),
        (
            "touching corners",
            vec![rect(0.0, 0.0, 50.0, 50.0), rect(50.0, 50.0, 100.0, 100.0)],
        ),
        (
            "identical",
            vec![circle((0.0, 0.0), 50.0), circle((0.0, 0.0), 50.0)],
        ),
        (
            "identical squares",
            vec![rect(0.0, 0.0, 80.0, 80.0), rect(0.0, 0.0, 80.0, 80.0)],
        ),
        (
            "nearly coincident",
            vec![rect(0.0, 0.0, 80.0, 80.0), rect(0.001, 0.0, 80.001, 80.0)],
        ),
        (
            "tangent circles",
            vec![circle((0.0, 0.0), 40.0), circle((80.0, 0.0), 40.0)],
        ),
        (
            "sliver overlap",
            vec![rect(0.0, 0.0, 80.0, 80.0), rect(79.5, 10.0, 150.0, 70.0)],
        ),
        (
            "ring and circle",
            vec![shape("donut"), circle((60.0, 0.0), 50.0)],
        ),
        ("star and pentagon", vec![shape("star"), shape("pentagon")]),
        (
            "plus and turned square",
            vec![
                shape("plus"),
                Affine::rotate_about(0.6, (60.0, 60.0)) * rect(20.0, 20.0, 100.0, 100.0),
            ],
        ),
        ("C and circle", vec![shape("C"), circle((60.0, 0.0), 45.0)]),
        (
            "blob and ellipse",
            vec![shape("blob"), moved(&shape("ellipse"), 150.0, 60.0)],
        ),
        (
            "comb and bar",
            vec![shape("comb"), rect(-10.0, 30.0, 110.0, 50.0)],
        ),
        (
            "three circles",
            vec![
                circle((0.0, 0.0), 50.0),
                circle((60.0, 0.0), 50.0),
                circle((30.0, 50.0), 50.0),
            ],
        ),
        (
            "squares in a row",
            vec![
                rect(0.0, 0.0, 40.0, 40.0),
                rect(40.0, 0.0, 80.0, 40.0),
                rect(80.0, 0.0, 120.0, 40.0),
                rect(20.0, 20.0, 100.0, 60.0),
            ],
        ),
        (
            "triangles sharing an edge",
            vec![
                polygon(&[(0.0, 0.0), (100.0, 0.0), (50.0, 80.0)]),
                polygon(&[(0.0, 0.0), (50.0, -80.0), (100.0, 0.0)]),
            ],
        ),
        (
            "disjoint",
            vec![circle((0.0, 0.0), 30.0), circle((100.0, 0.0), 30.0)],
        ),
    ]
}

/// Everything wrong with `op` over `operands`.
fn boolean_case(operands: &[BezPath], op: BoolOp) -> Vec<String> {
    let fills: Vec<Fill> = operands.iter().map(Fill::new).collect();
    let expected = |p| -> Option<bool> {
        let inside: Vec<Option<bool>> = fills.iter().map(|f| f.contains(p)).collect();
        match op {
            BoolOp::Union => inside.iter().copied().fold(Some(false), common::or),
            BoolOp::Intersect => inside.iter().copied().fold(Some(true), common::and),
            BoolOp::Subtract => inside[1..]
                .iter()
                .fold(inside[0], |acc, i| common::and(acc, i.map(|b| !b))),
            BoolOp::Exclude => inside.iter().try_fold(false, |odd, i| i.map(|b| odd ^ b)),
        }
    };
    let area = operands
        .iter()
        .map(Shape::bounding_box)
        .reduce(|a, b| a.union(b))
        .unwrap_or_default()
        .inflate(10.0, 10.0);
    let result = ondin_core::boolean::evaluate(op, operands);
    // A sliver however thin is a shape a boolean keeps; only a point is junk.
    common::check_read(
        result.as_ref(),
        area,
        MAX_SEGMENTS,
        op == BoolOp::Exclude,
        1e-6,
        expected,
    )
}

/// What still fails, and why. `Exclude` does no arithmetic (`boolean::exclude_of`
/// concatenates the outlines for an even-odd read), so where operands share only
/// part of an edge, or nearly share one, the edge stays in the result: the fill is
/// right and a stroke draws a line along it.
const KNOWN: [(&str, &str); 3] = [
    (
        "part of a side Exclude",
        "edges with the fill on both sides",
    ),
    (
        "inside, sharing a side Exclude",
        "edges with the fill on both sides",
    ),
    (
        "nearly coincident Exclude",
        "edges with the fill on both sides",
    ),
];

/// Every operand set under every operation matches the oracle and is well formed,
/// but for [`KNOWN`].
#[test]
fn booleans_match_the_oracle() {
    let cases: Vec<(String, (Vec<BezPath>, BoolOp))> = sets()
        .into_iter()
        .flat_map(|(name, operands)| {
            [
                BoolOp::Union,
                BoolOp::Intersect,
                BoolOp::Subtract,
                BoolOp::Exclude,
            ]
            .map(|op| (format!("{name} {op:?}"), (operands.clone(), op)))
        })
        .collect();
    common::run_all_known(&cases, &KNOWN, |(operands, op)| boolean_case(operands, *op));
}
