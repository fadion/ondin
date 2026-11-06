//! Arcs on the ellipse, corners on frames, and smoothing at any angle: the
//! outlines `geometry` builds for them, what the save file writes, and the
//! hit-test that follows the outline.

use ondin_core::geometry::{self, local_path};
use ondin_core::io;
use ondin_core::kurbo::{
    BezPath, ParamCurve, ParamCurveNearest, Point, RoundedRectRadii, Shape, Size,
};
use ondin_core::{
    Document, EllipseArc, GeometryPatch, IdSource, NodeKind, Operation, Resolved, Transaction,
    query,
};

fn ellipse(arc: EllipseArc) -> NodeKind {
    NodeKind::Ellipse {
        size: Size::new(100.0, 100.0),
        arc,
    }
}

fn arc(start: f64, sweep: f64, ratio: f64) -> EllipseArc {
    EllipseArc {
        start,
        sweep,
        ratio,
    }
}

/// Non-zero winding at `p`, the fill both rules agree on for these outlines.
fn filled(path: &BezPath, p: Point) -> bool {
    path.winding(p) != 0
}

/// A 270° wedge keeps three quadrants from three o'clock clockwise: the top
/// right is cut away. 90° is the bottom of the box.
#[test]
fn a_wedge_cuts_away_what_its_sweep_leaves() {
    let path = local_path(&ellipse(arc(0.0, 270.0, 0.0))).unwrap();
    assert!(filled(&path, Point::new(75.0, 75.0)), "bottom right");
    assert!(filled(&path, Point::new(25.0, 25.0)), "top left");
    assert!(!filled(&path, Point::new(75.0, 25.0)), "top right is cut");
    let quarter = local_path(&ellipse(arc(0.0, 90.0, 0.0))).unwrap();
    assert!(
        filled(&quarter, Point::new(70.0, 70.0)),
        "the first 90° runs to the bottom"
    );
}

/// A ring is a hole under the non-zero rule, and so a hole to the hit-test.
/// flip: the inner outline drawn the same way round as the outer → fails at
/// "the centre is a hole".
#[test]
fn a_ring_has_a_real_hole() {
    let kind = ellipse(arc(0.0, 360.0, 0.5));
    let path = local_path(&kind).unwrap();
    assert!(
        !filled(&path, Point::new(50.0, 50.0)),
        "the centre is a hole"
    );
    assert!(filled(&path, Point::new(50.0, 10.0)), "the band is filled");
    let (doc, id) = doc_with(kind);
    let res = Resolved::rebuild(&doc);
    let hit = |p| query::hit_test(&res, &doc, doc.first_page(), p, 0.0);
    assert!(
        hit(Point::new(50.0, 50.0)).is_empty(),
        "a click in the hole"
    );
    assert_eq!(hit(Point::new(50.0, 10.0)), vec![id]);
}

/// A ring sector keeps the band of its sweep only.
#[test]
fn a_ring_sector_is_the_band_of_its_sweep() {
    let path = local_path(&ellipse(arc(0.0, 270.0, 0.5))).unwrap();
    assert!(filled(&path, Point::new(50.0, 90.0)), "bottom of the band");
    assert!(!filled(&path, Point::new(50.0, 50.0)), "the hole");
    assert!(!filled(&path, Point::new(80.0, 20.0)), "the cut quadrant");
}

/// The patch normalizes: Start wraps at 360, Sweep stops at 0 and 360, Ratio at
/// 99%; and its inverse puts the old arc back.
#[test]
fn the_arc_patch_normalizes_and_inverts() {
    let next = GeometryPatch::Arc(arc(370.0, 400.0, 2.0))
        .applied_to(&ellipse(EllipseArc::default()))
        .unwrap();
    assert_eq!(next, ellipse(arc(10.0, 360.0, EllipseArc::MAX_RATIO)));
    let next = GeometryPatch::Arc(arc(-30.0, -5.0, 0.2))
        .applied_to(&ellipse(EllipseArc::default()))
        .unwrap();
    assert_eq!(next, ellipse(arc(330.0, 0.0, 0.2)));
}

fn doc_with(kind: NodeKind) -> (Document, ondin_core::NodeId) {
    let mut ids = IdSource::new(0x5A9E);
    let root = ids.mint();
    let id = ids.mint();
    let mut doc = Document::new(root);
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind,
        transform: None,
        name: None,
    }]))
    .unwrap();
    (doc, id)
}

/// The whole ellipse writes no arc; any other arc is written and read back.
/// flip: `EllipseArc::is_default` answering `false` → fails at "no arc".
#[test]
fn the_save_file_writes_the_arc_only_when_it_differs() {
    let (doc, _) = doc_with(ellipse(EllipseArc::default()));
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(!text.contains("\"arc\""), "no arc: {text}");

    let (doc, id) = doc_with(ellipse(arc(45.0, 180.0, 0.25)));
    let text = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(text.contains("\"arc\""));
    let loaded = io::load(text.as_bytes()).unwrap();
    assert_eq!(
        loaded.get(id).unwrap().kind(),
        &ellipse(arc(45.0, 180.0, 0.25))
    );
}

/// A frame rounds and smooths as a rect does, and its corners round-trip.
#[test]
fn a_frame_has_a_rects_corners() {
    let size = Size::new(120.0, 80.0);
    let radii = RoundedRectRadii::new(24.0, 8.0, 0.0, 16.0);
    let frame = NodeKind::Artboard {
        size,
        corner_radii: radii,
        corner_smoothing: 0.6,
    };
    let rect = NodeKind::Rect {
        size,
        corner_radii: radii,
        corner_smoothing: 0.6,
    };
    assert_eq!(local_path(&frame), local_path(&rect));
    assert!(
        !local_path(&frame).unwrap().contains(Point::new(1.0, 1.0)),
        "the rounded corner is cut"
    );
    let (doc, id) = doc_with(frame.clone());
    let loaded = io::load(&io::save(&doc).unwrap()).unwrap();
    assert_eq!(loaded.get(id).unwrap().kind(), &frame);
}

/// The generalised smoothing reproduces the rect's own on a square path: the
/// same construction with the turn at 90°.
/// flip: `smoothed_corner`'s arc spanning the whole turn rather than
/// `turn × (1 − s)` → fails at the sampled points.
#[test]
fn smoothing_at_any_angle_agrees_with_the_rects_at_ninety_degrees() {
    let size = Size::new(100.0, 100.0);
    let square = ondin_core::kurbo::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
    let rect = geometry::smoothed_rect(size, RoundedRectRadii::from_single_radius(20.0), 0.6);
    let general = geometry::round_corners_smoothed(&square, &[20.0; 4], 0.6);
    // Points on one outline lie on the other: sample the rect's and measure the
    // distance to the general one's segments.
    for seg in rect.segments() {
        for t in [0.0, 0.25, 0.5, 0.75] {
            let p = seg.eval(t);
            let d = general
                .segments()
                .map(|s| s.nearest(p, 1e-6).distance_sq.sqrt())
                .fold(f64::INFINITY, f64::min);
            assert!(d < 0.05, "{p:?} is {d} off the general outline");
        }
    }
}

/// Smoothing a star changes its outline, eases further down each leg than the
/// circular corner, and gives way where the legs are short.
#[test]
fn a_smoothed_star_reaches_further_along_its_legs() {
    let star = |s| NodeKind::Star {
        size: Size::new(200.0, 200.0),
        points: 5,
        inner_ratio: 0.382,
        corner_radius: 12.0,
        corner_smoothing: s,
    };
    let round = local_path(&star(0.0)).unwrap();
    let smooth = local_path(&star(0.6)).unwrap();
    assert_ne!(round, smooth);
    // The top point: the smoothed curve sits lower at the very tip, its arc shorter.
    let top = |p: &BezPath| p.bounding_box().y0;
    assert!(top(&smooth) > top(&round), "the tip is softer");
    // A huge radius on short legs: still a closed, finite outline.
    let huge = NodeKind::Polygon {
        size: Size::new(40.0, 40.0),
        sides: 3,
        corner_radius: 500.0,
        corner_smoothing: 1.0,
    };
    let path = local_path(&huge).unwrap();
    assert!(geometry::is_closed(&path));
    assert!(path.bounding_box().area().is_finite());
}

/// A path's smoothing is one value for the whole path and changes its rounded
/// outline; with no radius it changes nothing.
#[test]
fn a_paths_smoothing_applies_to_its_rounded_anchors() {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((100.0, 0.0));
    path.line_to((50.0, 80.0));
    path.close_path();
    let kind = |radii: Vec<f64>, s| NodeKind::Path {
        path: path.clone(),
        corner_radii: radii,
        corner_smoothing: s,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    };
    assert_ne!(
        local_path(&kind(vec![10.0; 3], 0.0)),
        local_path(&kind(vec![10.0; 3], 0.8))
    );
    assert_eq!(
        local_path(&kind(Vec::new(), 0.8)),
        Some(path.clone()),
        "no radius, nothing to smooth"
    );
}
