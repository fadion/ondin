//! *Outline stroke* (`build::stroke_ink`) over the catalogue of closed shapes and
//! open paths, at several widths, joins, caps and alignments, dashed and dotted,
//! per side on a rectangle, and thin, each result checked against the oracle in
//! `common`.
//!
//! The ink is what a stroke renders as: each segment's slab, each join (a mitre
//! bevelled past the stroke's limit, as SVG draws it) and each open end's cap. An
//! inside stroke is that band at twice the width cut to the shape, an outside one
//! the same band less the shape; a per-side stroke is the band of each side's own
//! path at that side's width.

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

/// The widths of a thin stroke, laid on the catalogue shrunk by `THIN_SCALE`.
const THIN_WIDTHS: [f64; 2] = [0.5, 1.0];

/// How much the catalogue is shrunk for a thin stroke, so the oracle's grid still
/// lands inside the ink.
const THIN_SCALE: f64 = 0.08;

/// One stroke of `kind`'s outline, checked.
///
/// The oracle's blind band is a third of the width at most, so a thin stroke is
/// not all blind band; a piece of ink a hundredth of the width's square is a
/// speck.
fn stroke_case(kind: &NodeKind, s: &Stroke) -> Vec<String> {
    let path = ondin_core::geometry::local_path(kind).expect("an outline");
    let align = match ondin_core::geometry::stroke_align_applies(kind) {
        true => s.align,
        false => StrokeAlign::Center,
    };
    let laid = |w: f64| match align {
        StrokeAlign::Center => w,
        _ => 2.0 * w,
    };
    // Each path the stroke lays and the width it shows, as `stroke_ink` lays them.
    let lays: Vec<(BezPath, f64)> =
        match ondin_core::geometry::effective_sides(kind, s).per_side(s.width) {
            None => vec![(path.clone(), s.width)],
            Some(sides) => sides
                .into_iter()
                .filter_map(|(side, w)| Some((ondin_core::geometry::side_path(kind, side)?, w)))
                .collect(),
        };
    let thinnest = lays.iter().map(|(_, w)| *w).fold(s.width, f64::min);
    let tol = common::TOL.min(thinnest / 3.0);
    let bands: Vec<Band> = lays
        .iter()
        .map(|(p, w)| {
            let dashes = ondin_core::geometry::resolved_dashes(s, *w, p);
            let inked = match dashes.is_empty() {
                true => p.clone(),
                false => BezPath::from_iter(kurbo::dash(p.iter(), s.dash_offset, &dashes)),
            };
            Band::new(
                &inked,
                laid(*w) / 2.0,
                Corners::Svg(s.join, s.miter_limit),
                s.cap,
            )
            .tol(tol)
        })
        .collect();
    let fill = Fill::new(&path).tol(tol);
    let expected = |p| {
        // Not a `try_fold`: an unknown band is outvoted by one that holds `p`.
        let mut b = Some(false);
        for band in &bands {
            b = common::or(b, band.contains(p));
        }
        match align {
            StrokeAlign::Center => b,
            StrokeAlign::Inside => common::and(b, fill.contains(p)),
            StrokeAlign::Outside => common::and(b, fill.contains(p).map(|f| !f)),
        }
    };
    let bounds = path.bounding_box();
    let reach =
        laid(s.width.max(thinnest)) * 1.5 + (0.05 * bounds.width().max(bounds.height())).min(5.0);
    let area = bounds.inflate(reach, reach);
    let result = stroke_ink(kind, &path, s);
    let max = match s.dashes.is_empty() {
        true => MAX_SEGMENTS,
        false => MAX_DASHED_SEGMENTS,
    };
    let speck = (0.1 * thinnest).powi(2).min(1.0);
    common::check_read(result.as_ref(), area, max, false, speck, expected)
}

fn path_kind(path: &BezPath) -> NodeKind {
    NodeKind::Path {
        path: path.clone(),
        corner_radii: Vec::new(),
    }
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

/// `path` shrunk about the origin by `THIN_SCALE`.
fn thin(path: &BezPath) -> BezPath {
    kurbo::Affine::scale(THIN_SCALE) * path
}

/// Every closed shape, stroked centred with each join and inside and outside with
/// mitres, matches the oracle and is well formed.
#[test]
fn outline_stroke_matches_the_oracle_on_closed_shapes() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (NodeKind, Stroke))> = Vec::new();
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
                let case = (path_kind(&path), s);
                cases.push((format!("{name} {w} {join:?} {align:?}"), case));
            }
        }
    }
    common::run_all(&cases, |(kind, s)| stroke_case(kind, s));
}

/// Every open path, with each cap and with mitre and round joins, matches the
/// oracle and is well formed.
#[test]
fn outline_stroke_matches_the_oracle_on_open_paths() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (NodeKind, Stroke))> = Vec::new();
    for (name, path) in common::open_paths() {
        for w in WIDTHS {
            for cap in [Cap::Butt, Cap::Round, Cap::Square] {
                for join in [Join::Miter, Join::Round] {
                    let s = stroke(w, join, cap, StrokeAlign::Center);
                    let case = (path_kind(&path), s);
                    cases.push((format!("{name} {w} {cap:?} {join:?}"), case));
                }
            }
        }
    }
    common::run_all(&cases, |(kind, s)| stroke_case(kind, s));
}

/// Dashed strokes, butt and round, on a circle, a square and a curve, centred, and
/// inside and outside where the outline is closed.
#[test]
fn outline_stroke_matches_the_oracle_when_dashed() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (NodeKind, Stroke))> = Vec::new();
    for (n, p) in [
        ("circle", common::circle((0.0, 0.0), 80.0)),
        (
            "square",
            kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1),
        ),
        (
            "s-curve",
            BezPath::from_svg("M0,0 C60,-90 100,90 160,0").unwrap(),
        ),
    ] {
        for cap in [Cap::Butt, Cap::Round] {
            for align in [
                StrokeAlign::Center,
                StrokeAlign::Inside,
                StrokeAlign::Outside,
            ] {
                let s = Stroke {
                    dashes: vec![14.0, 9.0],
                    ..stroke(6.0, Join::Miter, cap, align)
                };
                cases.push((format!("{n} {cap:?} {align:?}"), (path_kind(&p), s)));
            }
        }
    }
    common::run_all(&cases, |(kind, s)| stroke_case(kind, s));
}

/// Thin strokes — half a unit and one — on the catalogue shrunk to a few units,
/// solid and dotted: every piece of ink is kept, a dot of a dotted stroke included.
/// flip: `stroke_ink` dropping pieces under a square unit → fails at "points on the
/// wrong side" for the dotted cases.
#[test]
fn outline_stroke_matches_the_oracle_when_thin() {
    use kurbo::{Cap, Join};
    let mut cases: Vec<(String, (NodeKind, Stroke))> = Vec::new();
    let closed = common::closed_shapes()
        .into_iter()
        .map(|(n, p)| (n, p, true));
    let open = common::open_paths().into_iter().map(|(n, p)| (n, p, false));
    for (name, path, is_closed) in closed.chain(open) {
        let path = thin(&path);
        for w in THIN_WIDTHS {
            let mut strokes = vec![
                (
                    "miter",
                    stroke(w, Join::Miter, Cap::Butt, StrokeAlign::Center),
                ),
                (
                    "dotted",
                    Stroke {
                        dashes: vec![0.0, 3.0 * w],
                        ..stroke(w, Join::Round, Cap::Round, StrokeAlign::Center)
                    },
                ),
            ];
            if is_closed {
                strokes.push((
                    "inside",
                    stroke(w, Join::Miter, Cap::Butt, StrokeAlign::Inside),
                ));
            } else {
                strokes.push((
                    "round",
                    stroke(w, Join::Round, Cap::Round, StrokeAlign::Center),
                ));
            }
            for (what, s) in strokes {
                cases.push((format!("{name} {w} {what}"), (path_kind(&path), s)));
            }
        }
    }
    common::run_all(&cases, |(kind, s)| stroke_case(kind, s));
}

/// A rectangle stroked on some sides only — one side, or a width per side with one
/// left out — square and rounded, with each alignment and cap, matches the
/// oracle built from each side's own path.
#[test]
fn outline_stroke_matches_the_oracle_per_side() {
    use kurbo::{Cap, Join, RoundedRectRadii, Size};
    use ondin_core::StrokeSides;
    let mut cases: Vec<(String, (NodeKind, Stroke))> = Vec::new();
    for radius in [0.0, 12.0] {
        let kind = NodeKind::Rect {
            size: Size::new(100.0, 60.0),
            corner_radii: RoundedRectRadii::from_single_radius(radius),
            corner_smoothing: 0.0,
        };
        let custom = StrokeSides::Custom {
            top: 10.0,
            right: 4.0,
            bottom: 0.0,
            left: 4.0,
        };
        for sides in [StrokeSides::Top, custom] {
            for align in [
                StrokeAlign::Center,
                StrokeAlign::Inside,
                StrokeAlign::Outside,
            ] {
                for cap in [Cap::Butt, Cap::Round] {
                    let s = Stroke {
                        sides,
                        ..stroke(6.0, Join::Miter, cap, align)
                    };
                    let name = format!("r{radius} {sides:?} {align:?} {cap:?}");
                    cases.push((name, (kind.clone(), s)));
                }
            }
        }
    }
    common::run_all(&cases, |(kind, s)| stroke_case(kind, s));
}
