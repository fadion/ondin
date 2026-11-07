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
