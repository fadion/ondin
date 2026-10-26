//! Scene building, shared by both backends (§6.3).
//!
//! Walks the document in z-order (culled to the viewport) and issues draw calls
//! to a backend-agnostic [`ScenePainter`], so the CPU and GPU backends share one
//! walk.
//!
//! - **Brushes, not colours.** Gradients travel as `peniko::Brush` in the node's
//!   local space. An image's coordinates are its own pixel grid, so every draw
//!   carries a [`Framing`] beside the brush (see `framing_of`).
//! - **Artboards clip** unless `Node::clip` is off, which also turns off subtree
//!   culling.
//! - **Opacity is a layer**, pushed only when a node needs one: folding it into
//!   each child's alpha is wrong where children overlap.

use crate::renderer::{GhostNode, RenderOverrides, Viewport};
use kurbo::{Affine, BezPath, Rect, Shape};
use ondin_core::Brush;
use ondin_core::text::{Glyph as TextGlyph, TextLayout};
use ondin_core::{
    Document, Effect, Framing, MaskMode, NodeId, NodeKind, Paint, Resolved, Stroke, StrokeAlign,
    geometry,
};

const TOLERANCE: f64 = 0.1;

/// A stroke reduced to what a backend needs to draw it.
///
/// Always centred: inside and outside are built in [`paint_strokes`] from a
/// doubled width and a clip. `dashes` is the pattern as drawn
/// (`geometry::resolved_dashes`).
pub struct StrokePaint<'a> {
    pub brush: &'a Brush,
    /// How an image brush sits in the frame; [`None`] for other brushes or a lost
    /// image. The frame is the *shape's* box, not the ink's.
    pub framing: Option<Framing>,
    pub width: f64,
    pub join: kurbo::Join,
    pub cap: kurbo::Cap,
    /// Mitre limit as a ratio, kurbo's own unit.
    pub miter_limit: f64,
    pub dashes: Vec<f64>,
    pub dash_offset: f64,
}

/// How a clip path's interior is decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClipRule {
    /// Winding number ≠ 0.
    #[default]
    NonZero,
    /// Odd crossing count: "everything except this shape" (an outside stroke's
    /// clip), whatever the path's winding direction.
    EvenOdd,
}

impl From<ondin_core::FillRule> for ClipRule {
    /// Two types so the painter API does not depend on the document model.
    fn from(r: ondin_core::FillRule) -> Self {
        match r {
            ondin_core::FillRule::NonZero => ClipRule::NonZero,
            ondin_core::FillRule::EvenOdd => ClipRule::EvenOdd,
        }
    }
}

/// A run of shaped glyphs handed to a backend. Glyph positions are in
/// text-LOCAL space (y on the baseline); `transform` maps that to world space.
pub struct TextRun<'a> {
    /// The font blob (+ face index) as parley resolved it.
    pub font: &'a ondin_core::peniko::FontData,
    pub font_size: f32,
    /// Normalized variable-axis coordinates (i16, skrifa convention).
    pub coords: &'a [i16],
    pub brush: &'a Brush,
    /// As [`StrokePaint::framing`]; the type's own box is the frame, so an
    /// image-filled headline shows one picture across it, not one per run.
    pub framing: Option<Framing>,
    pub glyphs: &'a [TextGlyph],
}

/// Backend-agnostic drawing sink. Paths and glyphs are LOCAL, brushes in the same
/// space; `transform` maps them to world space.
///
/// An image brush needs `framing`, the map from source pixels to local space,
/// resolved once by [`ondin_core::ImageRef::framing`] so both backends and the SVG
/// writer agree; [`None`] for every other brush.
pub trait ScenePainter {
    /// Whether this painter can put `id`'s pixels on the page. The store belongs
    /// to the backend and the placeholder's geometry to the walk, so there is one
    /// drawing (§15 D179).
    fn has_image(&self, id: &ondin_core::ImageId) -> bool;

    /// Fill `path`, deciding its interior by `rule`. A parameter, not a mode:
    /// `vello_cpu` keeps a fill rule until changed, so a forgotten restore would
    /// tint later fills on one backend only.
    fn fill_path(
        &mut self,
        transform: Affine,
        path: &BezPath,
        brush: &Brush,
        framing: Option<Framing>,
        rule: ClipRule,
    );
    fn stroke_path(&mut self, transform: Affine, path: &BezPath, stroke: &StrokePaint<'_>);
    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>);

    /// Begin a composited group: everything until [`Self::pop_layer`] is drawn
    /// offscreen, clipped to `clip` (in `transform`'s space) if given, then
    /// composited at `opacity`.
    fn push_layer(
        &mut self,
        transform: Affine,
        clip: Option<&BezPath>,
        rule: ClipRule,
        opacity: f32,
    );

    /// Begin a layer composited over the enclosing layer **as an alpha mask**: what
    /// is there survives in proportion to the alpha drawn here (`Compose::DestIn`).
    /// The caller owes it an enclosing layer holding *only* what is to be masked,
    /// or the composite erases artwork behind it.
    fn push_mask_layer(&mut self);

    /// Begin a layer **filtered** before it is composited: a node's effect stack
    /// (§5.3a). `bounds` is the node's ink box in `transform`'s space, grown by
    /// [`ondin_core::effect::stack_escape`]; it comes from the walk so both
    /// backends allocate the same buffer. The default composites plainly, drawing
    /// the artwork unfiltered rather than dropping it; shipping backends override.
    fn push_effect_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        effects: &[ondin_core::Keyed<Effect>],
    ) {
        let _ = (bounds, effects);
        self.push_layer(transform, None, ClipRule::NonZero, 1.0);
    }

    fn pop_layer(&mut self);
}

/// Walk `page` and drive `painter`.
///
/// `overrides` carries uncommitted gesture state (§6.2, invariant 5). When empty,
/// the walk reduces to the committed document, bit-identical to `Resolved`'s world
/// transforms except for [`snapped_box_world`]'s pixel rounding.
pub fn build<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    page: NodeId,
    painter: &mut P,
) {
    build_of(doc, res, vp, overrides, &[page], painter);
}

/// Walk **these subtrees** and drive `painter`, in the order given.
///
/// [`build`] is one call of this; the general form exports a *selection*.
///
/// The mirror of `ondin_export::svg::svg_of`: each origin is painted under its
/// **parent's** world transform so the node's local composes once (§15 D258).
/// Ancestor clipping and masks do not travel (a layer exported alone is not cut
/// by its frame; a masked layer comes out whole), and nothing behind is painted.
/// Order is the caller's, in document order; nothing is deduplicated
/// (`build::outermost` is the caller's filter).
pub fn build_of<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    origins: &[NodeId],
    painter: &mut P,
) {
    let base = device_base(vp);
    for id in origins {
        let outer = doc
            .get(*id)
            .and_then(|n| n.parent())
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        paint_node(doc, res, overrides, *id, outer, vp.view, base, painter);
    }
}

/// Whether axis-aligned boxes snap to whole device pixels (see
/// [`snapped_box_world`]).
const SNAP_BOXES_TO_DEVICE_PIXELS: bool = true;

/// World → device pixels, the transform both painters apply on top of the world
/// transform this walk hands them; it must stay in step with `cpu::CpuPainter::base`
/// and `gpu::GpuScenePainter::base`.
pub(crate) fn device_base(vp: &Viewport) -> Affine {
    let sx = vp.pixel_size.0 as f64 / vp.view.width();
    let sy = vp.pixel_size.1 as f64 / vp.view.height();
    Affine::scale_non_uniform(sx, sy) * Affine::translate((-vp.view.min_x(), -vp.view.min_y()))
}

/// `world`, adjusted so a local `size` box lands on whole device pixels.
///
/// Kills the seam between shapes sharing an edge: vello blends each path's partial
/// coverage independently, so a mid-pixel shared edge leaks the backdrop (the
/// "conflation artifact", vello issue #49; no antialiasing setting helps). Both
/// shapes round the edge to the same integer on one global grid, so they meet
/// exactly.
///
/// Only for a box whose device transform is a pure scale and translate; a rotated
/// or skewed shape is returned untouched (a mitigation, not a fix).
///
/// Displacement is under half a pixel except where the collapse floor fires (a box
/// thinner than a pixel keeps one), which also scales ink measured in the node's
/// own space. Rounding changes as the view pans, so edges step rather than glide.
fn snapped_box_world(world: Affine, base: Affine, size: ondin_core::kurbo::Size) -> Affine {
    if !SNAP_BOXES_TO_DEVICE_PIXELS {
        return world;
    }
    let device = base * world;
    let [a, b, c, d, e, f] = device.as_coeffs();
    // Rotated, skewed or degenerate: nothing axis-aligned to round.
    const EPS: f64 = 1e-9;
    if b.abs() > EPS || c.abs() > EPS || a.abs() < EPS || d.abs() < EPS {
        return world;
    }
    // Round both edges, rebuild the scale from the span. A non-empty box must not
    // round away to nothing (a hairline would vanish), so a zero span keeps one
    // pixel in the axis's direction.
    let axis = |scale: f64, origin: f64, extent: f64| -> (f64, f64) {
        if extent.abs() < EPS {
            return (scale, origin);
        }
        let (near, far) = (origin, origin + scale * extent);
        let (rn, mut rf) = (near.round(), far.round());
        if (rf - rn).abs() < 1.0 {
            rf = rn + (far - near).signum();
        }
        ((rf - rn) / extent, rn)
    };
    let (sa, se) = axis(a, e, size.width);
    let (sd, sf) = axis(d, f, size.height);
    // A box already on the grid keeps its exact transform: the matrix inverse below
    // perturbs it in the fourteenth decimal, which `assert_preview_matches_commit`
    // sees.
    if sa == a && sd == d && se == e && sf == f {
        return world;
    }
    // Back into world space: the painters re-apply `base`.
    base.inverse() * Affine::new([sa, b, c, sd, se, sf])
}

/// The local box of a kind whose geometry *is* an axis-aligned box, so it can snap.
/// Not an ellipse or polygon: their box edges are not ink, so rounding would move
/// the drawing without making shapes meet.
fn snappable_box(kind: &NodeKind) -> Option<ondin_core::kurbo::Size> {
    match kind {
        NodeKind::Rect { size, .. } | NodeKind::Artboard { size, .. } => Some(*size),
        _ => None,
    }
}

/// How `brush` sits in `frame`: [`None`] for a solid, a gradient, or an image the
/// document has no entry for. The intrinsic size comes from the document's table,
/// not the decode cache (§5.5a), so a picture still decoding frames at its final
/// size.
fn framing_of(doc: &Document, brush: &Brush, frame: Rect) -> Option<Framing> {
    let Brush::Image(img) = brush else {
        return None;
    };
    let entry = doc.image(&img.image.id)?;
    Some(img.image.framing(frame, entry.width, entry.height))
}

/// Whether `brush` is a picture this painter cannot draw. An unknown id, an unread
/// link and undecodable bytes are one state on purpose (§15 D179).
fn missing_picture<P: ScenePainter>(painter: &P, brush: &Brush) -> bool {
    match brush {
        Brush::Image(img) => !painter.has_image(&img.image.id),
        _ => false,
    }
}

/// Fill `path` with `brush`, or with the placeholder when the picture is missing.
///
/// The one place a fill becomes ink, so frames and shapes show a lost picture the
/// same way.
fn fill_or_placeholder<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    path: &BezPath,
    brush: &Brush,
    framing: Option<Framing>,
    rule: ClipRule,
) {
    if missing_picture(painter, brush) {
        // Non-zero whatever the shape's rule: an even-odd placeholder would punch
        // holes in the one drawing meant to be unmistakably solid.
        paint_missing(painter, world, path);
        return;
    }
    clipped(painter, world, framing, |p| {
        p.fill_path(world, path, brush, framing, rule)
    });
}

/// Draw the placeholder over `path`: a grey ground, a rim and a cross.
///
/// Ink, not chrome: it goes through the artwork's own calls so exports show it
/// (the SVG writer draws it off the same `missing_placeholder`). Cut to the shape,
/// not its box.
fn paint_missing<P: ScenePainter>(painter: &mut P, world: Affine, path: &BezPath) {
    let Some(m) = ondin_core::missing_placeholder(path.bounding_box()) else {
        return;
    };
    painter.fill_path(
        world,
        path,
        &Brush::Solid(m.ground),
        None,
        ClipRule::NonZero,
    );
    let ink = Brush::Solid(m.ink);
    let line = || StrokePaint {
        brush: &ink,
        framing: None,
        width: m.width,
        join: kurbo::Join::Miter,
        cap: kurbo::Cap::Butt,
        miter_limit: 4.0,
        dashes: Vec::new(),
        dash_offset: 0.0,
    };
    painter.push_layer(world, Some(path), ClipRule::NonZero, 1.0);
    painter.stroke_path(world, &m.cross, &line());
    painter.stroke_path(world, path, &line());
    painter.pop_layer();
}

/// Run `draw` with `framing`'s clip in force, if it asked for one.
///
/// Only a letterboxing [`ondin_core::ImageFit::Fit`] does: peniko has no
/// transparent extend, so the band beside the picture is cut or the sampler would
/// smear edge pixels across it. The layer wraps the one draw.
fn clipped<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    framing: Option<Framing>,
    draw: impl FnOnce(&mut P),
) {
    match framing.and_then(|f| f.clip) {
        Some(rect) => {
            painter.push_layer(
                world,
                Some(&rect.to_path(TOLERANCE)),
                ClipRule::NonZero,
                1.0,
            );
            draw(painter);
            painter.pop_layer();
        }
        None => draw(painter),
    }
}

/// The clip a mask holds over the run of siblings after it, while that run is
/// walked. A layer nested inside the parent's (a mask has a clip per *run*, a
/// parent one for all children); it closes and reopens with the parent's layer
/// (see `RenderOverrides::escape_clip`), so it remembers the path, not just
/// whether a layer is open.
#[derive(Default)]
struct MaskRun {
    kind: Option<RunKind>,
    open: bool,
}

/// What a run is masked *by*, which is a different layer arrangement per mode.
enum RunKind {
    /// [`MaskMode::Shape`]: the mask's outline in the **parent's** space, pushed
    /// as the layer's clip. The mask itself is never drawn.
    Clip(BezPath),
    /// [`MaskMode::Alpha`]: the mask **node**, drawn at the *end* of the run and
    /// composited over it with [`ScenePainter::push_mask_layer`]. A node, not
    /// geometry, because what matters is the ink it lays down (fills, gradients,
    /// a photograph's alpha), which only drawing yields.
    Alpha(NodeId),
}

impl MaskRun {
    /// Push the run's layer if it has one and is not already open; `world` is the
    /// **parent's**. Opacity is 1.0 in both arms on purpose (an alpha mask's is
    /// already in its ink). The alpha arm's clip-less layer is an isolation group,
    /// so the final composite cannot reach the page behind.
    fn reopen<P: ScenePainter>(&mut self, painter: &mut P, world: Affine) {
        if self.open {
            return;
        }
        match &self.kind {
            Some(RunKind::Clip(clip)) => {
                painter.push_layer(world, Some(clip), ClipRule::NonZero, 1.0);
                self.open = true;
            }
            Some(RunKind::Alpha(_)) => {
                painter.push_layer(world, None, ClipRule::NonZero, 1.0);
                self.open = true;
            }
            None => {}
        }
    }

    /// The mask node to composite over this run before its layer is popped, or
    /// `None` for a shape run or one that never opened. Split from [`Self::close`]
    /// because drawing it needs `paint_node`'s context.
    fn alpha_to_composite(&self) -> Option<NodeId> {
        match self.kind {
            Some(RunKind::Alpha(id)) if self.open => Some(id),
            _ => None,
        }
    }

    /// Pop the run's layer if it is open. Idempotent.
    fn close<P: ScenePainter>(&mut self, painter: &mut P) {
        if self.open {
            painter.pop_layer();
            self.open = false;
        }
    }
}

/// Whether `id` is drawn, once a live preview has had its say.
///
/// Both mask modes ask this: hiding an alpha mask must reveal what it masks, not
/// erase it.
fn visible(ov: &RenderOverrides, doc: &Document, id: NodeId) -> bool {
    ov.get(id)
        .and_then(|o| o.visible)
        .unwrap_or_else(|| doc.shown(id))
}

/// The outline a mask node clips with, in its **parent's** space, read through the
/// overrides. The mirror of [`ondin_core::Resolved::mask_path`], arm for arm, with
/// `Painted`'s precedence so the two agree when nothing is in flight
/// (`assert_preview_matches_commit`).
fn mask_geometry(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
) -> Option<BezPath> {
    let node = doc.get(id)?;
    let over = ov.get(id);
    if !visible(ov, doc, id) {
        return None;
    }
    let kind = over
        .and_then(|o| o.kind.as_ref())
        .or_else(|| res.used_kind(doc, id))
        .unwrap_or(node.kind());
    let local = over
        .and_then(|o| o.transform)
        .or_else(|| res.used_local(doc, id))
        .unwrap_or_else(|| node.transform());
    let own = match kind {
        NodeKind::Boolean { .. } => ov
            .boolean_path(id)
            .or_else(|| res.boolean_path(id))
            .cloned(),
        NodeKind::Text { .. } => over
            .and_then(|o| o.text.as_ref())
            .or_else(|| res.text_layout(id))
            .map(ondin_core::text::outline)
            .filter(|p| !p.elements().is_empty()),
        NodeKind::Group | NodeKind::Root => {
            let inner: Vec<BezPath> = node
                .children()
                .iter()
                .filter(|c| !doc.get(**c).is_some_and(|n| n.mask()))
                .filter_map(|c| mask_geometry(doc, res, ov, *c))
                .collect();
            ondin_core::boolean::evaluate(ondin_core::BoolOp::Union, &inner)
        }
        kind => geometry::local_path(kind),
    }?;
    Some(local * own)
}

/// [`mask_geometry`] for a ghost, which carries its derived geometry.
fn ghost_mask_geometry(ghost: &GhostNode) -> Option<BezPath> {
    if !ghost.visible {
        return None;
    }
    let own = match &ghost.kind {
        NodeKind::Boolean { .. } => ghost.path.clone(),
        NodeKind::Text { .. } => ghost
            .text
            .as_ref()
            .map(ondin_core::text::outline)
            .filter(|p| !p.elements().is_empty()),
        NodeKind::Group | NodeKind::Root => {
            let inner: Vec<BezPath> = ghost
                .children
                .iter()
                .filter(|c| !c.mask)
                .filter_map(ghost_mask_geometry)
                .collect();
            ondin_core::boolean::evaluate(ondin_core::BoolOp::Union, &inner)
        }
        kind => geometry::local_path(kind),
    }?;
    Some(ghost.transform * own)
}

/// The visual state of a node once overrides are taken into account.
struct Painted<'a> {
    kind: &'a NodeKind,
    /// Resolved (`Document::resolved_paint`): every brush literal, read with
    /// `BrushSource::drawn`.
    paint: std::borrow::Cow<'a, Paint>,
    text: Option<&'a TextLayout>,
    /// The outline `Resolved` derived for a `Boolean`, in local space.
    path: Option<&'a BezPath>,
    /// The box a `Boolean` whose arithmetic was abandoned draws its placeholder in
    /// (§15 D298); `None` otherwise, including a correctly empty boolean.
    placeholder: Option<Rect>,
    /// How the geometry decides its interior; read by the fills only.
    rule: ClipRule,
}

#[allow(clippy::too_many_arguments)]
fn paint_node<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
    parent_world: Affine,
    view: Rect,
    base: Affine,
    painter: &mut P,
) {
    let Some(node) = doc.get(id) else { return };
    let over = ov.get(id);

    if !over
        .and_then(|o| o.visible)
        .unwrap_or_else(|| doc.shown(id))
    {
        return;
    }

    // Cull what misses the viewport, skipping a subtree only when the node's bounds
    // cover it: a group's, a clipping frame's, and a boolean's (its children are
    // never walked). A new `NodeKind` needs a look at this predicate.
    let bounds_cover_subtree = node.children().is_empty()
        || node.clip()
        || matches!(
            node.kind(),
            NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. }
        );
    // Never culled: a moving node (cached bounds describe where it *was*) or one
    // holding a ghost (the bounds cannot know about it). `ink_bounds`, not
    // `world_bounds` (§5.3a): a drop shadow lands outside the layer's box.
    if bounds_cover_subtree
        && !ov.moved(id)
        && ov.ghosts_of(id).next().is_none()
        && let Some(bounds) = res.ink_bounds(id)
        && !intersects(bounds, view)
    {
        return;
    }

    // A gesture's override first, then the used geometry (where container layout
    // placed the node).
    let local = over
        .and_then(|o| o.transform)
        .or_else(|| res.used_local(doc, id))
        .unwrap_or_else(|| node.transform());
    let world = parent_world * local;
    // A bound opacity is drawn as its variable's percentage, never its literal's 0.
    let opacity = over
        .and_then(|o| o.opacity)
        .unwrap_or_else(|| doc.drawn_opacity(id));

    // The box the paint is stored against: a gesture's kind is its stored one.
    let stored = over.and_then(|o| o.kind.as_ref()).unwrap_or(node.kind());
    let kind = over
        .and_then(|o| o.kind.as_ref())
        .or_else(|| res.used_kind(doc, id))
        .unwrap_or(node.kind());
    let painted = Painted {
        kind,
        // Gradients drawn on the box layout gives the layer, not the stored one.
        paint: ondin_core::paint_box::drawn_paint(
            doc.resolved_paint(
                id,
                over.and_then(|o| o.paint.as_ref()).unwrap_or(node.paint()),
            ),
            ondin_core::paint_box::paint_box(doc, node.size_vars(), stored),
            kind,
        ),
        // Committed only: a preview cannot change the rule (`absorb` refuses
        // `SetFillRule`), so there is no override to prefer here.
        rule: node.fill_rule().into(),
        text: over
            .and_then(|o| o.text.as_ref())
            .or_else(|| res.text_layout(id)),
        path: ov.boolean_path(id).or_else(|| res.boolean_path(id)),
        // Committed only, and only when no preview owns the node: a gesture
        // re-evaluating this boolean always leaves an entry, and without the gate a
        // drag would draw the placeholder over the last committed operand positions.
        placeholder: match ov.boolean_path(id) {
            Some(_) => None,
            None => ondin_core::boolean_placeholder(doc, res, id),
        },
    };

    // The snapped transform is this node's own ink, never its children's (passing
    // it down would compound the rounding). A layer a chrome ghost is glued to opts
    // out, with the ghost, since the larger offcut would round differently.
    let shape_world = match snappable_box(painted.kind).filter(|_| !ov.unsnapped(id)) {
        Some(size) => snapped_box_world(world, base, size),
        None => world,
    };

    let clip = match painted.kind {
        NodeKind::Artboard { .. } if node.clip() => geometry::local_path(painted.kind),
        _ => None,
    };
    // A frame's own stroke is painted after its contents and outside its clip, so
    // an inside stroke reads as a border and the clip does not cut a centred or
    // outside stroke. The opacity then moves to an outer group so the frame still
    // composites as one unit.
    let frame_stroke = matches!(painted.kind, NodeKind::Artboard { .. })
        && painted.paint.strokes.iter().any(|s| s.visible);
    // The effect layer is the outermost, in two load-bearing positions: outside the
    // clip, so a clipping frame's drop shadow is not cut off, and outside the
    // opacity, so a half-transparent layer casts a half-transparent shadow.
    let effects = doc.resolved_effects(
        id,
        over.and_then(|o| o.effects.as_deref())
            .unwrap_or(node.effects()),
    );
    let effects: &[ondin_core::Keyed<ondin_core::Effect>] = &effects;
    // `Some` exactly when a layer was pushed; a node with no cached bounds yet
    // draws unfiltered rather than not at all.
    let effect_layer = crate::effects::any_ink(effects)
        .then(|| effect_bounds(res, id, &painted, world, effects))
        .flatten();
    if let Some(ink) = effect_layer {
        painter.push_effect_layer(world, ink, effects);
    }

    let outer_opacity = frame_stroke && opacity < 1.0;
    if outer_opacity {
        painter.push_layer(world, None, ClipRule::NonZero, opacity);
    }
    let inner_opacity = if outer_opacity { 1.0 } else { opacity };
    let needs_layer = clip.is_some() || inner_opacity < 1.0;
    // The clip layer closes and reopens around a child leaving the frame
    // (`RenderOverrides::escape_clip`), which must draw unclipped yet in its own
    // z-place. A mask run closes and reopens with it, inside it (mask first out,
    // last in) or the pops unwind in the wrong order.
    let mut open = false;
    let mut mask = MaskRun::default();
    let reopen = |painter: &mut P, open: &mut bool, mask: &mut MaskRun| {
        if needs_layer && !*open {
            painter.push_layer(shape_world, clip.as_ref(), ClipRule::NonZero, inner_opacity);
            *open = true;
        }
        mask.reopen(painter, world);
    };
    // Ends the mask run, first compositing an alpha mask's ink inside its layer.
    let end_run = |painter: &mut P, mask: &mut MaskRun| {
        if let Some(id) = mask.alpha_to_composite() {
            painter.push_mask_layer();
            paint_node(doc, res, ov, id, world, view, base, painter);
            painter.pop_layer();
        }
        mask.close(painter);
    };
    let close = |painter: &mut P, open: &mut bool, mask: &mut MaskRun| {
        end_run(painter, mask);
        if *open {
            painter.pop_layer();
            *open = false;
        }
    };
    reopen(painter, &mut open, &mut mask);

    paint_shape(doc, &painted, shape_world, painter);

    // A boolean's children are its inputs, not artwork: the walk stops here.
    if matches!(painted.kind, NodeKind::Boolean { .. }) {
        close(painter, &mut open, &mut mask);
        debug_assert!(!outer_opacity, "only a frame opens the outer opacity layer");
        // Popped here, not in `close`, which the children loop also calls while the
        // node's effect layer must stay open; leaking it hides every later layer.
        if effect_layer.is_some() {
            painter.pop_layer();
        }
        return;
    }

    // Children and ghosts in one z-order: a ghost draws at its `InsertSubtree`
    // index.
    let mut ghosts: Vec<&crate::renderer::Ghost> = ov.ghosts_of(id).collect();
    ghosts.sort_by_key(|g| g.index);
    let mut next_ghost = 0usize;
    // The preview's order where it reorders this parent.
    let children = ov.children_of(id).unwrap_or(node.children());
    // `..=len` so anything indexed at or past the end still gets drawn.
    for pos in 0..=children.len() {
        while let Some(ghost) = ghosts.get(next_ghost).filter(|g| g.index <= pos) {
            if ov.ghosts_escape_clip() {
                close(painter, &mut open, &mut mask);
            } else {
                reopen(painter, &mut open, &mut mask);
            }
            paint_ghost(doc, ov, &ghost.node, world, base, painter);
            next_ghost += 1;
        }
        let Some(child) = children.get(pos) else {
            break;
        };
        // A mask is not artwork: it is not descended into and contributes the clip
        // every later sibling draws under.
        if let Some(node) = doc.get(*child)
            && node.mask()
        {
            end_run(painter, &mut mask);
            mask.kind = match node.mask_mode() {
                // No geometry means no clip: the run draws unmasked.
                MaskMode::Shape => mask_geometry(doc, res, ov, *child).map(RunKind::Clip),
                // An empty alpha mask legitimately erases everything above it; a
                // hidden one masks nothing.
                MaskMode::Alpha => visible(ov, doc, *child).then_some(RunKind::Alpha(*child)),
            };
            continue;
        }
        if ov.escapes_clip(*child) {
            close(painter, &mut open, &mut mask);
        } else {
            reopen(painter, &mut open, &mut mask);
        }
        paint_node(doc, res, ov, *child, world, view, base, painter);
    }

    close(painter, &mut open, &mut mask);
    if frame_stroke && let Some(frame) = geometry::local_path(painted.kind) {
        let box_ = frame.bounding_box();
        // `shape_world`, not `world`: the border must run along the box the
        // background filled, or snapping opens a half-pixel gap.
        paint_strokes(
            doc,
            painter,
            shape_world,
            painted.kind,
            &painted.paint,
            &frame,
            box_,
        );
    }
    if outer_opacity {
        painter.pop_layer();
    }
    if effect_layer.is_some() {
        painter.pop_layer();
    }
}

/// The ink box an effect layer is allocated from, in the node's **own** space.
///
/// `Resolved` caches it in world space, and a backend allocates from `base *
/// transform * bounds`, so a world rect would apply the world transform twice.
///
/// Built from what is being *drawn*, never `Resolved::ink_bounds` (§15 D341),
/// which is stale for a whole drag; `assert_preview_matches_commit` holds preview
/// and commit to the same box. A container's children are the one cached input,
/// so a preview moving a child *inside* a blurred group is a frame behind.
fn effect_bounds(
    res: &Resolved,
    id: NodeId,
    painted: &Painted<'_>,
    world: Affine,
    effects: &[ondin_core::Keyed<ondin_core::Effect>],
) -> Option<Rect> {
    let base = match painted.kind {
        NodeKind::Group | NodeKind::Root => res.inner_ink(id)?,
        // Includes the stroke expansion; declines a `Boolean`, which falls back
        // to the cache.
        _ => ondin_core::geometry::world_bounds_of_parts(
            painted.kind,
            &painted.paint,
            world,
            painted.text,
        )
        .or_else(|| res.inner_ink(id))?,
    };
    let ink = ondin_core::effect::escaped(base, ondin_core::effect::stack_escape(effects), world);
    Some(world.inverse().transform_rect_bbox(ink))
}

/// The world box a ghost's own ink covers: [`effect_bounds`]'s answer for a node
/// `Resolved` has never seen. A container is its children's boxes grown by their
/// stacks, a boolean uses `GhostNode::path`, anything else is
/// [`ondin_core::geometry::world_bounds_of_parts`]. Hidden or masking children
/// contribute nothing.
fn ghost_ink(ghost: &GhostNode, world: Affine) -> Option<Rect> {
    match &ghost.kind {
        NodeKind::Group | NodeKind::Root => ghost
            .children
            .iter()
            .filter(|c| c.visible && !c.mask)
            .filter_map(|c| {
                let child_world = world * c.transform;
                let base = ghost_ink(c, child_world)?;
                Some(ondin_core::effect::escaped(
                    base,
                    ondin_core::effect::stack_escape(&c.effects),
                    child_world,
                ))
            })
            .reduce(|a, b| a.union(b)),
        NodeKind::Boolean { .. } => ghost
            .path
            .as_ref()
            .filter(|p| !p.elements().is_empty())
            .map(|p| (world * p.clone()).bounding_box()),
        kind => ondin_core::geometry::world_bounds_of_parts(
            kind,
            &ghost.paint,
            world,
            ghost.text.as_ref(),
        ),
    }
}

/// Draw a ghost subtree: [`paint_node`]'s walk with children inline and no
/// culling. `doc` supplies the image table that framing reads.
fn paint_ghost<P: ScenePainter>(
    doc: &Document,
    ov: &RenderOverrides,
    ghost: &GhostNode,
    parent_world: Affine,
    base: Affine,
    painter: &mut P,
) {
    if !ghost.visible {
        return;
    }
    // Not in the document yet, so a bound paint reads in the default mode.
    let paint = doc.resolved_paint(ghost.id, &ghost.paint);
    let world = parent_world * ghost.transform;
    // Snaps as the node it will become, or the shape twitches on release; the
    // chrome ghost takes the node arm's exemption.
    let shape_world = match snappable_box(&ghost.kind).filter(|_| !ov.unsnapped(ghost.id)) {
        Some(size) => snapped_box_world(world, base, size),
        None => world,
    };
    let clip = match &ghost.kind {
        NodeKind::Artboard { .. } if ghost.clip => geometry::local_path(&ghost.kind),
        _ => None,
    };
    // The same two-layer arrangement as `paint_node`.
    let frame_stroke = matches!(ghost.kind, NodeKind::Artboard { .. })
        && ghost.paint.strokes.iter().any(|s| s.visible);
    // Measured via [`ghost_ink`], not `local_path`, which answers `None` for
    // groups, text and booleans and omits the stroke expansion.
    let ghost_effects = crate::effects::any_ink(&ghost.effects)
        .then(|| ghost_ink(ghost, world))
        .flatten()
        .map(|base| {
            let ink = ondin_core::effect::escaped(
                base,
                ondin_core::effect::stack_escape(&ghost.effects),
                world,
            );
            world.inverse().transform_rect_bbox(ink)
        });
    if let Some(ink) = ghost_effects {
        painter.push_effect_layer(world, ink, &ghost.effects);
    }

    let outer_opacity = frame_stroke && ghost.opacity < 1.0;
    if outer_opacity {
        painter.push_layer(world, None, ClipRule::NonZero, ghost.opacity);
    }
    let inner_opacity = if outer_opacity { 1.0 } else { ghost.opacity };
    let needs_layer = clip.is_some() || inner_opacity < 1.0;
    if needs_layer {
        painter.push_layer(shape_world, clip.as_ref(), ClipRule::NonZero, inner_opacity);
    }
    paint_shape(
        doc,
        &Painted {
            kind: &ghost.kind,
            paint: std::borrow::Cow::Borrowed(&*paint),
            // A `GhostNode` has no rule field; an even-odd ghost would need one.
            rule: ClipRule::NonZero,
            text: ghost.text.as_ref(),
            path: ghost.path.as_ref(),
            // Never for a ghost: the original underneath carries the mark.
            placeholder: None,
        },
        shape_world,
        painter,
    );
    // A ghost boolean's children are its inputs too: the walk stops, as in
    // `paint_node`.
    if matches!(ghost.kind, NodeKind::Boolean { .. }) {
        if needs_layer {
            painter.pop_layer();
        }
        // `ghost_ink` reads `GhostNode::path` for a boolean, so `ghost_effects`
        // can be `Some` here and both pops are load-bearing.
        if outer_opacity {
            painter.pop_layer();
        }
        if ghost_effects.is_some() {
            painter.pop_layer();
        }
        return;
    }
    // A ghost masks its own children; nothing leaves a ghost, so no `escape_clip`.
    let mut mask = MaskRun::default();
    let end_run = |painter: &mut P, mask: &mut MaskRun, ghost: &GhostNode| {
        if mask.alpha_to_composite().is_some() {
            painter.push_mask_layer();
            paint_ghost(doc, ov, ghost, world, base, painter);
            painter.pop_layer();
        }
        mask.close(painter);
    };
    // The ghost the open run is masked by; a ghost forest is not looked up by id.
    let mut alpha_ghost: Option<&GhostNode> = None;
    for child in &ghost.children {
        if child.mask {
            if let Some(g) = alpha_ghost.take() {
                end_run(painter, &mut mask, g);
            } else {
                mask.close(painter);
            }
            mask.kind = match child.mask_mode {
                MaskMode::Shape => ghost_mask_geometry(child).map(RunKind::Clip),
                // `alpha_ghost` must stay `None` for a hidden mask, or `end_run`
                // pushes the `DestIn` layer this arm declined.
                MaskMode::Alpha if child.visible => {
                    alpha_ghost = Some(child);
                    Some(RunKind::Alpha(child.id))
                }
                MaskMode::Alpha => None,
            };
            continue;
        }
        mask.reopen(painter, world);
        paint_ghost(doc, ov, child, world, base, painter);
    }
    if let Some(g) = alpha_ghost.take() {
        end_run(painter, &mut mask, g);
    } else {
        mask.close(painter);
    }
    if needs_layer {
        painter.pop_layer();
    }
    if frame_stroke && let Some(frame) = geometry::local_path(&ghost.kind) {
        let box_ = frame.bounding_box();
        paint_strokes(doc, painter, shape_world, &ghost.kind, &paint, &frame, box_);
    }
    if outer_opacity {
        painter.pop_layer();
    }
    if ghost_effects.is_some() {
        painter.pop_layer();
    }
}

/// One glyph run, in one brush.
///
/// Unrotated type is one [`ScenePainter::draw_text`]; a bent run is one call per
/// glyph under its own transform, since `draw_text` carries one transform per run.
/// A per-glyph transform moves the brush with the glyph, so a bent run under
/// anything but a solid is filled as an outline (`text::run_outline`) instead, or
/// every letter would restart the gradient.
fn draw_run(
    painter: &mut impl ScenePainter,
    world: Affine,
    run: &ondin_core::text::GlyphRun,
    brush: &Brush,
    framing: Option<Framing>,
) {
    if run.glyphs.iter().any(|g| g.rot != 0.0) && !matches!(brush, Brush::Solid(_)) {
        // Non-zero, the font's convention; even-odd would fill every `o`'s counter.
        painter.fill_path(
            world,
            &ondin_core::text::run_outline(run),
            brush,
            framing,
            ClipRule::NonZero,
        );
        return;
    }
    if run.glyphs.iter().all(|g| g.rot == 0.0) {
        painter.draw_text(
            world,
            &TextRun {
                font: &run.font,
                font_size: run.font_size,
                coords: &run.coords,
                brush,
                framing,
                glyphs: &run.glyphs,
            },
        );
        return;
    }
    for g in &run.glyphs {
        // The placement moves into the transform: a rotation applies *about* the
        // glyph's origin, or it would swing around the node's.
        let at = world
            * Affine::translate((f64::from(g.x), f64::from(g.y)))
            * Affine::rotate(f64::from(g.rot));
        let one = [ondin_core::text::Glyph {
            id: g.id,
            x: 0.0,
            y: 0.0,
            rot: 0.0,
        }];
        painter.draw_text(
            at,
            &TextRun {
                font: &run.font,
                font_size: run.font_size,
                coords: &run.coords,
                brush,
                framing,
                glyphs: &one,
            },
        );
    }
}

/// The filled outline of one decoration band, in the text node's local space.
///
/// Built here (parley draws no decoration) and filled rather than stroked, so all
/// four styles share one path (§6.3). Periods are multiples of the thickness.
/// Skip-ink is interval arithmetic on `DecorationInk::gaps`: dash and dot runs are
/// generated over the whole band then clipped, and the ribbon's phase is the
/// band's, so the rhythm survives a gap. Public for the SVG writer's outlined text.
pub fn decoration_path(ink: &ondin_core::DecorationInk) -> BezPath {
    use ondin_core::LineStyle;
    let band = ink.band;
    let t = band.height();
    if t <= 0.0 || band.width() <= 0.0 {
        return BezPath::new();
    }
    let kept = kept_spans(band, &ink.gaps);
    match ink.style {
        LineStyle::Solid => bars(band, &kept),
        LineStyle::Dashed => bars(band, &overlap(&dash_spans(band, t * 3.0, t * 2.0), &kept)),
        LineStyle::Dotted => bars(band, &overlap(&dash_spans(band, t, t), &kept)),
        LineStyle::Wavy => {
            let mut path = BezPath::new();
            for &(x0, x1) in &kept {
                wave(band, x0, x1, &mut path);
            }
            path
        }
    }
}

/// What is left of `band`'s width once the skipped spans are removed.
///
/// `gaps` arrives sorted and merged (core's `crossings`). A plain complement:
/// fragment width and gap air are decided once, in `ondin_core::text::crossings`.
fn kept_spans(band: Rect, gaps: &[(f64, f64)]) -> Vec<(f64, f64)> {
    if gaps.is_empty() {
        return vec![(band.x0, band.x1)];
    }
    let mut kept = Vec::with_capacity(gaps.len() + 1);
    let mut x = band.x0;
    for &(a, b) in gaps {
        if a > x {
            kept.push((x, a.min(band.x1)));
        }
        x = x.max(b);
    }
    if x < band.x1 {
        kept.push((x, band.x1));
    }
    kept
}

/// The overlap of two sorted, non-overlapping span lists.
fn overlap(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            out.push((lo, hi));
        }
        // Retire whichever span ends first; the other may still meet the next one.
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// One band-height rectangle per span.
fn bars(band: Rect, spans: &[(f64, f64)]) -> BezPath {
    let mut path = BezPath::new();
    for &(x0, x1) in spans {
        path.extend(
            Rect::new(x0, band.y0, x1, band.y1)
                .to_path(TOLERANCE)
                .iter(),
        );
    }
    path
}

/// `on`-long marks separated by `off`-long gaps, filling `band` from its left
/// edge. The final mark is clipped to the band rather than overhanging it.
fn dash_spans(band: Rect, on: f64, off: f64) -> Vec<(f64, f64)> {
    let period = on + off;
    if period <= 0.0 {
        return vec![(band.x0, band.x1)];
    }
    let mut spans = Vec::new();
    let mut x = band.x0;
    while x < band.x1 {
        spans.push((x, (x + on).min(band.x1)));
        x += period;
    }
    spans
}

/// A sine ribbon along `from..to` of `band`, appended to `path`: a polyline offset
/// half a thickness each way. One wavelength is six thicknesses, amplitude one.
///
/// The phase is measured from `band.x0`, not `from`, so the ribbon stays in step
/// across gaps. Its reach is `1.5t` from the band's middle, the number
/// `text::ink_extent` in core measures gaps against;
/// `tests::a_wavy_ribbon_stays_inside_the_reach_core_assumes` keeps them in step.
/// Plain backticks, not a link: the target is in a `#[cfg(test)]` module.
fn wave(band: Rect, from: f64, to: f64, path: &mut BezPath) {
    let t = band.height();
    let width = to - from;
    if width <= 0.0 {
        return;
    }
    let wavelength = t * 6.0;
    let amplitude = t;
    let mid = band.center().y;
    let steps = ((width / (wavelength / 8.0)).ceil() as usize).clamp(4, 4096);
    let dx = width / steps as f64;
    let at = |i: usize| {
        let x = from + dx * i as f64;
        let phase = (x - band.x0) / wavelength * std::f64::consts::TAU;
        (x, mid + phase.sin() * amplitude)
    };
    let (x0, y0) = at(0);
    path.move_to((x0, y0 - t * 0.5));
    for i in 1..=steps {
        let (x, y) = at(i);
        path.line_to((x, y - t * 0.5));
    }
    for i in (0..=steps).rev() {
        let (x, y) = at(i);
        path.line_to((x, y + t * 0.5));
    }
    path.close_path();
}

fn paint_shape<P: ScenePainter>(
    doc: &Document,
    painted: &Painted<'_>,
    world: Affine,
    painter: &mut P,
) {
    // A frame's fills, behind its children. Its strokes paint over the children
    // and outside the clip, which only `paint_node` can do, so a frame cannot fall
    // through to the generic tail. This loop is the tail's; if one grows a rule the
    // other must.
    if let NodeKind::Artboard { size } = painted.kind {
        let frame = Rect::new(0.0, 0.0, size.width, size.height);
        let path = frame.to_path(TOLERANCE);
        for fill in painted.paint.fills.iter().filter(|f| f.visible) {
            let framing = framing_of(doc, fill.brush.drawn(), frame);
            fill_or_placeholder(
                painter,
                world,
                &path,
                fill.brush.drawn(),
                framing,
                ClipRule::NonZero,
            );
        }
        return;
    }

    // Text: draw the glyph runs `Resolved` already shaped (§5.9; never shape during
    // the walk), one pass per visible fill.
    if let NodeKind::Text { .. } = painted.kind {
        let Some(layout) = painted.text else { return };
        let fallback = Brush::Solid(ondin_core::peniko::Color::BLACK);
        let inked = painted.paint.strokes.iter().any(|s| s.visible);
        let mut inks: Vec<&Brush> = painted
            .paint
            .fills
            .iter()
            .filter(|f| f.visible)
            .map(|f| f.brush.drawn())
            .collect();
        // Black only for a node with no ink at all: outlined type has a stroke and
        // no fill and must stay hollow.
        if inks.is_empty() && !inked {
            inks.push(&fallback);
        }
        // A run with its own colour draws once in it, else the node's whole fill
        // stack; paint order is per run (§15 D154).
        for run in &layout.runs {
            let own: Option<Brush> = run.color.map(Brush::Solid);
            let own_one = own.as_ref().map(|b| [b]);
            let run_inks: &[&Brush] = match &own_one {
                Some(one) => one,
                None => &inks,
            };
            for brush in run_inks {
                // The frame is the whole text box, not the run's advance.
                let framing = framing_of(doc, brush, layout.bounds());
                clipped(painter, world, framing, |p| {
                    draw_run(p, world, run, brush, framing);
                });
            }
        }
        // Type strokes along its glyph outlines (`ondin_core::text::outline`) in the
        // text box's frame; decoration bands are not stroked.
        if inked {
            let outline = ondin_core::text::outline(layout);
            if !outline.is_empty() {
                paint_strokes(
                    doc,
                    painter,
                    world,
                    painted.kind,
                    &painted.paint,
                    &outline,
                    layout.bounds(),
                );
            }
        }
        // Decorations draw after the glyphs; an empty `gaps` (skip-ink off) paints
        // across descenders.
        for ink in &layout.decorations {
            let path = decoration_path(ink);
            // Built flat, then only the finished outline is warped (type on a path).
            let path = match &layout.warp {
                Some(warp) => warp.warp(&path),
                None => path,
            };
            // The decoration's colour, else the first ink (not one per fill, which
            // would double its opacity); black for outlined type.
            let own = ink.color.map(Brush::Solid);
            let brush = own.as_ref().or(inks.first().copied()).unwrap_or(&fallback);
            let framing = framing_of(doc, brush, layout.bounds());
            // Non-zero: the text node's own rule is read by its mask and hit test.
            clipped(painter, world, framing, |p| {
                p.fill_path(world, &path, brush, framing, ClipRule::NonZero)
            });
        }
        return;
    }

    // A boolean the arithmetic gave up on draws the missing-picture placeholder over
    // the operands' box (`boolean_placeholder`, which hit testing also asks),
    // instead of its own paint.
    if let Some(area) = painted.placeholder {
        paint_missing(painter, world, &area.to_path(TOLERANCE));
        return;
    }

    let path = match painted.path {
        Some(derived) => derived.clone(),
        None => match local_path(painted.kind) {
            Some(path) => path,
            None => return,
        },
    };

    // Fills then strokes, in list order (the lists are the z-order). A line has no
    // interior to fill. The image frame is this path's box, shared by both.
    let frame = path.bounding_box();
    if !matches!(painted.kind, NodeKind::Line { .. }) {
        for fill in painted.paint.fills.iter().filter(|f| f.visible) {
            let framing = framing_of(doc, fill.brush.drawn(), frame);
            fill_or_placeholder(
                painter,
                world,
                &path,
                fill.brush.drawn(),
                framing,
                painted.rule,
            );
        }
    }

    paint_strokes(
        doc,
        painter,
        world,
        painted.kind,
        &painted.paint,
        &path,
        frame,
    );
}

/// Draw every visible stroke of `paint` on `shape`, in list order, building
/// inside/outside alignment out of a centred stroke.
///
/// No 2D backend has stroke alignment, so: stroke at **twice** the width and clip
/// away the half not wanted. One clip per run of consecutive same-aligned strokes
/// (runs, not a sort: list order is the z-order, §5.3); the run's ring is padded
/// for its widest reach (see [`outer_bounds`]).
///
/// `shape` is both stroked and the clip's source, even for a per-side stroke.
/// Alignment applies only where the kind has an interior
/// (`geometry::stroke_align_applies`, asked of the kind since a side path is
/// open). `frame` is what an image brush is framed in: the shape's box, not the
/// ink's, and for outlined type the text box.
fn paint_strokes<P: ScenePainter>(
    doc: &Document,
    painter: &mut P,
    world: Affine,
    kind: &NodeKind,
    paint: &Paint,
    shape: &BezPath,
    frame: Rect,
) {
    let visible: Vec<&Stroke> = paint
        .strokes
        .iter()
        .filter(|s| s.visible)
        .map(|s| &s.value)
        .collect();
    if visible.is_empty() {
        return;
    }
    let closed = geometry::stroke_align_applies(kind);
    let align_of = |s: &Stroke| if closed { s.align } else { StrokeAlign::Center };

    let mut at = 0usize;
    while at < visible.len() {
        let align = align_of(visible[at]);
        let mut end = at + 1;
        while end < visible.len() && align_of(visible[end]) == align {
            end += 1;
        }
        let run = &visible[at..end];
        at = end;

        if align == StrokeAlign::Center {
            for s in run {
                let framing = framing_of(doc, s.brush.drawn(), frame);
                clipped(painter, world, framing, |p| {
                    lay_sides(p, world, kind, s, shape, framing, false)
                });
            }
            continue;
        }
        // "Everything but the shape": it plus a big rect, taken even-odd.
        let complement = (align == StrokeAlign::Outside).then(|| {
            let reach = run
                .iter()
                .map(|s| {
                    miter_reach(
                        geometry::effective_sides(kind, s).max_width(s.width),
                        s.miter_limit,
                    )
                })
                .fold(0.0, f64::max);
            let mut path = outer_bounds(shape, reach).to_path(TOLERANCE);
            path.extend(shape.iter());
            path
        });
        let rule = match align {
            StrokeAlign::Outside => ClipRule::EvenOdd,
            _ => ClipRule::NonZero,
        };
        painter.push_layer(world, Some(complement.as_ref().unwrap_or(shape)), rule, 1.0);
        for s in run {
            let framing = framing_of(doc, s.brush.drawn(), frame);
            clipped(painter, world, framing, |p| {
                lay_sides(p, world, kind, s, shape, framing, true)
            });
        }
        painter.pop_layer();
    }
}

/// One stroke's ink: the whole outline, or one open path per side it occupies.
///
/// `doubled` is the aligned construction's twice-width stroke.
fn lay_sides<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    kind: &NodeKind,
    s: &Stroke,
    shape: &BezPath,
    framing: Option<Framing>,
    doubled: bool,
) {
    match geometry::effective_sides(kind, s).per_side(s.width) {
        None => lay_dashed(painter, world, shape, s, s.width, framing, doubled),
        Some(sides) => {
            for (side, width) in sides {
                let Some(edge) = geometry::side_path(kind, side) else {
                    continue;
                };
                lay_dashed(painter, world, &edge, s, width, framing, doubled);
            }
        }
    }
}

/// Lay `s` along `path` at `width`, its dash pattern resolved against the path and
/// the **nominal** width, not the doubled one. A fitted pattern on a multi-subpath
/// path is laid subpath by subpath (`geometry::dash_fit_pieces`).
fn lay_dashed<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    path: &BezPath,
    s: &Stroke,
    width: f64,
    framing: Option<Framing>,
    doubled: bool,
) {
    let laid = if doubled { width * 2.0 } else { width };
    let paint = |dashes: Vec<f64>| StrokePaint {
        brush: s.brush.drawn(),
        framing,
        width: laid,
        join: s.join,
        cap: s.cap,
        miter_limit: s.miter_limit,
        dashes,
        dash_offset: s.dash_offset,
    };
    match geometry::dash_fit_pieces(s, width, path) {
        Some(pieces) => {
            for (sub, dashes) in pieces {
                lay_stroke(painter, world, &sub, &paint(dashes));
            }
        }
        None => {
            let dashes = geometry::resolved_dashes(s, width, path);
            lay_stroke(painter, world, path, &paint(dashes));
        }
    }
}

/// Lay a stroke's ink along `path`: normally by asking the backend to stroke it,
/// but for ink so wide that stroking comes out wrong, by expanding the outline and
/// filling the result.
///
/// A very wide stroke leaves a hole in its own ink: the inner offset of a closed
/// contour turns inside out and nets to winding zero, and `kurbo::stroke` inverts
/// in the same place as the backend. So the expansion is taken apart: keep the
/// outer contours, add the outline's own interior, wind all alike, fill non-zero.
///
/// Gated, since this gives up the backend's expander: the ink must be at least as
/// wide as the shape is thin, which fires before the bug starts, never after.
/// `ui::MAX_STROKE_WIDTH` is 10 000, so such widths must work. Closed paths only
/// (read off the path: per-side strokes lie along open edges of a closed kind) and
/// solid strokes only (the plug would fill a dashed shape solid).
fn lay_stroke<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    path: &BezPath,
    paint: &StrokePaint<'_>,
) {
    let box_ = path.bounding_box();
    let closed = matches!(path.elements().last(), Some(kurbo::PathEl::ClosePath));
    if !closed || paint.width < box_.width().min(box_.height()) || !paint.dashes.is_empty() {
        painter.stroke_path(world, path, paint);
        return;
    }
    let style = kurbo::Stroke {
        width: paint.width,
        join: paint.join,
        miter_limit: paint.miter_limit,
        start_cap: paint.cap,
        end_cap: paint.cap,
        dash_pattern: paint.dashes.iter().copied().collect(),
        dash_offset: paint.dash_offset,
    };
    let expanded = kurbo::stroke(path, &style, &kurbo::StrokeOpts::default(), TOLERANCE);
    let mut ink = outermost_subpaths(&expanded);
    // Both halves are load-bearing: the outline's interior goes in, and every
    // contour is wound alike. The expansion alone reproduces the hole, and
    // flo_curves interior removal or union drops ink at some widths.
    ink.extend(path.iter());
    // Non-zero, not the node's rule: `wound_alike` arranged the winding so a
    // non-zero fill reads one solid band.
    let ink = wound_alike(&ink);
    painter.fill_path(world, &ink, paint.brush, paint.framing, ClipRule::NonZero);
}

/// `path` with every subpath another subpath's box swallows thrown away.
///
/// [`lay_stroke`] wants only an expansion's outer contours: a far-inset inner one
/// becomes a figure-eight with both windings, which [`wound_alike`] cannot
/// correct. Boxes are coarse and enough, since the contours are concentric.
fn outermost_subpaths(path: &BezPath) -> BezPath {
    let subs = geometry::subpaths(path);
    let boxes: Vec<Rect> = subs.iter().map(|s| s.bounding_box()).collect();
    let swallows = |big: &Rect, small: &Rect| {
        big.area() > small.area()
            && big.x0 <= small.x0
            && big.y0 <= small.y0
            && big.x1 >= small.x1
            && big.y1 >= small.y1
    };
    let mut out = BezPath::new();
    for (i, sub) in subs.iter().enumerate() {
        if !boxes
            .iter()
            .enumerate()
            .any(|(j, b)| j != i && swallows(b, &boxes[i]))
        {
            out.extend(sub.iter());
        }
    }
    out
}

/// `path`'s subpaths, every one wound positively, so a non-zero fill is the union
/// of the regions they enclose. It cannot express a hole.
fn wound_alike(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut sub = BezPath::new();
    // `Shape::area` is signed, so its sign *is* the direction.
    let flush = |sub: &mut BezPath, out: &mut BezPath| {
        if sub.elements().is_empty() {
            return;
        }
        if sub.area() < 0.0 {
            out.extend(sub.reverse_subpaths().iter());
        } else {
            out.extend(sub.iter());
        }
        *sub = BezPath::new();
    };
    for el in path.elements() {
        if matches!(el, kurbo::PathEl::MoveTo(_)) {
            flush(&mut sub, &mut out);
        }
        sub.push(*el);
    }
    flush(&mut sub, &mut out);
    out
}

/// How far past the outline a stroke of `width` reaches at a mitre: `width · limit`
/// from the vertex (the doubled stroke lays `width` either side of the flat).
fn miter_reach(width: f64, miter_limit: f64) -> f64 {
    width * miter_limit.max(1.0)
}

/// A box containing `path` and any stroke on it, for the outside clip's outer
/// ring. `reach` (see [`miter_reach`]) is required: the ring must clear the
/// stroke, not the shape, or it bites chunks out of a wide outside stroke.
fn outer_bounds(path: &BezPath, reach: f64) -> Rect {
    let b = path.bounding_box();
    let pad = (b.width() + b.height()).max(1.0) + reach;
    b.inflate(pad, pad)
}

/// Local-space outline for a node kind. Defined in `ondin_core::geometry` so the
/// SVG writer clips aligned strokes against the exact path this walk strokes.
fn local_path(kind: &NodeKind) -> Option<BezPath> {
    geometry::local_path(kind)
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.min_x() <= b.max_x()
        && a.max_x() >= b.min_x()
        && a.min_y() <= b.max_y()
        && a.max_y() >= b.min_y()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{PathEl, Point};

    /// An outside stroke's clip ring encloses the widest mitre ink, not just the
    /// shape plus its own size.
    #[test]
    fn an_outside_clip_clears_the_widest_mitre_it_could_have_to() {
        let shape = Rect::new(0.0, 0.0, 142.0, 142.0).to_path(TOLERANCE);
        for (width, limit) in [
            (1.0, 4.0),
            (10.0, 4.0),
            (241.0, 4.0),
            (241.0, 1.0),
            (600.0, 8.0),
        ] {
            let reach = miter_reach(width, limit);
            let ring = outer_bounds(&shape, reach);
            let ink = shape.bounding_box().inflate(reach, reach);
            assert!(
                ring.contains(ink.origin()) && ring.contains(Point::new(ink.x1, ink.y1)),
                "width {width} limit {limit}: the ring {ring:?} has to enclose everything \
                 the stroke can paint ({ink:?}) or it clips its own ink"
            );
        }
    }

    /// The mitre allowance is `width · limit`, floored at `width` (kurbo ignores a
    /// limit below 1).
    #[test]
    fn the_mitre_allowance_is_the_limit_times_the_width() {
        assert_eq!(miter_reach(10.0, 4.0), 40.0);
        assert_eq!(miter_reach(10.0, 1.0), 10.0);
        assert_eq!(
            miter_reach(10.0, 0.25),
            10.0,
            "a limit under 1 cannot shrink it"
        );
    }

    // --- decoration bands and skip-ink -------------------------------------

    /// A band 100 long and 2 thick, middle at y 11: dash period 10, wavelength 12.
    fn band(style: ondin_core::LineStyle, gaps: Vec<(f64, f64)>) -> ondin_core::DecorationInk {
        ondin_core::DecorationInk {
            band: Rect::new(0.0, 10.0, 100.0, 12.0),
            style,
            color: None,
            gaps,
        }
    }

    /// The x range of every subpath in a path of axis-aligned rectangles, in order.
    fn bars_of(path: &BezPath) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for el in path {
            let p = match el {
                PathEl::MoveTo(p) => {
                    out.push((p.x, p.x));
                    continue;
                }
                PathEl::LineTo(p) => p,
                _ => continue,
            };
            if let Some(last) = out.last_mut() {
                last.0 = last.0.min(p.x);
                last.1 = last.1.max(p.x);
            }
        }
        out
    }

    /// A gap is a hole in the band, not a shortened band.
    ///
    /// flip: `kept_spans` returns the whole band → fails at the two-bars length.
    #[test]
    fn a_skipped_span_leaves_a_hole_in_a_solid_band() {
        let ink = band(ondin_core::LineStyle::Solid, vec![(40.0, 60.0)]);
        let bars = bars_of(&decoration_path(&ink));
        assert_eq!(bars.len(), 2, "a hole makes two bars: {bars:?}");
        assert_eq!(bars[0], (0.0, 40.0));
        assert_eq!(bars[1], (60.0, 100.0));
        // A band with nothing in its way is whole.
        let whole = bars_of(&decoration_path(&band(
            ondin_core::LineStyle::Solid,
            vec![],
        )));
        assert_eq!(whole, vec![(0.0, 100.0)]);
    }

    /// A sliver fragment is drawn: whether it is worth drawing is core's call
    /// (`ondin_core::text::crossings`, §15 D356), so this pins the boundary.
    ///
    /// flip: a width `retain` in `kept_spans` → fails at the `(18.0, 19.5)` bar.
    #[test]
    fn a_sliver_is_drawn_because_the_judgement_is_cores() {
        let ink = band(
            ondin_core::LineStyle::Solid,
            vec![(10.0, 18.0), (19.5, 30.0), (33.0, 90.0)],
        );
        let bars = bars_of(&decoration_path(&ink));
        assert_eq!(
            bars,
            vec![(0.0, 10.0), (18.0, 19.5), (30.0, 33.0), (90.0, 100.0)],
            "every gap is a hole and every hole's complement is a bar: {bars:?}"
        );
    }

    /// The dash phase belongs to the band, not the fragment: a dash `20..26` cut by
    /// a gap `13..22` resumes as `22..26`, not a fresh dash at the gap's edge.
    ///
    /// flip: generate `dash_spans` per kept fragment → fails at the `22..26` bar
    /// (the right edge; the left is the gap boundary either way).
    #[test]
    fn a_dashed_band_keeps_the_bands_phase_across_a_gap() {
        let ink = band(ondin_core::LineStyle::Dashed, vec![(13.0, 22.0)]);
        let bars = bars_of(&decoration_path(&ink));
        assert!(
            bars.contains(&(10.0, 13.0)),
            "the dash the gap opens into is cut at the gap: {bars:?}"
        );
        assert!(
            bars.contains(&(22.0, 26.0)),
            "and the one it closes in resumes on the band's grid, not the gap's: \
             {bars:?}"
        );
        // The fixture is a dash run at all: 6 on, 4 off, ten periods.
        assert!(
            bars.contains(&(0.0, 6.0)) && bars.contains(&(90.0, 96.0)),
            "{bars:?}"
        );
    }

    /// The wave's phase belongs to the band too: a fragment starting at x 3 begins
    /// a quarter wavelength in, at the crest (upper edge 12).
    ///
    /// flip: measure the phase from `from` in `wave` → fails at the y 12 assertion
    /// (y comes back 10).
    #[test]
    fn a_wavy_band_keeps_the_bands_phase_across_a_gap() {
        let ink = band(ondin_core::LineStyle::Wavy, vec![(0.0, 3.0)]);
        let path = decoration_path(&ink);
        let Some(PathEl::MoveTo(start)) = path.elements().first().copied() else {
            panic!("a ribbon starts with a move: {:?}", path.elements().first());
        };
        assert!(
            (start.x - 3.0).abs() < 1e-9,
            "the fragment starts where the gap ended: {start:?}"
        );
        assert!(
            (start.y - 12.0).abs() < 0.01,
            "a quarter wavelength in is the crest, so the upper edge is 12: {start:?}"
        );
    }

    /// A wavy ribbon reaches exactly 1.5 thicknesses either side of the band's
    /// middle, the number `ondin_core::text`'s `ink_extent` cuts gaps against;
    /// tight in both directions.
    #[test]
    fn a_wavy_ribbon_stays_inside_the_reach_core_assumes() {
        let ink = band(ondin_core::LineStyle::Wavy, vec![]);
        let bounds = decoration_path(&ink).bounding_box();
        let (mid, t) = (ink.band.center().y, ink.band.height());
        assert!(
            (bounds.y0 - (mid - 1.5 * t)).abs() < 0.05
                && (bounds.y1 - (mid + 1.5 * t)).abs() < 0.05,
            "the ribbon must reach 1.5 thicknesses either side of {mid} at t {t}, \
             and no further: {bounds:?}"
        );
    }
}
