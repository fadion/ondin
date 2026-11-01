//! Blend modes through the scene walk and the CPU backend.

use ondin_core::kurbo::{Affine, BezPath, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BlendMode, Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};
use ondin_render::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};

struct Doc {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
}

/// A 100×100 frame on the first page, filled `ground` when given.
fn frame(ground: Option<[u8; 3]>) -> Doc {
    let mut ids = IdSource::new(5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    let page = doc.first_page();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: frame,
        parent: page,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    let mut d = Doc { doc, ids, frame };
    if let Some(rgb) = ground {
        fill(&mut d.doc, frame, rgb);
    }
    d
}

fn fill(doc: &mut Document, id: NodeId, [r, g, b]: [u8; 3]) {
    doc.apply(&Transaction(vec![Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(r, g, b, 255)).into(),
            visible: true,
        }]),
    }]))
    .unwrap();
}

/// A rect spanning `x0..x1` at full height, on top of `parent`'s children.
fn rect(d: &mut Doc, parent: NodeId, x0: f64, x1: f64, rgb: [u8; 3]) -> NodeId {
    let id = d.ids.mint();
    let index = d.doc.get(parent).unwrap().children().len();
    d.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(x1 - x0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((x0, 0.0))),
            name: None,
        }]))
        .unwrap();
    fill(&mut d.doc, id, rgb);
    id
}

fn group(d: &mut Doc, parent: NodeId) -> NodeId {
    let id = d.ids.mint();
    d.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }]))
        .unwrap();
    id
}

fn set(doc: &mut Document, id: NodeId, blend: BlendMode) {
    doc.apply(&Transaction(vec![Operation::SetBlend { id, blend }]))
        .unwrap();
}

/// The pixel at `x`, halfway down.
fn pixel(doc: &Document, x: usize) -> [u8; 4] {
    let res = Resolved::rebuild(doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (rgba, _, _) = VelloCpuRenderer::new().render_to_rgba(
        doc,
        &res,
        &vp,
        &ImageStore::new(),
        doc.first_page(),
    );
    let i = (50 * 100 + x) * 4;
    rgba[i..i + 4].try_into().unwrap()
}

fn near(got: [u8; 4], want: [u8; 3]) -> bool {
    got[..3]
        .iter()
        .zip(want)
        .all(|(g, w)| (*g as i32 - w as i32).abs() <= 2)
}

/// A Multiply rect over the frame's fill draws the channel-wise product.
/// flip: `blend_layer` answering `None` → fails at the product assertion.
#[test]
fn multiply_draws_the_product_of_the_two_colours() {
    let mut d = frame(Some([100, 150, 200]));
    let frame = d.frame;
    let top = rect(&mut d, frame, 0.0, 100.0, [200, 100, 50]);
    assert!(near(pixel(&d.doc, 50), [200, 100, 50]), "Normal covers");
    set(&mut d.doc, top, BlendMode::Multiply);
    let got = pixel(&d.doc, 50);
    assert!(near(got, [78, 59, 39]), "got {got:?}");
}

/// The board's case: a group holds a white rect and a Difference rect over it
/// that hangs off onto the frame's blue. Passing through, the overhang
/// differences against the blue; set to Normal, the group is drawn alone first
/// and the overhang has nothing beneath it, so it keeps its own colour.
/// flip: `BlendMode::isolates` answering `false` for Normal → fails at the
/// isolated overhang.
#[test]
fn pass_through_blends_with_the_frame_and_normal_isolates_the_group() {
    let mut d = frame(Some([0, 0, 255]));
    let frame = d.frame;
    let g = group(&mut d, frame);
    rect(&mut d, g, 0.0, 50.0, [255, 255, 255]);
    let disc = rect(&mut d, g, 25.0, 100.0, [240, 160, 80]);
    set(&mut d.doc, disc, BlendMode::Difference);

    let (on_card, overhang) = (40, 75);
    assert!(near(pixel(&d.doc, on_card), [15, 95, 175]));
    let got = pixel(&d.doc, overhang);
    assert!(near(got, [240, 160, 175]), "pass through: got {got:?}");

    set(&mut d.doc, g, BlendMode::Normal);
    assert!(near(pixel(&d.doc, on_card), [15, 95, 175]));
    let got = pixel(&d.doc, overhang);
    assert!(near(got, [240, 160, 80]), "isolated: got {got:?}");
}

/// Counts the blend layers the walk pushes, by mode, and checks the pops pair.
#[derive(Default)]
struct Blends {
    modes: Vec<BlendMode>,
    depth: i32,
}

impl ScenePainter for Blends {
    fn has_image(&self, _: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _: Affine,
        _: &BezPath,
        _: &Brush,
        _: Option<ondin_core::Framing>,
        _: ClipRule,
    ) {
    }
    fn stroke_path(&mut self, _: Affine, _: &BezPath, _: &StrokePaint<'_>) {}
    fn draw_text(&mut self, _: Affine, _: &TextRun<'_>) {}
    fn push_layer(&mut self, _: Affine, _: Option<&BezPath>, _: ClipRule, _: f32) {
        self.depth += 1;
    }
    fn push_mask_layer(&mut self) {
        self.depth += 1;
    }
    fn push_blend_layer(&mut self, mode: BlendMode) {
        self.modes.push(mode);
        self.depth += 1;
    }
    fn pop_layer(&mut self) {
        self.depth -= 1;
    }
}

fn blends(doc: &Document) -> Vec<BlendMode> {
    let res = Resolved::rebuild(doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let mut p = Blends::default();
    scene::build(
        doc,
        &res,
        &vp,
        &RenderOverrides::default(),
        doc.first_page(),
        &mut p,
    );
    assert_eq!(p.depth, 0, "every push popped");
    p.modes
}

/// The page and an unclipped top-level frame are isolated only once they hold a
/// blended layer, so the layer blends with the frame's content and never the
/// canvas ground; a nested frame passes through as a group does.
/// flip: dropping `holds_blend` from `blend_layer` → fails at the first
/// `[Normal, Normal, Multiply]`.
#[test]
fn a_top_level_frame_isolates_what_blends_inside_it() {
    let mut d = frame(None);
    let frame = d.frame;
    d.doc
        .apply(&Transaction(vec![Operation::SetClip {
            id: frame,
            clip: false,
        }]))
        .unwrap();
    let inner = d.ids.mint();
    d.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: inner,
            parent: frame,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(50.0, 50.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    d.doc
        .apply(&Transaction(vec![Operation::SetClip {
            id: inner,
            clip: false,
        }]))
        .unwrap();
    let r = rect(&mut d, inner, 0.0, 40.0, [10, 20, 30]);
    assert_eq!(blends(&d.doc), [], "nothing blends: no layer");
    set(&mut d.doc, r, BlendMode::Multiply);
    assert_eq!(
        blends(&d.doc),
        [BlendMode::Normal, BlendMode::Normal, BlendMode::Multiply],
        "the page and the top-level frame isolate; the nested one passes through"
    );
}

/// A blending shadow counts as a blend: the page and the top-level frame isolate
/// a child whose only blend is its Multiply drop shadow, and a loose blended
/// layer is isolated by the page alone.
/// flip: `blends_itself` not reading shadows → fails at "a blending shadow".
#[test]
fn a_blending_shadow_isolates_the_page_and_its_top_level_frame() {
    let mut d = frame(None);
    let frame = d.frame;
    d.doc
        .apply(&Transaction(vec![Operation::SetClip {
            id: frame,
            clip: false,
        }]))
        .unwrap();
    let card = rect(&mut d, frame, 20.0, 60.0, [242, 242, 245]);
    shadows(
        &mut d.doc,
        card,
        vec![hard(false, 20.0, [200, 40, 40], BlendMode::Multiply)],
    );
    assert_eq!(
        blends(&d.doc),
        [BlendMode::Normal, BlendMode::Normal, BlendMode::Multiply],
        "a blending shadow: the page, the frame, then the shadow's pass"
    );
    let page = d.doc.first_page();
    let loose = rect(&mut d, page, 70.0, 90.0, [240, 160, 80]);
    set(&mut d.doc, loose, BlendMode::Difference);
    shadows(&mut d.doc, card, Vec::new());
    assert_eq!(
        blends(&d.doc),
        [BlendMode::Normal, BlendMode::Difference],
        "a loose blended layer: the page alone"
    );
}

/// A hard shadow `dx` to the right in `rgb`, in `blend`.
fn hard(inner: bool, dx: f64, rgb: [u8; 3], blend: BlendMode) -> ondin_core::Effect {
    let shadow = ondin_core::Shadow {
        offset: ondin_core::kurbo::Vec2::new(dx, 0.0),
        blur: 0.0,
        spread: 0.0,
        color: Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255),
        blend,
    };
    ondin_core::Effect::new(match inner {
        true => ondin_core::EffectKind::InnerShadow(shadow),
        false => ondin_core::EffectKind::DropShadow(shadow),
    })
}

fn shadows(doc: &mut Document, id: NodeId, effects: Vec<ondin_core::Effect>) {
    doc.apply(&Transaction(vec![Operation::SetEffects {
        id,
        effects: keyed_by_position(effects),
    }]))
    .unwrap();
}

/// The board's case: a Multiply drop shadow deepens the ground it falls on
/// where a Normal one covers it — which takes the shadow out of the layer's
/// effect buffer, where there is no ground to multiply.
/// flip: `shadow_passes` answering `None` → fails at "multiplied".
#[test]
fn a_multiply_drop_shadow_deepens_the_ground() {
    let mut d = frame(Some([63, 154, 107]));
    let frame = d.frame;
    let card = rect(&mut d, frame, 20.0, 60.0, [242, 242, 245]);
    let red = [200, 40, 40];
    shadows(
        &mut d.doc,
        card,
        vec![hard(false, 20.0, red, BlendMode::Normal)],
    );
    assert!(near(pixel(&d.doc, 70), red), "Normal covers");
    shadows(
        &mut d.doc,
        card,
        vec![hard(false, 20.0, red, BlendMode::Multiply)],
    );
    let got = pixel(&d.doc, 70);
    assert!(near(got, [49, 24, 17]), "multiplied: got {got:?}");
    assert!(near(pixel(&d.doc, 40), [242, 242, 245]), "the card on top");
}

/// Two drop shadows stack as the one buffer stacks them, the later behind the
/// earlier: the Multiply red, listed first, multiplies the Normal blue under it.
/// flip: `shadow_passes` drawing drop shadows in list order → fails at "over
/// the blue".
#[test]
fn drop_shadows_keep_their_stacking_when_one_blends() {
    let mut d = frame(Some([63, 154, 107]));
    let frame = d.frame;
    let card = rect(&mut d, frame, 20.0, 60.0, [242, 242, 245]);
    shadows(
        &mut d.doc,
        card,
        vec![
            hard(false, 20.0, [200, 40, 40], BlendMode::Multiply),
            hard(false, 30.0, [0, 0, 200], BlendMode::Normal),
        ],
    );
    let got = pixel(&d.doc, 70);
    assert!(near(got, [0, 0, 31]), "over the blue: got {got:?}");
    assert!(near(pixel(&d.doc, 85), [0, 0, 200]), "the blue alone");
}

/// An inner shadow blends with the layer it is cast inside: Screen lightens a
/// dark card where Normal paints it red.
#[test]
fn an_inner_shadow_blends_with_its_layer() {
    let mut d = frame(Some([63, 154, 107]));
    let frame = d.frame;
    let card = rect(&mut d, frame, 20.0, 60.0, [40, 40, 40]);
    let red = [200, 40, 40];
    shadows(
        &mut d.doc,
        card,
        vec![hard(true, 10.0, red, BlendMode::Normal)],
    );
    assert!(near(pixel(&d.doc, 25), red), "Normal covers");
    shadows(
        &mut d.doc,
        card,
        vec![hard(true, 10.0, red, BlendMode::Screen)],
    );
    let got = pixel(&d.doc, 25);
    assert!(near(got, [209, 74, 74]), "screened: got {got:?}");
    assert!(
        near(pixel(&d.doc, 45), [40, 40, 40]),
        "the card past the band"
    );
}
