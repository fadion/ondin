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
    let arr = f.arrange(&[a, b, c]);
    let (tx, group) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[a, b, c], &arr).unwrap();
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

impl Fixture {
    /// A line in `parent` from `(x0, y)` to `(x1, y)`, frame space.
    fn line(&mut self, parent: NodeId, x0: f64, x1: f64, y: f64) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Line {
                end: Point::new(x1 - x0, 0.0),
            },
            transform: Some(Affine::translate((x0, y))),
            name: Some("Line".into()),
        }]));
        id
    }
}

/// A line across one circle cuts it into two faces, fills neither and is no
/// source; removing the top face leaves the circle's bottom half and the line
/// as it was.
/// flip: `arrange` without `cut_regions` → refuses `NoOverlap` at the fixture's
/// "an arrangement".
#[test]
fn a_line_across_a_shape_cuts_its_face_in_two() {
    let mut f = Fixture::new();
    let fr = f.frame;
    let a = f.circle(fr, 211.0, 140.0, BLUE, "Ellipse 1");
    let line = f.line(fr, 100.0, 400.0, 140.0);
    let before = f.doc.get(line).unwrap().clone();
    let arr = f.arrange(&[a, line]);
    assert_eq!(arr.regions.len(), 2, "two halves");
    assert_eq!(arr.sources, [a], "the line is no source");
    assert_eq!(arr.cutters, [line]);
    assert!(!arr.left_out, "the line took part");
    let top = arr.region_at(at(211.0, 100.0)).expect("the top half");
    let res = f.resolved();
    let (tx, built) = arr.remove(&f.doc, &res, &mut f.ids, &[top]).unwrap();
    f.commit(tx);
    let kept = built.kept[0];
    let left = f.world(kept);
    assert!(covers(&left, at(211.0, 180.0)), "the bottom half stays");
    assert!(!covers(&left, at(211.0, 100.0)), "the top half went");
    assert_eq!(f.doc.get(line), Some(&before), "the line is untouched");
}

/// A line through the three circles adds faces where it crosses them, and a line
/// that crosses nothing cuts nothing; a line alone is too few.
#[test]
fn a_line_adds_faces_only_where_it_crosses() {
    let mut f = Fixture::new();
    let fr = f.frame;
    let [a, b, c] = f.venn();
    let through = f.line(fr, 100.0, 420.0, 140.0);
    assert!(f.arrange(&[a, b, c, through]).regions.len() > 7);
    let lone = f.circle(fr, 600.0, 450.0, BLUE, "Lone");
    let wide = f.line(fr, 100.0, 420.0, 20.0);
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[lone, wide]).err(),
        Some(Refusal::NoOverlap),
        "the line misses the circle"
    );
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[wide]).err(),
        Some(Refusal::TooFew)
    );
    assert!(shape_builder::can_arrange(&f.doc, &[lone, wide]));
    assert!(!shape_builder::can_arrange(&f.doc, &[wide]));
}

impl Fixture {
    /// A group in `parent` at its top, holding nothing yet.
    fn group(&mut self, parent: NodeId) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }]));
        id
    }

    fn set(&mut self, op: Operation) {
        self.commit(Transaction(vec![op]));
    }

    /// The opacity `id` is drawn at, its parents' multiplied in.
    fn effective_opacity(&self, id: NodeId) -> f64 {
        let mut product = 1.0;
        let mut up = Some(id);
        while let Some(n) = up.and_then(|i| self.doc.get(i)) {
            product *= f64::from(self.doc.drawn_opacity(n.id()));
            up = n.parent();
        }
        product
    }
}

/// A mask is left out of the arrangement, so a group of a mask and the shape it
/// clips is too few, and nothing is painted where the mask drew nothing.
///
/// flip: `part` taking a mask → fails at `TooFew`.
#[test]
fn a_mask_takes_no_part() {
    let mut f = Fixture::new();
    let g = f.group(f.frame);
    let mask = f.circle(g, 100.0, 100.0, BLUE, "Mask");
    f.circle(g, 150.0, 100.0, ORANGE, "Shown");
    f.set(Operation::SetMask {
        id: mask,
        mask: true,
    });
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[g]).err(),
        Some(Refusal::TooFew)
    );
}

/// A locked shape and a shape in a hidden group take no part, and a picture-filled
/// one is said as left out.
#[test]
fn locked_hidden_and_pictured_shapes_are_left_out() {
    let mut f = Fixture::new();
    let a = f.circle(f.frame, 100.0, 100.0, BLUE, "A");
    let b = f.circle(f.frame, 150.0, 100.0, ORANGE, "B");
    f.set(Operation::SetLocked {
        id: b,
        locked: true,
    });
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[a, b]).err(),
        Some(Refusal::TooFew)
    );

    let mut f = Fixture::new();
    let a = f.circle(f.frame, 100.0, 100.0, BLUE, "A");
    let g = f.group(f.frame);
    let b = f.circle(g, 150.0, 100.0, ORANGE, "B");
    f.set(Operation::SetVisible {
        id: g,
        visible: false,
    });
    let res = f.resolved();
    assert_eq!(
        shape_builder::arrange(&f.doc, &res, &[a, b]).err(),
        Some(Refusal::TooFew)
    );

    let mut f = Fixture::new();
    let a = f.circle(f.frame, 100.0, 100.0, BLUE, "A");
    let b = f.circle(f.frame, 150.0, 100.0, ORANGE, "B");
    let c = f.circle(f.frame, 120.0, 140.0, GREEN, "C");
    f.set(Operation::SetFills {
        id: c,
        fills: keyed_by_position([Fill {
            brush: ondin_core::image_brush(ondin_core::ImageId("pic".into())).into(),
            visible: true,
        }]),
    });
    assert!(
        f.arrange(&[a, b, c]).left_out,
        "the picture is said as left out"
    );
}

/// *Divide* on a selected half-opaque group of two circles: the group goes, and
/// *Divided* takes its opacity, so every face still draws at half.
///
/// flip: `divide` without the stand-in's `carried_look` → fails at the opacity.
#[test]
fn divide_keeps_a_selected_groups_opacity() {
    let mut f = Fixture::new();
    let g = f.group(f.frame);
    f.circle(g, 100.0, 100.0, BLUE, "A");
    f.circle(g, 150.0, 100.0, ORANGE, "B");
    f.set(Operation::SetOpacity {
        id: g,
        opacity: 0.5,
    });
    let res = f.resolved();
    let arr = f.arrange(&[g]);
    let (tx, divided) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[g], &arr).unwrap();
    f.commit(tx);
    assert!(!f.doc.contains(g), "the emptied group goes");
    for face in f.doc.get(divided).unwrap().children().to_vec() {
        let o = f.effective_opacity(face);
        assert!((o - 0.5).abs() < 1e-6, "{o}");
    }
}

/// A face from a source inside a half-opaque group, moved out of it by *Divide*,
/// takes that opacity onto itself; a group holding a layer that takes no part
/// stays.
///
/// flip: `divide` without the opacity fold → fails at the half.
#[test]
fn divide_folds_a_left_parents_opacity_into_the_face() {
    let mut f = Fixture::new();
    let g = f.group(f.frame);
    let a = f.circle(g, 100.0, 100.0, BLUE, "A");
    let hidden = f.circle(g, 400.0, 400.0, GREEN, "Hidden");
    f.set(Operation::SetVisible {
        id: hidden,
        visible: false,
    });
    f.set(Operation::SetOpacity {
        id: g,
        opacity: 0.5,
    });
    let b = f.circle(f.frame, 150.0, 100.0, ORANGE, "B");
    let res = f.resolved();
    let arr = f.arrange(&[a, b]);
    let (tx, divided) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[a, b], &arr).unwrap();
    f.commit(tx);
    assert!(f.doc.contains(g), "the group holding a hidden layer stays");
    let faces = f.doc.get(divided).unwrap().children().to_vec();
    assert_eq!(faces.len(), arr.regions.len());
    let face = faces
        .iter()
        .copied()
        .find(|p| covers(&f.world(*p), at(60.0, 100.0)))
        .expect("A's own face");
    let o = f.effective_opacity(face);
    assert!((o - 0.5).abs() < 1e-6, "{o}");
}

/// An even-odd path of two crossing squares draws their overlap as a hole; the
/// arrangement offers no face there, and *Divide* paints nothing there.
///
/// flip: `filled_regions` passing every outline whole → the arrangement is
/// refused, failing at the fixture's "an arrangement".
#[test]
fn an_even_odd_hole_is_no_face() {
    let mut f = Fixture::new();
    let mut p = BezPath::new();
    for (x, y) in [(0.0, 0.0), (50.0, 50.0)] {
        p.move_to((x, y));
        p.line_to((x + 100.0, y));
        p.line_to((x + 100.0, y + 100.0));
        p.line_to((x, y + 100.0));
        p.close_path();
    }
    let path = f.ids.mint();
    f.commit(Transaction(vec![
        Operation::CreateNode {
            id: path,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Path {
                path: p,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: Default::default(),
            },
            transform: None,
            name: Some("Squares".into()),
        },
        Operation::SetFillRule {
            id: path,
            rule: ondin_core::FillRule::EvenOdd,
        },
    ]));
    let c = f.circle(f.frame, 150.0, 150.0, BLUE, "C");
    let arr = f.arrange(&[path, c]);
    assert_eq!(arr.region_at(at(75.0, 75.0)), None, "no face");
    assert!(arr.region_at(at(10.0, 10.0)).is_some());
    let res = f.resolved();
    let (tx, divided) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[path, c], &arr).unwrap();
    f.commit(tx);
    for face in f.doc.get(divided).unwrap().children().to_vec() {
        assert!(
            !covers(&f.world(face), at(75.0, 75.0)),
            "nothing paints the hole"
        );
    }
}

/// A circle and a copy of it turned about its centre trace one outline with
/// other control points: either the arrangement refuses, or its faces are few and
/// none overlaps another.
///
/// flip: `regions` keeping slivers by area alone → fails at the face count;
/// `arrange` without `overlapping` → fails at "no face twice".
#[test]
fn a_turned_copy_of_a_circle_leaves_no_slivers_or_doubles() {
    for turn in [0.3, std::f64::consts::FRAC_PI_4] {
        let mut f = Fixture::new();
        let a = f.circle(f.frame, 200.0, 200.0, BLUE, "A");
        let b = f.circle(f.frame, 200.0, 200.0, ORANGE, "B");
        let t = f.doc.get(b).unwrap().transform();
        f.set(Operation::SetTransform {
            id: b,
            transform: Affine::rotate_about(turn, Point::new(200.0, 200.0)) * t,
        });
        let res = f.resolved();
        match shape_builder::arrange(&f.doc, &res, &[a, b]) {
            Err(r) => assert_eq!(r, Refusal::Abandoned, "{turn}"),
            Ok(arr) => {
                assert!(
                    arr.regions.len() <= 3,
                    "{turn}: {} faces",
                    arr.regions.len()
                );
                let over_centre = arr
                    .regions
                    .iter()
                    .filter(|r| covers(&r.path, at(200.0, 200.0)))
                    .count();
                assert_eq!(over_centre, 1, "{turn}: no face twice");
            }
        }
    }
}

/// A merge whose arithmetic unwinds refuses the whole edit.
#[test]
fn an_unwound_merge_refuses() {
    let mut f = Fixture::new();
    let [a, b, c] = f.venn();
    let arr = f.arrange(&[a, b, c]);
    let lens = arr.region_at(at(261.0, 100.0)).unwrap();
    let res = f.resolved();
    ondin_core::boolean::poison_next(1);
    let got = arr.merge(&f.doc, &res, &mut f.ids, &[lens]);
    assert!(
        matches!(got, Err(ondin_core::OpError::BooleanAbandoned)),
        "{got:?}"
    );
}

/// A turned rect losing the lens it shares with a circle is rewritten as a path
/// that keeps the rest of it where it was drawn.
#[test]
fn a_turned_source_keeps_its_place() {
    let mut f = Fixture::new();
    let turn = Affine::rotate_about(0.5, Point::new(200.0, 150.0));
    let r = f.ids.mint();
    f.commit(Transaction(vec![Operation::CreateNode {
        id: r,
        parent: f.frame,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(200.0, 100.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(turn * Affine::translate((100.0, 100.0))),
        name: Some("Bar".into()),
    }]));
    let before = f.world(r).area().abs();
    let c = f.circle(f.frame, 300.0, 150.0, BLUE, "C");
    let arr = f.arrange(&[r, c]);
    let lens = (0..arr.regions.len())
        .find(|i| arr.regions[*i].covers.len() == 2)
        .expect("a lens");
    let lens_area = arr.regions[lens].path.area().abs();
    let res = f.resolved();
    let (tx, built) = arr.remove(&f.doc, &res, &mut f.ids, &[lens]).unwrap();
    f.commit(tx);
    let after = f.world(built.kept[0]);
    let want = before - lens_area;
    assert!(
        (after.area().abs() - want).abs() < 1.0,
        "{after:?} against {want}"
    );
    let p = turn * Point::new(110.0, 150.0);
    assert!(
        covers(&after, at(p.x, p.y)),
        "the far end stays where it was drawn"
    );
}

/// *Divide* over a credited group hands each face the credit.
///
/// flip: `divide` without `face_credit` → fails at the first face.
#[test]
fn divide_keeps_an_icons_credit() {
    let mut f = Fixture::new();
    let g = f.group(f.frame);
    f.circle(g, 100.0, 100.0, BLUE, "A");
    f.circle(g, 150.0, 100.0, ORANGE, "B");
    let credit = ondin_core::Credit {
        kind: ondin_core::CreditKind::Icon,
        text: "Icon by Ana".into(),
        author: "Ana".into(),
        author_url: String::new(),
        provider: "Icons".into(),
        provider_url: String::new(),
        source_url: String::new(),
        licence: "CC BY 4.0".into(),
        licence_url: String::new(),
        required: true,
        inserted: "2026-11-08".into(),
    };
    f.set(Operation::SetCredit {
        id: g,
        credit: Some(Box::new(credit.clone())),
    });
    let res = f.resolved();
    let arr = f.arrange(&[g]);
    let (tx, divided) = shape_builder::divide(&f.doc, &res, &mut f.ids, &[g], &arr).unwrap();
    f.commit(tx);
    for face in f.doc.get(divided).unwrap().children().to_vec() {
        assert_eq!(f.doc.get(face).unwrap().credit(), Some(&credit));
    }
}
