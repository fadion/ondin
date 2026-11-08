use super::*;

/// `tx`'s new layer's kind, and its outline carried into the world; the layer
/// is stroked and has no fill operation.
fn made(tx: &Transaction, parent_world: Affine) -> (NodeKind, BezPath) {
    let Operation::CreateNode {
        kind,
        transform: Some(t),
        ..
    } = &tx.0[0]
    else {
        panic!("a node first");
    };
    let mut world = geometry::local_path(kind).expect("an outline");
    world.apply_affine(parent_world * *t);
    assert!(
        matches!(&tx.0[1], Operation::SetStrokes { .. }) && tx.0.len() == 2,
        "stroked, and no fill op"
    );
    (kind.clone(), world)
}

/// A drawn spiral is a spiral layer that starts on the press and ends on the
/// release; its box is the true spiral's at the drag's radius, so filling it
/// stretches nothing and the turns stay evenly spaced.
#[test]
fn a_spiral_runs_from_the_press_to_the_release() {
    let (centre, end) = (Point::new(10.0, 20.0), Point::new(40.0, 60.0));
    let parent_world = Affine::translate((5.0, -3.0));
    let mut ids = crate::IdSource::new(1);
    for half_turns in [1, 2, 7, 12] {
        let tx = create_spiral(
            ids.mint(),
            ids.mint(),
            0,
            parent_world,
            centre,
            end,
            half_turns,
            solid_stroke(Color::BLACK, 2.0),
        );
        let (kind, world) = made(&tx, parent_world);
        let NodeKind::Spiral { size, turns } = kind else {
            panic!("a spiral layer");
        };
        assert_eq!(turns, f64::from(half_turns) / 2.0);
        let segs: Vec<_> = world.segments().collect();
        assert!(segs[0].start().distance(centre) < 1e-6, "{half_turns}");
        assert!(
            segs.last().unwrap().end().distance(end) < 1e-6,
            "{half_turns}"
        );
        let unit = crate::kurbo::Shape::bounding_box(&geometry::unit_spiral(turns));
        let r = (end - centre).hypot();
        assert!((size.width / unit.width() - r).abs() < 1e-9, "uniform");
        assert!((size.height / unit.height() - r).abs() < 1e-9, "uniform");
    }
}

/// A drawn arc is an arc layer from the press to the release at every sweep
/// the keys reach, either way round.
#[test]
fn an_arc_joins_the_drag_s_ends_at_every_sweep() {
    let (a, b) = (Point::new(440.0, 320.0), Point::new(580.0, 180.0));
    let parent_world = Affine::translate((30.0, 10.0));
    let mut ids = crate::IdSource::new(1);
    for rung in 0..=ARC_MAX_STEPS {
        let tx = create_arc(
            ids.mint(),
            ids.mint(),
            0,
            parent_world,
            a,
            b,
            arc_sweep(rung),
            solid_stroke(Color::BLACK, 2.0),
        );
        let (kind, world) = made(&tx, parent_world);
        assert!(matches!(kind, NodeKind::Arc { sweep, .. } if sweep == arc_sweep(rung)));
        let segs: Vec<_> = world.segments().collect();
        assert!(segs[0].start().distance(a) < 1e-6, "{rung}");
        assert!(segs.last().unwrap().end().distance(b) < 1e-6, "{rung}");
    }
}

/// The sweep ladder runs one step short of a whole turn either way, through 5°
/// on each side of the straight line and never onto it.
#[test]
fn the_arc_ladder_crosses_over_through_five_degrees() {
    let sweeps: Vec<f64> = (0..=ARC_MAX_STEPS).map(arc_sweep).collect();
    assert_eq!(sweeps[0], -345.0);
    assert_eq!(*sweeps.last().unwrap(), 345.0);
    assert!(sweeps.windows(2).all(|w| w[0] < w[1]), "rising");
    assert!(!sweeps.contains(&0.0), "never straight");
    let mid = sweeps.iter().position(|s| *s > 0.0).unwrap();
    assert_eq!(&sweeps[mid - 2..mid + 2], &[-15.0, -5.0, 5.0, 15.0]);
    assert_eq!(arc_sweep(ARC_DEFAULT_STEPS), 90.0, "a quarter to begin");
}

/// Resizing an arc by a side handle of its box holds the opposite edge and brings
/// the dragged one to the pointer, the arc growing evenly since a circle cannot
/// stretch.
/// flip: `resize_geometry` not forcing an even scale on an arc → fails at "the
/// dragged edge reaches the pointer".
#[test]
fn an_arc_resized_by_its_box_follows_the_drag() {
    let mut ids = crate::IdSource::new(0xA2C);
    let root = ids.mint();
    let arc = ids.mint();
    let mut doc = Document::new(root);
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: arc,
        parent: root,
        index: 0,
        kind: NodeKind::Arc {
            end: Point::new(100.0, 0.0),
            sweep: 180.0,
        },
        transform: Some(Affine::translate((20.0, 300.0))),
        name: None,
    }]))
    .expect("an arc");
    let res = Resolved::rebuild(&doc);
    let local = crate::local_box(&doc, &res, arc).expect("a box");
    let was = res.world_bounds(arc).expect("placed");
    for (handle, pointer) in [
        (Handle::Top, Point::new(70.0, was.y1 - 2.0 * was.height())),
        (
            Handle::Bottom,
            Point::new(70.0, was.y0 + 2.0 * was.height()),
        ),
    ] {
        let tx = resize_geometry(
            &doc,
            &res,
            arc,
            local,
            handle,
            pointer,
            Resize::geometry(false, false),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("the resize applies");
        let now = Resolved::rebuild(&after).world_bounds(arc).expect("placed");
        let (held, dragged) = match handle {
            Handle::Top => ((now.y1, was.y1), (now.y0, pointer.y)),
            _ => ((now.y0, was.y0), (now.y1, pointer.y)),
        };
        assert!(
            (held.0 - held.1).abs() < 1e-6,
            "{handle:?}: the held edge stays: {now:?}"
        );
        assert!(
            (dragged.0 - dragged.1).abs() < 1e-6,
            "{handle:?}: the dragged edge reaches the pointer: {now:?}"
        );
    }
}
