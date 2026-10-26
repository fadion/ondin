//! A gradient drawn on a box other than the one it is stored against: the scene
//! and the SVG export both map it (`paint_box::drawn_paint`, `paint_box::node_paint`).

use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::peniko::{Color, Gradient};
use ondin_core::{
    Brush, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, Operation, Resolved,
    Transaction, VarKind, Variable, keyed_by_position,
};
use ondin_render::{VelloCpuRenderer, Viewport};

/// A 200 × 100 Dark frame holding a rect whose width is bound to a variable
/// reading 50 in Light and 100 in Dark, filled black to white across Light's 50.
fn stretched() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x9B);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (board, rect) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let w = ids.mint_var();
    let ramp = Gradient::new_linear((0.0, 0.0), (50.0, 0.0))
        .with_stops([Color::BLACK, Color::WHITE].as_slice());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: board,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(ramp.into()).into(),
                visible: true,
            }]),
        },
        Operation::AddMode {
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: w,
                name: "w".into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(50.0)), (dark, Cell::Number(100.0))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::SetMode {
            id: board,
            mode: Some(dark),
        },
    ]))
    .expect("the fixture");
    doc.apply(&Transaction(ondin_core::bind_size_ops(
        &doc,
        rect,
        [Some(w), None],
    )))
    .expect("the bound width");
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(rect).map(|r| r.width()),
        Some(100.0),
        "fixture: drawn at Dark's 100"
    );
    (doc, res, board)
}

/// **The scene draws the gradient across the box it is drawn at**: three
/// quarters along Dark's 100 is three quarters up the ramp, not past its end.
///
/// flip: `paint_node` handing the paint to the scene without `drawn_paint` →
/// fails at the sample (white, the ramp's pad).
#[test]
fn the_scene_draws_a_gradient_across_its_drawn_box() {
    let (doc, res, _) = stretched();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 200.0, 100.0),
        pixel_size: (200, 100),
    };
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &vp,
        &ondin_render::ImageStore::new(),
        doc.first_page(),
    );
    let px = rgba[(50 * 200 + 75) * 4] as i32;
    assert!((150..235).contains(&px), "three quarters up the ramp: {px}");
}

/// **The SVG export writes the gradient mapped onto the drawn box**, as the
/// canvas draws it.
///
/// flip: `node_paint` handing `drawn_paint` the stored literal's box → fails at
/// the transform (no `gradientTransform` at all).
#[test]
fn the_svg_export_maps_a_gradient_onto_its_drawn_box() {
    let (doc, res, board) = stretched();
    let out = ondin_export::svg::svg(&doc, &res, doc.first_page(), Some(board));
    assert!(
        out.contains(r#"gradientTransform="matrix(2 0 0 1 0 0)""#),
        "{out}"
    );
}
