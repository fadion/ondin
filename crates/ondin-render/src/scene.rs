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
//! - **A blend mode is the outermost layer**, outside the effect layer, pushed
//!   only when the node isolates (`BlendMode::isolates`) or is a top-level frame
//!   holding a blended layer.

use crate::renderer::{GhostNode, RenderOverrides, Viewport};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use ondin_core::Brush;
use ondin_core::text::{Glyph as TextGlyph, TextLayout};
use ondin_core::{
    BlendMode, Document, Effect, Framing, MaskMode, NodeId, NodeKind, Paint, Resolved, Stroke,
    StrokeAlign, geometry,
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

    /// [`Self::push_mask_layer`] reading the drawn ink's **luminance** rather than
    /// its alpha: what is there survives in proportion to the brightness drawn
    /// here times its alpha (sRGB weights 0.2125, 0.7154, 0.0721, vello's). The
    /// default masks by alpha; shipping backends override.
    fn push_luminance_mask_layer(&mut self) {
        self.push_mask_layer();
    }

    /// Begin a layer composited over the enclosing layer **as an eraser**: what is
    /// there survives in proportion to the alpha *not* drawn here
    /// (`Compose::DestOut`). The caller owes it an enclosing layer holding only
    /// what is to be erased, as [`Self::push_mask_layer`] does. The default
    /// composites plainly; shipping backends override.
    fn push_erase_layer(&mut self) {
        self.push_layer(Affine::IDENTITY, None, ClipRule::NonZero, 1.0);
    }

    /// Begin a layer composited over the enclosing layer **by adding** to it
    /// (`Compose::Plus`): what fills a hole [`Self::push_erase_layer`] left, in
    /// proportion to how far it was erased. The default composites plainly;
    /// shipping backends override.
    fn push_add_layer(&mut self) {
        self.push_layer(Affine::IDENTITY, None, ClipRule::NonZero, 1.0);
    }

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

    /// [`Self::push_effect_layer`] keeping only the shadows `effects` casts, the
    /// layer itself left out (`effects::run_shadows`): one pass of a layer whose
    /// shadows blend ([`shadow_passes`]). The default keeps the layer too.
    fn push_shadow_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        effects: &[ondin_core::Keyed<Effect>],
    ) {
        self.push_effect_layer(transform, bounds, effects);
    }

    /// Begin an unclipped layer composited over the enclosing one by `mode`
    /// ([`ondin_core::Node::blend`]). Normal is an isolation group. The default
    /// composites plainly; shipping backends override.
    fn push_blend_layer(&mut self, mode: ondin_core::BlendMode) {
        let _ = mode;
        self.push_layer(Affine::IDENTITY, None, ClipRule::NonZero, 1.0);
    }

    fn pop_layer(&mut self);

    /// The opaque colour the target holds before the walk draws, which a
    /// background blur at the top of the walk reads behind the artwork: the
    /// canvas's ground. `None` where any ground is laid after the drawing (every
    /// export), and the backdrop is then transparent where nothing is drawn.
    fn ground(&self) -> Option<ondin_core::peniko::Color> {
        None
    }

    /// Whether a background blur's backdrop is drawn through this painter
    /// ([`paint_backdrop`]). One measuring ink or tracing outlines says no: the
    /// backdrop lies inside the layer's own outline, and repainting what is
    /// behind it would measure or trace that twice.
    fn paints_backdrops(&self) -> bool {
        true
    }
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

/// [`build`] skipping what misses every rectangle of `cull`, world rectangles
/// inside the view: the walk for redrawing parts of a canvas. Everything else is
/// `vp`'s, so what is drawn inside `cull` is what [`build`] draws there.
pub fn build_within<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    page: NodeId,
    cull: &[Rect],
    painter: &mut P,
) {
    walk(doc, res, vp, overrides, &[page], cull, painter);
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
    walk(doc, res, vp, overrides, origins, &[vp.view], painter);
}

/// [`build_of`] culled to `cull` rather than the view.
fn walk<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    origins: &[NodeId],
    cull: &[Rect],
    painter: &mut P,
) {
    let base = device_base(vp);
    let edges = Edges::of(doc, res, overrides, origins, cull, base);
    let walk = Walk::new(origins);
    if let Some(ink) = overrides.outline() {
        let mut outline = OutlinePainter {
            inner: painter,
            ink: Brush::Solid(ink),
            base,
            erasing: Vec::new(),
        };
        walk_origins(doc, res, overrides, cull, base, &edges, &mut outline, &walk);
    } else {
        walk_origins(doc, res, overrides, cull, base, &edges, painter, &walk);
    }
}

/// Paint each of `walk`'s origins under its parent's world transform, stopping
/// once a backdrop's repaint has reached its layer.
#[allow(clippy::too_many_arguments)]
fn walk_origins<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    overrides: &RenderOverrides,
    cull: &[Rect],
    base: Affine,
    edges: &Edges<'_>,
    painter: &mut P,
    walk: &Walk<'_>,
) {
    for id in walk.origins {
        if walk.stopped() {
            return;
        }
        let outer = doc
            .get(*id)
            .and_then(|n| n.parent())
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        paint_node(
            doc, res, overrides, *id, outer, cull, base, edges, painter, walk,
        );
    }
}

/// What a walk carries beyond the view: the subtrees it started from and the
/// containers open above the node it is painting, which together say what a
/// background blur reads behind a layer ([`paint_backdrop`]).
struct Walk<'a> {
    origins: &'a [NodeId],
    /// Each container the walk is inside, outermost first.
    open: std::cell::RefCell<Vec<Open>>,
    /// How many shadow passes ([`shadow_passes`]) the walk is inside: a repaint
    /// draws those whole, since the shadow lies behind everything in the layer.
    in_shadow: std::cell::Cell<usize>,
    /// Set while the walk repaints a backdrop, which it stops short of.
    backdrop: Option<Backdrop>,
}

/// A container the walk is inside: the world transform it was painted under,
/// and whether it draws its children into a layer of its own that composites
/// as a whole (a blend, effect or opacity layer), which is where a backdrop
/// stops.
#[derive(Clone, Copy)]
struct Open {
    id: NodeId,
    parent_world: Affine,
    isolates: bool,
}

/// A backdrop being repainted: everything drawn before `upto`, from `bare` (the
/// nearest container that isolates, drawn without the layers that isolate it)
/// or from the walk's origins when none does.
struct Backdrop {
    upto: NodeId,
    bare: Option<NodeId>,
    reached: std::cell::Cell<bool>,
}

impl<'a> Walk<'a> {
    fn new(origins: &'a [NodeId]) -> Self {
        Self {
            origins,
            open: Default::default(),
            in_shadow: Default::default(),
            backdrop: None,
        }
    }

    /// Whether a backdrop's repaint has reached its layer, after which nothing
    /// more is drawn.
    fn stopped(&self) -> bool {
        self.backdrop.as_ref().is_some_and(|b| b.reached.get())
    }

    /// Whether `id` is the container a backdrop is repainted from.
    fn bare(&self, id: NodeId) -> bool {
        self.backdrop.as_ref().is_some_and(|b| b.bare == Some(id))
    }
}

/// Outline mode: the walk as it is, every path it fills or strokes drawn instead
/// as a one-device-pixel hairline of that path in `ink`, and type in `ink`.
///
/// **Every layer is dropped** — opacity, clip, mask and effect alike — so what a
/// frame clips or a mask hides is shown, which is what the view is for: finding
/// what the artwork covers. Dropped, not pushed plain, so a deep document pays no
/// offscreen buffers; a push forwards nothing and so a pop forwards nothing.
struct OutlinePainter<'a, P: ScenePainter> {
    inner: &'a mut P,
    ink: Brush,
    /// World → device pixels, to size the hairline in device pixels.
    base: Affine,
    /// One entry per open layer, true for an eraser: what an eraser fills is no
    /// artwork, so it draws no hairline.
    erasing: Vec<bool>,
}

impl<P: ScenePainter> OutlinePainter<'_, P> {
    fn hairline(&mut self, transform: Affine, path: &BezPath) {
        // Stroked in world space, the path mapped there first: `transform` may
        // scale unevenly (a sub-pixel box snapped to a pixel, a skew), and a
        // width under it would come out thicker on one axis than the other.
        // `base` is the camera's even scale, so a width under it is one width.
        let scale = (base_linear_det(self.base)).abs().sqrt();
        if !scale.is_finite() || scale <= 0.0 || base_linear_det(transform) == 0.0 {
            return;
        }
        let path = transform * path.clone();
        let stroke = StrokePaint {
            brush: &self.ink,
            framing: None,
            width: 1.0 / scale,
            join: kurbo::Join::Round,
            cap: kurbo::Cap::Butt,
            miter_limit: 4.0,
            dashes: Vec::new(),
            dash_offset: 0.0,
        };
        self.inner.stroke_path(Affine::IDENTITY, &path, &stroke);
    }
}

/// The determinant of `a`'s linear part — its area scale.
fn base_linear_det(a: Affine) -> f64 {
    let [m11, m12, m21, m22, _, _] = a.as_coeffs();
    m11 * m22 - m12 * m21
}

impl<P: ScenePainter> ScenePainter for OutlinePainter<'_, P> {
    fn has_image(&self, id: &ondin_core::ImageId) -> bool {
        self.inner.has_image(id)
    }
    fn fill_path(
        &mut self,
        transform: Affine,
        path: &BezPath,
        _brush: &Brush,
        _framing: Option<Framing>,
        _rule: ClipRule,
    ) {
        if !self.erasing.contains(&true) {
            self.hairline(transform, path);
        }
    }
    fn stroke_path(&mut self, transform: Affine, path: &BezPath, _stroke: &StrokePaint<'_>) {
        self.hairline(transform, path);
    }
    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>) {
        let run = TextRun {
            brush: &self.ink,
            framing: None,
            ..*run
        };
        self.inner.draw_text(transform, &run);
    }
    fn push_layer(&mut self, _: Affine, _: Option<&BezPath>, _: ClipRule, _: f32) {
        self.erasing.push(false);
    }
    fn push_mask_layer(&mut self) {
        self.erasing.push(false);
    }
    fn push_erase_layer(&mut self) {
        self.erasing.push(true);
    }
    fn push_add_layer(&mut self) {
        self.erasing.push(false);
    }
    fn push_effect_layer(&mut self, _: Affine, _: Rect, _: &[ondin_core::Keyed<Effect>]) {
        self.erasing.push(false);
    }
    fn pop_layer(&mut self) {
        self.erasing.pop();
    }
    fn paints_backdrops(&self) -> bool {
        false
    }
}

/// The device box of what the walk draws, never smaller: clips are not applied,
/// a stroke is grown by its widest mitre or cap, a glyph run by three ems, and an
/// effect layer counts its whole buffer box, escape and all.
struct BoundsPainter {
    /// World → device pixels.
    base: Affine,
    ink: Option<Rect>,
}

impl BoundsPainter {
    fn add(&mut self, transform: Affine, local: Rect) {
        let device = (self.base * transform).transform_rect_bbox(local);
        self.ink = Some(self.ink.map_or(device, |r| r.union(device)));
    }
}

impl ScenePainter for BoundsPainter {
    /// A missing picture's placeholder lies inside the same path.
    fn has_image(&self, _: &ondin_core::ImageId) -> bool {
        true
    }
    fn fill_path(&mut self, t: Affine, path: &BezPath, _: &Brush, _: Option<Framing>, _: ClipRule) {
        self.add(t, path.bounding_box());
    }
    fn stroke_path(&mut self, transform: Affine, path: &BezPath, stroke: &StrokePaint<'_>) {
        let reach = stroke.width / 2.0 * stroke.miter_limit.max(std::f64::consts::SQRT_2);
        self.add(transform, path.bounding_box().inflate(reach, reach));
    }
    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>) {
        let Some(first) = run.glyphs.first() else {
            return;
        };
        let at = |g: &TextGlyph| {
            Rect::new(
                f64::from(g.x),
                f64::from(g.y),
                f64::from(g.x),
                f64::from(g.y),
            )
        };
        let origins = run.glyphs.iter().fold(at(first), |r, g| r.union(at(g)));
        let em = 3.0 * f64::from(run.font_size);
        self.add(transform, origins.inflate(em, em));
    }
    fn push_layer(&mut self, _: Affine, _: Option<&BezPath>, _: ClipRule, _: f32) {}
    fn push_mask_layer(&mut self) {}
    fn push_effect_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        _: &[ondin_core::Keyed<Effect>],
    ) {
        self.add(transform, bounds);
    }
    fn pop_layer(&mut self) {}
    fn paints_backdrops(&self) -> bool {
        false
    }
}

/// The device box of everything each of `tops` — children of `page` — draws
/// under `overrides`, beside the box of every ghost hanging off the page: the
/// damage a preview can do there. `None` when the page's children do not draw
/// independently of each other: a mask among them clips the ones after it, and a
/// preview reordering them changes which covers which.
pub fn top_level_ink(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    page: NodeId,
    tops: &[NodeId],
) -> Option<Vec<Rect>> {
    let node = doc.get(page)?;
    let masked = |c: &NodeId| doc.get(*c).is_some_and(|n| n.mask());
    if overrides.children_of(page).is_some() || node.children().iter().any(masked) {
        return None;
    }
    let world = res.world_transform(page).unwrap_or(Affine::IDENTITY);
    let items = tops
        .iter()
        .map(|id| Top::Node(*id))
        .chain(overrides.ghosts_of(page).map(|g| Top::Ghost(&g.node)));
    Some(
        items
            .filter_map(|item| ink_of(doc, res, vp, overrides, item, world))
            .collect(),
    )
}

/// The children of `page` that are or hold each of `ids`, once each. The page
/// itself, nodes on other pages and nodes the document does not hold name none.
pub fn tops_of(doc: &Document, page: NodeId, ids: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
    // A moved frame names every node under it, so the walk up is a set's work.
    let mut seen = rustc_hash::FxHashSet::default();
    let mut tops: Vec<NodeId> = Vec::new();
    'ids: for id in ids {
        if !seen.insert(id) {
            continue;
        }
        let mut at = id;
        loop {
            let Some(parent) = doc.get(at).and_then(|n| n.parent()) else {
                continue 'ids;
            };
            if parent == page {
                break;
            }
            at = parent;
        }
        if at == id || seen.insert(at) {
            tops.push(at);
        }
    }
    tops
}

/// The device rectangles a frame drawn under `before` has to redraw to show
/// `after`, the document and the view standing still: what each top-level item
/// either preview touches ([`RenderOverrides::touched`]) draws under both. `None`
/// when that cannot be said ([`top_level_ink`]'s cases).
pub fn damage(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    page: NodeId,
    before: &RenderOverrides,
    after: &RenderOverrides,
) -> Option<Vec<Rect>> {
    // The page itself stands for its ghosts, which `top_level_ink` measures
    // whatever is named.
    let tops = tops_of(doc, page, before.touched().chain(after.touched()));
    let mut out = top_level_ink(doc, res, vp, before, page, &tops)?;
    out.extend(top_level_ink(doc, res, vp, after, page, &tops)?);
    Some(out)
}

/// `damage` (device boxes on `page`) with what it changes through a background
/// blur added: a change behind a glass shows in the glass as far as the blur
/// reaches, past the box of the change itself. Followed through glass behind
/// glass, one glass further each round.
pub fn spread_through_backdrops(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    page: NodeId,
    damage: Vec<Rect>,
) -> Vec<Rect> {
    let base = device_base(vp);
    let glass: Vec<(Rect, f64)> = ondin_core::build::subtree_nodes(doc, &[page])
        .into_iter()
        .filter_map(|id| {
            let node = doc.get(id).filter(|n| !n.mask())?;
            let stack = overrides
                .get(id)
                .and_then(|o| o.effects.as_deref())
                .unwrap_or(node.effects());
            let radius = ondin_core::effect::backdrop_radius(&doc.resolved_effects(id, stack))?;
            let world = res.world_transform(id)?;
            let ink = base.transform_rect_bbox(res.ink_bounds(id)?);
            Some((
                ink,
                ondin_core::effect::reach(radius) * stretch(base * world),
            ))
        })
        .collect();
    let mut out = damage.clone();
    let mut frontier = damage;
    for _ in 0..glass.len() {
        let mut next = Vec::new();
        for d in &frontier {
            for (ink, reach) in &glass {
                let hit = d.inflate(*reach, *reach).intersect(*ink);
                if hit.width() > 0.0
                    && hit.height() > 0.0
                    && !out.iter().chain(&next).any(|o| o.contains_rect(hit))
                {
                    next.push(hit);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        out.extend_from_slice(&next);
        frontier = next;
    }
    out
}

/// The device box of what `overrides` changes about `page`'s drawing: each
/// top-level item it touches and each ghost, as drawn under it. Empty for a
/// preview that touches nothing; `None` in [`top_level_ink`]'s cases.
pub fn preview_ink(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    page: NodeId,
) -> Option<Vec<Rect>> {
    let tops = tops_of(doc, page, overrides.touched());
    top_level_ink(doc, res, vp, overrides, page, &tops)
}

/// The device box `world` covers through `vp`.
pub fn device_box(vp: &Viewport, world: Rect) -> Rect {
    device_base(vp).transform_rect_bbox(world)
}

/// One thing a page draws among its children.
#[derive(Clone, Copy)]
enum Top<'a> {
    Node(NodeId),
    Ghost(&'a GhostNode),
}

/// [`BoundsPainter`]'s box for `item`, walked as [`walk`] walks it — through
/// outline mode's painter when that is on.
fn ink_of(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    ov: &RenderOverrides,
    item: Top<'_>,
    page_world: Affine,
) -> Option<Rect> {
    fn draw<P: ScenePainter>(
        doc: &Document,
        res: &Resolved,
        ov: &RenderOverrides,
        item: Top<'_>,
        (world, view, base): (Affine, &[Rect], Affine),
        painter: &mut P,
    ) {
        let edges = Edges::none();
        let walk = Walk::new(&[]);
        match item {
            Top::Node(id) => {
                paint_node(doc, res, ov, id, world, view, base, &edges, painter, &walk)
            }
            Top::Ghost(g) => paint_ghost(doc, ov, g, true, world, base, &edges, painter, None),
        }
    }
    let base = device_base(vp);
    let at = (page_world, &[vp.view][..], base);
    let mut bounds = BoundsPainter { base, ink: None };
    match ov.outline() {
        Some(ink) => {
            let mut outline = OutlinePainter {
                inner: &mut bounds,
                ink: Brush::Solid(ink),
                base,
                erasing: Vec::new(),
            };
            draw(doc, res, ov, item, at, &mut outline);
        }
        None => draw(doc, res, ov, item, at, &mut bounds),
    }
    bounds.ink
}

/// Whether axis-aligned boxes and centred strokes snap to whole device pixels
/// (see [`snapped_box_world`] and [`snapped_stroke`]).
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

/// Whether the centred strokes of node `id` of `kind` go through the
/// [`snapped_stroke`] rounding: not for a box, whose whole transform snaps
/// instead, nor type, nor a layer exempt from snapping
/// (`RenderOverrides::unsnapped`). Not in outline mode either, which traces a
/// stroke's centre line and would trace the snapped ink's rectangles instead.
fn strokes_snap(kind: &NodeKind, ov: &RenderOverrides, id: NodeId) -> bool {
    ov.outline().is_none()
        && !ov.unsnapped(id)
        && snappable_box(kind).is_none()
        && !matches!(kind, NodeKind::Text { .. })
}

/// The ink of the centred stroke `s` along `path`, in local space, with every
/// edge on a whole device pixel through `device`, to fill in the stroke's brush.
/// `None` unless every segment is a horizontal or vertical line, `device` scales
/// both axes alike, and the stroke is undashed with butt or square caps and,
/// where the path turns, mitred joins.
///
/// Built as one rectangle per segment, reaching over a join by half the width and
/// past an end by its cap, so a right-angle mitre comes out square. **Each edge
/// rounds on its own, as a box edge does** ([`snapped_box_world`]): a stroke and a
/// box ending at the same world edge end on the same pixel. Rounding a centre
/// line and a width instead puts one edge a pixel off whenever the width rounds,
/// leaving a sliver of stroke under a box that covered it.
///
/// A stroke under a pixel wide keeps its width, centred in the one row its edges
/// round into, so a zoomed-out grid of hairlines stays as light. When both edges
/// round to one pixel boundary, it is laid against that boundary on the side the
/// box edges rounding there put it ([`Edges`]), or where it lies when none does.
fn snapped_stroke(
    path: &BezPath,
    device: Affine,
    s: &Stroke,
    edges: &Edges<'_>,
) -> Option<BezPath> {
    use kurbo::{Cap, Join};
    if !SNAP_BOXES_TO_DEVICE_PIXELS
        || s.width.is_nan()
        || s.width <= 0.0
        || !s.dashes.is_empty()
        || s.cap == Cap::Round
    {
        return None;
    }
    // Device pixels within which two coordinates count as one.
    const SAME: f64 = 1e-3;
    let [a, b, c, d, _, _] = device.as_coeffs();
    let scale = a.abs();
    if b.abs() > 1e-9 || c.abs() > 1e-9 || scale < 1e-9 || (d.abs() - scale).abs() > 1e-9 * scale {
        return None;
    }
    let aligned = |p: Point, q: Point| (p.x - q.x).abs() < SAME || (p.y - q.y).abs() < SAME;
    // Each subpath's corners on the device, and whether it closes.
    let mut subpaths: Vec<(Vec<Point>, bool)> = Vec::new();
    for el in (device * path.clone()).elements() {
        match *el {
            PathEl::MoveTo(p) => subpaths.push((vec![p], false)),
            PathEl::LineTo(p) => {
                let (pts, _) = subpaths.last_mut()?;
                let last = *pts.last()?;
                if !aligned(last, p) {
                    return None;
                }
                if (p - last).hypot() >= SAME {
                    pts.push(p);
                }
            }
            PathEl::ClosePath => {
                let (pts, closed) = subpaths.last_mut()?;
                if !aligned(*pts.last()?, pts[0]) {
                    return None;
                }
                if pts.len() > 1 && (*pts.last()? - pts[0]).hypot() < SAME {
                    pts.pop();
                }
                *closed = true;
            }
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => return None,
        }
    }
    // A bevel or round join, or a mitre cut short of a right angle, does not
    // square the corner a rectangle per segment gives.
    let squared = s.join == Join::Miter && s.miter_limit >= std::f64::consts::SQRT_2;
    if !squared
        && subpaths
            .iter()
            .any(|(pts, closed)| *closed || pts.len() > 2)
    {
        return None;
    }
    let laid = s.width * scale;
    let half = laid / 2.0;
    let cap = if s.cap == Cap::Square { half } else { 0.0 };
    // Which side of the pixel boundary `at` a segment across `c`, running over
    // `along`, belongs on: before it (`Some(true)`) when every box edge rounding
    // to `at` alongside it has the line before it, after it when every one has it
    // after, `None` when none decides or two disagree.
    let side = |c: f64, along: (f64, f64), horizontal: bool, at: f64| {
        let mut vote = None;
        for r in edges.boxes() {
            let (span, sides) = if horizontal {
                ((r.x0, r.x1), [r.y0, r.y1])
            } else {
                ((r.y0, r.y1), [r.x0, r.x1])
            };
            if span.1 <= along.0 || span.0 >= along.1 {
                continue;
            }
            for e in sides.into_iter().filter(|e| e.round() == at) {
                let before = if c + half <= e + SAME {
                    true
                } else if c - half >= e - SAME {
                    false
                } else {
                    continue;
                };
                if vote.is_some_and(|v| v != before) {
                    return None;
                }
                vote = Some(before);
            }
        }
        vote
    };
    // Across a segment: its two edges rounded. Under a pixel wide, its own width
    // centred in the row they round to, so the row carries exactly its ink; where
    // they round together, against that boundary on the side [`side`] gives, as a
    // box edge there rounds to it too.
    let across = |c: f64, along: (f64, f64), horizontal: bool| {
        let (near, far) = ((c - half).round(), (c + half).round());
        if far <= near {
            match side(c, along, horizontal, near) {
                Some(true) => (near - laid, near),
                Some(false) => (near, near + laid),
                None => (c - half, c + half),
            }
        } else if laid < 1.0 {
            let mid = (near + far) / 2.0;
            (mid - half, mid + half)
        } else {
            (near, far)
        }
    };
    // Along a segment: its ends rounded, at least one pixel apart.
    let along = |lo: f64, hi: f64| {
        let (lo, hi) = (lo.round(), hi.round());
        (lo, hi.max(lo + 1.0))
    };
    let mut ink = BezPath::new();
    for (pts, closed) in &subpaths {
        let n = pts.len();
        let segments = if *closed && n > 2 { n } else { n - 1 };
        for i in 0..segments {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            let reach_p = if *closed || i > 0 { half } else { cap };
            let reach_q = if *closed || i + 1 < segments {
                half
            } else {
                cap
            };
            let span = |u: f64, v: f64| {
                if u <= v {
                    along(u - reach_p, v + reach_q)
                } else {
                    along(v - reach_q, u + reach_p)
                }
            };
            let r = if (p.y - q.y).abs() < SAME {
                let (x0, x1) = span(p.x, q.x);
                let (y0, y1) = across((p.y + q.y) / 2.0, (p.x.min(q.x), p.x.max(q.x)), true);
                Rect::new(x0, y0, x1, y1)
            } else {
                let (y0, y1) = span(p.y, q.y);
                let (x0, x1) = across((p.x + q.x) / 2.0, (p.y.min(q.y), p.y.max(q.y)), false);
                Rect::new(x0, y0, x1, y1)
            };
            ink.extend(r.path_elements(TOLERANCE));
        }
    }
    Some(device.inverse() * ink)
}

/// The axis-aligned boxes a walk draws, on the device as laid before rounding,
/// for [`snapped_stroke`] to keep a hairline on its side of a box edge both of its
/// edges round onto. Gathered from `Resolved` on first use, so a frame drawing no
/// such line pays nothing. A box a gesture moves or reshapes is left out: its
/// cached geometry is stale.
pub(crate) struct Edges<'a> {
    from: Option<EdgeSource<'a>>,
    boxes: std::cell::OnceCell<Vec<Rect>>,
}

struct EdgeSource<'a> {
    doc: &'a Document,
    res: &'a Resolved,
    ov: &'a RenderOverrides,
    origins: &'a [NodeId],
    cull: &'a [Rect],
    base: Affine,
}

impl<'a> Edges<'a> {
    fn of(
        doc: &'a Document,
        res: &'a Resolved,
        ov: &'a RenderOverrides,
        origins: &'a [NodeId],
        cull: &'a [Rect],
        base: Affine,
    ) -> Self {
        Self {
            from: Some(EdgeSource {
                doc,
                res,
                ov,
                origins,
                cull,
                base,
            }),
            boxes: std::cell::OnceCell::new(),
        }
    }

    /// No boxes, for a walk measuring ink, where a hairline's place within its
    /// pixel is moot.
    fn none() -> Self {
        Self {
            from: None,
            boxes: std::cell::OnceCell::new(),
        }
    }

    fn boxes(&self) -> &[Rect] {
        self.boxes.get_or_init(|| {
            let mut out = Vec::new();
            if let Some(s) = &self.from {
                for id in s.origins {
                    gather_boxes(s, *id, &mut out);
                }
            }
            out
        })
    }
}

/// Push the device box of every snapping box under `id` that the walk would draw
/// inside `s.cull`, culled as the walk culls. A box under a device pixel in either
/// axis is skipped: the collapse floor moves its far edge off the rounding rule.
fn gather_boxes(s: &EdgeSource<'_>, id: NodeId, out: &mut Vec<Rect>) {
    let Some(node) = s.doc.get(id) else { return };
    if matches!(node.kind(), NodeKind::Slice { .. })
        || node.mask()
        || !s.doc.shown_node(node)
        || s.ov.moved(id)
    {
        return;
    }
    let covers_subtree = node.children().is_empty()
        || node.clip()
        || matches!(
            node.kind(),
            NodeKind::Boolean { .. } | NodeKind::Group | NodeKind::Root
        );
    if covers_subtree
        && let Some(bounds) = s.res.ink_bounds(id)
        && !s.cull.iter().any(|v| intersects(bounds, *v))
    {
        return;
    }
    if s.ov.get(id).is_none()
        && !s.ov.unsnapped(id)
        && let Some(size) = snappable_box(s.res.used_kind_of(node))
        && let Some(world) = s.res.world_transform(id)
    {
        let device = s.base * world;
        let [_, b, c, _, _, _] = device.as_coeffs();
        if b.abs() < 1e-9 && c.abs() < 1e-9 {
            let r = device.transform_rect_bbox(Rect::from_origin_size(Point::ORIGIN, size));
            if r.width() >= 1.0 && r.height() >= 1.0 {
                out.push(r);
            }
        }
    }
    if matches!(node.kind(), NodeKind::Boolean { .. }) {
        return;
    }
    for child in node.children() {
        gather_boxes(s, *child, out);
    }
}

/// How `brush` sits in the box `frame` gives: [`None`] for a solid, a gradient, or
/// an image the document has no entry for. The intrinsic size comes from the
/// document's table, not the decode cache (§5.5a), so a picture still decoding
/// frames at its final size. `frame` runs only for a picture: a curved path's box
/// is a root solve per segment, which every shape would otherwise pay.
fn framing_of(doc: &Document, brush: &Brush, frame: impl FnOnce() -> Rect) -> Option<Framing> {
    let Brush::Image(img) = brush else {
        return None;
    };
    let entry = doc.image(&img.image.id)?;
    Some(img.image.framing(frame(), entry.width, entry.height))
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
    /// [`MaskMode::Alpha`] or [`MaskMode::Luminance`] (the `bool`): the mask
    /// **node**, drawn at the *end* of the run and composited over it with
    /// [`ScenePainter::push_mask_layer`] or
    /// [`ScenePainter::push_luminance_mask_layer`]. A node, not geometry, because
    /// what matters is the ink it lays down (fills, gradients, a photograph's
    /// alpha), which only drawing yields.
    Ink(NodeId, bool),
}

/// Open the layer that composites a mask's ink, by its luminance or its alpha.
fn push_ink_mask<P: ScenePainter>(painter: &mut P, luminance: bool) {
    match luminance {
        true => painter.push_luminance_mask_layer(),
        false => painter.push_mask_layer(),
    }
}

impl MaskRun {
    /// Push the run's layer if it has one and is not already open; `world` is the
    /// **parent's**. Opacity is 1.0 in both arms on purpose (an ink mask's is
    /// already in its ink). The ink arm's clip-less layer is an isolation group,
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
            Some(RunKind::Ink(..)) => {
                painter.push_layer(world, None, ClipRule::NonZero, 1.0);
                self.open = true;
            }
            None => {}
        }
    }

    /// The mask node to composite over this run before its layer is popped, and
    /// whether by luminance, or `None` for a shape run or one that never opened.
    /// Split from [`Self::close`] because drawing it needs `paint_node`'s context.
    fn ink_to_composite(&self) -> Option<(NodeId, bool)> {
        match self.kind {
            Some(RunKind::Ink(id, luminance)) if self.open => Some((id, luminance)),
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
/// Every mask mode asks this: hiding an ink mask must reveal what it masks, not
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
    /// The outline to draw, in local space, when one is already built: a
    /// `Boolean`'s derived one, or `Resolved::shape_path` for a kind no preview
    /// changes. `None` builds it from [`Self::kind`].
    path: Option<&'a BezPath>,
    /// The box a `Boolean` whose arithmetic was abandoned draws its placeholder in
    /// (§15 D298); `None` otherwise, including a correctly empty boolean.
    placeholder: Option<Rect>,
    /// How the geometry decides its interior; read by the fills only.
    rule: ClipRule,
    /// Device pixels per local unit when the walk runs at level of detail
    /// ([`stretch`]), `None` when every glyph is drawn.
    detail: Option<f64>,
    /// The local-to-device transform when the node's centred strokes snap to
    /// device pixels ([`strokes_snap`]), with the box edges they keep their side
    /// of; `None` when they are laid as stored.
    stroke_device: Option<(Affine, &'a Edges<'a>)>,
    /// `Filters` rows applied to the inks instead of the pixels
    /// ([`folded_filters`]), in order; `paint` is already through them, a text
    /// run's and a decoration's own colour are taken through them as drawn.
    filters: Vec<[f32; 20]>,
}

/// One pass of a layer whose effect stack holds a blending shadow
/// ([`shadow_passes`]): the stack it is drawn with, whether only the shadows it
/// casts are kept, and the mode they composite by.
struct FxPass {
    effects: Vec<ondin_core::Keyed<Effect>>,
    shadows: bool,
    blend: BlendMode,
}

/// The passes a layer is drawn in when a visible shadow of `effects` blends, or
/// `None` when the one effect layer does. Bottom to top: each drop shadow alone,
/// the last first since `effects::run` stacks a later one behind an earlier;
/// then the layer with its own appearance and no shadow; then each inner shadow
/// alone, in list order. A shadow's pass keeps the appearance effects, which
/// shape the silhouette it is cast from.
///
/// Alone, a shadow composites through its own blend layer onto what lies under
/// the layer, which the one effect buffer does not hold.
fn shadow_passes(effects: &[ondin_core::Keyed<Effect>]) -> Option<Vec<FxPass>> {
    use ondin_core::EffectKind;
    fn shadow(e: &ondin_core::Keyed<Effect>) -> Option<&ondin_core::Shadow> {
        match &e.kind {
            EffectKind::DropShadow(s) | EffectKind::InnerShadow(s)
                if e.visible && s.color.components[3] > 0.0 =>
            {
                Some(s)
            }
            _ => None,
        }
    }
    if !effects.iter().filter_map(shadow).any(|s| s.blends()) {
        return None;
    }
    let appearance: Vec<_> = effects
        .iter()
        .filter(|e| {
            !matches!(
                e.kind,
                EffectKind::DropShadow(_) | EffectKind::InnerShadow(_)
            )
        })
        .cloned()
        .collect();
    let alone = |e: &ondin_core::Keyed<Effect>, s: &ondin_core::Shadow| {
        let mut effects = appearance.clone();
        effects.push(e.clone());
        FxPass {
            effects,
            shadows: true,
            blend: s.blend,
        }
    };
    let drop = |e: &&ondin_core::Keyed<Effect>| matches!(e.kind, EffectKind::DropShadow(_));
    let mut passes: Vec<FxPass> = effects
        .iter()
        .rev()
        .filter(drop)
        .filter_map(|e| shadow(e).map(|s| alone(e, s)))
        .collect();
    passes.push(FxPass {
        effects: appearance.clone(),
        shadows: false,
        blend: BlendMode::Normal,
    });
    passes.extend(
        effects
            .iter()
            .filter(|e| !drop(e))
            .filter_map(|e| shadow(e).map(|s| alone(e, s))),
    );
    Some(passes)
}

/// Draw a layer's `passes` through `draw`, each shadow pass in its own mode,
/// inside the layer's `blend_layer`. A layer that only isolates (Normal) has its
/// shadow passes drawn outside it: a shadow blends with what lies under its
/// layer, which an isolation group would hide. Under any other mode the layer's
/// mode composites its shadows too.
fn draw_passes<P: ScenePainter>(
    painter: &mut P,
    blend_layer: Option<BlendMode>,
    passes: &[FxPass],
    mut draw: impl FnMut(&mut P, &FxPass),
) {
    let shadows_outside = blend_layer == Some(BlendMode::Normal);
    let mut open = false;
    for p in passes {
        let inside = !(p.shadows && shadows_outside);
        match blend_layer {
            Some(mode) if inside && !open => {
                painter.push_blend_layer(mode);
                open = true;
            }
            Some(_) if !inside && open => {
                painter.pop_layer();
                open = false;
            }
            _ => {}
        }
        if blends(p.blend) {
            painter.push_blend_layer(p.blend);
        }
        draw(painter, p);
        if blends(p.blend) {
            painter.pop_layer();
        }
    }
    if open {
        painter.pop_layer();
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_node<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
    parent_world: Affine,
    view: &[Rect],
    base: Affine,
    edges: &Edges<'_>,
    painter: &mut P,
    walk: &Walk<'_>,
) {
    paint_node_pass(
        doc,
        res,
        ov,
        id,
        parent_world,
        view,
        base,
        edges,
        painter,
        walk,
        None,
    );
}

/// [`paint_node`], or one of its [`FxPass`]es when `pass` is given — drawn
/// inside the node's blend layer, which the first call pushed.
#[allow(clippy::too_many_arguments)]
fn paint_node_pass<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
    parent_world: Affine,
    view: &[Rect],
    base: Affine,
    edges: &Edges<'_>,
    painter: &mut P,
    walk: &Walk<'_>,
    pass: Option<&FxPass>,
) {
    let Some(node) = doc.get(id) else { return };
    // A backdrop's repaint draws what comes before its layer, and then nothing;
    // inside a shadow pass the whole layer casts, so it does not stop there.
    if let Some(b) = &walk.backdrop
        && walk.in_shadow.get() == 0
    {
        if b.upto == id {
            b.reached.set(true);
        }
        if b.reached.get() {
            return;
        }
    }
    // The container a backdrop is repainted from draws without the layers that
    // isolate it: the backdrop is what lies inside them.
    let bare = walk.bare(id);
    let over = ov.get(id);

    // A slice is never drawn.
    if matches!(node.kind(), NodeKind::Slice { .. })
        || !over
            .and_then(|o| o.visible)
            .unwrap_or_else(|| doc.shown_node(node))
    {
        return;
    }

    // Cull what misses the viewport, skipping a subtree only when the node's bounds
    // cover it: a group's, a clipping frame's, and a boolean's (its children are
    // never walked). A new `NodeKind` needs a look at this predicate.
    //
    // Outline mode draws what a frame clips, and a clipping frame's ink is its box
    // alone, as is every ancestor's share of it: there only a leaf and a boolean
    // are culled.
    let outline = ov.outline().is_some();
    let bounds_cover_subtree = node.children().is_empty()
        || (node.clip() && !outline)
        || matches!(node.kind(), NodeKind::Boolean { .. })
        || (matches!(node.kind(), NodeKind::Group | NodeKind::Root) && !outline);
    // Never culled: a moving node (cached bounds describe where it *was*) or one
    // holding a ghost (the bounds cannot know about it). `ink_bounds`, not
    // `world_bounds` (§5.3a): a drop shadow lands outside the layer's box. A
    // level-of-detail walk also skips what lands under half a device pixel.
    if bounds_cover_subtree
        && !ov.moved(id)
        && ov.ghosts_of(id).next().is_none()
        && let Some(bounds) = res.ink_bounds(id)
        && (!view.iter().any(|v| intersects(bounds, *v))
            || (ov.level_of_detail() && sub_pixel(bounds, base)))
    {
        return;
    }

    // A gesture's override first, then the used geometry (where container layout
    // placed the node).
    let local = over
        .and_then(|o| o.transform)
        .unwrap_or_else(|| res.used_local_of(node));
    let world = parent_world * local;
    // A bound opacity is drawn as its variable's percentage, never its literal's 0.
    let opacity = match bare {
        true => 1.0,
        false => over
            .and_then(|o| o.opacity)
            .unwrap_or_else(|| doc.drawn_opacity_of(node)),
    };

    // The box the paint is stored against: a gesture's kind is its stored one.
    let stored = over.and_then(|o| o.kind.as_ref()).unwrap_or(node.kind());
    let kind = over
        .and_then(|o| o.kind.as_ref())
        .unwrap_or_else(|| res.used_kind_of(node));
    let mut painted = Painted {
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
        // The cached outline only for the kind it was built from: a gesture
        // reshaping the node draws from its own.
        path: ov
            .boolean_path(id)
            .or_else(|| res.boolean_path(id))
            .or_else(|| {
                over.and_then(|o| o.kind.as_ref())
                    .is_none()
                    .then(|| res.shape_path(id))
                    .flatten()
            }),
        // Committed only, and only when no preview owns the node: a gesture
        // re-evaluating this boolean always leaves an entry, and without the gate a
        // drag would draw the placeholder over the last committed operand positions.
        placeholder: match ov.boolean_path(id) {
            Some(_) => None,
            None => ondin_core::boolean_placeholder(doc, res, id),
        },
        detail: ov.level_of_detail().then(|| stretch(base * world)),
        stroke_device: strokes_snap(kind, ov, id).then(|| (base * world, edges)),
        filters: Vec::new(),
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
    // The effect layer is outermost but for the blend layer, in two load-bearing
    // positions: outside the clip, so a clipping frame's drop shadow is not cut
    // off, and outside the opacity, so a half-transparent layer casts a
    // half-transparent shadow.
    let stack = doc.resolved_effects(
        id,
        over.and_then(|o| o.effects.as_deref())
            .unwrap_or(node.effects()),
    );
    let stack = if ov.level_of_detail() {
        detail_effects(stack, base * world)
    } else {
        stack
    };
    // Filters that recolour the inks exactly leave the stack, so a layer with
    // nothing else needs no buffer.
    let stack = match folded_filters(&stack, &painted) {
        Some(rows) => {
            painted.paint = std::borrow::Cow::Owned(recoloured(&painted.paint, &rows));
            painted.filters = rows;
            let rest = stack
                .iter()
                .filter(|e| !matches!(e.kind, ondin_core::EffectKind::Filters(_)));
            std::borrow::Cow::Owned(rest.cloned().collect())
        }
        None => stack,
    };
    let stack: &[ondin_core::Keyed<ondin_core::Effect>] = &stack;
    let effects = pass.map_or(stack, |p| &p.effects[..]);
    // `Some` exactly when a layer was pushed; a node with no cached bounds yet
    // draws unfiltered rather than not at all. Sized from the whole stack, so
    // every pass reads its silhouette from the same buffer box.
    let effect_layer = (!bare && crate::effects::any_ink(effects))
        .then(|| effect_bounds(res, id, &painted, world, stack))
        .flatten();
    // The layer's effects carry a child's ink past the child's own box, so the
    // children are culled against the view grown back by the stack's escape.
    let child_view: std::borrow::Cow<'_, [Rect]> = match effect_layer {
        Some(_) => {
            let e = ondin_core::effect::stack_escape(stack);
            let back = kurbo::Insets::new(e.x1, e.y1, e.x0, e.y0);
            let grown = view
                .iter()
                .map(|v| ondin_core::effect::escaped(*v, back, world));
            std::borrow::Cow::Owned(grown.collect())
        }
        None => std::borrow::Cow::Borrowed(view),
    };
    let view: &[Rect] = &child_view;
    // Behind everything the layer draws, its own shadows and blend included: what
    // a background blur shows is what was there before the layer. A mask's ink
    // is drawn into its mask layer, where a backdrop would become mask.
    if pass.is_none()
        && !bare
        && !node.mask()
        && walk.backdrop.is_none()
        && painter.paints_backdrops()
        && opacity > 0.0
        && let Some(radius) = ondin_core::effect::backdrop_radius(stack)
        && let Some(shape) = backdrop_shape(doc, res, ov, id, &painted)
    {
        let at = Outline {
            at: shape_world,
            path: &shape,
            rule: painted.rule,
        };
        paint_backdrop(
            (doc, res, ov),
            id,
            at,
            radius,
            opacity,
            base,
            edges,
            painter,
            walk,
        );
    }
    // The blend layer is outside even the effect layer: a layer's shadow blends
    // with it. A pass is drawn inside the one its first call pushed
    // ([`draw_passes`]).
    let blend = over.and_then(|o| o.blend).unwrap_or(node.blend());
    let blend_layer = match pass {
        None if !bare => blend_layer(doc, res, ov, id, painted.kind, blend),
        _ => None,
    };
    if pass.is_none()
        && effect_layer.is_some()
        && let Some(passes) = shadow_passes(stack)
    {
        draw_passes(painter, blend_layer, &passes, |painter, p| {
            walk.in_shadow
                .set(walk.in_shadow.get() + usize::from(p.shadows));
            paint_node_pass(
                doc,
                res,
                ov,
                id,
                parent_world,
                view,
                base,
                edges,
                painter,
                walk,
                Some(p),
            );
            walk.in_shadow
                .set(walk.in_shadow.get() - usize::from(p.shadows));
        });
        return;
    }
    if let Some(mode) = blend_layer {
        painter.push_blend_layer(mode);
    }
    if let Some(ink) = effect_layer {
        match pass {
            Some(p) if p.shadows => painter.push_shadow_layer(world, ink, effects),
            _ => painter.push_effect_layer(world, ink, effects),
        }
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
    // Ends the mask run, first compositing an ink mask's ink inside its layer.
    let end_run = |painter: &mut P, mask: &mut MaskRun| {
        if let Some((id, luminance)) = mask.ink_to_composite() {
            push_ink_mask(painter, luminance);
            paint_node(doc, res, ov, id, world, view, base, edges, painter, walk);
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
        if blend_layer.is_some() {
            painter.pop_layer();
        }
        return;
    }

    // A background blur under this node reads back to here if this node's
    // children are drawn into a layer of its own.
    walk.open.borrow_mut().push(Open {
        id,
        parent_world,
        isolates: blend_layer.is_some()
            || effect_layer.is_some()
            || outer_opacity
            || inner_opacity < 1.0,
    });
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
            let top = matches!(node.kind(), NodeKind::Root);
            paint_ghost(doc, ov, &ghost.node, top, world, base, edges, painter, None);
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
                // An empty ink mask legitimately erases everything above it; a
                // hidden one masks nothing.
                mode @ (MaskMode::Alpha | MaskMode::Luminance) => visible(ov, doc, *child)
                    .then_some(RunKind::Ink(*child, mode == MaskMode::Luminance)),
            };
            continue;
        }
        if ov.escapes_clip(*child) {
            close(painter, &mut open, &mut mask);
        } else {
            reopen(painter, &mut open, &mut mask);
        }
        paint_node(
            doc, res, ov, *child, world, view, base, edges, painter, walk,
        );
        if walk.stopped() {
            break;
        }
    }
    walk.open.borrow_mut().pop();

    // A repaint that stopped short of a backdrop's layer leaves out what comes
    // after it: an ink mask composited at the end of its run, and the stroke.
    let stopped = walk.stopped();
    if stopped {
        mask.close(painter);
        if open {
            painter.pop_layer();
        }
    } else {
        close(painter, &mut open, &mut mask);
    }
    if !stopped
        && frame_stroke
        && let Some(frame) = geometry::local_path(painted.kind)
    {
        // `shape_world`, not `world`: the border must run along the box the
        // background filled, or snapping opens a half-pixel gap.
        paint_strokes(
            doc,
            painter,
            shape_world,
            painted.kind,
            &painted.paint,
            &frame,
            ClipRule::NonZero,
            || frame.bounding_box(),
            None,
        );
    }
    if outer_opacity {
        painter.pop_layer();
    }
    if effect_layer.is_some() {
        painter.pop_layer();
    }
    if blend_layer.is_some() {
        painter.pop_layer();
    }
}

/// The mode of the layer `id` composites through, or `None` when it draws
/// straight into its parent's.
///
/// A page holding anything that blends is isolated, so it blends with the page's
/// artwork and never the canvas ground, which every export lays after drawing. A
/// top-level frame holding a blended layer is isolated too, so what is inside
/// blends with the frame's content alone: the frame then looks the same on the
/// canvas as exported alone.
fn blend_layer(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
    kind: &NodeKind,
    blend: BlendMode,
) -> Option<BlendMode> {
    if blend.isolates(kind) {
        return Some(blend);
    }
    let parent = doc.get(id).and_then(|n| n.parent());
    let page = matches!(kind, NodeKind::Root) && parent.is_none();
    let top_frame = matches!(kind, NodeKind::Artboard { .. })
        && parent
            .and_then(|p| doc.get(p))
            .is_some_and(|p| matches!(p.kind(), NodeKind::Root));
    ((page || top_frame) && holds_blend(doc, res, ov, id)).then_some(BlendMode::Normal)
}

/// Whether a layer under `id`, ghosts included, blends with what is beneath it.
///
/// Answered from `Resolved`'s cache, and as yes whenever the preview blends
/// anything anywhere: an isolation group around artwork that does not blend
/// changes no pixel, so too many costs only a layer.
fn holds_blend(doc: &Document, res: &Resolved, ov: &RenderOverrides, id: NodeId) -> bool {
    let Some(node) = doc.get(id) else {
        return false;
    };
    ov.blends()
        || ov
            .children_of(id)
            .unwrap_or(node.children())
            .iter()
            .any(|c| res.blends_within(*c))
}

/// Whether `g`, or a ghost under it, blends: its mode or a shadow's.
pub(crate) fn ghost_blends(g: &GhostNode) -> bool {
    blends(g.blend) || shadows_blend(&g.effects) || g.children.iter().any(ghost_blends)
}

/// Whether a visible shadow in `effects` blends.
pub(crate) fn shadows_blend(effects: &[ondin_core::Keyed<Effect>]) -> bool {
    effects.iter().any(|e| match &e.kind {
        ondin_core::EffectKind::DropShadow(s) | ondin_core::EffectKind::InnerShadow(s) => {
            e.visible && s.blends()
        }
        _ => false,
    })
}

/// The backends' mix for `mode`; both draw all sixteen. Pass through never
/// reaches a layer, and is Normal if it does.
pub(crate) fn mix(mode: BlendMode) -> ondin_core::peniko::Mix {
    use ondin_core::peniko::Mix;
    match mode {
        BlendMode::PassThrough | BlendMode::Normal => Mix::Normal,
        BlendMode::Darken => Mix::Darken,
        BlendMode::Multiply => Mix::Multiply,
        BlendMode::ColorBurn => Mix::ColorBurn,
        BlendMode::Lighten => Mix::Lighten,
        BlendMode::Screen => Mix::Screen,
        BlendMode::ColorDodge => Mix::ColorDodge,
        BlendMode::Overlay => Mix::Overlay,
        BlendMode::SoftLight => Mix::SoftLight,
        BlendMode::HardLight => Mix::HardLight,
        BlendMode::Difference => Mix::Difference,
        BlendMode::Exclusion => Mix::Exclusion,
        BlendMode::Hue => Mix::Hue,
        BlendMode::Saturation => Mix::Saturation,
        BlendMode::Color => Mix::Color,
        BlendMode::Luminosity => Mix::Luminosity,
    }
}

/// Whether `mode` mixes with what is beneath rather than covering it.
pub(crate) fn blends(mode: BlendMode) -> bool {
    !matches!(mode, BlendMode::Normal | BlendMode::PassThrough)
}

/// Whether a world `ink` box lands under half a device pixel on both axes, which
/// a level-of-detail walk skips along with everything under it.
fn sub_pixel(ink: Rect, base: Affine) -> bool {
    let device = base.transform_rect_bbox(ink);
    device.width() < 0.5 && device.height() < 0.5
}

/// The colour matrices of `stack`'s `Filters` rows when applying them to
/// `painted`'s inks draws exactly what filtering its pixels would, or `None`.
///
/// A matrix is affine, so it commutes with the convex mix of inks inside the
/// layer, and the shadows are cast from alpha it never changes. What breaks that
/// is refused: a container (its children are not its inks), a non-solid ink, a
/// layer blur (its mixing happens before or after the filter in list order), and
/// a clamp on any ink while the layer holds more than one colour.
fn folded_filters(
    stack: &[ondin_core::Keyed<Effect>],
    painted: &Painted<'_>,
) -> Option<Vec<[f32; 20]>> {
    use ondin_core::EffectKind;
    let rows: Vec<[f32; 20]> = stack
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.kind {
            EffectKind::Filters(f) if !f.is_neutral() => Some(crate::effects::filter_matrix(f)),
            _ => None,
        })
        .collect();
    let blurs = stack
        .iter()
        .any(|e| e.visible && matches!(e.kind, EffectKind::LayerBlur { radius } if radius > 0.0));
    let container = matches!(
        painted.kind,
        NodeKind::Group | NodeKind::Root | NodeKind::Artboard { .. }
    );
    if rows.is_empty() || blurs || container || painted.placeholder.is_some() {
        return None;
    }
    let inks = inks_of(painted)?;
    let one_colour = inks.windows(2).all(|w| w[0].components == w[1].components);
    (one_colour || inks.iter().all(|c| !through(*c, &rows).1)).then_some(rows)
}

/// Every colour `painted` can draw in — visible fills and strokes, a text run's
/// and a decoration's own, the black of type with no ink — or `None` when one
/// is not a solid colour.
fn inks_of(painted: &Painted<'_>) -> Option<Vec<ondin_core::peniko::Color>> {
    let solid = |b: &Brush| match b {
        Brush::Solid(c) => Some(*c),
        _ => None,
    };
    let fills = painted.paint.fills.iter().filter(|f| f.visible);
    let strokes = painted.paint.strokes.iter().filter(|s| s.visible);
    let mut inks: Vec<_> = fills
        .map(|f| solid(f.brush.drawn()))
        .chain(strokes.map(|s| solid(s.brush.drawn())))
        .collect::<Option<_>>()?;
    if let NodeKind::Text { .. } = painted.kind {
        if inks.is_empty() {
            inks.push(ondin_core::peniko::Color::BLACK);
        }
        if let Some(layout) = painted.text {
            inks.extend(layout.runs.iter().filter_map(|r| r.color));
            inks.extend(layout.decorations.iter().filter_map(|d| d.color));
        }
    }
    Some(inks)
}

/// `c` through each of `rows` in turn, clamped between them as the pixel passes
/// clamp, and whether any clamp bit.
fn through(c: ondin_core::peniko::Color, rows: &[[f32; 20]]) -> (ondin_core::peniko::Color, bool) {
    rows.iter().fold((c, false), |(c, clamped), m| {
        let (out, now) = crate::effects::matrix_colour(c, m);
        (out, clamped || now)
    })
}

/// `paint` with every solid fill and stroke taken [`through`] `rows`.
fn recoloured(paint: &Paint, rows: &[[f32; 20]]) -> Paint {
    let mut out = paint.clone();
    for f in out.fills.iter_mut() {
        if let Brush::Solid(c) = f.brush.drawn() {
            f.brush = Brush::Solid(through(*c, rows).0).into();
        }
    }
    for s in out.strokes.iter_mut() {
        if let Brush::Solid(c) = s.brush.drawn() {
            s.brush = Brush::Solid(through(*c, rows).0).into();
        }
    }
    out
}

/// The most `device` lengthens any local distance: its largest singular value, so
/// a length under a pixel through it is under a pixel in every direction.
fn stretch(device: Affine) -> f64 {
    let [a, b, c, d, _, _] = device.as_coeffs();
    let sum = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    ((sum + (sum * sum - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt()
}

/// `effects` without each shadow and layer blur whose reach is under one device
/// pixel through `device` (the node's local-to-device transform): drawn, it would
/// move the edge by less than a pixel, and it costs an offscreen buffer. `Filters`
/// recolour every pixel and always stay. A shadow tucked under a translucent layer
/// is what the cut can lose.
fn detail_effects(
    effects: std::borrow::Cow<'_, [ondin_core::Keyed<Effect>]>,
    device: Affine,
) -> std::borrow::Cow<'_, [ondin_core::Keyed<Effect>]> {
    use ondin_core::EffectKind;
    use ondin_core::effect::reach;
    let stretch = stretch(device);
    let unseen = |e: &ondin_core::Keyed<Effect>| {
        let local = match &e.kind {
            EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
                s.offset.hypot() + reach(s.blur) + s.spread.abs()
            }
            EffectKind::LayerBlur { radius } | EffectKind::BackgroundBlur { radius } => {
                reach(*radius)
            }
            EffectKind::Filters(_) => return false,
        };
        local * stretch < 1.0
    };
    if !effects.iter().any(unseen) {
        return effects;
    }
    std::borrow::Cow::Owned(effects.iter().filter(|e| !unseen(e)).cloned().collect())
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
        // Everything the node draws, a stroke's mitres and a text's descenders
        // included; declines a `Boolean`, which falls back to the cache.
        _ => ondin_core::geometry::world_ink_of_parts(
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

/// The outline a background blur shows through, in the node's own space: its
/// shape, a boolean's result, type's glyphs, a group's children's outlines
/// united as a mask's are. `None` when there is no area to show it in.
fn backdrop_shape(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
    painted: &Painted<'_>,
) -> Option<BezPath> {
    let path = match painted.kind {
        NodeKind::Text { .. } => ondin_core::text::outline(painted.text?),
        NodeKind::Group | NodeKind::Root => {
            let node = doc.get(id)?;
            let inner: Vec<BezPath> = ov
                .children_of(id)
                .unwrap_or(node.children())
                .iter()
                .filter(|c| !doc.get(**c).is_some_and(|n| n.mask()))
                .filter_map(|c| mask_geometry(doc, res, ov, *c))
                .collect();
            ondin_core::boolean::evaluate(ondin_core::BoolOp::Union, &inner)?
        }
        kind => painted
            .path
            .cloned()
            .or_else(|| geometry::local_path(kind))?,
    };
    (!path.elements().is_empty()).then_some(path)
}

/// A closed outline and the transform it is drawn under.
#[derive(Clone, Copy)]
struct Outline<'a> {
    at: Affine,
    path: &'a BezPath,
    rule: ClipRule,
}

/// Draw what lies behind node `id`, blurred by `radius`, inside its outline,
/// replacing what is already there at the node's `opacity`: its background
/// blur.
///
/// What lies behind is everything the walk drew before the node inside the
/// nearest enclosing container that draws into a layer of its own (CSS's
/// backdrop root), or since the walk began when none does, repainted into an
/// effect buffer blurred as a layer blur is. The outline is erased at the
/// opacity and the blurred copy added into the hole at the same opacity, which
/// is the mix `(1 − o)·under + o·blurred` at any coverage. Drawn over instead,
/// the erased pixels would be faded a second time. Where a clip layer sits
/// between that container and the node, the erase reaches only into the clip
/// layer, and a translucent backdrop lets what is under it through. The canvas's ground ([`ScenePainter::ground`]) is part of a
/// backdrop read from the walk's start. A layer met inside a repaint draws
/// without its own backdrop, which bounds the cost at one repaint per layer.
#[allow(clippy::too_many_arguments)]
fn paint_backdrop<P: ScenePainter>(
    (doc, res, ov): (&Document, &Resolved, &RenderOverrides),
    id: NodeId,
    shape: Outline<'_>,
    radius: f64,
    opacity: f32,
    base: Affine,
    edges: &Edges<'_>,
    painter: &mut P,
    walk: &Walk<'_>,
) {
    let reach = ondin_core::effect::reach(radius);
    // What the blur reads: the outline's box grown by the blur's reach.
    let local = shape.path.bounding_box().inflate(reach, reach);
    // The repaint culls to that box in the world, a little wider so an edge's
    // antialiasing is not cut.
    let device = (base * shape.at)
        .transform_rect_bbox(local)
        .inflate(2.0, 2.0);
    let world = base.inverse().transform_rect_bbox(device);

    painter.push_erase_layer();
    painter.fill_path(
        shape.at,
        shape.path,
        &Brush::Solid(ondin_core::peniko::Color::BLACK.with_alpha(opacity)),
        None,
        shape.rule,
    );
    painter.pop_layer();

    painter.push_add_layer();
    painter.push_layer(shape.at, Some(shape.path), shape.rule, opacity);
    let blur =
        ondin_core::keyed_by_position([Effect::new(ondin_core::EffectKind::LayerBlur { radius })]);
    painter.push_effect_layer(shape.at, local, &blur);
    let root = walk
        .open
        .borrow()
        .iter()
        .rev()
        .find(|o| o.isolates)
        .copied();
    let repaint = Walk {
        origins: walk.origins,
        open: Default::default(),
        in_shadow: Default::default(),
        backdrop: Some(Backdrop {
            upto: id,
            bare: root.map(|o| o.id),
            reached: std::cell::Cell::new(false),
        }),
    };
    let cull = [world];
    match root {
        Some(o) => paint_node(
            doc,
            res,
            ov,
            o.id,
            o.parent_world,
            &cull,
            base,
            edges,
            painter,
            &repaint,
        ),
        None => {
            if let Some(ground) = painter.ground() {
                let rect = world.to_path(TOLERANCE);
                let ink = Brush::Solid(ground);
                painter.fill_path(Affine::IDENTITY, &rect, &ink, None, ClipRule::NonZero);
            }
            walk_origins(doc, res, ov, &cull, base, edges, painter, &repaint);
        }
    }
    painter.pop_layer();
    painter.pop_layer();
    painter.pop_layer();
}

/// The world box a ghost's own ink covers: [`effect_bounds`]'s answer for a node
/// `Resolved` has never seen. A container is its children's boxes grown by their
/// stacks, a boolean uses `GhostNode::path`, anything else is
/// [`ondin_core::geometry::world_ink_of_parts`]. Hidden or masking children
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
        NodeKind::Boolean { .. } => ghost.path.as_ref().and_then(|p| {
            ondin_core::geometry::world_ink_of_path(p, &ghost.paint, &ghost.kind, world)
        }),
        kind => {
            ondin_core::geometry::world_ink_of_parts(kind, &ghost.paint, world, ghost.text.as_ref())
        }
    }
}

/// Draw a ghost subtree: [`paint_node`]'s walk with children inline and no
/// culling. `doc` supplies the image table that framing reads; `top` says the
/// ghost lands directly on a page, for [`blend_layer`]'s top-level frame; `pass`
/// is one of its [`FxPass`]es, as for [`paint_node_pass`].
#[allow(clippy::too_many_arguments)]
fn paint_ghost<P: ScenePainter>(
    doc: &Document,
    ov: &RenderOverrides,
    ghost: &GhostNode,
    top: bool,
    parent_world: Affine,
    base: Affine,
    edges: &Edges<'_>,
    painter: &mut P,
    pass: Option<&FxPass>,
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
    // A pass's own stack, in a box sized from the whole one, as in `paint_node_pass`.
    let effects = pass.map_or(&ghost.effects[..], |p| &p.effects[..]);
    let ghost_effects = crate::effects::any_ink(effects)
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
    let blend_layer = if pass.is_some() {
        None
    } else if ghost.blend.isolates(&ghost.kind) {
        Some(ghost.blend)
    } else {
        let top_frame = top && matches!(ghost.kind, NodeKind::Artboard { .. });
        (top_frame && ghost.children.iter().any(ghost_blends)).then_some(BlendMode::Normal)
    };
    if pass.is_none()
        && ghost_effects.is_some()
        && let Some(passes) = shadow_passes(&ghost.effects)
    {
        draw_passes(painter, blend_layer, &passes, |painter, p| {
            paint_ghost(
                doc,
                ov,
                ghost,
                top,
                parent_world,
                base,
                edges,
                painter,
                Some(p),
            );
        });
        return;
    }
    if let Some(mode) = blend_layer {
        painter.push_blend_layer(mode);
    }
    if let Some(ink) = ghost_effects {
        match pass {
            Some(p) if p.shadows => painter.push_shadow_layer(world, ink, effects),
            _ => painter.push_effect_layer(world, ink, effects),
        }
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
            detail: None,
            stroke_device: strokes_snap(&ghost.kind, ov, ghost.id).then(|| (base * world, edges)),
            filters: Vec::new(),
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
        if blend_layer.is_some() {
            painter.pop_layer();
        }
        return;
    }
    // A ghost masks its own children; nothing leaves a ghost, so no `escape_clip`.
    let mut mask = MaskRun::default();
    let end_run = |painter: &mut P, mask: &mut MaskRun, ghost: &GhostNode| {
        if let Some((_, luminance)) = mask.ink_to_composite() {
            push_ink_mask(painter, luminance);
            paint_ghost(doc, ov, ghost, false, world, base, edges, painter, None);
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
                // pushes the mask layer this arm declined.
                mode @ (MaskMode::Alpha | MaskMode::Luminance) if child.visible => {
                    alpha_ghost = Some(child);
                    Some(RunKind::Ink(child.id, mode == MaskMode::Luminance))
                }
                MaskMode::Alpha | MaskMode::Luminance => None,
            };
            continue;
        }
        mask.reopen(painter, world);
        paint_ghost(doc, ov, child, false, world, base, edges, painter, None);
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
        paint_strokes(
            doc,
            painter,
            shape_world,
            &ghost.kind,
            &paint,
            &frame,
            ClipRule::NonZero,
            || frame.bounding_box(),
            None,
        );
    }
    if outer_opacity {
        painter.pop_layer();
    }
    if ghost_effects.is_some() {
        painter.pop_layer();
    }
    if blend_layer.is_some() {
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

/// The font size, in device pixels, under which a level-of-detail walk draws a
/// run as bars: type that small cannot be read, and its glyphs are the costliest
/// ink a zoomed-out page holds.
const GREEK_BELOW: f64 = 4.0;

/// `run` as bars when it is set under [`GREEK_BELOW`] device pixels at `stretch`
/// device pixels per local unit, or `None` when it is drawn as glyphs: always for
/// type on a path and for rotated glyphs.
///
/// One bar per span of glyphs on one baseline, over the band from 0.15 to 0.45 of
/// the font size above it: about the ink a line of lowercase lays down, so the
/// line keeps its colour, length and weight. The last glyph's advance is not
/// stored and is taken as half the font size.
fn greeked(run: &ondin_core::text::GlyphRun, stretch: f64, layout: &TextLayout) -> Option<BezPath> {
    let size = f64::from(run.font_size);
    // Asked this way round so a `NaN` scale draws glyphs.
    let small = size * stretch < GREEK_BELOW;
    if !small || layout.warp.is_some() || run.glyphs.iter().any(|g| g.rot != 0.0) {
        return None;
    }
    let mut bars = BezPath::new();
    let mut bar = |x0: f64, x1: f64, y: f64| {
        let r = Rect::new(x0, y - 0.45 * size, x1 + 0.5 * size, y - 0.15 * size);
        bars.extend(r.path_elements(TOLERANCE));
    };
    // The span being gathered: its left edge, its last glyph and its baseline.
    let mut open: Option<(f64, f64, f64)> = None;
    for g in &run.glyphs {
        let (x, y) = (f64::from(g.x), f64::from(g.y));
        open = match open {
            Some((x0, last, at)) if at == y && x >= last => Some((x0, x, at)),
            Some((x0, last, at)) => {
                bar(x0, last, at);
                Some((x, x, y))
            }
            None => Some((x, x, y)),
        };
    }
    if let Some((x0, last, at)) = open {
        bar(x0, last, at);
    }
    Some(bars)
}

/// The share of a greeked bar's alpha that real type lays down: glyphs at these
/// sizes ink about 0.7 of the bar's area, so a bar at full alpha reads as type
/// set much bolder than it is.
const GREEK_INK: f32 = 0.7;

/// `brush` at [`GREEK_INK`] of its alpha, for a greeked run's bars.
fn greek_ink(brush: &Brush) -> Brush {
    let mut out = brush.clone();
    match &mut out {
        Brush::Solid(c) => *c = c.multiply_alpha(GREEK_INK),
        Brush::Gradient(g) => g.opacity *= GREEK_INK,
        Brush::Image(i) => i.sampler.alpha *= GREEK_INK,
    }
    out
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
    if let NodeKind::Artboard { size, .. } = painted.kind {
        let frame = Rect::new(0.0, 0.0, size.width, size.height);
        // The rounded outline, the same one the children are clipped to.
        let path = geometry::local_path(painted.kind).unwrap_or_else(|| frame.to_path(TOLERANCE));
        for fill in painted.paint.fills.iter().filter(|f| f.visible) {
            let framing = framing_of(doc, fill.brush.drawn(), || frame);
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
        // The fill and stroke inks are already through the folded filters; a
        // colour the layout carries goes through them here.
        let recolour = |c| Brush::Solid(through(c, &painted.filters).0);
        let fallback = recolour(ondin_core::peniko::Color::BLACK);
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
        // Outlined type that is greeked fills its bars in its first stroke's ink.
        let stroke_one = painted
            .paint
            .strokes
            .iter()
            .find(|s| s.visible)
            .map(|s| [s.brush.drawn()]);
        let mut all_greeked = true;
        // A run with its own colour draws once in it, else the node's whole fill
        // stack; paint order is per run (§15 D154).
        for run in &layout.runs {
            let bars = painted.detail.and_then(|s| greeked(run, s, layout));
            all_greeked &= bars.is_some();
            let own: Option<Brush> = run.color.map(recolour);
            let own_one = own.as_ref().map(|b| [b]);
            let run_inks: &[&Brush] = match (&own_one, &bars, &stroke_one) {
                (Some(one), _, _) => one,
                (None, Some(_), Some(one)) if inks.is_empty() => one,
                _ => &inks,
            };
            for brush in run_inks {
                // The frame is the whole text box, not the run's advance.
                let framing = framing_of(doc, brush, || layout.bounds());
                clipped(painter, world, framing, |p| match &bars {
                    Some(bars) => {
                        p.fill_path(world, bars, &greek_ink(brush), framing, ClipRule::NonZero)
                    }
                    None => draw_run(p, world, run, brush, framing),
                });
            }
        }
        // Type strokes along its glyph outlines (`ondin_core::text::outline`) in the
        // text box's frame; decoration bands are not stroked. Wholly greeked type
        // skips it: the outline is every glyph's, the cost the bars avoid.
        if inked && !all_greeked {
            let outline = ondin_core::text::outline(layout);
            if !outline.is_empty() {
                paint_strokes(
                    doc,
                    painter,
                    world,
                    painted.kind,
                    &painted.paint,
                    &outline,
                    ClipRule::NonZero,
                    || layout.bounds(),
                    None,
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
            let own = ink.color.map(recolour);
            let brush = own.as_ref().or(inks.first().copied()).unwrap_or(&fallback);
            let framing = framing_of(doc, brush, || layout.bounds());
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
        Some(built) => std::borrow::Cow::Borrowed(built),
        None => match local_path(painted.kind) {
            Some(path) => std::borrow::Cow::Owned(path),
            None => return,
        },
    };

    // Fills then strokes, in list order (the lists are the z-order). A line has no
    // interior to fill. The image frame is this path's box, shared by both.
    let frame = || path.bounding_box();
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
        painted.rule,
        frame,
        painted.stroke_device,
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
/// ink's, and for outlined type the text box. `rule` is how `shape` decides its
/// interior, as its fill reads it. `device`, when given, snaps each centred
/// stroke's edges to device pixels ([`snapped_stroke`]).
#[allow(clippy::too_many_arguments)]
fn paint_strokes<P: ScenePainter>(
    doc: &Document,
    painter: &mut P,
    world: Affine,
    kind: &NodeKind,
    paint: &Paint,
    shape: &BezPath,
    rule: ClipRule,
    frame: impl Fn() -> Rect,
    device: Option<(Affine, &Edges<'_>)>,
) {
    let frame = &frame;
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
                let snapped = device
                    .filter(|_| {
                        geometry::effective_sides(kind, s)
                            .per_side(s.width)
                            .is_none()
                    })
                    .and_then(|(d, edges)| snapped_stroke(shape, d, s, edges));
                match snapped {
                    Some(ink) => clipped(painter, world, framing, |p| {
                        p.fill_path(world, &ink, s.brush.drawn(), framing, ClipRule::NonZero)
                    }),
                    None => clipped(painter, world, framing, |p| {
                        lay_sides(p, world, kind, s, shape, framing, false)
                    }),
                }
            }
            continue;
        }
        // A non-zero path or glyph outline may cross itself, and a complement taken
        // even-odd counts the overlap as outside: the ink is drawn whole and the
        // shape's own fill erased from it instead. The other kinds never overlap
        // themselves and keep the one clip layer.
        if align == StrokeAlign::Outside
            && rule == ClipRule::NonZero
            && matches!(kind, NodeKind::Path { .. } | NodeKind::Text { .. })
        {
            painter.push_layer(world, None, ClipRule::NonZero, 1.0);
            for s in run {
                let framing = framing_of(doc, s.brush.drawn(), frame);
                clipped(painter, world, framing, |p| {
                    lay_sides(p, world, kind, s, shape, framing, true)
                });
            }
            painter.push_erase_layer();
            let opaque = Brush::Solid(ondin_core::peniko::Color::BLACK);
            painter.fill_path(world, shape, &opaque, None, ClipRule::NonZero);
            painter.pop_layer();
            painter.pop_layer();
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
        let clip_rule = match align {
            StrokeAlign::Outside => ClipRule::EvenOdd,
            _ => rule,
        };
        painter.push_layer(
            world,
            Some(complement.as_ref().unwrap_or(shape)),
            clip_rule,
            1.0,
        );
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

    /// The level-of-detail cut drops a shadow reaching under a device pixel and
    /// keeps a `Filters` row, which recolours every pixel at any zoom.
    ///
    /// flip: `detail_effects` treating `Filters` as reach 0 → fails at "kept".
    #[test]
    fn the_detail_cut_keeps_filters_and_drops_a_sub_pixel_shadow() {
        let dim = Effect::new(ondin_core::EffectKind::Filters(ondin_core::Filters {
            brightness: 0.5,
            ..Default::default()
        }));
        let shadow = Effect::new(ondin_core::EffectKind::DropShadow(ondin_core::Shadow {
            offset: kurbo::Vec2::new(0.0, 4.0),
            blur: 0.0,
            spread: 0.0,
            color: ondin_core::peniko::Color::BLACK,
            blend: BlendMode::Normal,
        }));
        let stack = ondin_core::keyed_by_position([dim, shadow]);
        let cut = detail_effects(std::borrow::Cow::Owned(stack), Affine::scale(0.2));
        assert_eq!(cut.len(), 1, "the 0.8 px shadow is cut");
        assert!(
            matches!(cut[0].kind, ondin_core::EffectKind::Filters(_)),
            "kept"
        );
    }

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
