//! M4 scene-building + culling verification (no GPU required).
//!
//! The shared `scene::build` walk is exercised with a recording painter, so the
//! culling logic used by BOTH backends is validated on CPU. On-device
//! rasterization correctness (color, geometry) is covered by the CPU pixel tests
//! in `ondin-export`, since both backends share this exact scene walk.

use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BoolOp, Document, Fill, IdSource, MaskMode, NodeId, NodeKind, Operation, Resolved, StrokeAlign,
    Transaction,
};
use ondin_render::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use ondin_render::{RenderOverrides, Viewport};

#[derive(Default)]
struct RecordingPainter {
    fills: Vec<Affine>,
    strokes: usize,
    text_glyphs: usize,
    /// Layers pushed, and whether each carried a clip path — enough to assert
    /// that artboards clip and that transparent nodes get their own layer.
    layers: Vec<(bool, f32)>,
    /// `push_mask_layer` calls — an alpha mask's composite, which is a different
    /// kind of layer from anything in `layers` above.
    mask_layers: usize,
    depth: i32,
    max_depth: i32,
}

impl ScenePainter for RecordingPainter {
    // Every picture is available to a recorder: these observe what the walk
    // *emits*, and answering `false` would have the walk emit the missing-image
    // placeholder instead (§15 D179) — a different question from the one each of
    // these tests asks.
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        transform: Affine,
        _path: &ondin_core::kurbo::BezPath,
        _brush: &Brush,
        _framing: Option<ondin_core::Framing>,
        _rule: ondin_render::scene::ClipRule,
    ) {
        self.fills.push(transform);
    }
    fn stroke_path(
        &mut self,
        _transform: Affine,
        _path: &ondin_core::kurbo::BezPath,
        _stroke: &StrokePaint<'_>,
    ) {
        self.strokes += 1;
    }
    fn draw_text(&mut self, _transform: Affine, run: &TextRun<'_>) {
        self.text_glyphs += run.glyphs.len();
    }
    fn push_layer(
        &mut self,
        _transform: Affine,
        clip: Option<&ondin_core::kurbo::BezPath>,
        _rule: ClipRule,
        opacity: f32,
    ) {
        self.layers.push((clip.is_some(), opacity));
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
    }
    fn push_mask_layer(&mut self) {
        // Counted apart from `layers`, because it is not one: an alpha mask's
        // composite carries no clip and no opacity, so it would be
        // indistinguishable from the isolation layer that encloses it.
        self.mask_layers += 1;
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
    }
    fn pop_layer(&mut self) {
        self.depth -= 1;
    }
}

/// Artboard with two rects: one inside the viewport, one far outside.
fn two_rect_doc() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let near = ids.mint();
    let far = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(2000.0, 2000.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: near,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
        Operation::CreateNode {
            id: far,
            parent: ab,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((1500.0, 1500.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: near,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: far,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
                visible: true,
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, near, far)
}

#[test]
fn culling_excludes_out_of_view_nodes() {
    let (doc, res, _near, _far) = two_rect_doc();
    // Viewport covers only the top-left corner (the near rect), not the far one.
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);

    // Only the near rect should have been painted (artboard has no background).
    assert_eq!(painter.fills.len(), 1, "exactly the in-view rect is drawn");
}

#[test]
fn full_view_draws_all_shapes() {
    let (doc, res, _near, _far) = two_rect_doc();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);
    assert_eq!(painter.fills.len(), 2, "both rects drawn when both in view");
}

#[test]
fn gpu_scene_builds_without_device() {
    // Building a vello::Scene needs no GPU; this exercises the GPU painter path.
    let (doc, res, _n, _f) = two_rect_doc();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (256, 256),
    };
    let _scene = ondin_render::gpu::build_scene(
        &doc,
        &res,
        &vp,
        &RenderOverrides::default(),
        &ondin_render::ImageStore::new(),
    );
    // No panic == pass; the scene is populated via the shared walk.
}

#[test]
fn artboards_push_a_clip_layer_so_content_cannot_overflow() {
    let (doc, res, _n, _f) = two_rect_doc();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);

    assert_eq!(painter.layers.len(), 1, "one layer, for the artboard");
    assert!(painter.layers[0].0, "the artboard layer must carry a clip");
    assert_eq!(painter.depth, 0, "every pushed layer was popped");
}

/// "Clip content", off. The frame stops pushing a clip layer — and stops being
/// a culling boundary with it, because its own bounds no longer say anything
/// about where its children can paint. A child outside the frame but inside the
/// viewport must still be drawn.
#[test]
fn a_frame_with_clipping_off_neither_clips_nor_culls_its_contents() {
    let (mut doc, _res, _near, far) = two_rect_doc();
    let ab = doc.get(doc.root()).unwrap().children()[0];
    // Put the second rect well outside the 2000×2000 frame.
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: far,
        transform: Affine::translate((2600.0, 2600.0)),
    }]))
    .unwrap();

    let vp = Viewport {
        // Wide enough to hold both the frame and the escapee.
        view: Rect::new(0.0, 0.0, 3000.0, 3000.0),
        pixel_size: (400, 400),
    };
    let count = |doc: &Document| {
        let res = Resolved::rebuild(doc);
        let mut painter = RecordingPainter::default();
        scene::build(doc, &res, &vp, &RenderOverrides::default(), &mut painter);
        assert_eq!(painter.depth, 0, "every pushed layer was popped");
        (painter.layers.clone(), painter.fills.len())
    };

    let (layers, fills) = count(&doc);
    assert_eq!(layers.len(), 1, "clipping on: the frame's own layer");
    assert!(layers[0].0, "clipping on: that layer carries a clip");
    assert_eq!(fills, 2, "both rects are still walked");

    doc.apply(&Transaction(vec![Operation::SetClip {
        id: ab,
        clip: false,
    }]))
    .unwrap();
    let (layers, fills) = count(&doc);
    assert!(
        layers.is_empty(),
        "clipping off: no layer at all, got {layers:?}"
    );
    assert_eq!(fills, 2, "the escaped child must still be drawn");
}

#[test]
fn transparent_nodes_get_their_own_layer_rather_than_faded_paint() {
    // Folding opacity into each child's alpha is wrong for overlapping content;
    // a real layer composites the subtree as a unit.
    let (mut doc, _res, near, _far) = two_rect_doc();
    doc.apply(&Transaction(vec![Operation::SetOpacity {
        id: near,
        opacity: 0.5,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);

    assert!(
        painter.layers.iter().any(|(clip, a)| !clip && *a == 0.5),
        "expected an un-clipped 0.5 layer, got {:?}",
        painter.layers
    );
    assert!(
        painter.max_depth >= 2,
        "the layer nests inside the artboard"
    );
    assert_eq!(painter.depth, 0);
}

#[test]
fn fully_opaque_unclipped_nodes_cost_no_layer() {
    let (doc, res, _n, _f) = two_rect_doc();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);
    // Two rects, both opaque: only the artboard's clip layer exists.
    assert_eq!(painter.layers.len(), 1);
}

#[test]
fn gradient_fills_reach_the_backend_intact() {
    use ondin_core::kurbo::Point;
    use ondin_core::peniko::Gradient;

    struct BrushRecorder(Vec<Brush>);
    impl ScenePainter for BrushRecorder {
        // Every picture is available to a recorder: these observe what the walk
        // *emits*, and answering `false` would have the walk emit the missing-image
        // placeholder instead (§15 D179) — a different question from the one each of
        // these tests asks.
        fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
            true
        }
        fn fill_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            brush: &Brush,
            _framing: Option<ondin_core::Framing>,
            _rule: ondin_render::scene::ClipRule,
        ) {
            self.0.push(brush.clone());
        }
        fn stroke_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            _s: &StrokePaint<'_>,
        ) {
        }
        fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
        fn push_layer(
            &mut self,
            _t: Affine,
            _c: Option<&ondin_core::kurbo::BezPath>,
            _r: ClipRule,
            _o: f32,
        ) {
        }
        fn push_mask_layer(&mut self) {}
        fn pop_layer(&mut self) {}
    }

    let (mut doc, _res, near, _far) = two_rect_doc();
    let gradient = Gradient::new_linear(Point::new(0.0, 0.0), Point::new(20.0, 20.0)).with_stops([
        (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
        (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
    ]);
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: near,
        fills: vec![Fill {
            brush: Brush::Gradient(gradient.into()),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = BrushRecorder(Vec::new());
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);

    // The gradient must survive as a gradient — collapsing it to a colour (or
    // dropping the fill) is what made gradients invisible before.
    let gradients = painter
        .0
        .iter()
        .filter(|b| matches!(b, Brush::Gradient(_)))
        .count();
    assert_eq!(gradients, 1, "got {:?}", painter.0);
}

/// **The walk frames an image against the shape's own box, and hands the same
/// framing to the shape's stroke.**
///
/// Two claims the pixel tests in `ondin-export` cannot make, because they read
/// the finished raster and this is about what the walk *decides*:
///
/// - a fill gets a framing and a solid does not, and
/// - the frame is the **shape's** box rather than the ink's. A ten-wide centred
///   stroke reaches five units either side, so an ink-box frame would be 30×30
///   where the shape is 20×20 — and the picture behind the border would sit at a
///   different scale from the picture inside it, drifting further apart every
///   time the stroke was widened. The two framings being *equal* is the
///   assertion; asserting only that the stroke has one would pass either way.
#[test]
fn an_image_is_framed_by_its_shape_and_its_stroke_agrees() {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};

    #[derive(Default)]
    struct FramingRecorder {
        fills: Vec<(Brush, Option<ondin_core::Framing>)>,
        strokes: Vec<Option<ondin_core::Framing>>,
    }
    impl ScenePainter for FramingRecorder {
        // Every picture is available to a recorder: these observe what the walk
        // *emits*, and answering `false` would have the walk emit the missing-image
        // placeholder instead (§15 D179) — a different question from the one each of
        // these tests asks.
        fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
            true
        }
        fn fill_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            brush: &Brush,
            framing: Option<ondin_core::Framing>,
            _rule: ondin_render::scene::ClipRule,
        ) {
            self.fills.push((brush.clone(), framing));
        }
        fn stroke_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            s: &StrokePaint<'_>,
        ) {
            self.strokes.push(s.framing);
        }
        fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
        fn push_layer(
            &mut self,
            _t: Affine,
            _c: Option<&ondin_core::kurbo::BezPath>,
            _r: ClipRule,
            _o: f32,
        ) {
        }
        fn push_mask_layer(&mut self) {}
        fn pop_layer(&mut self) {}
    }

    let (mut doc, _res, near, _far) = two_rect_doc();
    let id = ImageId("sha256:framed".into());
    doc.apply(&Transaction(vec![
        Operation::AddImage {
            id: id.clone(),
            entry: ImageEntry {
                // The bytes never decode here — the walk reads the intrinsic
                // size out of the table, which is the point of storing it.
                source: ImageSource::Embedded(vec![0].into()),
                format: ImageFormat::Png,
                width: 10,
                height: 20,
            },
        },
        Operation::SetFills {
            id: near,
            fills: vec![Fill {
                brush: image_brush(id.clone()),
                visible: true,
            }],
        },
        Operation::SetStrokes {
            id: near,
            strokes: vec![ondin_core::Stroke {
                brush: image_brush(id.clone()),
                width: 10.0,
                align: ondin_core::StrokeAlign::Center,
                ..Default::default()
            }],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    };
    let mut painter = FramingRecorder::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);

    let (image_fills, solid_fills): (Vec<_>, Vec<_>) = painter
        .fills
        .iter()
        .partition(|(b, _)| matches!(b, Brush::Image(_)));
    assert_eq!(image_fills.len(), 1, "the near rect's one image fill");
    assert!(
        solid_fills.iter().all(|(_, f)| f.is_none()),
        "a solid is already in the path's space and must carry no framing"
    );

    let framing = image_fills[0].1.expect("an image fill is framed");
    // Cover a 20×20 box with a 10×20 source: scale 2, so the picture is 20 wide
    // and 40 tall, centred — ten units hanging off the top and the bottom.
    let corners = (
        framing.transform * ondin_core::kurbo::Point::ZERO,
        framing.transform * ondin_core::kurbo::Point::new(10.0, 20.0),
    );
    assert_eq!(
        corners,
        (
            ondin_core::kurbo::Point::new(0.0, -10.0),
            ondin_core::kurbo::Point::new(20.0, 30.0)
        ),
        "the frame is the shape's local 20×20 box"
    );

    assert_eq!(painter.strokes.len(), 1, "one centred stroke");
    assert_eq!(
        painter.strokes[0],
        Some(framing),
        "the stroke frames the picture exactly as the fill does"
    );
}

// --- stroke alignment -----------------------------------------------------

/// Records exactly what the alignment construction is: the stroke widths issued
/// and the clip rules pushed.
#[derive(Default)]
struct StrokeRecorder {
    widths: Vec<f64>,
    clips: Vec<ClipRule>,
    depth_at_stroke: Vec<i32>,
    depth: i32,
}

impl ScenePainter for StrokeRecorder {
    // Every picture is available to a recorder: these observe what the walk
    // *emits*, and answering `false` would have the walk emit the missing-image
    // placeholder instead (§15 D179) — a different question from the one each of
    // these tests asks.
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _t: Affine,
        _p: &ondin_core::kurbo::BezPath,
        _b: &Brush,
        _f: Option<ondin_core::Framing>,
        _rule: ondin_render::scene::ClipRule,
    ) {
    }
    fn stroke_path(&mut self, _t: Affine, _p: &ondin_core::kurbo::BezPath, s: &StrokePaint<'_>) {
        self.widths.push(s.width);
        self.depth_at_stroke.push(self.depth);
    }
    fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _t: Affine,
        _c: Option<&ondin_core::kurbo::BezPath>,
        rule: ClipRule,
        _o: f32,
    ) {
        self.clips.push(rule);
        self.depth += 1;
    }
    fn push_mask_layer(&mut self) {
        self.depth += 1;
    }
    fn pop_layer(&mut self) {
        self.depth -= 1;
    }
}

/// A lone rect (no artboard, so no clip layer of its own) with a 4-wide stroke.
fn bare_stroked_rect(align: StrokeAlign, closed: bool) -> (Document, Resolved) {
    let mut ids = IdSource::new(77);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let shape = ids.mint();
    let kind = if closed {
        NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
        }
    } else {
        NodeKind::Line {
            end: ondin_core::kurbo::Point::new(40.0, 0.0),
        }
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: shape,
            parent: ab,
            index: 0,
            kind,
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: shape,
        strokes: vec![ondin_core::Stroke {
            brush: Brush::Solid(Color::BLACK),
            width: 4.0,
            join: ondin_core::kurbo::Join::Miter,
            cap: ondin_core::kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            sides: Default::default(),
            miter_limit: 4.0,
            dash_fit: false,
            align,
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn record_strokes(doc: &Document, res: &Resolved) -> StrokeRecorder {
    let mut r = StrokeRecorder::default();
    scene::build(
        doc,
        res,
        &Viewport {
            view: ondin_core::kurbo::Rect::new(-100.0, -100.0, 200.0, 200.0),
            pixel_size: (300, 300),
        },
        &RenderOverrides::default(),
        &mut r,
    );
    r
}

/// A centred stroke must stay a plain single stroke at its authored width — the
/// overwhelmingly common case pays nothing for a feature it does not use.
#[test]
fn a_centred_stroke_needs_no_extra_layer() {
    let (doc, res) = bare_stroked_rect(StrokeAlign::Center, true);
    let r = record_strokes(&doc, &res);
    assert_eq!(r.widths, vec![4.0]);
    // Only the artboard's own clip layer.
    assert_eq!(r.clips.len(), 1);
    assert_eq!(r.depth_at_stroke, vec![1]);
}

/// Inside and outside both double the width and add a clip layer; they differ
/// only in the rule, because "outside" is a complement.
#[test]
fn aligned_strokes_double_the_width_inside_a_clip() {
    for (align, rule) in [
        (StrokeAlign::Inside, ClipRule::NonZero),
        (StrokeAlign::Outside, ClipRule::EvenOdd),
    ] {
        let (doc, res) = bare_stroked_rect(align, true);
        let r = record_strokes(&doc, &res);
        assert_eq!(r.widths, vec![8.0], "{align:?}: width not doubled");
        assert_eq!(
            r.clips,
            vec![ClipRule::NonZero, rule],
            "{align:?}: artboard clip, then the stroke's"
        );
        assert_eq!(
            r.depth_at_stroke,
            vec![2],
            "{align:?}: stroke must be drawn inside its clip"
        );
        assert_eq!(r.depth, 0, "{align:?}: every layer popped");
    }
}

/// An open shape has no interior, so the construction must not fire — it would
/// clip half the stroke away for no reason.
#[test]
fn an_open_shape_strokes_centred_whatever_its_alignment_says() {
    let (doc, res) = bare_stroked_rect(StrokeAlign::Outside, false);
    let r = record_strokes(&doc, &res);
    assert_eq!(r.widths, vec![4.0], "width must not be doubled");
    assert_eq!(r.clips.len(), 1, "no clip beyond the artboard's");
}

/// A recorder that keeps the *order* of the paints reaching the backend, and
/// which brush each was, so a stacking test can say more than "two of them".
#[derive(Default)]
struct PaintOrder {
    /// One entry per paint, in the order it was drawn: `("fill"|"stroke", red)`.
    /// The red channel identifies the paint, which is enough to tell them apart
    /// and reads better in a failure than a whole `Color`.
    seen: Vec<(&'static str, u8)>,
}

fn red_of(brush: &Brush) -> u8 {
    match brush {
        Brush::Solid(c) => c.to_rgba8().r,
        _ => 0,
    }
}

impl ScenePainter for PaintOrder {
    // Every picture is available to a recorder: these observe what the walk
    // *emits*, and answering `false` would have the walk emit the missing-image
    // placeholder instead (§15 D179) — a different question from the one each of
    // these tests asks.
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _t: Affine,
        _p: &ondin_core::kurbo::BezPath,
        brush: &Brush,
        _f: Option<ondin_core::Framing>,
        _rule: ondin_render::scene::ClipRule,
    ) {
        self.seen.push(("fill", red_of(brush)));
    }
    fn stroke_path(&mut self, _t: Affine, _p: &ondin_core::kurbo::BezPath, s: &StrokePaint<'_>) {
        self.seen.push(("stroke", red_of(s.brush)));
    }
    fn draw_text(&mut self, _t: Affine, r: &TextRun<'_>) {
        self.seen.push(("text", red_of(r.brush)));
    }
    fn push_layer(
        &mut self,
        _t: Affine,
        _c: Option<&ondin_core::kurbo::BezPath>,
        _rule: ClipRule,
        _o: f32,
    ) {
    }
    fn push_mask_layer(&mut self) {}
    fn pop_layer(&mut self) {}
}

fn solid(red: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(red, 0, 0, 255))
}

fn fill(red: u8, visible: bool) -> Fill {
    Fill {
        brush: solid(red),
        visible,
    }
}

fn stroke(red: u8, visible: bool) -> ondin_core::Stroke {
    ondin_core::Stroke {
        brush: solid(red),
        width: 2.0,
        join: ondin_core::kurbo::Join::Miter,
        cap: ondin_core::kurbo::Cap::Butt,
        dashes: Vec::new(),
        dash_offset: 0.0,
        sides: Default::default(),
        miter_limit: 4.0,
        dash_fit: false,
        align: StrokeAlign::Center,
        visible,
    }
}

/// A lone rect carrying whatever paint lists the caller hands it.
fn painted_rect(fills: Vec<Fill>, strokes: Vec<ondin_core::Stroke>) -> (Document, Resolved) {
    let mut ids = IdSource::new(0xBEEF);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let shape = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: shape,
        parent: root,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills { id: shape, fills },
        Operation::SetStrokes { id: shape, strokes },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn record_order(doc: &Document, res: &Resolved) -> Vec<(&'static str, u8)> {
    let mut r = PaintOrder::default();
    scene::build(
        doc,
        res,
        &Viewport {
            view: Rect::new(-100.0, -100.0, 200.0, 200.0),
            pixel_size: (300, 300),
        },
        &RenderOverrides::default(),
        &mut r,
    );
    r.seen
}

/// **Every visible fill, then every visible stroke, in list order.** The Fill and
/// Stroke panels have always been able to add a second paint; until this the walk
/// took `.find(|p| p.visible)` and drew only the first, so the `+` added ink that
/// was listed, saved, and never painted — and, for a stroke, still enlarged the
/// selection box and the export viewBox on the way out.
#[test]
fn all_visible_paints_are_drawn_fills_first_then_strokes() {
    let (doc, res) = painted_rect(
        vec![fill(1, true), fill(2, true)],
        vec![stroke(3, true), stroke(4, true)],
    );
    assert_eq!(
        record_order(&doc, &res),
        vec![("fill", 1), ("fill", 2), ("stroke", 3), ("stroke", 4)]
    );
}

/// Visibility is `Fill::visible` / `Stroke::visible`, never an alpha of zero — a
/// 0-alpha paint is still visible and still expands the bounds. A hidden paint is
/// skipped without renumbering the ones around it.
#[test]
fn a_hidden_paint_is_skipped_and_the_rest_keep_their_order() {
    let (doc, res) = painted_rect(
        vec![fill(1, true), fill(2, false), fill(3, true)],
        vec![stroke(4, false), stroke(5, true)],
    );
    assert_eq!(
        record_order(&doc, &res),
        vec![("fill", 1), ("fill", 3), ("stroke", 5)]
    );
}

/// A line has no interior, so its fills are not drawn however many it has — but
/// its strokes still all are.
#[test]
fn a_line_stacks_its_strokes_and_ignores_its_fills() {
    let mut ids = IdSource::new(0xF00D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let line = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: line,
        parent: root,
        index: 0,
        kind: NodeKind::Line {
            end: ondin_core::kurbo::Point::new(40.0, 0.0),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: line,
            fills: vec![fill(9, true)],
        },
        Operation::SetStrokes {
            id: line,
            strokes: vec![stroke(1, true), stroke(2, true)],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(record_order(&doc, &res), vec![("stroke", 1), ("stroke", 2)]);
}

// --- one clip per run of same-aligned strokes ------------------------------

/// Two outside strokes on one shape. **One clip, not two** — a clip is a geometric
/// mask, so applying the same one twice is applying it once, and the complement path
/// it is built from used to be rebuilt per stroke per frame.
///
/// The widths are the assertion that both strokes still draw, and that both are still
/// doubled: sharing the layer must not collapse the paint, only the mask.
#[test]
fn two_outside_strokes_share_one_clip_instead_of_pushing_one_each() {
    let aligned = |align: StrokeAlign, red: u8, width: f64| ondin_core::Stroke {
        align,
        width,
        ..stroke(red, true)
    };
    let (doc, res) = painted_rect(
        vec![fill(1, true)],
        vec![
            aligned(StrokeAlign::Outside, 2, 3.0),
            aligned(StrokeAlign::Outside, 3, 5.0),
        ],
    );
    let r = record_strokes(&doc, &res);
    assert_eq!(
        r.clips,
        vec![ClipRule::EvenOdd],
        "one even-odd clip for the run of two, where each stroke used to push its own"
    );
    assert_eq!(r.widths, vec![6.0, 10.0], "both strokes still doubled");
    assert_eq!(
        r.depth_at_stroke,
        vec![1, 1],
        "and both drawn inside that one clip"
    );
    assert_eq!(r.depth, 0);
}

/// **Runs, not a sort.** The list order is the z-order, so a centred stroke between
/// two outside ones breaks the run: grouping across it would move paint. Two clips
/// here is the correct answer and three was the old one.
#[test]
fn a_differently_aligned_stroke_breaks_the_run_rather_than_being_reordered() {
    let aligned = |align: StrokeAlign, red: u8| ondin_core::Stroke {
        align,
        ..stroke(red, true)
    };
    let (doc, res) = painted_rect(
        vec![],
        vec![
            aligned(StrokeAlign::Outside, 1),
            aligned(StrokeAlign::Center, 2),
            aligned(StrokeAlign::Outside, 3),
        ],
    );
    let r = record_strokes(&doc, &res);
    assert_eq!(
        r.clips,
        vec![ClipRule::EvenOdd, ClipRule::EvenOdd],
        "one clip per run, and the centred stroke is a break"
    );
    assert_eq!(
        r.widths,
        vec![4.0, 2.0, 4.0],
        "doubled, plain, doubled — in list order"
    );
    assert_eq!(
        r.depth_at_stroke,
        vec![1, 0, 1],
        "the centred one is outside any clip; the other two are each inside one"
    );
}

/// An aligned **per-side** stroke is four paths under one clip, not four clips: the
/// sides are one paint. Already true of the SVG writer, and now of the walk.
#[test]
fn the_four_sides_of_one_aligned_stroke_share_that_strokes_clip() {
    let (doc, res) = painted_rect(
        vec![],
        vec![ondin_core::Stroke {
            align: StrokeAlign::Outside,
            sides: ondin_core::StrokeSides::Custom {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 4.0,
            },
            ..stroke(1, true)
        }],
    );
    let r = record_strokes(&doc, &res);
    assert_eq!(r.clips, vec![ClipRule::EvenOdd], "one clip for the paint");
    assert_eq!(
        r.widths,
        vec![2.0, 4.0, 6.0, 8.0],
        "each side doubled from its own width"
    );
    assert_eq!(r.depth_at_stroke, vec![1, 1, 1, 1]);
}

// --- text strokes ----------------------------------------------------------

/// A text node carrying whatever paint lists the caller hands it, shaped for real
/// (Inter is bundled, so `Resolved::rebuild` produces a layout).
fn painted_text(
    fills: Vec<Fill>,
    strokes: Vec<ondin_core::Stroke>,
) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x7E47);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let t = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: t,
        parent: root,
        index: 0,
        kind: NodeKind::Text {
            content: "Hi".into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 40.0,
                weight: 400,
                italic: false,
                line_height: Some(ondin_core::Length::Em(1.2)),
                ..Default::default()
            }),
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
    doc.apply(&Transaction(vec![
        Operation::SetFills { id: t, fills },
        Operation::SetStrokes { id: t, strokes },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, t)
}

/// **Outlined type: a stroke on a text node, aligned outside.** The model has always
/// been able to say it and the walk used to drop it — text took fills and nothing
/// else. It goes through the same construction a shape's stroke does, on the outline
/// of its own glyphs, so the doubled width and the even-odd complement clip are the
/// evidence that it is the *same* path and not a special case.
#[test]
fn text_strokes_its_glyph_outlines_and_honours_the_alignment() {
    let (doc, res, _t) = painted_text(
        vec![fill(1, true)],
        vec![ondin_core::Stroke {
            align: StrokeAlign::Outside,
            width: 3.0,
            ..stroke(2, true)
        }],
    );
    let r = record_strokes(&doc, &res);
    assert_eq!(
        r.widths,
        vec![6.0],
        "one stroke over the whole glyph outline, doubled for the outside construction"
    );
    assert_eq!(
        r.clips,
        vec![ClipRule::EvenOdd],
        "clipped to everything but the glyphs, which is what makes it *outside*"
    );
    assert_eq!(r.depth_at_stroke, vec![1]);
    assert_eq!(r.depth, 0);
}

/// The z-order on type is the z-order on a shape: every visible fill, then every
/// visible stroke.
#[test]
fn a_text_nodes_fills_come_before_its_strokes() {
    let (doc, res, _t) = painted_text(
        vec![fill(1, true), fill(2, true)],
        vec![stroke(3, true), stroke(4, true)],
    );
    assert_eq!(
        record_order(&doc, &res),
        vec![("text", 1), ("text", 2), ("stroke", 3), ("stroke", 4)]
    );
}

/// **The black fallback is for a text node with no ink at all, not for one with no
/// fill.** Outlined type *is* a stroke and no fill, and a fallback that ignored the
/// stroke list would fill every hollow letter solid black — the one drawing the
/// feature exists to avoid.
#[test]
fn outlined_type_gets_no_black_stand_in_fill() {
    let (doc, res, _t) = painted_text(vec![], vec![stroke(9, true)]);
    let seen = record_order(&doc, &res);
    assert_eq!(
        seen,
        vec![("stroke", 9)],
        "the stroke and nothing else — a ('text', 0) entry here is the black \
         fallback filling the letters in"
    );

    // And the fallback is still there for a text node with nothing on it at all:
    // ink that vanished when its one fill was hidden would look like the text had
    // gone.
    let (doc, res, _t) = painted_text(vec![], vec![]);
    assert_eq!(record_order(&doc, &res), vec![("text", 0)], "black");
    // A hidden stroke is not ink, so it does not suppress the fallback either.
    let (doc, res, _t) = painted_text(vec![], vec![stroke(9, false)]);
    assert_eq!(record_order(&doc, &res), vec![("text", 0)]);
}

// --- frame strokes ---------------------------------------------------------

/// Enough of the sequence to say *where* a frame's stroke was drawn: what happened,
/// and at what layer depth.
#[derive(Default)]
struct Sequence {
    seen: Vec<(&'static str, i32)>,
    layers: Vec<(bool, f32)>,
    depth: i32,
}

impl ScenePainter for Sequence {
    // Every picture is available to a recorder: these observe what the walk
    // *emits*, and answering `false` would have the walk emit the missing-image
    // placeholder instead (§15 D179) — a different question from the one each of
    // these tests asks.
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _t: Affine,
        _p: &ondin_core::kurbo::BezPath,
        _b: &Brush,
        _f: Option<ondin_core::Framing>,
        _rule: ondin_render::scene::ClipRule,
    ) {
        self.seen.push(("fill", self.depth));
    }
    fn stroke_path(&mut self, _t: Affine, _p: &ondin_core::kurbo::BezPath, _s: &StrokePaint<'_>) {
        self.seen.push(("stroke", self.depth));
    }
    fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _t: Affine,
        clip: Option<&ondin_core::kurbo::BezPath>,
        _rule: ClipRule,
        opacity: f32,
    ) {
        self.layers.push((clip.is_some(), opacity));
        self.depth += 1;
    }
    fn push_mask_layer(&mut self) {
        self.depth += 1;
    }
    fn pop_layer(&mut self) {
        self.depth -= 1;
    }
}

/// A clipping frame with a background, one child, and the strokes the caller hands it.
fn stroked_frame(strokes: Vec<ondin_core::Stroke>, opacity: f32) -> (Document, Resolved) {
    let mut ids = IdSource::new(0xFA3E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let child = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 60.0),
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: ab,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::CreateNode {
            id: child,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((5.0, 5.0))),
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![
        Operation::SetFills {
            id: child,
            fills: vec![fill(7, true)],
        },
        Operation::SetStrokes { id: ab, strokes },
        Operation::SetOpacity { id: ab, opacity },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn record_sequence(doc: &Document, res: &Resolved) -> Sequence {
    let mut s = Sequence::default();
    scene::build(
        doc,
        res,
        &Viewport {
            view: Rect::new(-200.0, -200.0, 400.0, 400.0),
            pixel_size: (300, 300),
        },
        &RenderOverrides::default(),
        &mut s,
    );
    assert_eq!(s.depth, 0, "every pushed layer was popped");
    s
}

/// **A frame draws a stroke, over its contents and outside its own clip.** Both
/// halves are the feature: under the children a border would be covered by the very
/// layers it encloses, and inside the frame's own clip a centred stroke would lose
/// its outer half and an outside one would be erased outright — which is the drawing
/// that read as "frames still draw no strokes at all".
#[test]
fn a_frame_strokes_over_its_contents_and_outside_its_clip() {
    let (doc, res) = stroked_frame(vec![stroke(1, true)], 1.0);
    let s = record_sequence(&doc, &res);
    assert_eq!(
        s.seen,
        vec![
            // Background and child, both inside the frame's clip layer.
            ("fill", 1),
            ("fill", 1),
            // The border, after them and outside it.
            ("stroke", 0),
        ]
    );
    assert_eq!(s.layers, vec![(true, 1.0)], "just the frame's own clip");
}

/// An outside stroke on a frame is the construction it is on a rect: doubled and
/// clipped to the complement of the frame's box. The clip it must *not* be inside is
/// the frame's own — hence the depths.
#[test]
fn an_outside_stroke_on_a_frame_is_built_from_the_frames_own_box() {
    let (doc, res) = stroked_frame(
        vec![ondin_core::Stroke {
            align: StrokeAlign::Outside,
            width: 4.0,
            ..stroke(1, true)
        }],
        1.0,
    );
    let mut r = StrokeRecorder::default();
    scene::build(
        &doc,
        &res,
        &Viewport {
            view: Rect::new(-200.0, -200.0, 400.0, 400.0),
            pixel_size: (300, 300),
        },
        &RenderOverrides::default(),
        &mut r,
    );
    assert_eq!(r.widths, vec![8.0], "doubled");
    assert_eq!(
        r.clips,
        vec![ClipRule::NonZero, ClipRule::EvenOdd],
        "the frame's own clip, then the stroke's complement"
    );
    assert_eq!(
        r.depth_at_stroke,
        vec![1],
        "inside the stroke's clip and *not* inside the frame's — depth 2 would mean \
         the border was being clipped away by the box it draws on"
    );
    assert_eq!(r.depth, 0);
}

/// **A translucent frame still composites as one unit.** The stroke has to sit
/// outside the clip layer, so the opacity moves to an outer group rather than staying
/// on the clip — two groups would let a translucent border blend against its own
/// contents instead of with them.
#[test]
fn a_translucent_frame_with_a_border_composites_as_one_group() {
    let (doc, res) = stroked_frame(vec![stroke(1, true)], 0.5);
    let s = record_sequence(&doc, &res);
    assert_eq!(
        s.layers,
        vec![(false, 0.5), (true, 1.0)],
        "an unclipped 0.5 group holding a fully-opaque clip group"
    );
    assert_eq!(
        s.seen,
        vec![("fill", 2), ("fill", 2), ("stroke", 1)],
        "contents inside both, the border inside only the opacity group"
    );

    // And with no stroke on it nothing changes: one layer doing both jobs, exactly as
    // before frames stroked.
    let (doc, res) = stroked_frame(vec![], 0.5);
    let s = record_sequence(&doc, &res);
    assert_eq!(
        s.layers,
        vec![(true, 0.5)],
        "no extra layer for a plain frame"
    );
}

/// **A frame paints its whole fill stack, in list order, behind its children and
/// under its border** (§15 D400).
///
/// The frame arm of `scene::paint_shape` used to be `if let Some(brush) =
/// background`: one fill, unconditionally visible, with nowhere for a second to go.
/// It is the same loop the generic tail runs now, and this pins the three things
/// that loop has to get right and the old branch could not express:
///
/// - **Every visible fill draws**, not just the first.
/// - **In list order**, so the last element is the last painted — the list is the
///   z-order, and the panel's rows are its reverse. A stack drawn backwards is
///   invisible on any opaque frame and obvious on a translucent one.
/// - **A hidden fill draws nothing**, which is what the eye on the row means and
///   what a frame's ground had no way to say at all.
///
/// And the placement is unchanged: all of it still lands *inside* the clip layer
/// and *before* the child, with the stroke after both and outside (§15 D144).
///
/// ⚠️ **The recorder captures brushes, because `Sequence` cannot.** Its `seen` is
/// `("fill", depth)` with the paint thrown away, so it would report three fills for
/// a stack drawn in any order and for a hidden one drawn by mistake — every
/// assertion above would have been vacuous over it. Flipping the `.filter(|f|
/// f.visible)` off fails on the length; reversing the iterator fails on the order.
#[test]
fn a_frames_fill_stack_paints_in_order_behind_its_children() {
    #[derive(Default)]
    struct Inks {
        seen: Vec<(Brush, i32)>,
        strokes: Vec<i32>,
        depth: i32,
    }
    impl ScenePainter for Inks {
        fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
            true
        }
        fn fill_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            b: &Brush,
            _f: Option<ondin_core::Framing>,
            _rule: ClipRule,
        ) {
            self.seen.push((b.clone(), self.depth));
        }
        fn stroke_path(
            &mut self,
            _t: Affine,
            _p: &ondin_core::kurbo::BezPath,
            _s: &StrokePaint<'_>,
        ) {
            self.strokes.push(self.depth);
        }
        fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
        fn push_layer(
            &mut self,
            _t: Affine,
            _clip: Option<&ondin_core::kurbo::BezPath>,
            _rule: ClipRule,
            _opacity: f32,
        ) {
            self.depth += 1;
        }
        fn push_mask_layer(&mut self) {
            self.depth += 1;
        }
        fn pop_layer(&mut self) {
            self.depth -= 1;
        }
    }

    let ground = Color::from_rgb8(0xFF, 0xFF, 0xFF);
    let over = Color::from_rgb8(0x3C, 0x78, 0xDC);
    let never = Color::from_rgb8(0xEB, 0x6E, 0x5A);

    // `stroked_frame` gives a clipping frame with one white fill, one child and a
    // border; two more fills go on top of the white — the middle one hidden.
    let (mut doc, _) = stroked_frame(vec![stroke(1, true)], 1.0);
    let ab = doc.get(doc.root()).unwrap().children()[0];
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: ab,
        fills: vec![
            Fill {
                brush: Brush::Solid(ground),
                visible: true,
            },
            Fill {
                brush: Brush::Solid(never),
                visible: false,
            },
            Fill {
                brush: Brush::Solid(over),
                visible: true,
            },
        ],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    let mut r = Inks::default();
    scene::build(
        &doc,
        &res,
        &Viewport {
            view: Rect::new(-200.0, -200.0, 400.0, 400.0),
            pixel_size: (300, 300),
        },
        &RenderOverrides::default(),
        &mut r,
    );
    assert_eq!(r.depth, 0, "every pushed layer was popped");

    // Three fills: the frame's two visible ones and then the child's. The hidden
    // one is absent, and the child's is last — so the two frame fills are behind
    // the contents, which is the half of a frame's paint that is not a shape's.
    let inks: Vec<Brush> = r.seen.iter().map(|(b, _)| b.clone()).collect();
    assert_eq!(
        inks.len(),
        3,
        "two visible frame fills and the child's, not four: {inks:?}"
    );
    assert_eq!(
        inks[0],
        Brush::Solid(ground),
        "the bottom of the stack first"
    );
    assert_eq!(inks[1], Brush::Solid(over), "then the one above it");
    assert!(
        !inks.contains(&Brush::Solid(never)),
        "the hidden fill is not painted: {inks:?}"
    );

    // And the placement is what it always was: both frame fills inside the clip,
    // the border after them and outside it.
    assert_eq!(
        r.seen.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        vec![1, 1, 1],
        "every fill inside the frame's own clip layer"
    );
    assert_eq!(r.strokes, vec![0], "the border outside it");
}

/// **A run with its own colour draws once in it; a run without one draws the whole
/// stack.** The loop inversion `for run { for ink }` is the entirety of per-run
/// colour at the render boundary — `TextRun` already carried a brush and both
/// backends already honoured it, so what was node-level was the walk (§15 D154).
///
/// Two fills and a colour on the first character: the coloured run must produce
/// exactly **one** draw in its own ink rather than one per fill, because a run's
/// override is a single colour and not a stack. Repeating it per fill would double
/// the opacity of a translucent one.
#[test]
fn a_coloured_run_draws_once_and_the_rest_draw_the_fill_stack() {
    let (mut doc, _res, t) = painted_text(vec![fill(1, true), fill(2, true)], vec![]);
    let node_style = ondin_core::TextStyle {
        font_family: "Inter".into(),
        font_size: 40.0,
        line_height: Some(ondin_core::Length::Em(1.2)),
        ..Default::default()
    };
    let mut spans = ondin_core::CharSpans::default();
    spans.set(
        0..1,
        ondin_core::CharAttr::Color(Some(Color::from_rgba8(200, 0, 0, 255))),
        &node_style,
    );
    doc.apply(&Transaction(vec![Operation::SetTextSpans { id: t, spans }]))
        .expect("colour the first character");
    let res = Resolved::rebuild(&doc);

    let seen: Vec<(&str, u8)> = record_order(&doc, &res)
        .into_iter()
        .filter(|(k, _)| *k == "text")
        .collect();
    // "H" once in its own 200; "i" twice, once per fill.
    assert_eq!(
        seen,
        vec![("text", 200), ("text", 1), ("text", 2)],
        "the coloured run draws once and the uncoloured one draws the stack"
    );
}

/// The whole viewport, so nothing in these fixtures is culled and a missing fill
/// means the walk decided not to draw it rather than that it fell off the edge.
fn whole_view() -> Viewport {
    Viewport {
        view: Rect::new(0.0, 0.0, 2000.0, 2000.0),
        pixel_size: (400, 400),
    }
}

fn record(doc: &Document, res: &Resolved) -> RecordingPainter {
    let mut painter = RecordingPainter::default();
    scene::build(
        doc,
        res,
        &whole_view(),
        &RenderOverrides::default(),
        &mut painter,
    );
    painter
}

fn set_mask(doc: &mut Document, id: NodeId) {
    doc.apply(&Transaction(vec![Operation::SetMask { id, mask: true }]))
        .unwrap();
}

/// A mask does two things at once and this asserts both: it stops painting, and
/// it puts a clip over the siblings above it.
///
/// **Nested inside the frame's clip, not beside it.** `max_depth` is the
/// assertion that says so — a mask layer pushed at the same level as the
/// artboard's would unwind in the wrong order and take the frame's clip off
/// early, which is invisible in a fixture with only one of the two.
#[test]
fn a_mask_paints_nothing_and_clips_the_run_above_it() {
    let (mut doc, _res, near, _far) = two_rect_doc();
    assert_eq!(
        record(&doc, &Resolved::rebuild(&doc)).fills.len(),
        2,
        "fixture: both rects paint until one of them is a mask"
    );

    set_mask(&mut doc, near);
    let painter = record(&doc, &Resolved::rebuild(&doc));

    assert_eq!(
        painter.fills.len(),
        1,
        "the mask is not artwork, so only the layer above it paints"
    );
    assert_eq!(
        painter.layers.len(),
        2,
        "the frame's clip, and the mask's over the run above it"
    );
    assert!(
        painter.layers.iter().all(|(clipped, _)| *clipped),
        "both layers carry a clip path: {:?}",
        painter.layers
    );
    assert_eq!(
        painter.max_depth, 2,
        "the mask's layer sits inside the frame's"
    );
    assert_eq!(painter.depth, 0, "every pushed layer was popped");
}

/// A second mask **ends** the first one's run rather than nesting inside it, so
/// the layers above it are clipped once and not twice.
///
/// `max_depth` is again what separates the two readings: nesting would reach
/// three, and a fixture with one mask cannot tell the difference. The fill count
/// says the other half — two masks means two layers that do not paint.
#[test]
fn a_second_mask_ends_the_first_ones_run_instead_of_nesting() {
    let (mut doc, _res, near, far) = two_rect_doc();
    let ab = doc.get(doc.root()).unwrap().children()[0];
    let mut ids = IdSource::new(0xBEEF);
    let (m2, top) = (ids.mint(), ids.mint());
    // Children end up bottom-first as [near, far, m2, top].
    for (id, index) in [(m2, 2), (top, 3)] {
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: ab,
            index,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((5.0, 5.0))),
            name: None,
        }]))
        .unwrap();
    }
    // A fill, or the layer never paints and the count below would be measuring
    // the fixture rather than the run rule.
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: top,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
            visible: true,
        }],
    }]))
    .unwrap();
    assert_eq!(
        doc.get(ab).unwrap().children(),
        &[near, far, m2, top],
        "fixture: the order the run rule below is read against"
    );
    assert_eq!(
        record(&doc, &Resolved::rebuild(&doc)).fills.len(),
        3,
        "fixture: three of the four paint before any of them is a mask"
    );

    set_mask(&mut doc, near);
    set_mask(&mut doc, m2);
    let painter = record(&doc, &Resolved::rebuild(&doc));

    assert_eq!(
        painter.fills.len(),
        2,
        "two masks, two layers left to paint"
    );
    assert_eq!(
        painter.layers.len(),
        3,
        "the frame's clip and one layer per mask"
    );
    assert_eq!(
        painter.max_depth, 2,
        "the second mask replaced the first rather than stacking on it"
    );
    assert_eq!(painter.depth, 0, "every pushed layer was popped");
}

/// **An alpha mask draws its own ink, inside a composite layer, after the run.**
///
/// Every clause is an assertion here because each is a different way to get it
/// wrong, and the recording painter is the only place the *order* is visible at
/// all — the raster test in `ondin-export` proves the pixels are right and would
/// pass against several arrangements that are wrong for other fixtures.
///
/// - It **paints** (two fills where a shape mask leaves one): an alpha mask's ink
///   is the mask, so a walk that skipped it the way it skips a shape mask would
///   erase the run entirely.
/// - Its layer is a **`push_mask_layer`**, not an ordinary one — a plain layer
///   would draw the mask *over* the artwork as if it were artwork.
/// - It is **inside** the run's isolation layer (`max_depth` 3 under the frame's
///   clip), which is what stops the composite reaching the page behind it.
#[test]
fn an_alpha_mask_paints_its_ink_into_a_composite_layer_after_the_run() {
    let (mut doc, _res, near, _far) = two_rect_doc();
    set_mask(&mut doc, near);
    doc.apply(&Transaction(vec![Operation::SetMaskMode {
        id: near,
        mode: ondin_core::MaskMode::Alpha,
    }]))
    .unwrap();
    let painter = record(&doc, &Resolved::rebuild(&doc));

    assert_eq!(
        painter.fills.len(),
        2,
        "the masked layer and the mask's own ink, which *is* the mask"
    );
    assert_eq!(
        painter.mask_layers, 1,
        "and the mask's ink went into a composite layer, not an ordinary one"
    );
    assert_eq!(
        painter.layers.len(),
        2,
        "the frame's clip, and the run's isolation layer — which carries no clip"
    );
    assert!(
        !painter.layers[1].0,
        "an alpha run isolates rather than clips: {:?}",
        painter.layers
    );
    assert_eq!(
        painter.max_depth, 3,
        "frame clip, then the run's isolation, then the composite inside it"
    );
    assert_eq!(painter.depth, 0, "every pushed layer was popped");
}

/// A hidden mask masks nothing.
///
/// The other reading — "clip everything away" — is the one a caller falls into
/// by treating a `None` geometry as an empty clip, and it makes hiding a mask
/// look like deleting the artwork above it.
#[test]
fn a_hidden_mask_clips_nothing_rather_than_everything() {
    let (mut doc, _res, near, _far) = two_rect_doc();
    set_mask(&mut doc, near);
    doc.apply(&Transaction(vec![Operation::SetVisible {
        id: near,
        visible: false,
    }]))
    .unwrap();

    let painter = record(&doc, &Resolved::rebuild(&doc));
    assert_eq!(painter.fills.len(), 1, "the layer above still paints");
    assert_eq!(
        painter.layers.len(),
        1,
        "and only the frame's clip was pushed: {:?}",
        painter.layers
    );
}

// --- text decorations, and skip-ink through the whole walk ------------------

/// Counts the **subpaths** of every filled path, which for a text node carrying
/// only fills is one entry per decoration band: glyphs go to `draw_text`, so
/// nothing else reaches `fill_path` at all.
#[derive(Default)]
struct BandRecorder {
    /// The x extent of every subpath of every filled path, in paint order — one
    /// entry per bar of decoration ink.
    bars: Vec<(f64, f64)>,
}

impl ScenePainter for BandRecorder {
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(
        &mut self,
        _t: Affine,
        p: &ondin_core::kurbo::BezPath,
        _b: &Brush,
        _f: Option<ondin_core::Framing>,
        _rule: ondin_render::scene::ClipRule,
    ) {
        use ondin_core::kurbo::PathEl;
        for el in p.elements() {
            match *el {
                PathEl::MoveTo(q) => self.bars.push((q.x, q.x)),
                PathEl::LineTo(q) => {
                    if let Some(last) = self.bars.last_mut() {
                        last.0 = last.0.min(q.x);
                        last.1 = last.1.max(q.x);
                    }
                }
                _ => {}
            }
        }
    }
    fn stroke_path(&mut self, _t: Affine, _p: &ondin_core::kurbo::BezPath, _s: &StrokePaint<'_>) {}
    fn draw_text(&mut self, _t: Affine, _r: &TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _t: Affine,
        _c: Option<&ondin_core::kurbo::BezPath>,
        _rule: ClipRule,
        _o: f32,
    ) {
    }
    fn push_mask_layer(&mut self) {}
    fn pop_layer(&mut self) {}
}

/// A decorated text node, shaped for real — Inter is bundled, so `Resolved::rebuild`
/// produces a layout with real glyph outlines behind it.
fn decorated_text(
    content: &str,
    underline: bool,
    strikethrough: bool,
) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x5C1B);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let t = ids.mint();
    let line = Some(ondin_core::Decoration::default());
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: t,
        parent: root,
        index: 0,
        kind: NodeKind::Text {
            content: content.into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 40.0,
                underline: underline.then_some(line).flatten(),
                strikethrough: strikethrough.then_some(line).flatten(),
                ..Default::default()
            }),
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
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: t,
        fills: vec![fill(1, true)],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, t)
}

fn record_bands(doc: &Document, res: &Resolved) -> BandRecorder {
    let mut r = BandRecorder::default();
    scene::build(
        doc,
        res,
        &Viewport {
            view: ondin_core::kurbo::Rect::new(-100.0, -100.0, 400.0, 400.0),
            pixel_size: (500, 500),
        },
        &RenderOverrides::default(),
        &mut r,
    );
    r
}

/// **Skip-ink, end to end through the walk both backends share** (§15 D356).
///
/// The two halves are pinned either side of the boundary already —
/// `ondin_core::text` computes the gaps and `scene::decoration_path` honours the
/// ones it is handed — and this is the seam between them: a real document, shaped
/// for real, painted through `scene::build`, where the band arrives at the painter
/// in pieces because `gypsy` puts four descenders through it.
///
/// A text node with fills and no strokes reaches `fill_path` for its decorations and
/// nothing else — glyphs go to `draw_text` — so what the recorder sees *is* the
/// drawing.
///
/// ⚠️ The comparison is the whole test. `ill` has no descender and arrives as one
/// piece, which is what says the pieces are not an artefact of how a band is built;
/// and the same `gypsy` with a **strikethrough** arrives as one piece too, which is
/// the css-text-decor-4 rule reaching the canvas rather than living in a comment.
///
/// **Five bars for four descenders, and not one of them a speck.** Measured at 40px
/// against a thickness of 2.73, the widths are `2.73, 2.73, 7.23, 32.39, 7.50`:
/// `gypsy` opens with a descender and follows it with another, so two of the five are
/// the tight bearings around and between the `g` and the `y`. Those two measured
/// `0.18` and `0.51` when the clearance was allowed to eat a bar whole, and read as
/// dirt; the floor in `ondin_core::text::give_air` now holds them at a square of ink
/// instead of deleting them, which is what stopped the drawing popping between two
/// shapes as the type size crossed a threshold (§15 D356).
///
/// The assertion is on **every bar clearing the thickness**, not on the count: the
/// count moves with the clearance, and the floor is what this is really about.
#[test]
fn an_underlines_ink_arrives_at_the_painter_in_pieces() {
    let (bars, t) = record_bands_of("gypsy", true, false);
    assert_eq!(bars.len(), 5, "four descenders, five pieces: {bars:?}");
    for w in &bars {
        assert!(
            *w >= t * 0.99,
            "no bar is narrower than the band is thick: {bars:?} against {t}"
        );
    }
    let (whole, _) = record_bands_of("ill", true, false);
    assert_eq!(
        whole.len(),
        1,
        "nothing in `ill` descends, so its rule is one bar: {whole:?}"
    );
    let (struck, _) = record_bands_of("gypsy", false, true);
    assert_eq!(
        struck.len(),
        1,
        "a line-through is meant to cross the letters: {struck:?}"
    );
}

/// **A word that is all descenders still keeps a rule** — the tiers in
/// `ondin_core::text::crossings` seen from the far end of the pipeline (§15 D356).
///
/// **This test asserted the opposite for one commit, and the reversal is the record.**
/// With a fixed clearance and a fixed speck rule, `ggg`, `jjj`, `gg` and a lone `j`
/// painted nothing at all — measured, reported, and answered by deciding which
/// fragments are rules *before* any air is taken from them. At 40px `ggg` now paints
/// four bars, `jjj` and `gg` three, and the `j` one.
///
/// ⚠️ **`jjj`'s count has been wrong twice in this comment, in both directions**: one
/// while a back-off kept only the widest fragment of a band with no room, three now
/// that air can no longer erase a fragment at all. Both were guesses from symmetry
/// with `ggg`, and the second is only right because it was measured.
///
/// ⚠️ **The `j`'s single bar is 2.60 against a thickness of 2.73** — *below* it, and
/// asserted rather than described. A bar narrower than the floor to begin with gets no
/// air rather than being padded out, so this is the `j`'s own bearing, untouched:
/// "keeps something" and "keeps something a rule would have kept" are different
/// claims and only the first is true of a lone `j`.
///
/// The `typography` arm is the control: ordinary copy has to be untouched by all of
/// it — six bars, 132 units of rule, every one clearing the thickness.
#[test]
fn a_word_of_nothing_but_descenders_still_keeps_a_rule() {
    for (word, expected) in [("ggg", 4), ("jjj", 3), ("gg", 3), ("j", 1)] {
        let (bars, _) = record_bands_of(word, true, false);
        assert_eq!(
            bars.len(),
            expected,
            "`{word}` keeps a rule where it has room for one: {bars:?}"
        );
    }
    let (j, tj) = record_bands_of("j", true, false);
    assert!(
        j[0] < tj && j[0] > tj * 0.9,
        "the j's one mark is just under a thickness wide — tier 3: {j:?} against {tj}"
    );
    let (typo, t) = record_bands_of("typography", true, false);
    assert_eq!(typo.len(), 6, "ordinary copy keeps its rule: {typo:?}");
    assert!(
        typo.iter().sum::<f64>() > 100.0 && typo.iter().all(|w| *w >= t),
        "and keeps most of it, in bars that are bars: {typo:?}"
    );
}

/// **A group whose mask masks internally is culled on the box its own children
/// draw under, and it used to be culled on a tenth of it.**
///
/// `[S4.1-L2-01]`, §15 D460, and this is the half that names the symptom.
/// `Resolved::mask_path`'s `Group` arm skips a mask *inside* a mask group and
/// unions the rest un-narrowed — so the outline that clips kept 400×400 — while
/// all three bounds computations applied the inner mask and kept 40×40. This walk
/// sets `bounds_cover_subtree` for a `Group` and returns early when `ink_bounds`
/// misses the view, so the container's whole subtree was pruned on a box a tenth
/// of the clip.
///
/// **The result is a layer that is on screen and clickable and is not painted.**
/// Panning brings it back, which is the shape of report this arrives as.
///
/// ⚠️ **Two viewports and a control, because one number proves nothing here.**
/// With the whole page in view the fill was always emitted — the 40×40 corner is
/// inside any view that contains everything — so a test that only rendered the
/// page would have been green throughout. The viewport that matters is one
/// **over the artwork and clear of the corner**, and the control is the same tree
/// with the inner mask flag off, which was never affected.
///
/// ⚠️ **Flipped** by restoring `mask_ink = ci` in `resolve::resolve_subtree` —
/// the *full* pass, not the incremental one, since `Resolved::rebuild` is what
/// this fixture uses: fails on the scrolled viewport at 0 fills against 1. The
/// page-wide assertion and the control both stay green under it, which is what
/// says this fixture is about the cull rather than about masking generally.
/// Reverting `recompute_bounds` instead leaves this test **green**, and
/// `ondin-core`'s `a_mask_group_that_masks_internally_still_clips_with_its_whole_union`
/// is where that half is caught.
#[test]
fn a_group_masked_by_a_group_that_masks_internally_is_not_culled_away() {
    fn fixture(inner_masks: bool) -> (Document, Resolved) {
        let mut ids = IdSource::new(0x5A5A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, g, m, inner_mask, inner_big, outer_big) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let node = |id, parent, index, kind| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        };
        let square = |w: f64| NodeKind::Rect {
            size: Size::new(w, w),
            corner_radii: RoundedRectRadii::default(),
        };
        doc.apply(&Transaction(vec![
            node(
                ab,
                root,
                0,
                NodeKind::Artboard {
                    size: Size::new(2000.0, 2000.0),
                },
            ),
            node(g, ab, 0, NodeKind::Group),
            // Bottom-first inside `G`: the mask group, then the artwork.
            node(m, g, 0, NodeKind::Group),
            node(outer_big, g, 1, square(400.0)),
            // And the same shape one level down inside `M`.
            node(inner_mask, m, 0, square(40.0)),
            node(inner_big, m, 1, square(400.0)),
            Operation::SetFills {
                id: outer_big,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
                    visible: true,
                }],
            },
            Operation::SetFills {
                id: inner_big,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::WHITE),
                    visible: true,
                }],
            },
            Operation::SetMask { id: m, mask: true },
        ]))
        .unwrap();
        if inner_masks {
            doc.apply(&Transaction(vec![Operation::SetMask {
                id: inner_mask,
                mask: true,
            }]))
            .unwrap();
        }
        let res = Resolved::rebuild(&doc);
        (doc, res)
    }

    let fills_in = |doc: &Document, res: &Resolved, view: Rect| {
        let vp = Viewport {
            view,
            pixel_size: (400, 400),
        };
        let mut p = RecordingPainter::default();
        scene::build(doc, res, &vp, &RenderOverrides::default(), &mut p);
        p.fills.len()
    };

    let (doc, res) = fixture(true);
    let page = Rect::new(0.0, 0.0, 2000.0, 2000.0);
    // Over the artwork and clear of the 40×40 corner the old box stopped at.
    let scrolled = Rect::new(100.0, 100.0, 300.0, 300.0);

    assert_eq!(
        fills_in(&doc, &res, page),
        1,
        "fixture: with the page in view the artwork draws, clipped by the 400×400 \
         outline — this was always true and is why the defect was invisible"
    );
    assert_eq!(
        fills_in(&doc, &res, scrolled),
        1,
        "and scrolled onto it, it must still draw: the container is culled on \
         `ink_bounds`, which must be the box its children are clipped to"
    );

    // The control: the same tree with no inner mask, which was never affected.
    let (doc, res) = fixture(false);
    assert_eq!(fills_in(&doc, &res, scrolled), 1);
}

/// Every painted bar's width, and the thickness of the band they came from — the
/// second is what the "not a bar" rule is measured against, read off the layout
/// rather than written down here.
fn record_bands_of(content: &str, underline: bool, strikethrough: bool) -> (Vec<f64>, f64) {
    let (doc, res, id) = decorated_text(content, underline, strikethrough);
    let thickness = res
        .text_layout(id)
        .and_then(|l| l.decorations.first())
        .map(|d| d.band.height())
        .expect("the fixture decorates something");
    let bars = record_bands(&doc, &res)
        .bars
        .into_iter()
        .map(|(a, b)| b - a)
        .collect();
    (bars, thickness)
}

// --- the subtree cull's kind list ------------------------------------------

/// One small rect inside the viewport, plus `n` containers of `kind` parked far
/// off to the right, each holding two overlapping rects.
///
/// The on-screen rect is not decoration: without it the **root**'s own bounds
/// would miss the viewport and the whole walk would return at the first node, so
/// every container would be "culled" for a reason that says nothing about the
/// list under test.
fn parked_containers(kind: NodeKind, n: usize) -> (Document, Resolved) {
    let mut ids = IdSource::new(0xC011);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let solid = || {
        vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
            visible: true,
        }]
    };
    let rect = || NodeKind::Rect {
        size: Size::new(20.0, 20.0),
        corner_radii: RoundedRectRadii::default(),
    };
    let c_kind_takes_paint = matches!(kind, NodeKind::Boolean { .. });
    let seen = ids.mint();
    let mut ops = vec![
        Operation::CreateNode {
            id: seen,
            parent: root,
            index: 0,
            kind: rect(),
            transform: Some(Affine::translate((-900.0, -900.0))),
            name: None,
        },
        Operation::SetFills {
            id: seen,
            fills: solid(),
        },
    ];
    for i in 0..n {
        let (c, a, b) = (ids.mint(), ids.mint(), ids.mint());
        ops.push(Operation::CreateNode {
            id: c,
            parent: root,
            index: i + 1,
            kind: kind.clone(),
            transform: Some(Affine::translate((10_000.0, 0.0))),
            name: None,
        });
        for (j, dx) in [(0usize, 0.0), (1, 50.0)] {
            let id = if j == 0 { a } else { b };
            ops.push(Operation::CreateNode {
                id,
                parent: c,
                index: j,
                kind: rect(),
                transform: Some(Affine::translate((dx, 0.0))),
                name: None,
            });
            ops.push(Operation::SetFills { id, fills: solid() });
        }
        // ⚠️ **Only the boolean**, and the asymmetry is forced rather than
        // chosen: a `Group` takes no paint, so a `SetFills` on one is
        // `WrongKindForOp` and the fixture will not build. It matters because a
        // boolean with no fill draws *nothing* — so a version of this fixture
        // that skipped the fill for symmetry's sake would measure 1 on both arms
        // for a reason that has nothing to do with the cull, which is the
        // vacuity this whole suite keeps meeting.
        if c_kind_takes_paint {
            ops.push(Operation::SetFills {
                id: c,
                fills: solid(),
            });
        }
    }
    doc.apply(&Transaction(ops)).expect("build the parked doc");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// Fills issued for `n` parked containers of `kind`, with the viewport nowhere
/// near them.
fn parked_fills(kind: NodeKind, n: usize) -> usize {
    let (doc, res) = parked_containers(kind, n);
    let vp = Viewport {
        view: Rect::new(-1000.0, -1000.0, -500.0, -500.0),
        pixel_size: (500, 500),
    };
    let mut painter = RecordingPainter::default();
    scene::build(&doc, &res, &vp, &RenderOverrides::default(), &mut painter);
    assert_eq!(painter.depth, 0, "every pushed layer was popped");
    painter.fills.len()
}

/// **An off-screen boolean is culled, exactly as an off-screen group is** —
/// `[S11.1-L4-05]`, §15 D536.
///
/// `paint_node`'s `bounds_cover_subtree` enumerates the kinds whose cached
/// bounds may stand for their whole subtree, and `Boolean` was missing from the
/// list. A boolean has children, does not clip and is neither `Group` nor
/// `Root`, so the predicate came out `false` and the cull short-circuited before
/// it ever consulted the bounds — **every boolean in the document was walked,
/// tessellated and encoded on every frame, wherever the view was.** In release,
/// against a counting painter, 800 booleans parked off screen issued 801 fills
/// and cost 0.106 ms of walk alone; 800 groups issued 1 and cost 0.0116 ms.
///
/// ⚠️ **Flipped both ways, and only one of them bit — which is the more useful
/// half and is why this paragraph is longer than the test.** Removing
/// `NodeKind::Boolean` from the list fails the boolean arm at **4 against 1**,
/// the predicted site, and leaves the group arm green. Removing
/// `NodeKind::Group` **changes nothing at all**: both arms stay green.
///
/// **So the group arm is not a control for the list, and this test's prose said
/// it was until the flip was run.** With `Group` off the list a parked group is
/// no longer culled *as a subtree* — but the walk then descends to its children,
/// which are leaf rects, `children().is_empty()` makes the predicate true for
/// each of them, and every one is culled individually. Same fill count, more
/// walking. What the group arm actually controls for is the **harness**: that
/// the containers are reachable, that the viewport is genuinely empty, and that
/// a fill count of 1 is the on-screen rect rather than a walk that stopped early.
///
/// ⚠️ **And that is why nobody saw this predicate be wrong for as long as it
/// was.** A group's descendants get culled leaf by leaf whatever the list says,
/// so the list has never been observable in a fill count *for a group*. A
/// boolean is the one kind where it is, because the walk stops dead there — its
/// children are operands and are never painted, so a boolean that is not culled
/// as a subtree is not culled at all. The kind that was missing from the list is
/// the only kind whose absence the list can be caught by.
///
/// ⚠️ **A boolean is the *strongest* member of that list, not a doubtful one.**
/// The walk stops dead at a boolean — its children are its operands, not artwork
/// — so its cached ink bounds cover its whole subtree exactly, where a group's
/// merely happen to. That is the argument for the fix, and it is stronger than
/// the one the two kinds already on the list rest on.
#[test]
fn an_off_screen_boolean_is_culled_like_an_off_screen_group() {
    assert_eq!(
        parked_fills(NodeKind::Group, 3),
        1,
        "control: only the on-screen rect — a parked group's subtree is skipped"
    );
    assert_eq!(
        parked_fills(NodeKind::Boolean { op: BoolOp::Union }, 3),
        1,
        "and a parked boolean's is too: its bounds cover its subtree exactly, \
         because the walk stops at it"
    );
}

/// **And the cull is flat in the number of parked booleans**, which is the half
/// the fills count cannot say on its own.
///
/// A walk that culled the *result* but still descended would issue one fill and
/// still cost O(n); this pins the shape rather than the total. Asserted as a
/// count of fills at two sizes rather than as a time, because a timing assertion
/// in this suite would be a flake — the 0.106 ms against 0.0116 ms measurement
/// lives in the doc above and in the finding, where nothing has to re-run it.
#[test]
fn the_boolean_cull_does_not_grow_with_the_number_parked() {
    let one = parked_fills(NodeKind::Boolean { op: BoolOp::Union }, 1);
    let many = parked_fills(NodeKind::Boolean { op: BoolOp::Union }, 40);
    assert_eq!((one, many), (1, 1), "40 parked booleans cost what 1 does");
}

/// **Hiding a mask reveals what it masked, in both modes** — §15 D566.
///
/// §15 D282 opens its rule with this case: *"`None` means masks nothing, never
/// 'clip everything away': a **hidden mask**, an empty text node, an empty group
/// and a cancelled boolean all answer it"*. Only the `Shape` arm honoured it,
/// through `mask_geometry`'s first line; the `Alpha` arm consulted no visibility
/// at all, so `end_run` pushed a `DestIn` layer and then drew nothing into it —
/// an alpha multiply by zero. Measured in release on a 100×100 white mask over a
/// 200×200 red rect: hiding a `Shape` mask gave 40,000 non-transparent pixels and
/// hiding an `Alpha` mask gave **0**.
///
/// ⚠️ **Asserted on the walk rather than on pixels**, which is both cheaper and
/// sharper: `mask_layers` is the `DestIn` composite itself, so a hidden alpha
/// mask that pushes *no* mask layer is the mechanism rather than a consequence of
/// it. The visible row is the control — without it, "0 mask layers" is equally
/// consistent with a walk that stopped compositing altogether.
///
/// ⚠️ **The counter-quote, so the next reader does not have to find it.** D282
/// also says *"an alpha mask with no ink legitimately erases the run — that is
/// the mode being honest"*. That sentence is about **content**; this is about the
/// eye icon, which the sentence above it names. The corroboration is that
/// `query::hit_test` still returned the masked content and `world_bounds` was
/// still the hidden mask's own box, so one document behaved three inconsistent
/// ways at once.
///
/// ⚠️ **Flip-checked, and it bit somewhere other than where this doc first said
/// it would.** Dropping the `visible(…)` term from the `Alpha` arm was predicted
/// to fail on the **fill count**; it fails on `mask_layers`, at `(1, 1, 0)`
/// against `(1, 0, 0)`. The fill count is 1 either way — the hidden mask paints
/// nothing whether or not a composite is opened for it, which is the whole
/// mechanism: an empty `DestIn` layer multiplies the run's alpha by zero *after*
/// the run has been drawn. So the pixels vanish while the walk still records one
/// fill, and **`mask_layers` is the only assertion here with teeth**. A test
/// written from the reported symptom — "nothing is on screen" — would have
/// counted fills and gone green.
#[test]
fn a_hidden_mask_reveals_the_run_above_it_in_both_modes() {
    let with = |mode: MaskMode, shown: bool| {
        let (mut doc, _res, near, _far) = two_rect_doc();
        set_mask(&mut doc, near);
        doc.apply(&Transaction(vec![
            Operation::SetMaskMode { id: near, mode },
            Operation::SetVisible {
                id: near,
                visible: shown,
            },
        ]))
        .unwrap();
        let p = record(&doc, &Resolved::rebuild(&doc));
        (p.fills.len(), p.mask_layers, p.depth)
    };

    // The control: a *visible* alpha mask does composite, and the mask itself is
    // drawn into that layer — so the fill count is two, not one.
    assert_eq!(
        with(MaskMode::Alpha, true),
        (2, 1, 0),
        "fixture: a visible alpha mask pushes its composite and paints into it"
    );

    // Hidden, the two modes have to agree, and what they agree on is that the
    // layer above draws.
    assert_eq!(
        with(MaskMode::Alpha, false),
        (1, 0, 0),
        "a hidden alpha mask masks nothing rather than erasing everything"
    );
    assert_eq!(
        with(MaskMode::Shape, false),
        (1, 0, 0),
        "which is what the shape arm has always done"
    );
}
