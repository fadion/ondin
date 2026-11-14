//! Visual centres (`centroid.rs`): the centre of mass of a layer's shown ink, and
//! aligning on it.
//!
//! Every layer sits in a frame translated by (50, 30), so the world space ink is
//! weighed in is never the parent's.

use ondin_core::build::Axis;
use ondin_core::centroid::{self, VisualCentre};
use ondin_core::kurbo::{Affine, BezPath, Point, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Stroke, Transaction,
    keyed_by_position,
};

/// What to set on a new node, given its id.
type Then = fn(NodeId) -> Operation;

fn filled(id: NodeId) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::BLACK).into(),
            visible: true,
        }]),
    }
}

fn stroked(id: NodeId) -> Operation {
    Operation::SetStrokes {
        id,
        strokes: keyed_by_position([Stroke {
            width: 2.0,
            ..Stroke::default()
        }]),
    }
}

fn hidden(id: NodeId) -> Operation {
    Operation::SetVisible { id, visible: false }
}

fn masking(id: NodeId) -> Operation {
    Operation::SetMask { id, mask: true }
}

struct Fixture {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0xCE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((50.0, 30.0))),
            name: None,
        }]))
        .expect("the frame");
        Self { doc, ids, frame }
    }

    /// A node of `kind` on top of `parent`'s children, at `at`, with `then` set.
    fn add(&mut self, parent: NodeId, kind: NodeKind, at: (f64, f64), then: &[Then]) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).map_or(0, |n| n.children().len());
        let mut ops = vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }];
        ops.extend(then.iter().map(|op| op(id)));
        self.doc.apply(&Transaction(ops)).expect("the node");
        id
    }

    fn rect(
        &mut self,
        parent: NodeId,
        (x, y, w, h): (f64, f64, f64, f64),
        then: &[Then],
    ) -> NodeId {
        let kind = NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        self.add(parent, kind, (x, y), then)
    }

    fn group(&mut self) -> NodeId {
        self.add(self.frame, NodeKind::Group, (0.0, 0.0), &[])
    }

    fn centre(&self, id: NodeId) -> VisualCentre {
        centroid::visual_centre(&self.doc, &Resolved::rebuild(&self.doc), id).expect("a centre")
    }
}

fn path(points: &[(f64, f64)], closed: bool) -> NodeKind {
    let mut p = BezPath::new();
    p.move_to(points[0]);
    for q in &points[1..] {
        p.line_to(*q);
    }
    if closed {
        p.close_path();
    }
    NodeKind::Path {
        path: p,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    }
}

/// A play triangle 7 wide and 8 tall: its centroid is a third of its width from
/// the flat side, 1.17 right of its box's centre.
fn play() -> NodeKind {
    path(&[(0.0, 0.0), (7.0, 4.0), (0.0, 8.0)], true)
}

/// A filled triangle weighs at its centroid, in world space.
#[test]
fn a_filled_triangle_centres_on_its_centroid() {
    let mut f = Fixture::new();
    let t = f.add(f.frame, play(), (100.0, 100.0), &[filled]);
    let VisualCentre::Ink(at) = f.centre(t) else {
        panic!("weighed by ink: {:?}", f.centre(t))
    };
    assert!(
        at.distance(Point::new(150.0 + 7.0 / 3.0, 134.0)) < 1e-6,
        "{at:?}"
    );
}

/// A stroke-only L weighs by its stroke's ink, toward its corner, not on its box.
#[test]
fn a_stroked_l_centres_toward_its_corner() {
    let mut f = Fixture::new();
    let l = path(&[(0.0, 0.0), (0.0, 40.0), (40.0, 40.0)], false);
    let l = f.add(f.frame, l, (0.0, 0.0), &[stroked]);
    let VisualCentre::Ink(at) = f.centre(l) else {
        panic!("weighed by ink")
    };
    // Two arms 40 long and 2 wide meeting at (50, 70): their centres are (50, 50)
    // and (70, 70), so the centroid is about (60, 60); the box's is (70, 50).
    assert!(at.distance(Point::new(60.0, 60.0)) < 0.6, "{at:?}");
}

/// In a group, two overlapping layers count their overlap once, and a hidden
/// layer counts not at all.
///
/// flip: `visual_centre` summing the rings without the union → fails at "overlap once".
#[test]
fn overlaps_count_once_and_hidden_layers_not_at_all() {
    let mut f = Fixture::new();
    let g = f.group();
    f.rect(g, (0.0, 0.0, 10.0, 10.0), &[filled]);
    f.rect(g, (0.0, 0.0, 20.0, 1.0), &[filled]);
    f.rect(g, (200.0, 200.0, 50.0, 50.0), &[filled, hidden]);
    // The union: 10 × 10 about x 5 and a 10 × 1 strip about x 15, area 110.
    let want_x = 50.0 + (5.0 * 100.0 + 15.0 * 10.0) / 110.0;
    let at = f.centre(g).at();
    assert!((at.x - want_x).abs() < 1e-6, "overlap once: {at:?}");
}

/// A mask's run is weighed as it draws: cut to the mask, which weighs nothing.
#[test]
fn a_masked_run_is_cut_to_its_mask() {
    let mut f = Fixture::new();
    let g = f.group();
    f.rect(g, (0.0, 0.0, 10.0, 10.0), &[filled, masking]);
    f.rect(g, (0.0, 0.0, 40.0, 10.0), &[filled]);
    let at = f.centre(g).at();
    assert!(
        at.distance(Point::new(55.0, 35.0)) < 1e-6,
        "the left 10 only: {at:?}"
    );
}

/// A frame is a box and is measured as one; a layer with no ink of area falls
/// back to its box too.
#[test]
fn a_frame_and_an_unpainted_layer_centre_on_their_box() {
    let mut f = Fixture::new();
    assert_eq!(
        f.centre(f.frame),
        VisualCentre::Box(Point::new(250.0, 180.0))
    );
    let bare = f.rect(f.frame, (0.0, 0.0, 10.0, 20.0), &[]);
    assert_eq!(f.centre(bare), VisualCentre::Box(Point::new(55.0, 40.0)));
}

/// Aligning by visual centre puts each layer's centroid on the line, moving it
/// along that axis only.
#[test]
fn align_puts_the_centroid_on_the_line() {
    let mut f = Fixture::new();
    let t = f.add(f.frame, play(), (100.0, 100.0), &[filled]);
    let res = Resolved::rebuild(&f.doc);
    let (tx, text) = centroid::align(&f.doc, &res, &[t], 250.0, Axis::X);
    assert_eq!(text, 0);
    f.doc.apply(&tx).expect("the align");
    let at = f.centre(t).at();
    assert!(at.distance(Point::new(250.0, 134.0)) < 1e-6, "{at:?}");
}

fn unfilled(id: NodeId) -> Operation {
    Operation::SetFills {
        id,
        fills: Vec::new(),
    }
}

fn clear(id: NodeId) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::TRANSPARENT).into(),
            visible: true,
        }]),
    }
}

fn faded(id: NodeId) -> Operation {
    Operation::SetOpacity { id, opacity: 0.0 }
}

/// A clipping frame's content is weighed as it draws, cut to the frame; with
/// the clip off, all of it weighs.
/// flip: `collect` skipping the clip cut → fails at "cut to the frame".
#[test]
fn a_clipping_frame_weighs_only_what_it_shows() {
    let mut f = Fixture::new();
    let g = f.group();
    let frame = NodeKind::Artboard {
        size: Size::new(100.0, 100.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    };
    let inner = f.add(g, frame, (0.0, 0.0), &[unfilled]);
    f.rect(inner, (0.0, 0.0, 300.0, 100.0), &[filled]);
    assert!(
        f.doc.get(inner).unwrap().clip(),
        "the fixture: a frame clips"
    );
    let at = f.centre(g).at();
    assert!(
        at.distance(Point::new(100.0, 80.0)) < 1e-6,
        "cut to the frame: {at:?}"
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetClip {
            id: inner,
            clip: false,
        }]))
        .unwrap();
    let at = f.centre(g).at();
    assert!(
        at.distance(Point::new(200.0, 80.0)) < 1e-6,
        "the control: {at:?}"
    );
}

/// A wholly transparent fill, as an icon's keyline box has, and a layer at
/// opacity 0 do not weigh; a half-opaque one weighs whole.
/// flip: `collect` weighing a fill without `inks` → fails at "a clear keyline".
#[test]
fn paint_that_draws_nothing_does_not_weigh() {
    let mut f = Fixture::new();
    let g = f.group();
    f.rect(g, (0.0, 0.0, 100.0, 100.0), &[clear]);
    f.rect(g, (0.0, 0.0, 100.0, 100.0), &[filled, faded]);
    f.rect(g, (0.0, 0.0, 10.0, 10.0), &[filled]);
    let at = f.centre(g).at();
    assert!(
        at.distance(Point::new(55.0, 35.0)) < 1e-6,
        "a clear keyline: {at:?}"
    );
    let half = |id| Operation::SetOpacity { id, opacity: 0.5 };
    let g = f.group();
    f.rect(g, (0.0, 0.0, 10.0, 10.0), &[filled, half]);
    f.rect(g, (90.0, 0.0, 10.0, 10.0), &[filled]);
    let at = f.centre(g).at();
    assert!(
        (at.x - 100.0).abs() < 1e-6,
        "partial opacity weighs whole: {at:?}"
    );
}

/// A fill weighs by its fill rule: an inner square wound as the outer is a hole
/// even-odd and solid non-zero.
/// flip: `collect` reading every fill non-zero → fails at "the hole".
#[test]
fn a_fill_weighs_by_its_fill_rule() {
    let mut f = Fixture::new();
    let mut p = BezPath::new();
    for (lo, hi) in [(0.0, 30.0), (0.0, 10.0)] {
        p.move_to((lo, lo));
        p.line_to((hi, lo));
        p.line_to((hi, hi));
        p.line_to((lo, hi));
        p.close_path();
    }
    let kind = NodeKind::Path {
        path: p,
        corner_radii: Vec::new(),
        corner_smoothing: 0.0,
        mirror: ondin_core::symmetry::PathMirror::NONE,
    };
    let even_odd = |id| Operation::SetFillRule {
        id,
        rule: ondin_core::FillRule::EvenOdd,
    };
    let holed = f.add(f.frame, kind.clone(), (0.0, 0.0), &[filled, even_odd]);
    let solid = f.add(f.frame, kind, (0.0, 0.0), &[filled]);
    // 900 about 15 less 100 about 5, over 800.
    let at = f.centre(holed).at();
    assert!(
        at.distance(Point::new(66.25, 46.25)) < 1e-6,
        "the hole: {at:?}"
    );
    let at = f.centre(solid).at();
    assert!(at.distance(Point::new(65.0, 45.0)) < 1e-6, "{at:?}");
}

/// Ink under a turned and scaled group is weighed in world space: the triangle's
/// centroid lands where the group's transform takes it.
/// flip: `collect` keeping only the world transform's translation → fails at
/// "in world space".
#[test]
fn ink_under_a_turned_scaled_group_weighs_in_world_space() {
    let mut f = Fixture::new();
    let g = f.group();
    let turn = Affine::translate((100.0, 100.0))
        * Affine::rotate(std::f64::consts::FRAC_PI_2)
        * Affine::scale(2.0);
    f.doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: g,
            transform: turn,
        }]))
        .unwrap();
    let t = f.add(g, play(), (0.0, 0.0), &[filled]);
    let want = Affine::translate((50.0, 30.0)) * turn * Point::new(7.0 / 3.0, 4.0);
    let at = f.centre(t).at();
    assert!(
        at.distance(want) < 1e-6,
        "in world space: {at:?} want {want:?}"
    );
    let at = f.centre(g).at();
    assert!(at.distance(want) < 1e-6, "{at:?}");
}
