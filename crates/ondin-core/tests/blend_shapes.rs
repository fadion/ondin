//! *Blend shapes* (`blend.rs`): the steps between two shapes.
//!
//! The fixture is a circle and a rounded square on a frame translated by (50, 30),
//! so the world space the outlines are matched in is never the parent's.

use ondin_core::blend::{self, Blend, Refusal};
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Shape, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};

const BLUE: Color = Color::from_rgb8(0x60, 0x80, 0xd0);
const ORANGE: Color = Color::from_rgb8(0xd0, 0xa0, 0x50);

struct Fixture {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    circle: NodeId,
    square: NodeId,
}

fn filled(color: Color) -> Vec<ondin_core::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(color).into(),
        visible: true,
    }])
}

impl Fixture {
    /// A blue circle r 30 centred on (110, 260) under an orange 166-square at
    /// (530, 176) with corners of 16, the circle at half opacity.
    fn new() -> Self {
        let mut ids = IdSource::new(0xB1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, circle, square) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(800.0, 420.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((50.0, 30.0))),
                name: None,
            },
            Operation::CreateNode {
                id: circle,
                parent: frame,
                index: 0,
                kind: NodeKind::Ellipse {
                    size: Size::new(60.0, 60.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((80.0, 230.0))),
                name: Some("Circle".into()),
            },
            Operation::SetFills {
                id: circle,
                fills: filled(BLUE),
            },
            Operation::SetOpacity {
                id: circle,
                opacity: 0.5,
            },
            Operation::CreateNode {
                id: square,
                parent: frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(166.0, 166.0),
                    corner_radii: RoundedRectRadii::from_single_radius(16.0),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((530.0, 176.0))),
                name: Some("Square".into()),
            },
            Operation::SetFills {
                id: square,
                fills: filled(ORANGE),
            },
        ]))
        .expect("build the fixture");
        Self {
            doc,
            ids,
            frame,
            circle,
            square,
        }
    }

    fn blend(&mut self, b: Blend) -> NodeId {
        let res = Resolved::rebuild(&self.doc);
        let (tx, group) = blend::blend(
            &self.doc,
            &res,
            &mut self.ids,
            &[self.square, self.circle],
            b,
        )
        .expect("a blend");
        self.doc.apply(&tx).expect("the blend applies");
        group
    }
}

const FIVE: Blend = Blend {
    steps: 5,
    paint: true,
    start: Vec2::new(0.0, 0.0),
    reverse: false,
};

/// The steps between the two shapes, `steps` of them, inside the group.
fn steps_of(f: &Fixture, group: NodeId) -> Vec<NodeId> {
    let kids = f.doc.get(group).unwrap().children();
    kids[1..kids.len() - 1].to_vec()
}

/// A group named *Blend* takes the two shapes and five plain paths between them,
/// the circle lowest, the steps running up to the square on top, each shape
/// still drawn where it was.
/// flip: `blend` without its `Reparent`s → fails at "the group is the whole blend".
#[test]
fn the_group_holds_both_shapes_and_the_steps_between() {
    let mut f = Fixture::new();
    let before = Resolved::rebuild(&f.doc);
    let was = [f.circle, f.square].map(|id| before.world_transform(id).unwrap());
    let group = f.blend(FIVE);
    assert_eq!(
        f.doc.get(f.frame).unwrap().children(),
        &[group],
        "the group is the whole blend"
    );
    let g = f.doc.get(group).unwrap();
    assert_eq!(g.name(), "Blend");
    let kids = g.children();
    assert_eq!(kids.len(), 7);
    assert_eq!((kids[0], kids[6]), (f.circle, f.square));
    for (k, step) in steps_of(&f, group).iter().enumerate() {
        let n = f.doc.get(*step).unwrap();
        assert!(matches!(n.kind(), NodeKind::Path { .. }));
        assert_eq!(n.name(), format!("Step {}", k + 1));
    }
    let after = Resolved::rebuild(&f.doc);
    for (id, w) in [f.circle, f.square].into_iter().zip(was) {
        let now = after.world_transform(id).unwrap();
        assert!(
            now.as_coeffs()
                .iter()
                .zip(w.as_coeffs())
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "drawn where it was"
        );
    }
}

/// A shape from another parent, a moved group, comes into the blend's group and
/// is still drawn where it was.
/// flip: `blend` without its `SetTransform`s → fails at "drawn where it was".
#[test]
fn a_shape_from_another_parent_keeps_its_place() {
    let mut f = Fixture::new();
    let holder = f.ids.mint();
    let res = Resolved::rebuild(&f.doc);
    let circle_world = res.world_transform(f.circle).unwrap();
    let frame_world = res.world_transform(f.frame).unwrap();
    let holder_local = Affine::translate((20.0, 10.0));
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: holder,
                parent: f.frame,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(holder_local),
                name: None,
            },
            Operation::Reparent {
                id: f.circle,
                new_parent: holder,
                index: 0,
            },
            Operation::SetTransform {
                id: f.circle,
                transform: (frame_world * holder_local).inverse() * circle_world,
            },
        ]))
        .unwrap();
    f.blend(FIVE);
    let now = Resolved::rebuild(&f.doc).world_transform(f.circle).unwrap();
    assert!(
        now.as_coeffs()
            .iter()
            .zip(circle_world.as_coeffs())
            .all(|(a, b)| (a - b).abs() < 1e-9),
        "drawn where it was"
    );
}

/// Step k of n sits k / (n + 1) of the way between the two centres, and grows
/// from one size to the other: the middle step of five is half way.
#[test]
fn the_middle_step_is_half_way_in_place_size_and_paint() {
    let mut f = Fixture::new();
    let group = f.blend(FIVE);
    let res = Resolved::rebuild(&f.doc);
    let middle = steps_of(&f, group)[2];
    let world = res.world_transform(middle).unwrap() * res.local_path(&f.doc, middle).unwrap();
    let b = world.bounding_box();
    // Centres (160, 290) and (663, 289) in world space; sizes 60 and 166.
    let want = Point::new((160.0 + 663.0) / 2.0, (290.0 + 289.0) / 2.0);
    assert!(
        (b.center() - want).hypot() < 0.5,
        "{:?} vs {want:?}",
        b.center()
    );
    assert!((b.width() - 113.0).abs() < 0.5, "{}", b.width());

    let n = f.doc.get(middle).unwrap();
    let Some(Brush::Solid(c)) = n.paint().fills[0].brush.literal() else {
        panic!("a solid fill");
    };
    let half = |x: u8, y: u8| (f32::from(x) + f32::from(y)) / 2.0 / 255.0;
    let [r, g, bl, _] = c.components;
    assert!((r - half(0x60, 0xd0)).abs() < 1e-3);
    assert!((g - half(0x80, 0xa0)).abs() < 1e-3);
    assert!((bl - half(0xd0, 0x50)).abs() < 1e-3);
    assert!((n.opacity() - 0.75).abs() < 1e-6, "opacity mixed");
}

/// Off, every step wears the first shape's paint and opacity.
#[test]
fn without_paint_every_step_wears_the_first_shapes() {
    let mut f = Fixture::new();
    let group = f.blend(Blend {
        paint: false,
        ..FIVE
    });
    for step in steps_of(&f, group) {
        let n = f.doc.get(step).unwrap();
        assert_eq!(
            n.paint().fills[0].brush.literal(),
            Some(&Brush::Solid(BLUE))
        );
        assert_eq!(n.opacity(), 0.5);
    }
}

/// Matching from another corner starts the circle's outline elsewhere, so the
/// steps differ; the first step's first anchor sits by the chosen corner.
#[test]
fn match_points_from_moves_where_the_outlines_pair_up() {
    let first_anchor = |start: Vec2| {
        let mut f = Fixture::new();
        let group = f.blend(Blend { start, ..FIVE });
        let step = steps_of(&f, group)[0];
        let NodeKind::Path { path, .. } = f.doc.get(step).unwrap().kind() else {
            panic!("a path");
        };
        match path.elements()[0] {
            ondin_core::kurbo::PathEl::MoveTo(p) => p,
            _ => panic!("starts with a move"),
        }
    };
    let top_left = first_anchor(Vec2::new(0.0, 0.0));
    let bottom_right = first_anchor(Vec2::new(1.0, 1.0));
    assert!(bottom_right.x > top_left.x && bottom_right.y > top_left.y);
}

/// A locked shape, and two operands of a boolean, are refused: in the boolean the
/// group would be one more operand.
#[test]
fn locked_shapes_and_boolean_operands_are_refused() {
    let mut f = Fixture::new();
    f.doc
        .apply(&Transaction(vec![Operation::SetLocked {
            id: f.square,
            locked: true,
        }]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Locked)
    );

    let mut f = Fixture::new();
    let boolean = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: boolean,
                parent: f.frame,
                index: 2,
                kind: NodeKind::Boolean {
                    op: ondin_core::node::BoolOp::Union,
                },
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: f.circle,
                new_parent: boolean,
                index: 0,
            },
            Operation::Reparent {
                id: f.square,
                new_parent: boolean,
                index: 1,
            },
        ]))
        .unwrap();
    assert_eq!(
        blend::pair(&f.doc, &[f.circle, f.square]),
        Err(Refusal::Inside)
    );
}

/// Text, pictures and any count but two are refused by name.
#[test]
fn text_pictures_and_other_counts_are_refused() {
    let mut f = Fixture::new();
    assert_eq!(blend::pair(&f.doc, &[f.circle]), Err(Refusal::Count));
    let text = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: text,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Text {
                content: "Hi".into(),
                style: Default::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    assert_eq!(blend::pair(&f.doc, &[f.circle, text]), Err(Refusal::Text));
    assert_eq!(Refusal::Text.says(), "Blend shapes can't take text");
}
