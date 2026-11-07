//! `rewrite_path`'s no-op guard over a path the pen did not write: both sides are
//! compared in the pen's canonical form (explicit closing segment, trimmed radii), or a
//! click would become a `SetGeometry`.

use super::*;
use crate::IdSource;

/// The `<polygon>` shape as `svg_in` stores it: a closed path with an implicit closing segment.
fn imported_triangle() -> BezPath {
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((80.0, 0.0));
    p.line_to((80.0, 60.0));
    p.close_path();
    p
}

fn a_node() -> NodeId {
    IdSource::new(0x5A1).mint()
}

/// A click that changed nothing plans nothing over an imported closed path; the fixture
/// assertion shows the pen round trip does move it.
///
/// flip: replace the canonicalisation with a raw comparison against `current` → fails at "a click that changed nothing rewrote the geometry"
#[test]
fn a_no_op_over_an_imported_closed_path_plans_nothing() {
    let stored = imported_triangle();
    let subpaths = pen_anchors(&stored, &[]);

    assert_ne!(
        pen_path(&subpaths).elements(),
        stored.elements(),
        "the fixture: the round trip must move, or there is nothing here to guard"
    );

    let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
    assert_eq!(
        tx,
        Transaction(vec![]),
        "a click that changed nothing rewrote the geometry"
    );
}

/// A click over untrimmed stored radii plans nothing; the path is the pen's own
/// spelling so the radii are the only difference, since `pen_radii` trims trailing zeros.
#[test]
fn a_no_op_over_untrimmed_stored_radii_plans_nothing() {
    let mut stored = BezPath::new();
    stored.move_to((0.0, 0.0));
    stored.line_to((80.0, 0.0));
    stored.line_to((80.0, 60.0));
    let radii = [0.0, 0.0, 0.0];
    let subpaths = pen_anchors(&stored, &radii);

    assert_eq!(
        pen_path(&subpaths).elements(),
        stored.elements(),
        "the fixture: an open subpath must round-trip exactly, so the radii \
         are the only difference left"
    );
    assert_ne!(
        pen_radii(&subpaths).as_slice(),
        radii.as_slice(),
        "the fixture: the trim must move the list, or there is nothing to guard"
    );

    let tx = rewrite_path(
        a_node(),
        Affine::IDENTITY,
        &subpaths,
        Some((&stored, &radii)),
    );
    assert_eq!(
        tx,
        Transaction(vec![]),
        "a click that changed nothing rewrote the radii"
    );
}

/// Control: a real radius edit still commits (a point move would not catch a guard that swallows radii).
#[test]
fn a_radius_actually_set_still_plans_a_write() {
    let stored = imported_triangle();
    let mut subpaths = pen_anchors(&stored, &[]);
    subpaths[0].anchors[1].radius = 4.0;

    let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
    assert_eq!(tx.0.len(), 1, "a real radius edit was swallowed: {tx:?}");
    assert!(matches!(
        tx.0[0],
        Operation::SetGeometry {
            geometry: GeometryPatch::Path { .. },
            ..
        }
    ));
}
