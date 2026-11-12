//! Absolute insets on frames through the whole model (§15 D871, D874) — the
//! document, `Resolved`'s layout pass and `build::keep_insets` together, the way
//! a commit uses them.
//!
//! `container`'s own tests pin the arithmetic; these pin that it is wired: a
//! frame resize re-places its pinned children, an edit to a pinned child becomes
//! an edit of its insets, and incremental `update` stays equal to `rebuild`
//! throughout.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::{
    Document, GeometryPatch, History, IdSource, Insets, LengthPct, NodeId, NodeKind, Operation,
    Resolved, Transaction,
};

fn rect(w: f64, h: f64) -> NodeKind {
    NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn px(v: f64) -> Option<LengthPct> {
    Some(LengthPct::Px(v))
}

struct Scene {
    doc: Document,
    res: Resolved,
    history: History,
    ids: IdSource,
    root: NodeId,
}

impl Scene {
    fn new() -> Self {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let doc = Document::new(root);
        let res = Resolved::rebuild(&doc);
        Scene {
            doc,
            res,
            history: History::default(),
            ids,
            root,
        }
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind, at: (f64, f64)) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }]);
        id
    }

    /// The commit path, as the app's session runs it: `keep_insets` first, then
    /// history, then an incremental `update` — checked against a full `rebuild`
    /// every time.
    fn commit(&mut self, ops: Vec<Operation>) {
        let tx = ondin_core::build::keep_insets(&self.doc, &self.res, Transaction(ops));
        let dirty = self
            .history
            .commit(&mut self.doc, tx)
            .expect("the edit applies");
        self.res.update(&self.doc, &dirty);
        self.assert_equal_to_rebuild();
    }

    fn undo(&mut self) {
        let dirty = self
            .history
            .undo(&mut self.doc)
            .expect("the undo applies")
            .expect("something to undo");
        self.res.update(&self.doc, &dirty);
        self.assert_equal_to_rebuild();
    }

    fn assert_equal_to_rebuild(&self) {
        let fresh = Resolved::rebuild(&self.doc);
        for id in ondin_core::subtree_nodes(&self.doc, &[self.root]) {
            assert_eq!(
                self.res.used_local(&self.doc, id),
                fresh.used_local(&self.doc, id),
                "used local of {id:?}"
            );
            assert_eq!(
                self.res.used_kind(&self.doc, id),
                fresh.used_kind(&self.doc, id),
                "used kind of {id:?}"
            );
            assert_eq!(
                self.res.world_bounds(id),
                fresh.world_bounds(id),
                "bounds of {id:?}"
            );
            assert_eq!(
                self.res.text_layout(id),
                fresh.text_layout(id),
                "text of {id:?}"
            );
        }
    }

    fn pin(&mut self, id: NodeId, insets: Insets) {
        self.commit(vec![Operation::SetInsets { id, insets }]);
    }

    fn resize(&mut self, id: NodeId, w: f64, h: f64) {
        self.commit(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::Size(Size::new(w, h)),
        }]);
    }

    fn bounds(&self, id: NodeId) -> Rect {
        self.res.world_bounds(id).expect("measured")
    }
}

/// Resizing a frame re-places what is pinned in it and leaves what is not — the
/// feature in one test. A right-pinned rect follows the right edge, a rect pinned
/// on both sides stretches, and an unpinned one stays put, exactly as every
/// document written before insets expects.
#[test]
fn resizing_a_frame_re_places_its_pinned_children_and_only_them() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let right = s.add(f, rect(100.0, 50.0), (10.0, 10.0));
    let both = s.add(f, rect(100.0, 50.0), (10.0, 80.0));
    let free = s.add(f, rect(100.0, 50.0), (10.0, 140.0));
    s.pin(
        right,
        Insets {
            right: px(10.0),
            ..Default::default()
        },
    );
    s.pin(
        both,
        Insets {
            left: px(10.0),
            right: px(10.0),
            ..Default::default()
        },
    );
    assert_eq!(s.bounds(right), Rect::new(190.0, 10.0, 290.0, 60.0));
    assert_eq!(s.bounds(both), Rect::new(10.0, 80.0, 290.0, 130.0));

    s.resize(f, 500.0, 200.0);
    assert_eq!(
        s.bounds(right),
        Rect::new(390.0, 10.0, 490.0, 60.0),
        "followed the edge"
    );
    assert_eq!(
        s.bounds(both),
        Rect::new(10.0, 80.0, 490.0, 130.0),
        "stretched"
    );
    assert_eq!(
        s.bounds(free),
        Rect::new(10.0, 140.0, 110.0, 190.0),
        "stayed put"
    );
    assert_eq!(
        s.doc.get(both).unwrap().kind(),
        &rect(100.0, 50.0),
        "the stretch is used geometry: the document still holds what was typed"
    );
}

/// A frame stretched by its own insets hands its children the stretched size —
/// the reason `update` places parents first.
///
/// **Flip:** `used_geometry` reading the parent frame's *document* kind instead of
/// its used one fails the first assertion, 80 against 280 — the grandchild placed
/// against the 100-wide frame that was typed rather than the 300-wide one drawn.
#[test]
fn a_stretched_frame_places_its_own_children_against_its_used_size() {
    let mut s = Scene::new();
    let outer = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let inner = s.add(outer, frame(100.0, 100.0), (0.0, 0.0));
    let leaf = s.add(inner, rect(20.0, 20.0), (0.0, 0.0));
    s.pin(
        inner,
        Insets {
            left: px(0.0),
            right: px(0.0),
            ..Default::default()
        },
    );
    s.pin(
        leaf,
        Insets {
            right: px(0.0),
            ..Default::default()
        },
    );
    assert_eq!(
        s.bounds(leaf).x0,
        280.0,
        "at the right of a 300-wide inner frame"
    );

    s.resize(outer, 400.0, 200.0);
    assert_eq!(
        s.bounds(leaf).x0,
        380.0,
        "the grandchild followed a frame that only moved because its parent did"
    );
}

/// Moving a pinned child is an edit of its insets. The move arrives as the
/// `SetTransform` any tool writes; `keep_insets` turns it into the `right` that
/// puts the child there, and undo takes both back.
#[test]
fn moving_a_pinned_child_rewrites_its_insets_and_undo_restores_them() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let child = s.add(f, rect(100.0, 50.0), (10.0, 10.0));
    let pinned = Insets {
        right: px(10.0),
        top: Some(LengthPct::Percent(5.0)),
        ..Default::default()
    };
    s.pin(child, pinned);

    // Drawn at (190, 10); the tool asks for 40 to the left and 10 down.
    s.commit(vec![Operation::SetTransform {
        id: child,
        transform: Affine::translate((150.0, 20.0)),
    }]);
    let now = *s.doc.get(child).unwrap().insets();
    assert_eq!(now.right, px(50.0));
    assert_eq!(
        now.top,
        Some(LengthPct::Percent(10.0)),
        "still a percentage"
    );
    assert_eq!(s.bounds(child), Rect::new(150.0, 20.0, 250.0, 70.0));

    s.resize(f, 400.0, 200.0);
    assert_eq!(
        s.bounds(child).x0,
        250.0,
        "and it is pinned where it was dropped"
    );

    s.undo();
    s.undo();
    assert_eq!(*s.doc.get(child).unwrap().insets(), pinned);
    assert_eq!(s.bounds(child), Rect::new(190.0, 10.0, 290.0, 60.0));
}

/// Resizing a stretched child re-pins its far edge rather than fighting it: the
/// size the tool asks for is the size drawn.
#[test]
fn resizing_a_stretched_child_moves_its_far_inset() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let child = s.add(f, rect(100.0, 50.0), (0.0, 0.0));
    s.pin(
        child,
        Insets {
            left: px(10.0),
            right: px(10.0),
            ..Default::default()
        },
    );
    s.resize(child, 200.0, 50.0);
    assert_eq!(s.doc.get(child).unwrap().insets().right, px(90.0));
    assert_eq!(s.bounds(child), Rect::new(10.0, 0.0, 210.0, 50.0));
}

/// A path pinned on both sides follows its frame's resize by stretching its
/// points; its drawn points written back as its own (the node tool's first edit,
/// the Position card's unpin) draw it where it was, the insets untouched.
/// flip: `inset_stretch` answering no for a path → fails at "stretched with the
/// frame"; `baked_ops` writing no path geometry → fails at "the drawn points are
/// its own".
#[test]
fn a_pinned_path_stretches_with_its_frame_and_writing_it_back_keeps_it() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let mut p = ondin_core::kurbo::BezPath::new();
    p.move_to((0.0, 40.0));
    p.line_to((50.0, 0.0));
    p.line_to((100.0, 40.0));
    let kind = NodeKind::Path {
        path: p,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    };
    let path = s.add(f, kind, (10.0, 10.0));
    let insets = Insets {
        left: px(10.0),
        right: px(190.0),
        ..Default::default()
    };
    s.pin(path, insets);
    assert_eq!(
        s.bounds(path),
        Rect::new(10.0, 10.0, 110.0, 50.0),
        "pinned where drawn"
    );
    s.resize(f, 400.0, 200.0);
    assert_eq!(
        s.bounds(path),
        Rect::new(10.0, 10.0, 210.0, 50.0),
        "stretched with the frame"
    );

    let drawn = s.res.used_kind(&s.doc, path).unwrap().clone();
    let NodeKind::Path { path: points, .. } = &drawn else {
        panic!("drawn as a path");
    };
    let local = s.res.used_local(&s.doc, path).unwrap();
    let node = s.doc.get(path).unwrap().clone();
    s.commit(ondin_core::build::baked_ops(path, &node, local, &drawn));
    let NodeKind::Path { path: stored, .. } = s.doc.get(path).unwrap().kind() else {
        panic!("still a path");
    };
    assert_eq!(stored, points, "the drawn points are its own");
    assert_eq!(
        *s.doc.get(path).unwrap().insets(),
        insets,
        "the insets untouched"
    );
    assert_eq!(
        s.bounds(path),
        Rect::new(10.0, 10.0, 210.0, 50.0),
        "drawn where it was"
    );
}

/// Rounding a stretched rect's corners leaves it stretched. A corner radius is
/// applied to a kind still at its *stored* size, so treating it as a resize — as
/// `keep_insets` did until `GeometryPatch::resizes` — re-pinned the far edge at
/// that size and drew the rect 100 wide.
///
/// **Flip:** `resizes` answering `true` for `CornerRadius` fails the insets
/// assertion, `right` rewritten to 190.
#[test]
fn rounding_a_stretched_rects_corners_leaves_it_stretched() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let child = s.add(f, rect(100.0, 50.0), (0.0, 0.0));
    let both = Insets {
        left: px(10.0),
        right: px(10.0),
        ..Default::default()
    };
    s.pin(child, both);
    s.commit(vec![Operation::SetGeometry {
        id: child,
        geometry: GeometryPatch::CornerRadius(8.0),
    }]);
    assert_eq!(
        *s.doc.get(child).unwrap().insets(),
        both,
        "insets untouched"
    );
    assert_eq!(
        s.bounds(child),
        Rect::new(10.0, 0.0, 290.0, 50.0),
        "still stretched"
    );
}

/// A layer moved into a frame in the same edit is pinned against the frame it
/// lands in, not the one it left.
#[test]
fn a_reparented_child_is_pinned_against_its_new_frame() {
    let mut s = Scene::new();
    let a = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let b = s.add(s.root, frame(500.0, 200.0), (400.0, 0.0));
    let child = s.add(a, rect(100.0, 50.0), (10.0, 10.0));
    s.pin(
        child,
        Insets {
            right: px(10.0),
            ..Default::default()
        },
    );
    // What `reparent_preserving_world` emits: the move, then the local transform
    // that keeps it where it is drawn — (190, 10) in `a` is (-210, 10) in `b`.
    let index = s.doc.get(b).unwrap().children().len();
    s.commit(vec![
        Operation::Reparent {
            id: child,
            new_parent: b,
            index,
        },
        Operation::SetTransform {
            id: child,
            transform: Affine::translate((-210.0, 10.0)),
        },
    ]);
    assert_eq!(
        s.doc.get(child).unwrap().insets().right,
        px(610.0),
        "500 less -210 less 100"
    );
    assert_eq!(
        s.bounds(child),
        Rect::new(190.0, 10.0, 290.0, 60.0),
        "it did not move"
    );
}

/// A pinned child written back to exactly where it is drawn is not an edit. Its
/// drawn placement is not its stored transform — pinned `right: 10` draws it at
/// 190 while the document still holds 10 — so the tool's write-back would read as
/// a change to the no-op test (§15 D428) and commit an invisible undo step.
///
/// **The transaction comes back empty**, and that is the assertion rather than
/// `changes_nothing`, which answers `false` for an empty transaction — so the
/// session re-checks for empty after the conversion, which this found missing.
///
/// **Flip:** without the `unmoved` strip in `keep_insets` the transaction keeps
/// its `SetTransform`.
#[test]
fn a_pinned_child_written_back_where_it_is_drawn_changes_nothing() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let child = s.add(f, rect(100.0, 50.0), (10.0, 10.0));
    s.pin(
        child,
        Insets {
            right: px(10.0),
            ..Default::default()
        },
    );
    let tx = Transaction(vec![Operation::SetTransform {
        id: child,
        transform: Affine::translate((190.0, 10.0)),
    }]);
    let out = ondin_core::build::keep_insets(&s.doc, &s.res, tx);
    assert!(out.0.is_empty(), "left as {:?}", out.0);
}

/// On a document with no insets `keep_insets` hands the transaction back as it
/// came — the guarantee that makes running it on every commit free.
#[test]
fn keep_insets_is_the_identity_where_nothing_is_pinned() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let child = s.add(f, rect(100.0, 50.0), (10.0, 10.0));
    let ops = vec![
        Operation::SetTransform {
            id: child,
            transform: Affine::translate((40.0, 40.0)),
        },
        Operation::SetGeometry {
            id: child,
            geometry: GeometryPatch::Size(Size::new(10.0, 10.0)),
        },
    ];
    let out = ondin_core::build::keep_insets(&s.doc, &s.res, Transaction(ops.clone()));
    assert_eq!(out.0, ops);
}

/// Insets survive a save and a load, and an unpinned layer's bytes do not change
/// — the field is additive, with no schema bump (§5.11).
#[test]
fn insets_round_trip_through_the_save_format_and_are_absent_when_unset() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(300.0, 200.0), (0.0, 0.0));
    let plain = s.add(f, rect(100.0, 50.0), (10.0, 10.0));
    let pinned = s.add(f, rect(100.0, 50.0), (10.0, 80.0));
    let insets = Insets {
        left: Some(LengthPct::Percent(25.0)),
        right: px(12.5),
        margin_auto: ondin_core::AutoMargins {
            left: true,
            right: true,
            ..Default::default()
        },
        ..Default::default()
    };
    s.pin(pinned, insets);
    let bytes = ondin_core::io::save(&s.doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert_eq!(
        text.matches("\"insets\"").count(),
        1,
        "only the pinned layer writes the field"
    );
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(*back.get(pinned).unwrap().insets(), insets);
    assert!(back.get(plain).unwrap().insets().is_unset());
}

/// A closed path tracing the box `x0, y0` to `x1, y1`.
fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> NodeKind {
    let mut p = ondin_core::kurbo::BezPath::new();
    p.move_to((x0, y0));
    p.line_to((x1, y0));
    p.line_to((x1, y1));
    p.line_to((x0, y1));
    p.close_path();
    NodeKind::Path {
        path: p,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    }
}

/// Every side at `p` percent.
fn all_pct(p: f64) -> Insets {
    let v = Some(LengthPct::Percent(p));
    Insets {
        top: v,
        right: v,
        bottom: v,
        left: v,
        ..Default::default()
    }
}

/// A 24 frame holding a plain group pinned at 0% on every side, which holds a
/// rect over the whole box and a group holding a path from 4 to 20: an icon.
/// The frame, the group, the inner group and the path.
fn icon(s: &mut Scene) -> (NodeId, NodeId, NodeId, NodeId) {
    let f = s.add(s.root, frame(24.0, 24.0), (100.0, 100.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(24.0, 24.0), (0.0, 0.0));
    let inner = s.add(g, NodeKind::Group, (0.0, 0.0));
    let path = s.add(inner, square(4.0, 4.0, 20.0, 20.0), (0.0, 0.0));
    s.pin(g, all_pct(0.0));
    (f, g, inner, path)
}

/// A plain group pinned on both edges scales its contents with the frame, a
/// group inside it included, each layer keeping its share of the box and a
/// path's points scaled rather than its stroke.
/// flip: `used_geometry` without the stretched-parent branch → fails at "the
/// group fills the frame", a group's box being its children's.
#[test]
fn a_pinned_group_scales_its_contents_with_the_frame() {
    let mut s = Scene::new();
    let (f, g, _, path) = icon(&mut s);
    assert_eq!(
        s.bounds(path),
        Rect::new(104.0, 104.0, 120.0, 120.0),
        "at its own size it is drawn as specified"
    );
    s.resize(f, 48.0, 48.0);
    assert_eq!(
        s.bounds(g),
        Rect::new(100.0, 100.0, 148.0, 148.0),
        "the group fills the frame"
    );
    assert_eq!(
        s.bounds(path),
        Rect::new(108.0, 108.0, 140.0, 140.0),
        "the path scales with the icon"
    );
    assert!(
        matches!(s.res.used_kind(&s.doc, path), Some(NodeKind::Path { .. })),
        "its points are scaled, not its transform"
    );
    s.undo();
    assert_eq!(s.bounds(path), Rect::new(104.0, 104.0, 120.0, 120.0));
}

/// A 24 frame holding a ring, a boolean subtracting a circle from 6 to 18 from
/// one from 2 to 22, `grouped` in a plain group. The frame and the ring.
fn ring(s: &mut Scene, grouped: bool) -> (NodeId, NodeId) {
    let f = s.add(s.root, frame(24.0, 24.0), (100.0, 100.0));
    let parent = match grouped {
        true => s.add(f, NodeKind::Group, (0.0, 0.0)),
        false => f,
    };
    let ring = s.add(
        parent,
        NodeKind::Boolean {
            op: ondin_core::BoolOp::Subtract,
        },
        (0.0, 0.0),
    );
    let circle = |d: f64| NodeKind::Ellipse {
        size: Size::new(d, d),
        arc: Default::default(),
    };
    s.add(ring, circle(20.0), (2.0, 2.0));
    s.add(ring, circle(12.0), (6.0, 6.0));
    (f, ring)
}

/// A boolean scales with its frame by its operands, pinned itself or inside a
/// pinned group: its outline is drawn from the stretched operands.
/// flip: `placed_by_contents` answering no for a boolean → fails at "a pinned
/// ring".
#[test]
fn a_boolean_scales_by_its_operands() {
    for grouped in [false, true] {
        let mut s = Scene::new();
        let (f, ring) = ring(&mut s, grouped);
        let pinned = match grouped {
            false => ring,
            true => s.doc.get(ring).unwrap().parent().unwrap(),
        };
        // Where it is drawn: 2 in from each side of the 24.
        s.pin(pinned, all_pct(2.0 / 24.0 * 100.0));
        s.resize(f, 48.0, 48.0);
        let outline = ondin_core::kurbo::Shape::bounding_box(
            s.res.boolean_path(ring).expect("the ring has an outline"),
        );
        let world = s
            .res
            .world_transform(ring)
            .unwrap()
            .transform_rect_bbox(outline);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(
            near(world.x0, 104.0) && near(world.x1, 144.0) && near(world.y1, 144.0),
            "{}: {world:?}",
            if grouped {
                "a ring in a pinned group"
            } else {
                "a pinned ring"
            }
        );
    }
}

/// A layer moved inside a stretched group stays where it was put: it is
/// written back at half the distance in the group's own space, and the
/// group's box holds.
/// flip: `keep_insets` without `keep_stretched` → fails at "drawn where it
/// was moved".
#[test]
fn a_layer_moved_inside_a_stretched_group_stays_where_it_was_put() {
    let mut s = Scene::new();
    let (f, _, inner, path) = icon(&mut s);
    s.resize(f, 48.0, 48.0);
    let drawn = s.res.used_local(&s.doc, path).unwrap();
    s.commit(vec![Operation::SetTransform {
        id: path,
        transform: Affine::translate((4.0, 0.0)) * drawn,
    }]);
    assert_eq!(
        s.bounds(path),
        Rect::new(112.0, 108.0, 144.0, 140.0),
        "drawn where it was moved"
    );
    let stored = s.doc.get(path).unwrap().transform();
    let inner_at = s.doc.get(inner).unwrap().transform();
    assert!(
        ((inner_at * stored).translation().x - 2.0).abs() < 1e-9,
        "half the distance in the group's own space: {stored:?} under {inner_at:?}"
    );
}

/// An edit that changes a pinned group's union without moving a layer —
/// deleting, hiding or adding one — moves nothing already drawn, at the
/// group's own size and stretched alike: a frame 200 × 100 holding a group
/// pinned 0% on every side, with rect A on the left half and B on the right.
/// flip: `keep_stretched` without its `reunited` groups → B is drawn across
/// the whole box, failing at "B stays put".
#[test]
fn deleting_hiding_or_adding_in_a_pinned_group_moves_nothing_else() {
    for stretched in [false, true] {
        for edit in ["delete", "hide", "add"] {
            let mut s = Scene::new();
            let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
            let g = s.add(f, NodeKind::Group, (0.0, 0.0));
            let a = s.add(g, rect(100.0, 100.0), (0.0, 0.0));
            let b = s.add(g, rect(100.0, 100.0), (100.0, 0.0));
            s.pin(g, all_pct(0.0));
            if stretched {
                s.resize(f, 400.0, 100.0);
            }
            let (was_a, was_b) = (s.bounds(a), s.bounds(b));
            match edit {
                "delete" => s.commit(vec![Operation::DeleteNode { id: a }]),
                "hide" => s.commit(vec![Operation::SetVisible {
                    id: a,
                    visible: false,
                }]),
                _ => {
                    s.add(g, rect(50.0, 50.0), (200.0, 0.0));
                }
            }
            assert_eq!(
                s.bounds(b),
                was_b,
                "{edit}, stretched {stretched}: B stays put"
            );
            if edit == "add" {
                assert_eq!(s.bounds(a), was_a, "{edit}, stretched {stretched}: A too");
            }
        }
    }
}

/// A frame with a layout inside a stretched group keeps its share of the box,
/// as a frame without one does, and its items are laid again at that share, as
/// the browser re-lays them: frame 200 × 100 resized to 400, the group pinned
/// 0% holding rect A on the left half and frame L, laying out its 20-wide rect
/// C centred, on the right half. L is drawn at 200–400 and C at 290–310.
/// flip: `used_geometry` placing a layout root by `root_used` alone → L stays
/// at 100–200, failing at "its share"; `place_node` without `relaid_at_share`
/// → C at 240–260, failing at "centred in its share".
#[test]
fn a_laid_out_frame_in_a_stretched_group_keeps_its_share() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(100.0, 100.0), (0.0, 0.0));
    let l = s.add(g, frame(100.0, 100.0), (100.0, 0.0));
    let c = s.add(l, rect(20.0, 20.0), (0.0, 0.0));
    s.commit(vec![Operation::SetDisplay {
        id: l,
        display: Some(ondin_core::container::Display::Flex(
            ondin_core::container::Flex {
                justify_content: ondin_core::container::JustifyContent::Center,
                ..Default::default()
            },
        )),
    }]);
    assert_eq!(s.bounds(c).x0, 140.0, "the fixture: centred in L");
    s.pin(g, all_pct(0.0));
    s.resize(f, 400.0, 100.0);
    assert_eq!(
        s.bounds(l),
        Rect::new(200.0, 0.0, 400.0, 100.0),
        "its share"
    );
    assert_eq!(
        (s.bounds(c).x0, s.bounds(c).x1),
        (290.0, 310.0),
        "centred in its share"
    );
}

/// A group with a layout inside a stretched group keeps its share of the box,
/// as a frame with one does, and its items are laid again at that share: frame
/// 200 × 100 resized to 400, the group pinned 0% holding rect A on the left
/// half and group L, hugging its 20-wide rect C centred with 40 of padding, on
/// the right half. L's box is 200–400 and C is drawn at 290–310.
/// flip: `container::root_in_stretch` without a group's box as its contents →
/// L is placed as specified, C at 140, failing at "its share"; `place_node`
/// without `relaid_at_share` → C at 240, failing at "centred in its share".
#[test]
fn a_laid_out_group_in_a_stretched_group_keeps_its_share() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(100.0, 100.0), (0.0, 0.0));
    let l = s.add(g, NodeKind::Group, (100.0, 0.0));
    let c = s.add(l, rect(20.0, 20.0), (0.0, 0.0));
    s.commit(vec![Operation::SetDisplay {
        id: l,
        display: Some(ondin_core::container::Display::Flex(
            ondin_core::container::Flex {
                justify_content: ondin_core::container::JustifyContent::Center,
                padding: [40.0; 4],
                ..Default::default()
            },
        )),
    }]);
    assert_eq!(
        s.bounds(c),
        Rect::new(140.0, 40.0, 160.0, 60.0),
        "the fixture: padded in L"
    );
    s.pin(g, all_pct(0.0));
    s.resize(f, 400.0, 100.0);
    assert_eq!(
        (
            s.res.used_local(&s.doc, l).map(|a| a.translation().x),
            s.res.used_frame(l)
        ),
        (Some(200.0), Some(Size::new(200.0, 100.0))),
        "its share"
    );
    assert_eq!(
        (s.bounds(c).x0, s.bounds(c).x1),
        (290.0, 310.0),
        "centred in its share"
    );
}

/// A path its insets squeeze to no height keeps its end inset through an
/// unrelated nudge, and comes back at its own height when the frame grows.
/// flip: `keep_insets` judging stretch on the used kind → bottom 40, failing
/// at "bottom kept".
#[test]
fn a_squeezed_path_keeps_its_end_inset_through_a_nudge() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 300.0), (0.0, 0.0));
    let p = s.add(f, square(0.0, 0.0, 100.0, 180.0), (0.0, 60.0));
    let px = |v: f64| Some(LengthPct::Px(v));
    s.pin(
        p,
        Insets {
            left: px(0.0),
            top: px(60.0),
            bottom: px(60.0),
            ..Default::default()
        },
    );
    s.resize(f, 200.0, 100.0);
    let drawn = s.res.used_local(&s.doc, p).unwrap();
    s.commit(vec![Operation::SetTransform {
        id: p,
        transform: Affine::translate((10.0, 0.0)) * drawn,
    }]);
    assert_eq!(
        s.doc.get(p).unwrap().insets().bottom,
        px(60.0),
        "bottom kept"
    );
}

/// A layer nudged inside a group squeezed to no width keeps its share there:
/// back at the group's own width, it is drawn where it was.
/// flip: `keep_stretched` without its flat-axis arm → B at 0–200, failing at
/// "its share".
#[test]
fn a_nudge_in_a_squeezed_group_keeps_the_layers_share() {
    let mut s = Scene::new();
    let f = s.add(s.root, frame(200.0, 100.0), (0.0, 0.0));
    let g = s.add(f, NodeKind::Group, (0.0, 0.0));
    s.add(g, rect(100.0, 100.0), (0.0, 0.0));
    let b = s.add(g, rect(100.0, 100.0), (100.0, 0.0));
    let px = Some(LengthPct::Px(0.0));
    s.pin(
        g,
        Insets {
            left: px,
            right: px,
            top: px,
            bottom: px,
            ..Default::default()
        },
    );
    s.resize(f, 0.0, 100.0);
    let drawn = s.res.used_local(&s.doc, b).unwrap();
    s.commit(vec![Operation::SetTransform {
        id: b,
        transform: Affine::translate((0.0, 5.0)) * drawn,
    }]);
    s.resize(f, 200.0, 100.0);
    let at = s.bounds(b);
    assert!(
        (at.x0 - 100.0).abs() < 1e-6 && (at.x1 - 200.0).abs() < 1e-6,
        "its share: {at:?}"
    );
}

/// A ratio layer its insets stretch on one axis derives the other from it, as
/// a browser lays `aspect-ratio`: pinned left, right and top, 16 : 9 on H, a
/// frame grown from 400 to 800 draws it 800 × 450.
/// flip: `container::place` ignoring its ratio → 800 × 225, failing at
/// "derived".
#[test]
fn a_ratio_layer_stretched_on_one_axis_derives_the_other() {
    use ondin_core::container::{AspectRatio, RatioSide};
    let mut s = Scene::new();
    let f = s.add(s.root, frame(400.0, 600.0), (0.0, 0.0));
    let r = s.add(f, rect(400.0, 225.0), (0.0, 0.0));
    let mut item = *s.doc.get(r).unwrap().item();
    item.aspect_ratio = Some(AspectRatio {
        ratio: 16.0 / 9.0,
        side: RatioSide::Height,
    });
    s.commit(vec![Operation::SetLayoutItem { id: r, item }]);
    let px = |v: f64| Some(LengthPct::Px(v));
    s.pin(
        r,
        Insets {
            left: px(0.0),
            right: px(0.0),
            top: px(0.0),
            ..Default::default()
        },
    );
    s.resize(f, 800.0, 600.0);
    let b = s.bounds(r);
    assert!(
        (b.width() - 800.0).abs() < 1e-6 && (b.height() - 450.0).abs() < 1e-6,
        "derived: {b:?}"
    );
}

/// A layer moved out past a stretched group's box grows the box without moving
/// anything already drawn: the pinned group takes new insets.
/// flip: `keep_stretched` skipping the groups above → fails at "the rect has
/// not moved".
#[test]
fn growing_a_stretched_groups_box_moves_nothing_else() {
    let mut s = Scene::new();
    let (f, g, _, path) = icon(&mut s);
    s.resize(f, 48.0, 48.0);
    let rect_id = s.doc.get(g).unwrap().children()[0];
    let before = s.bounds(rect_id);
    let drawn = s.res.used_local(&s.doc, path).unwrap();
    s.commit(vec![Operation::SetTransform {
        id: path,
        transform: Affine::translate((24.0, 0.0)) * drawn,
    }]);
    assert_eq!(s.bounds(rect_id), before, "the rect has not moved");
    assert_eq!(
        s.bounds(path),
        Rect::new(132.0, 108.0, 164.0, 140.0),
        "the path is where it was put"
    );
    assert_ne!(
        *s.doc.get(g).unwrap().insets(),
        all_pct(0.0),
        "the group took new insets"
    );
}
