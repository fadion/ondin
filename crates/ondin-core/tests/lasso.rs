//! `query::touches_loop`, the lasso's pick: a layer is taken when its shape, not its
//! box, touches the loop's area.

use ondin_core::kurbo::{Affine, Point, Size};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction, touches_loop,
};

/// A page holding a diagonal line from (40, 30) to (260, 210), a 50×50 square at
/// (170, 56), and a group of one 20×20 rect at (400, 400).
fn fixture() -> (Document, NodeId, NodeId, NodeId, NodeId) {
    let mut ids = IdSource::new(0x1A55);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (line, square, group, inner) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let rect = |size: f64| NodeKind::Rect {
        size: Size::new(size, size),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: line,
            parent: page,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(220.0, 180.0),
            },
            transform: Some(Affine::translate((40.0, 30.0))),
            name: None,
        },
        Operation::CreateNode {
            id: square,
            parent: page,
            index: 1,
            kind: rect(50.0),
            transform: Some(Affine::translate((170.0, 56.0))),
            name: None,
        },
        Operation::CreateNode {
            id: group,
            parent: page,
            index: 2,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: inner,
            parent: group,
            index: 0,
            kind: rect(20.0),
            transform: Some(Affine::translate((400.0, 400.0))),
            name: None,
        },
    ]))
    .expect("the fixture");
    (doc, line, square, group, inner)
}

/// A square loop centred on `c`, `r` from it each way.
fn around(c: (f64, f64), r: f64) -> Vec<Point> {
    vec![
        Point::new(c.0 - r, c.1 - r),
        Point::new(c.0 + r, c.1 - r),
        Point::new(c.0 + r, c.1 + r),
        Point::new(c.0 - r, c.1 + r),
    ]
}

/// A loop inside the line's box that never reaches the line takes the square and
/// not the line; one across the line takes it.
/// flip: `touches_within` answering by world bounds alone → fails at "not its box".
#[test]
fn the_shape_not_the_box_is_what_the_loop_touches() {
    let (doc, line, square, ..) = fixture();
    let res = Resolved::rebuild(&doc);
    let hits = |lasso: &[Point], id| touches_loop(&res, &doc, id, lasso, 0.1);

    let off_the_line = around((200.0, 80.0), 40.0);
    assert!(hits(&off_the_line, square));
    assert!(!hits(&off_the_line, line), "not its box");

    let across = around((150.0, 120.0), 10.0);
    assert!(hits(&across, line), "the line runs through it");
    assert!(!hits(&across, square));
}

/// A loop wholly inside a shape touches it, though no outline crosses; a group is
/// touched through its child.
/// flip: `touches_within` without its `hits_own_shape` test → fails at "inside".
#[test]
fn a_loop_inside_a_shape_and_a_group_through_its_child() {
    let (doc, _, square, group, inner) = fixture();
    let res = Resolved::rebuild(&doc);
    let inside = around((195.0, 81.0), 5.0);
    assert!(touches_loop(&res, &doc, square, &inside, 0.1), "inside");

    let over_child = around((410.0, 410.0), 20.0);
    assert!(touches_loop(&res, &doc, group, &over_child, 0.1));
    assert!(touches_loop(&res, &doc, inner, &over_child, 0.1));
    assert!(!touches_loop(
        &res,
        &doc,
        group,
        &around((480.0, 480.0), 20.0),
        0.1
    ));
}

/// A figure-eight's lobes wind opposite ways and both count, so a shape wholly in
/// one lobe is inside it whichever way the loop was drawn; two points have no area.
/// flip: `inside_loop` reading `w > 0` → fails at the first orientation, whose
/// right lobe winds negative.
#[test]
fn a_figure_eight_covers_both_lobes() {
    let (doc, _, square, ..) = fixture();
    let res = Resolved::rebuild(&doc);
    // Crosses itself at (100, 81) at 45°, so the right lobe spans y 11..151 at
    // x 170 and wider beyond: the square (170..220, 56..106) lies wholly in it.
    let mut eight = vec![
        Point::new(-200.0, -219.0),
        Point::new(400.0, 381.0),
        Point::new(400.0, -219.0),
        Point::new(-200.0, 381.0),
    ];
    assert!(touches_loop(&res, &doc, square, &eight, 0.1));
    eight.reverse();
    assert!(
        touches_loop(&res, &doc, square, &eight, 0.1),
        "drawn backwards"
    );
    assert!(!touches_loop(&res, &doc, square, &eight[..2], 0.1));
}

/// A horizontal line, whose box has no height, is lassoed across its middle; a
/// group is not taken through a hidden child; a turned square whose box the loop
/// enters but whose shape it misses is not taken.
/// flip: `touches_within`'s `near` test through `intersect(..).area() > 0.0` →
/// fails at "a flat line".
#[test]
fn flat_lines_hidden_children_and_turned_shapes() {
    let (mut doc, _, _, group, inner) = fixture();
    let mut ids = IdSource::new(0x1A56);
    let page = doc.first_page();
    let (flat, turned) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: flat,
            parent: page,
            index: 3,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: Some(Affine::translate((40.0, 300.0))),
            name: None,
        },
        // A 100×100 square turned 45° about its corner at (600, 100): a diamond whose
        // box runs x 529–671, y 100–241.
        Operation::CreateNode {
            id: turned,
            parent: page,
            index: 4,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(
                Affine::translate((600.0, 100.0)) * Affine::rotate(std::f64::consts::FRAC_PI_4),
            ),
            name: None,
        },
        Operation::SetVisible {
            id: inner,
            visible: false,
        },
    ]))
    .expect("the extras");
    let res = Resolved::rebuild(&doc);
    assert!(
        touches_loop(&res, &doc, flat, &around((90.0, 300.0), 10.0), 0.1),
        "a flat line"
    );
    assert!(
        !touches_loop(&res, &doc, group, &around((410.0, 410.0), 20.0), 0.1),
        "not through a hidden child"
    );
    // The box's top-left corner region, outside the diamond.
    assert!(
        !touches_loop(&res, &doc, turned, &around((540.0, 112.0), 6.0), 0.1),
        "the turned shape, not its box"
    );
    assert!(touches_loop(
        &res,
        &doc,
        turned,
        &around((600.0, 170.0), 6.0),
        0.1
    ));
}
