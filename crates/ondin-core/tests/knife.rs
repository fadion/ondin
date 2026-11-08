//! The knife (`knife.rs`): a line across closed shapes cuts each into the pieces it
//! parts, every piece keeping its shape's look, name and slot.
//!
//! The shapes sit on a frame translated by (50, 30), so the world space the line is
//! drawn in is never the parent's.

use ondin_core::kurbo::{Affine, Point, Shape, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position, knife,
};

const BLUE: Color = Color::from_rgb8(0x6d, 0x8c, 0xd9);
const ORANGE: Color = Color::from_rgb8(0xd9, 0xa3, 0x5b);

struct Fixture {
    doc: Document,
    hist: History,
    ids: IdSource,
    page: NodeId,
    frame: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0x4B);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let mut hist = History::new();
        let frame = ids.mint();
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id: frame,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(800.0, 600.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((50.0, 30.0))),
                name: None,
            }]),
        )
        .unwrap();
        Self {
            doc,
            hist,
            ids,
            page,
            frame,
        }
    }

    /// A `w` × `h` rect at `(x, y)` in the frame, filled `color`.
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, color: Color, name: &str) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(self.frame).unwrap().children().len();
        let tx = Transaction(vec![
            Operation::CreateNode {
                id,
                parent: self.frame,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(w, h),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((x, y))),
                name: Some(name.into()),
            },
            Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(color).into(),
                    visible: true,
                }]),
            },
        ]);
        self.hist.commit(&mut self.doc, tx).expect("commit");
        id
    }

    /// Cut along the world points `trail` with `selection` selected, and commit.
    fn cut(&mut self, selection: &[NodeId], trail: &[Point]) -> knife::Cut {
        let res = Resolved::rebuild(&self.doc);
        let (tx, cut) = knife::cut(&self.doc, &res, &mut self.ids, self.page, selection, trail)
            .expect("the cut");
        if !tx.0.is_empty() {
            self.hist.commit(&mut self.doc, tx).expect("commit");
        }
        cut
    }

    /// The frame's children: name, fill and world area of each.
    fn children(&self) -> Vec<(String, Option<Brush>, f64)> {
        let res = Resolved::rebuild(&self.doc);
        self.doc
            .get(self.frame)
            .unwrap()
            .children()
            .iter()
            .map(|&id| {
                let n = self.doc.get(id).unwrap();
                let fill = n
                    .paint()
                    .fills
                    .first()
                    .and_then(|f| f.brush.literal().cloned());
                let area = (res.world_transform(id).unwrap()
                    * res.local_path(&self.doc, id).unwrap())
                .area()
                .abs();
                (n.name().to_string(), fill, area)
            })
            .collect()
    }
}

/// With nothing selected, a line across two rects cuts both, each into two paths
/// that keep its name, paint and place in the stack, and add up to its area.
///
/// flip: `cut` naming the extra pieces with a fresh *Path* name → fails at the names.
#[test]
fn a_line_cuts_every_shape_under_it_keeping_their_looks() {
    let mut f = Fixture::new();
    f.rect(0.0, 0.0, 100.0, 60.0, BLUE, "Left");
    f.rect(200.0, 0.0, 100.0, 60.0, ORANGE, "Right");
    // World y 30 + 20: a third of the way down both.
    let cut = f.cut(&[], &[Point::new(0.0, 50.0), Point::new(400.0, 50.0)]);
    assert_eq!((cut.shapes, cut.pieces, cut.instances), (2, 4, false));
    assert_eq!(cut.made.len(), 4, "every piece named");
    let kids = f.children();
    let names: Vec<&str> = kids.iter().map(|k| k.0.as_str()).collect();
    assert_eq!(names, ["Left", "Left", "Right", "Right"]);
    for (name, fill, _) in &kids {
        let want = if name == "Left" { BLUE } else { ORANGE };
        assert_eq!(fill, &Some(Brush::Solid(want)), "{name} keeps its paint");
    }
    let left: f64 = kids.iter().filter(|k| k.0 == "Left").map(|k| k.2).sum();
    assert!((left - 6000.0).abs() < 1e-6, "{left}");
    let mut areas: Vec<f64> = kids.iter().filter(|k| k.0 == "Left").map(|k| k.2).collect();
    areas.sort_by(f64::total_cmp);
    assert!((areas[0] - 2000.0).abs() < 1e-6 && (areas[1] - 4000.0).abs() < 1e-6);
    assert!(
        f.doc
            .get(f.doc.get(f.frame).unwrap().children()[0])
            .is_some_and(|n| matches!(n.kind(), NodeKind::Path { .. })),
        "a primitive becomes a path"
    );
}

/// With a selection, only it is cut; a line that crosses nothing cuts nothing.
#[test]
fn a_selection_is_all_a_line_cuts() {
    let mut f = Fixture::new();
    let left = f.rect(0.0, 0.0, 100.0, 60.0, BLUE, "Left");
    f.rect(200.0, 0.0, 100.0, 60.0, ORANGE, "Right");
    let cut = f.cut(&[left], &[Point::new(0.0, 50.0), Point::new(400.0, 50.0)]);
    assert_eq!((cut.shapes, cut.pieces), (1, 2));
    assert_eq!(f.children().len(), 3);

    let none = f.cut(&[], &[Point::new(0.0, 500.0), Point::new(400.0, 500.0)]);
    assert_eq!(none, knife::Cut::default(), "nothing to cut here");
    assert_eq!(f.children().len(), 3);
}

impl Fixture {
    fn commit(&mut self, ops: Vec<Operation>) {
        self.hist
            .commit(&mut self.doc, Transaction(ops))
            .expect("commit");
    }

    /// A path in the frame from frame-space subpaths, each closed, filled by `rule`.
    fn path(&mut self, subpaths: &[&[(f64, f64)]], rule: ondin_core::FillRule) -> NodeId {
        let mut path = ondin_core::kurbo::BezPath::new();
        for sub in subpaths {
            path.move_to(sub[0]);
            for p in &sub[1..] {
                path.line_to(*p);
            }
            path.close_path();
        }
        let id = self.ids.mint();
        let index = self.doc.get(self.frame).unwrap().children().len();
        self.commit(vec![
            Operation::CreateNode {
                id,
                parent: self.frame,
                index,
                kind: NodeKind::Path {
                    path,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: Default::default(),
                },
                transform: None,
                name: Some("Path".into()),
            },
            Operation::SetFillRule { id, rule },
        ]);
        id
    }

    /// Every layer in the frame drawn over the world point `p`, by its own rule.
    fn covering(&self, p: Point) -> usize {
        let res = Resolved::rebuild(&self.doc);
        self.doc
            .get(self.frame)
            .unwrap()
            .children()
            .iter()
            .filter(|&&id| {
                let outline =
                    res.world_transform(id).unwrap() * res.local_path(&self.doc, id).unwrap();
                let w = ondin_core::geometry::winding(&outline, p);
                match self.doc.get(id).unwrap().fill_rule() {
                    ondin_core::FillRule::NonZero => w != 0,
                    ondin_core::FillRule::EvenOdd => w % 2 != 0,
                }
            })
            .count()
    }
}

/// World point of frame point `(x, y)`.
fn w(x: f64, y: f64) -> Point {
    Point::new(x + 50.0, y + 30.0)
}

/// With nothing selected, a shape inside a hidden group, and a locked one, are
/// not cut: neither is a visible, unlocked shape.
///
/// flip: `shape_builder::part` testing only the layer's own eye (`doc.shown`) →
/// fails at "hidden".
#[test]
fn hidden_and_locked_shapes_are_left_whole() {
    let mut f = Fixture::new();
    let g = f.ids.mint();
    f.commit(vec![
        Operation::CreateNode {
            id: g,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::SetVisible {
            id: g,
            visible: false,
        },
    ]);
    let inner = f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "Inner");
    let at = f.doc.get(f.frame).unwrap().children().len() - 1;
    f.commit(vec![Operation::Reparent {
        id: inner,
        new_parent: g,
        index: 0,
    }]);
    assert_eq!(at, 1, "the fixture: the rect was beside the group");
    let line = [w(50.0, -10.0), w(50.0, 110.0)];
    assert_eq!(f.cut(&[], &line), knife::Cut::default(), "hidden");

    let mut f = Fixture::new();
    let locked = f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "Locked");
    f.commit(vec![Operation::SetLocked {
        id: locked,
        locked: true,
    }]);
    assert_eq!(f.cut(&[], &line), knife::Cut::default(), "locked");
}

/// A straight cut down the seam two rects share, which runs along an edge of each,
/// parts neither.
///
/// Two defences hold it, and either alone passes: `chords` dropping a run along
/// the outline, and `pieces` dropping a piece under `MIN_PIECE_AREA`.
/// flip: both removed → fails at the first assertion.
#[test]
fn a_line_along_an_edge_cuts_nothing() {
    let mut f = Fixture::new();
    f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "Top");
    f.rect(0.0, 100.0, 100.0, 100.0, ORANGE, "Bottom");
    let cut = f.cut(&[], &[w(0.0, -10.0), w(0.0, 210.0)]);
    assert_eq!(cut, knife::Cut::default());
    let cut = f.cut(&[], &[w(-10.0, 100.0), w(110.0, 100.0)]);
    assert_eq!(cut, knife::Cut::default(), "nor along the seam itself");
}

/// A freehand X drawn in one stroke, looping round outside the square, parts it in
/// four along both passes.
///
/// flip: `pieces` applying the chords without `split_at_crossings` → fails at the
/// count (two).
#[test]
fn a_self_crossing_line_cuts_along_both_passes() {
    let mut f = Fixture::new();
    f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "Square");
    let trail = [
        w(-10.0, 30.0),
        w(110.0, 70.0),
        w(110.0, 120.0),
        w(-10.0, 120.0),
        w(-10.0, 70.0),
        w(110.0, 30.0),
    ];
    let cut = f.cut(&[], &trail);
    assert_eq!(cut.pieces, 4);
    let total: f64 = f.children().iter().map(|k| k.2).sum();
    assert!((total - 10_000.0).abs() < 1e-3, "{total}");
}

/// The cut reads a path by its fill rule: a same-way inner square under non-zero
/// stays solid, and the crossing of two squares under even-odd stays a hole.
///
/// flip: `parted` without `boolean::filled_faces` → fails at "solid" (and "hole").
#[test]
fn a_cut_keeps_what_the_fill_rule_draws() {
    use ondin_core::FillRule;
    let outer: &[(f64, f64)] = &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    let inner: &[(f64, f64)] = &[(30.0, 30.0), (70.0, 30.0), (70.0, 70.0), (30.0, 70.0)];
    let mut f = Fixture::new();
    f.path(&[outer, inner], FillRule::NonZero);
    assert_eq!(f.covering(w(50.0, 50.0)), 1, "the fixture draws its middle");
    let cut = f.cut(&[], &[w(10.0, -10.0), w(10.0, 110.0)]);
    assert!(cut.pieces >= 2, "{cut:?}");
    assert_eq!(f.covering(w(50.0, 50.0)), 1, "solid");
    assert_eq!(f.covering(w(5.0, 50.0)), 1);

    let crossing: &[(f64, f64)] = &[(50.0, 50.0), (150.0, 50.0), (150.0, 150.0), (50.0, 150.0)];
    let mut f = Fixture::new();
    f.path(&[outer, crossing], FillRule::EvenOdd);
    assert_eq!(f.covering(w(75.0, 75.0)), 0, "the fixture: a hole");
    let cut = f.cut(&[], &[w(25.0, -10.0), w(25.0, 110.0)]);
    assert!(cut.pieces >= 2, "{cut:?}");
    assert_eq!(f.covering(w(75.0, 75.0)), 0, "hole");
    assert_eq!(f.covering(w(10.0, 10.0)), 1);
    assert_eq!(f.covering(w(120.0, 120.0)), 1);
}

/// A shape its parent lays out is left whole: a new piece would join the flow
/// and move it and its siblings.
///
/// flip: `shape_builder::part` without its `parent_lays_out` test → fails at the
/// assertion.
#[test]
fn a_shape_in_a_layout_is_left_whole() {
    let mut f = Fixture::new();
    f.commit(vec![Operation::SetDisplay {
        id: f.frame,
        display: Some(ondin_core::container::Display::Flex(Default::default())),
    }]);
    f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "A");
    let res = Resolved::rebuild(&f.doc);
    let a = f.doc.get(f.frame).unwrap().children()[0];
    let b = res.world_bounds(a).unwrap();
    let line = [
        Point::new(b.center().x, b.y0 - 10.0),
        Point::new(b.center().x, b.y1 + 10.0),
    ];
    assert_eq!(f.cut(&[], &line), knife::Cut::default());
}

/// An instance's shape under the line is left whole and reported, so the status
/// can say to detach it; the document is untouched.
///
/// flip: `subjects` without its `instance_root` arm → fails at `instances`.
#[test]
fn an_instance_is_reported_not_cut() {
    let mut f = Fixture::new();
    f.rect(0.0, 0.0, 100.0, 100.0, BLUE, "Inside");
    f.commit(vec![Operation::SetComponent {
        id: f.frame,
        component: true,
    }]);
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[ondin_core::build::Placement {
            nodes: f.doc.capture_subtree(f.frame).unwrap(),
            parent: f.page,
            index: None,
        }],
        Default::default(),
    );
    f.hist.commit(&mut f.doc, tx).unwrap();
    let before = f.doc.clone();
    let res = Resolved::rebuild(&f.doc);
    let b = res.world_bounds(made[0]).unwrap();
    let line = [
        Point::new(b.x0 + 50.0, b.y0 - 10.0),
        Point::new(b.x0 + 50.0, b.y1 + 10.0),
    ];
    let cut = f.cut(&[made[0]], &line);
    assert!(cut.instances, "{cut:?}");
    assert_eq!(cut.shapes, 0);
    assert_eq!(f.doc, before);
}

/// A turned ellipse cut through comes apart in pieces whose areas add up to its.
#[test]
fn a_turned_ellipse_cuts_into_its_area() {
    let mut f = Fixture::new();
    let id = f.ids.mint();
    f.commit(vec![Operation::CreateNode {
        id,
        parent: f.frame,
        index: 0,
        kind: NodeKind::Ellipse {
            size: Size::new(200.0, 80.0),
            arc: Default::default(),
        },
        transform: Some(Affine::translate((100.0, 100.0)) * Affine::rotate(0.5)),
        name: Some("Oval".into()),
    }]);
    let before: f64 = f.children().iter().map(|k| k.2).sum();
    let res = Resolved::rebuild(&f.doc);
    let centre = res.world_bounds(id).unwrap().center();
    let cut = f.cut(
        &[],
        &[
            Point::new(0.0, centre.y + 7.0),
            Point::new(600.0, centre.y - 7.0),
        ],
    );
    assert_eq!((cut.shapes, cut.pieces), (1, 2));
    let after: f64 = f.children().iter().map(|k| k.2).sum();
    assert!(
        (after - before).abs() < before * 1e-3,
        "{after} against {before}"
    );
}
