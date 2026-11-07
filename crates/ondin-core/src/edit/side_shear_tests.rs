use super::*;

/// The corner of `box_` that `handle` grabs and the corner of the side facing it; corners
/// rather than midpoints, since a midpoint of the held side stays put under any shear.
fn probe(box_: Rect, handle: Handle) -> (Point, Point) {
    match handle {
        Handle::Top => (
            Point::new(box_.min_x(), box_.min_y()),
            Point::new(box_.min_x(), box_.max_y()),
        ),
        Handle::Bottom => (
            Point::new(box_.min_x(), box_.max_y()),
            Point::new(box_.min_x(), box_.min_y()),
        ),
        Handle::Left => (
            Point::new(box_.min_x(), box_.min_y()),
            Point::new(box_.max_x(), box_.min_y()),
        ),
        Handle::Right => (
            Point::new(box_.max_x(), box_.min_y()),
            Point::new(box_.min_x(), box_.min_y()),
        ),
        _ => unreachable!("only edges shear"),
    }
}

/// The middle of the side `handle` owns, where the press goes since the lean is measured from the press.
fn grip(box_: Rect, handle: Handle) -> Point {
    match handle {
        Handle::Top => Point::new(box_.center().x, box_.min_y()),
        Handle::Bottom => Point::new(box_.center().x, box_.max_y()),
        Handle::Left => Point::new(box_.min_x(), box_.center().y),
        Handle::Right => Point::new(box_.max_x(), box_.center().y),
        _ => unreachable!("only edges shear"),
    }
}

/// Every edge leans its own side and holds the one facing it.
///
/// flip: `Handle::Left`'s held edge moved to `box_.min_x()` → fails at "Left: the held side moved"
/// flip: `Handle::Bottom`'s span sign reversed → fails at "Bottom: the grabbed side moved"
#[test]
fn every_edge_leans_its_own_side_and_holds_the_one_facing_it() {
    // 200 by 120, so a transposed span shows as a different magnitude.
    let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
    for handle in Handle::EDGES {
        let press = grip(box_, handle);
        let at = match handle {
            Handle::Top | Handle::Bottom => press + Vec2::new(20.0, 0.0),
            _ => press + Vec2::new(0.0, 20.0),
        };
        let shear = side_shear(box_, handle, SideDrag { press, at }, false)
            .unwrap_or_else(|| panic!("{handle:?} is an edge and must shear"));

        let (grabbed, held) = probe(box_, handle);
        let still = shear * held;
        assert!(
            (still - held).hypot() < 1e-9,
            "{handle:?}: the held side moved, by {:?}",
            still - held
        );
        let want = at - press;
        let moved = shear * grabbed;
        assert!(
            ((moved - grabbed) - want).hypot() < 1e-9,
            "{handle:?}: the grabbed side moved by {:?}, wanted {want:?}",
            moved - grabbed
        );
    }
}

/// Shift snaps the lean to 15° steps before the clamp; only a drag past the clamp tells
/// the orders apart, and the unsnapped 18.43° case shows the snap really moved it.
#[test]
fn shift_snaps_a_lean_to_fifteen_degree_steps_before_the_clamp_pulls_it_in() {
    let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
    let press = Point::new(100.0, 0.0);
    // `skew_x` puts `tan(a)` in the matrix's xy slot.
    let lean_of = |a: Affine| a.as_coeffs()[2].atan();
    let drag = |dx: f64, snap: bool| {
        side_shear(
            box_,
            Handle::Top,
            SideDrag {
                press,
                at: press + Vec2::new(dx, 0.0),
            },
            snap,
        )
        .expect("an edge shears")
    };

    // 40 across a 120 drop is atan(1/3) = 18.43°. The sign is asserted: a rightward
    // drag is a negative `skew_x` here, and magnitude alone would pass a wrong lean.
    assert!(
        (lean_of(drag(40.0, false)).to_degrees() + 18.4349).abs() < 1e-3,
        "the fixture's own angle moved: {}",
        lean_of(drag(40.0, false)).to_degrees()
    );
    assert!(
        (lean_of(drag(40.0, true)).to_degrees() + 15.0).abs() < 1e-9,
        "Shift did not snap: {}",
        lean_of(drag(40.0, true)).to_degrees()
    );
    assert!(
        (lean_of(drag(100_000.0, true)) + MAX_SKEW).abs() < 1e-9,
        "the clamp did not run after the snap: {}",
        lean_of(drag(100_000.0, true)).to_degrees()
    );
}

/// A corner handle and a box too thin to lean have no lean.
#[test]
fn a_corner_and_a_slit_have_no_lean() {
    let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
    let drag = SideDrag {
        press: Point::new(0.0, 0.0),
        at: Point::new(20.0, 20.0),
    };
    assert!(side_shear(box_, Handle::TopLeft, drag, false).is_none());
    let slit = Rect::new(0.0, 0.0, 200.0, MIN_EXTENT * 0.5);
    assert!(side_shear(slit, Handle::Top, drag, false).is_none());
}
