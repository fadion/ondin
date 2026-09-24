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
    }
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
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

/// Rounding a stretched rect's corners leaves it stretched. A corner radius is
/// applied to a kind still at its *stored* size, so treating it as a resize — as
/// `keep_insets` did until `GeometryPatch::resizes` — re-pinned the far edge at
/// that size and drew the rect 100 wide (`arch-scribe`'s finding).
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
