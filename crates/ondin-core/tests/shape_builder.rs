//! The shape builder's merge and remove, and *Divide* (`shape_builder.rs`).
//!
//! The fixture is the design's three circles, r 78, on a frame translated by
//! (50, 30), so the world space the arrangement is worked in is never the parent's.

use ondin_core::kurbo::{Affine, BezPath, Point, Shape, Size};
use ondin_core::peniko::Color;
use ondin_core::shape_builder::{self, Arrangement, Refusal};
use ondin_core::{
    Brush, Document, Fill, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};

const BLUE: Color = Color::from_rgb8(0x6d, 0x8c, 0xd9);
const ORANGE: Color = Color::from_rgb8(0xd9, 0xa3, 0x5b);
const GREEN: Color = Color::from_rgb8(0x6f, 0xb3, 0x8a);

struct Fixture {
    doc: Document,
    hist: History,
    ids: IdSource,
    frame: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0x5B);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut hist = History::new();
        let frame = ids.mint();
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::CreateNode {
                id: frame,
                parent: root,
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
            frame,
        }
    }

    /// A circle of radius 78 centred on `(x, y)` in `parent`, filled `color`.
    fn circle(&mut self, parent: NodeId, x: f64, y: f64, color: Color, name: &str) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent,
                index,
                kind: NodeKind::Ellipse {
                    size: Size::new(156.0, 156.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((x - 78.0, y - 78.0))),
                name: Some(name.into()),
            },
            Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(color).into(),
                    visible: true,
                }]),
            },
        ]));
        id
    }

    fn venn(&mut self) -> [NodeId; 3] {
        let f = self.frame;
        [
            self.circle(f, 211.0, 140.0, BLUE, "Ellipse 1"),
            self.circle(f, 311.0, 140.0, ORANGE, "Ellipse 2"),
            self.circle(f, 261.0, 222.0, GREEN, "Ellipse 3"),
        ]
    }

    fn resolved(&self) -> Resolved {
        Resolved::rebuild(&self.doc)
    }

    fn commit(&mut self, tx: Transaction) {
        self.hist.commit(&mut self.doc, tx).expect("commit");
    }

    fn arrange(&self, selection: &[NodeId]) -> Arrangement {
        shape_builder::arrange(&self.doc, &self.resolved(), selection).expect("an arrangement")
    }

    /// `id`'s outline in world space.
    fn world(&self, id: NodeId) -> BezPath {
        let res = self.resolved();
        res.world_transform(id).unwrap() * res.local_path(&self.doc, id).unwrap()
    }

    fn fill(&self, id: NodeId) -> Option<Brush> {
        let f = self.doc.get(id)?.paint().fills.first()?;
        f.brush.literal().cloned()
    }

    fn name(&self, id: NodeId) -> &str {
        self.doc.get(id).unwrap().name()
    }
}

/// Where the world points of the design's faces are, in frame space plus (50, 30).
fn at(x: f64, y: f64) -> Point {
    Point::new(x + 50.0, y + 30.0)
}

fn covers(p: &BezPath, q: Point) -> bool {
    ondin_core::geometry::winding(p, q) != 0
}

/// The design's R4: a drag from the left circle's own face across the lens and the
/// right circle's own face merges the three into one blue path, which lands above
/// Ellipse 2 and below Ellipse 3; Ellipses 1 and 2 keep their names and only their
/// parts inside the green circle.
#[test]
fn a_merge_takes_the_first_faces_paint_and_lands_above_what_it_took_from() {
    let mut f = Fixture::new();
    let [a, b, c] = f.venn();
    let arr = f.arrange(&[a, b, c]);
    assert_eq!(arr.regions.len(), 7);
    let picked: Vec<usize> = [at(160.0, 128.0), at(261.0, 100.0), at(362.0, 126.0)]
        .iter()
        .map(|p| arr.region_at(*p).expect("a face under each point"))
        .collect();
    let res = f.resolved();
    let (tx, built) = arr
        .merge(&f.doc, &res, &mut f.ids, &picked)
        .expect("a merge");
    f.commit(tx);

    let merged = built.merged.expect("a merge makes a path");
    assert_eq!(
        f.fill(merged),
        Some(Brush::Solid(BLUE)),
        "the first face's paint"
    );
    let kids = f.doc.get(f.frame).unwrap().children().to_vec();
    assert_eq!(kids.len(), 4);
    assert_eq!(
        kids[2], merged,
        "above Ellipse 2, below Ellipse 3: {kids:?}"
    );
    assert_eq!(f.name(kids[0]), "Ellipse 1");
    assert_eq!(f.name(kids[1]), "Ellipse 2");
    assert_eq!(kids[3], c, "Ellipse 3 was never touched");
    assert!(matches!(
        f.doc.get(kids[0]).unwrap().kind(),
        NodeKind::Path { .. }
    ));
    assert_eq!(built.kept, vec![kids[0], kids[1], c]);

    let shape = f.world(merged);
    for p in [at(160.0, 128.0), at(261.0, 100.0), at(362.0, 126.0)] {
        assert!(covers(&shape, p), "the merged path covers {p:?}");
    }
    assert!(
        !covers(&shape, at(261.0, 160.0)),
        "the triple face was not crossed"
    );
    let left = f.world(kids[0]);
    assert!(
        !covers(&left, at(160.0, 128.0)),
        "Ellipse 1 lost its own face"
    );
    assert!(
        covers(&left, at(225.0, 190.0)),
        "and kept its part inside the green"
    );
}

/// An Alt-drag across the lens two circles share cuts it out of both, and makes
/// nothing.
#[test]
fn a_removal_cuts_the_faces_out_of_every_shape_over_them() {
    let mut f = Fixture::new();
    let [a, b, c] = f.venn();
    let arr = f.arrange(&[a, b, c]);
    let lens = arr.region_at(at(261.0, 100.0)).unwrap();
    let triple = arr.region_at(at(261.0, 160.0)).unwrap();
    let res = f.resolved();
    let (tx, built) = arr
        .remove(&f.doc, &res, &mut f.ids, &[lens, triple])
        .unwrap();
    f.commit(tx);
    assert_eq!(built.merged, None);
    assert_eq!(f.doc.get(f.frame).unwrap().children().len(), 3);
    for id in &built.kept {
        let p = f.world(*id);
        assert!(!covers(&p, at(261.0, 100.0)) && !covers(&p, at(261.0, 160.0)));
    }
    assert!(covers(&f.world(built.kept[0]), at(160.0, 128.0)));
}

/// A shape every face of which is taken is removed, not left as an empty path.
#[test]
fn a_shape_left_with_nothing_is_removed() {
    let mut f = Fixture::new();
    let a = f.circle(f.frame, 100.0, 100.0, BLUE, "Small");
    let b = f.circle(f.frame, 150.0, 100.0, ORANGE, "Other");
    let arr = f.arrange(&[a, b]);
    let all: Vec<usize> = (0..arr.regions.len())
        .filter(|i| arr.regions[*i].covers.contains(&0))
        .collect();
    let res = f.resolved();
    let (tx, built) = arr.merge(&f.doc, &res, &mut f.ids, &all).unwrap();
    f.commit(tx);
    assert!(!f.doc.contains(a));
    assert_eq!(
        built.kept.len(),
        1,
        "only the other shape is left: {built:?}"
    );
}

/// *Divide* cuts three circles into seven paths that cover what each showed with its
/// paint, named after the shape whose paint they kept, in a group named *Divided*
/// in the topmost source's slot.
#[test]
fn divide_makes_one_path_per_face_painted_as_it_showed() {
    let mut f = Fixture::new();
    let [a, b, c] = f.venn();
    let union = ondin_core::evaluate_boolean(
        ondin_core::BoolOp::Union,
        &[f.world(a), f.world(b), f.world(c)],
    )
    .unwrap()
    .area()
    .abs();
    let res = f.resolved();
    let (tx, group) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[a, b, c]).unwrap();
    f.commit(tx);
    assert_eq!(f.name(group), shape_builder::DIVIDED);
    assert_eq!(f.doc.get(f.frame).unwrap().children(), &[group]);
    let paths = f.doc.get(group).unwrap().children().to_vec();
    assert_eq!(paths.len(), 7);
    let named = |n: &str| paths.iter().filter(|p| f.name(**p) == n).count();
    assert_eq!(
        (named("Ellipse 1"), named("Ellipse 2"), named("Ellipse 3")),
        (1, 2, 4)
    );
    // The lens shows the orange circle above the blue one.
    let lens = paths
        .iter()
        .find(|p| covers(&f.world(**p), at(261.0, 100.0)))
        .unwrap();
    assert_eq!(f.fill(*lens), Some(Brush::Solid(ORANGE)));
    let total: f64 = paths.iter().map(|p| f.world(*p).area()).sum();
    assert!(
        (total - union).abs() < union * 1e-4,
        "faces {total}, union {union}"
    );
}

/// A selection with one closed shape, or shapes that do not overlap, is refused,
/// and text left out is said.
#[test]
fn too_few_or_apart_shapes_are_refused() {
    let mut f = Fixture::new();
    let a = f.circle(f.frame, 100.0, 100.0, BLUE, "A");
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[a]).unwrap_err(),
        Refusal::TooFew
    );
    let b = f.circle(f.frame, 400.0, 100.0, BLUE, "B");
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[a, b]).unwrap_err(),
        Refusal::NoOverlap
    );
    assert!(shape_builder::can_arrange(&f.doc, &[a, b]));
    assert!(!shape_builder::can_arrange(&f.doc, &[a]));
}

/// A circle inside a group cuts the others where it is drawn, and the merged path
/// lands in that group's space without moving.
#[test]
fn shapes_under_different_parents_cut_each_other_where_they_are_drawn() {
    let mut f = Fixture::new();
    let a = f.circle(f.frame, 211.0, 140.0, BLUE, "A");
    let g = f.ids.mint();
    f.commit(Transaction(vec![Operation::CreateNode {
        id: g,
        parent: f.frame,
        index: 1,
        kind: NodeKind::Group,
        transform: Some(Affine::translate((100.0, 0.0))),
        name: None,
    }]));
    let b = f.circle(g, 211.0, 140.0, ORANGE, "B");
    let arr = f.arrange(&[a, g]);
    assert_eq!(
        arr.sources,
        vec![a, b],
        "the group is opened into its contents"
    );
    let lens = arr.region_at(at(261.0, 140.0)).unwrap();
    let res = f.resolved();
    let (tx, built) = arr.merge(&f.doc, &res, &mut f.ids, &[lens]).unwrap();
    f.commit(tx);
    let merged = built.merged.unwrap();
    assert_eq!(f.doc.get(merged).unwrap().parent(), Some(g));
    assert!(covers(&f.world(merged), at(261.0, 140.0)));
    assert!(!covers(&f.world(merged), at(211.0, 140.0)));
    assert_eq!(
        f.fill(merged),
        Some(Brush::Solid(ORANGE)),
        "B is over the lens"
    );
}
