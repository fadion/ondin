//! Scene building, shared by both backends (§6.3).
//!
//! Walks the document in z-order (culled to the viewport), converts each
//! visible node to a local `BezPath` plus its brushes, and issues draw calls to
//! a backend-agnostic [`ScenePainter`]. The CPU (`vello_cpu`) and GPU (`vello`)
//! backends each implement `ScenePainter`, so this walk — the actual scene
//! logic — is written once.
//!
//! Three things this layer gets right that a flatter walk cannot:
//!
//! - **Brushes, not colours.** Gradients travel through as `peniko::Brush` in
//!   the node's own local space, which is exactly the space the shape path is
//!   in, so no brush transform is needed and both backends can take them
//!   verbatim. The model has always been able to express gradients; folding
//!   them to a single colour here is what made them look broken. **An image is
//!   the exception**: its coordinates are its own pixel grid rather than the
//!   node's local space, so every draw carries a [`Framing`] beside the brush —
//!   see `framing_of`.
//! - **Artboards clip.** Content that overflows a frame is cut off, matching
//!   the artboard's role as a page rather than a suggestion — unless the layer
//!   turns it off (`Node::clip`), which also turns off subtree culling, since
//!   the frame no longer bounds what is inside it.
//! - **Opacity is a layer.** Folding a group's opacity into each child's alpha
//!   is only equivalent when nothing overlaps; two overlapping half-transparent
//!   children must not show through each other. Nodes below 1.0 get a real
//!   layer.
//!
//! Layers are not free, so they are pushed only when a node actually needs one.

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
/// No alignment field: by the time a stroke reaches a backend it is always
/// centred, because that is the only kind any 2D rasterizer has. Inside and
/// outside are built here, out of a doubled width and a clip (see
/// [`paint_strokes`]).
///
/// No `sides` field either, and for the same shape of reason: a per-side stroke
/// has already been resolved into one call per side, each along the open path
/// that side occupies, so a backend only ever sees a path and a width.
///
/// `dashes` is likewise the pattern as it will actually be drawn — the dot
/// substitution and the fit-to-corners scaling are `geometry::resolved_dashes`'s
/// and happen before this is built.
pub struct StrokePaint<'a> {
    pub brush: &'a Brush,
    /// How an image brush sits in the frame — [`None`] for every other brush,
    /// and for an image whose entry the document has lost. The frame is the
    /// *shape's* box, not the ink's: a stroke is a border on a picture's frame,
    /// so widening it must not re-frame the picture.
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
    /// Winding number ≠ 0. Every clip in the walk except the outside-stroke one.
    #[default]
    NonZero,
    /// Odd crossing count. Needed for "everything except this shape", which is
    /// what an outside stroke clips to — and unlike a winding trick, it does not
    /// care which direction the user happened to draw the path in.
    EvenOdd,
}

impl From<ondin_core::FillRule> for ClipRule {
    /// **The model's rule and the painter's are the same question**, asked once of a
    /// node's geometry and once of a clip path. They are two types because this one
    /// is the backend's vocabulary and predates the model having any rule at all —
    /// keeping them separate is what stops `ondin-render`'s painter API depending on
    /// the document model for an enum with two variants.
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
    /// The font blob (+ face index) to draw from, exactly as parley resolved it.
    pub font: &'a ondin_core::peniko::FontData,
    pub font_size: f32,
    /// Normalized variable-axis coordinates (i16, skrifa convention).
    pub coords: &'a [i16],
    pub brush: &'a Brush,
    /// As [`StrokePaint::framing`]: the type's own box is the frame, so an
    /// image-filled headline shows one picture across the whole of it rather
    /// than one per run.
    pub framing: Option<Framing>,
    pub glyphs: &'a [TextGlyph],
}

/// Backend-agnostic drawing sink. Coordinates in `path`/glyphs are LOCAL;
/// `transform` maps them to world space. Brushes are in the same local space,
/// so a backend passes a solid or a gradient through untouched.
///
/// **An image brush is the one that needs more than the brush**, because where
/// a gradient's coordinates are authored in local space a picture's are its own
/// pixel grid, which knows nothing of the shape it is filling. `framing` is that
/// missing map — source pixels → this local space — resolved by
/// [`ondin_core::ImageRef::framing`] once, here, so both backends and the SVG
/// writer answer the same. It is [`None`] for every other brush.
pub trait ScenePainter {
    /// Whether this painter can put `id`'s pixels on the page.
    ///
    /// **The walk owns the placeholder's drawing and the painter owns this
    /// knowledge**, which is the only split that keeps one drawing: the three
    /// unresolvable states (§15 D179) are all "the store has no pixels", and the
    /// store belongs to the backend while the geometry belongs here. A backend
    /// answering by drawing its own placeholder would be two drawings that have to
    /// be kept the same, on two backends the whole of §3 exists to keep identical.
    fn has_image(&self, id: &ondin_core::ImageId) -> bool;

    /// Fill `path`, deciding its interior by `rule` (§15 D239).
    ///
    /// **The rule is a parameter and not a mode set beside the call**, because both
    /// backends have one piece of global state for it and a mode is how the two come
    /// to disagree: `vello_cpu` keeps a fill rule on its context until something
    /// changes it, so a caller that set even-odd and forgot to restore it would tint
    /// every later fill on that backend and nothing on the other.
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

    /// Begin a composited group: everything until the matching [`Self::pop_layer`] is
    /// drawn offscreen, clipped to `clip` (in `transform`'s space) if given,
    /// then composited at `opacity`.
    fn push_layer(
        &mut self,
        transform: Affine,
        clip: Option<&BezPath>,
        rule: ClipRule,
        opacity: f32,
    );

    /// Begin a layer whose drawing is composited over the enclosing layer **as an
    /// alpha mask**: what is already there survives in proportion to the alpha
    /// drawn here, and the colour drawn here is discarded.
    ///
    /// `Compose::DestIn`, which both backends take as an ordinary `BlendMode` on
    /// their own `push_layer` — so this is one mechanism rather than two, and the
    /// GPU path needs nothing vello does not already do. Popped by
    /// [`Self::pop_layer`] like any other layer.
    ///
    /// **A method of its own rather than a blend argument on `push_layer`**,
    /// because it takes none of that function's other decisions: no clip, no fill
    /// rule, no opacity. Adding a fifth parameter that seven of the eight call
    /// sites would pass a default to is how a signature stops being readable.
    ///
    /// The caller owes it an enclosing layer that holds *only* what is to be
    /// masked. Without that isolation the composite reaches the page behind it and
    /// erases the artwork the mask was never pointed at.
    fn push_mask_layer(&mut self);

    /// Begin a layer whose drawing is **filtered** before it is composited —
    /// a node's effect stack (§5.3a). Popped by [`Self::pop_layer`].
    ///
    /// `bounds` is the node's ink box **in `transform`'s space**, already grown
    /// by [`ondin_core::effect::stack_escape`], so it is the region the filtered
    /// result may occupy rather than the region the layer draws into. A backend
    /// allocates from it; getting it from the walk rather than measuring the
    /// drawing means both backends allocate the same buffer for the same document
    /// and the CPU one can be a golden.
    ///
    /// **The default is to ignore the effects and composite plainly.** A layer is
    /// still pushed and popped, so the walk's push/pop pairing holds for a backend
    /// that has not implemented the passes yet — it draws the artwork unfiltered
    /// rather than dropping it, which is the failure mode worth having. Every
    /// backend that ships to a user overrides this.
    fn push_effect_layer(&mut self, transform: Affine, bounds: Rect, effects: &[Effect]) {
        let _ = (bounds, effects);
        self.push_layer(transform, None, ClipRule::NonZero, 1.0);
    }

    fn pop_layer(&mut self);
}

/// Walk the document and drive `painter`.
///
/// `overrides` carries uncommitted gesture state (§6.2, invariant 5). When it
/// is empty — every export path, and any frame without a live gesture — the
/// walk reduces exactly to the committed document, and the world transforms it
/// composes are bit-identical to the ones in `Resolved`.
///
/// **One transform is deliberately not**: an axis-aligned box that does not
/// already land on whole device pixels is rounded onto them by
/// [`snapped_box_world`], which is a function of the viewport and so cannot come
/// out of `Resolved` at all. A box already on the grid is returned untouched, so
/// the paragraph above still holds wherever it can.
pub fn build<P: ScenePainter>(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    painter: &mut P,
) {
    build_of(doc, res, vp, overrides, &[doc.root()], painter);
}

/// Walk **these subtrees** and drive `painter`, in the order given.
///
/// [`build`] is one call of this — a whole page is the root on its own — and the
/// general form is what an export of a *selection* needs: something has to be able
/// to say "only these layers", and a viewport cannot, since a box drawn round a
/// layer also contains whatever else happens to overlap it.
///
/// **The mirror of `ondin_export::svg::svg_of`**, deliberately and down to the
/// transform: each origin is painted under its **parent's** world transform, which
/// composes with the node's own local exactly once. Handing it the node's own world
/// instead applies that local twice — the bug §15 D258 records against the SVG
/// writer, which is the same expression in the other renderer and is why this one
/// is written to look like it.
///
/// Two consequences worth stating rather than discovering:
///
/// - **Ancestor clipping does not travel**, and **a mask is clipping.** A layer
///   inside a clipping frame, exported on its own, is not cut by that frame — the
///   walk starts at the layer — and a masked layer exported on its own comes out
///   whole for the same reason, since the mask is a *sibling* this walk was never
///   pointed at. Exporting the mask itself gives the shape it would have drawn,
///   which is the same rule read from the other end: an origin is artwork.
///   `svg_of` behaves the same way, so the two writers agree, and "export this
///   layer" meaning the whole layer is the answer a user expects anyway.
/// - **Nothing behind is painted**, including an artboard's background, unless the
///   artboard is itself an origin. That is what makes a selection export come out
///   on transparency without anyone asking for it.
///
/// Order is the caller's and is kept, for [`svg_of`](ondin_core)'s reason: the last
/// thing painted is on top, so a caller handing over a selection owes it in
/// document order. Nothing is deduplicated either — `build::outermost` is the
/// filter, and it is the caller's because this walk cannot know whether a group and
/// something inside it were both meant.
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

/// Whether axis-aligned boxes are snapped to whole device pixels — see
/// [`snapped_box_world`]. One switch, because the thing it trades is felt rather
/// than measured and the answer may well be "turn it off".
const SNAP_BOXES_TO_DEVICE_PIXELS: bool = true;

/// World → device pixels, the transform **both** painters apply on top of the
/// world transform this walk hands them (`cpu::CpuPainter::base`,
/// `gpu::GpuScenePainter::base`). Derived here so the walk can reason in the
/// space the rasterizer actually samples in; it must stay in step with those two.
pub(crate) fn device_base(vp: &Viewport) -> Affine {
    let sx = vp.pixel_size.0 as f64 / vp.view.width();
    let sy = vp.pixel_size.1 as f64 / vp.view.height();
    Affine::scale_non_uniform(sx, sy) * Affine::translate((-vp.view.min_x(), -vp.view.min_y()))
}

/// `world`, adjusted so a local `size` box lands on whole device pixels.
///
/// **This exists to kill the seam between two shapes that share an edge.** Vello
/// computes coverage per path and alpha-blends each one independently, so where a
/// shared edge falls mid-pixel each shape covers that pixel partially and what is
/// *behind* both leaks through the gap between them — the "conflation artifact",
/// vello's open issue #49. For an edge at fraction `f` across a pixel the leak is
/// `f(1-f)` of the fill→backdrop contrast, peaking at a quarter of it: a `#666`
/// pair on a white frame measured 102 → **140** at `f = 0.5`. It is not a geometry
/// bug and no antialiasing setting touches it — `Area`, `Msaa8` and `Msaa16` all
/// measured 140 on device, because vello's MSAA improves *coverage* and then
/// collapses it to a scalar before blending exactly as `Area` does.
///
/// Rounding both edges to the grid removes it rather than attenuating it: the two
/// shapes round the shared edge to the *same* integer, so they still meet exactly
/// and the edge they meet on carries no antialiasing to conflate. The property
/// that makes this work is that everything snaps to one global grid, so anything
/// that abutted still abuts — a frame's background closes up with the shapes on
/// it, and a child that sat flush against a frame's inner edge stays flush.
///
/// **Only for a box whose device transform is a pure scale and translate.** A
/// rotated or skewed shape has no axis-aligned edges to round and is returned
/// untouched, so a rotated pair still conflates; that residual is a diagonal
/// fringe rather than a straight run and is far less visible, but it is a real
/// gap and the reason this is a mitigation rather than a fix.
///
/// The displacement is bounded by half a device pixel **except where the collapse
/// floor fires**: a box thinner than a pixel has its far edge pushed out to a whole
/// one, so that edge can move by up to about one and a half, and the node's
/// local→device scale on that axis grows with it — a 2-unit bar at 25% goes from
/// 0.25 to 0.5, taking any ink measured in its own space (a stroke width, a corner
/// radius) with it. Both are sub-pixel wherever the floor can fire, which is the
/// only reason growing a hairline beats deleting it.
///
/// What it does cost is that the rounding changes as the view pans,
/// which makes a gap between two shapes breathe by a pixel and edges step rather
/// than glide. That is the trade [`SNAP_BOXES_TO_DEVICE_PIXELS`] switches off.
fn snapped_box_world(world: Affine, base: Affine, size: ondin_core::kurbo::Size) -> Affine {
    if !SNAP_BOXES_TO_DEVICE_PIXELS {
        return world;
    }
    let device = base * world;
    let [a, b, c, d, e, f] = device.as_coeffs();
    // Rotated or skewed: no axis-aligned edge to round, so leave it alone. Also
    // bail on a degenerate axis, where the box has no extent to preserve.
    const EPS: f64 = 1e-9;
    if b.abs() > EPS || c.abs() > EPS || a.abs() < EPS || d.abs() < EPS {
        return world;
    }
    // Round both edges of each axis to the grid, then rebuild the scale from the
    // rounded span. **A non-empty box must not round away to nothing** — at low
    // zoom a thin shape is a fraction of a pixel, and collapsing it would delete
    // a hairline rather than snap it — so a span that rounds to zero keeps one
    // pixel, in the direction the axis actually runs.
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
    // **A box already on the grid keeps its exact transform.** Not an
    // optimization: `build` promises the world transforms it composes are
    // bit-identical to `Resolved`'s, and the round trip below is a matrix inverse
    // that perturbs an untouched transform in the fourteenth decimal. That is
    // invisible on screen and very visible to `assert_preview_matches_commit`,
    // which compares a preview against a commit exactly.
    if sa == a && sd == d && se == e && sf == f {
        return world;
    }
    // Back into world space: the painters re-apply `base`, so hand them a world
    // transform that composes to the snapped device one.
    base.inverse() * Affine::new([sa, b, c, sd, se, sf])
}

/// The local box a node's outline is built from, for the kinds whose geometry
/// *is* an axis-aligned box and can therefore be snapped to the pixel grid.
///
/// Deliberately not every kind with a `size`. An ellipse's box edges are tangent
/// points rather than ink, and a polygon's are wherever its vertices happen to
/// land, so rounding them would move the drawing without ever making two shapes
/// meet — the abutment this is for is rectangles and frames.
fn snappable_box(kind: &NodeKind) -> Option<ondin_core::kurbo::Size> {
    match kind {
        NodeKind::Rect { size, .. } | NodeKind::Artboard { size, .. } => Some(*size),
        _ => None,
    }
}

/// How `brush` sits in `frame`, for the one brush kind that needs telling.
///
/// [`None`] for a solid or a gradient — both are already in the space the path
/// is in — and also for an image the document has no entry for. That second case
/// costs nothing to get wrong, because the same missing entry means the store
/// cannot produce pixels either and the draw is skipped; it is spelled as a
/// `?` rather than a fallback so that "the document does not know this image"
/// has exactly one answer.
///
/// **The intrinsic size comes from the document's table, not from the decode
/// cache.** The table stores it precisely so that layout never waits on a
/// decode (§5.5a), and the walk is layout: a picture that has not finished
/// decoding must still frame at the size it will be, or every image would jump
/// on the frame it first appears.
fn framing_of(doc: &Document, brush: &Brush, frame: Rect) -> Option<Framing> {
    let Brush::Image(img) = brush else {
        return None;
    };
    let entry = doc.image(&img.image.id)?;
    Some(img.image.framing(frame, entry.width, entry.height))
}

/// Whether `brush` is a picture this painter cannot draw — the merged condition
/// §15 D179 insists stays merged.
///
/// An unknown id, a link nothing has read off disk, and bytes that will not
/// decode are one state on purpose: they are one thing to fix and the user can
/// act on them the same way, so telling them apart would buy a distinction
/// nobody can use and cost three drawings to keep in step.
fn missing_picture<P: ScenePainter>(painter: &P, brush: &Brush) -> bool {
    match brush {
        Brush::Image(img) => !painter.has_image(&img.image.id),
        _ => false,
    }
}

/// Fill `path` with `brush`, or with the placeholder when the picture is missing.
///
/// **The one place a fill becomes ink**, so a frame's fills and a shape's cannot
/// disagree about what a lost picture looks like. That mattered more when the two
/// were different things in the model (§15 D400) and it is still the reason the
/// frame arm of [`paint_shape`] calls this rather than filling directly.
fn fill_or_placeholder<P: ScenePainter>(
    painter: &mut P,
    world: Affine,
    path: &BezPath,
    brush: &Brush,
    framing: Option<Framing>,
    rule: ClipRule,
) {
    if missing_picture(painter, brush) {
        // **The placeholder is non-zero whatever the shape's rule is**, and that is
        // deliberate: it is a grey slab standing in for a picture, not a reading of
        // the artwork. An even-odd placeholder would put holes in the one drawing
        // whose whole job is to be unmistakably solid.
        paint_missing(painter, world, path);
        return;
    }
    clipped(painter, world, framing, |p| {
        p.fill_path(world, path, brush, framing, rule)
    });
}

/// Draw the placeholder over `path`: a grey ground, a rim and a cross.
///
/// **Ink, not chrome** — it goes through the same `fill_path`/`stroke_path` the
/// artwork does, so `ondin export --png` shows it and both backends draw it from
/// one description (§15 D179; the SVG writer draws the same picture in markup,
/// off the same `missing_placeholder`). That is deliberate even though it means
/// an export can contain a mark the document does not describe: a picture you
/// cannot draw is a hole either way, and a hole nobody can see is the one that
/// gets shipped.
///
/// **Cut to the shape, not to its box.** An image fill in a circle is a circle,
/// so the rim follows the path and the cross is clipped by it — a rectangle drawn
/// over a round shape would say the placeholder is a layer of its own.
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
/// Only a letterboxing [`ondin_core::ImageFit::Fit`] does — peniko has no
/// transparent extend, so the band beside a contained picture is cut rather than
/// left to the sampler, which would smear the edge pixels across it. The layer
/// is pushed around the *one* draw, so two fills on a shape do not share a clip
/// neither of them asked for.
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

/// The clip a mask holds over the run of siblings above it, while that run is
/// being walked.
///
/// **A layer of its own, nested inside whatever the parent already pushed.** It
/// cannot be folded into the parent's clip: a parent has one clip for all of its
/// children and a mask has one for a *run* of them, so a frame with two masks in
/// it needs three different clips over three stretches of the same child list.
///
/// It closes and reopens with the parent's layer for the reason
/// `RenderOverrides::escape_clip` gives — a child on its way out of a frame draws
/// unclipped and unmasked, in its own place among its siblings — which is why
/// this remembers the path rather than just whether a layer is open.
#[derive(Default)]
struct MaskRun {
    kind: Option<RunKind>,
    open: bool,
}

/// What a run is masked *by*, which is a different layer arrangement per mode.
enum RunKind {
    /// [`MaskMode::Shape`]: the mask's outline in the **parent's** space, pushed
    /// as the layer's clip. One layer, and the mask itself is never drawn.
    Clip(BezPath),
    /// [`MaskMode::Alpha`]: the mask **node**, to be drawn once the run is
    /// complete and composited over it with [`ScenePainter::push_mask_layer`].
    ///
    /// A node rather than geometry, because an alpha mask is not geometry: what
    /// matters is the *ink* it lays down — its fills, its gradients, its
    /// photograph's own alpha channel — so the only way to ask for it is to draw
    /// it. Which is also why this arrives at the *end* of the run: the mask
    /// composites over what it masks, so what it masks has to be there first.
    Alpha(NodeId),
}

impl MaskRun {
    /// Push the run's layer if it has one and is not already open. `world` is the
    /// **parent's** world transform, the space a clip path is in.
    ///
    /// **Opacity 1.0 in both arms, and that is a decision rather than a default.**
    /// Neither mode reads the mask node's own `opacity`. For a shape mask the
    /// outline says which pixels survive and nothing about how much of them does;
    /// for an alpha mask the answer comes from the ink, and `opacity` is already
    /// part of that ink when the mask is drawn — folding it in here as well would
    /// apply it twice.
    ///
    /// The alpha arm pushes a layer with **no clip**: it is an isolation group, so
    /// that the composite at the end of the run reaches the run's own drawing and
    /// not the page behind it.
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

    /// The mask node whose ink must be composited over this run **before** its
    /// layer is popped, or `None` for a shape run or a run that never opened.
    ///
    /// Split out rather than done inside [`Self::close`] because drawing it is a
    /// recursive walk needing the whole of `paint_node`'s context, which this
    /// struct deliberately does not hold.
    fn alpha_to_composite(&self) -> Option<NodeId> {
        match self.kind {
            Some(RunKind::Alpha(id)) if self.open => Some(id),
            _ => None,
        }
    }

    /// Pop the run's layer if it is open. Idempotent, so a caller can close
    /// without first asking whether there is a run at all.
    fn close<P: ScenePainter>(&mut self, painter: &mut P) {
        if self.open {
            painter.pop_layer();
            self.open = false;
        }
    }
}

/// Whether `id` is drawn, once a live preview has had its say.
///
/// **Named because both mask modes owe the same question and only one of them
/// asked it** (§15 D566). This is the expression [`mask_geometry`] opened with;
/// the `Alpha` arm of `paint_node`'s mask branch consulted no visibility at all,
/// so hiding an alpha mask erased everything it masked instead of revealing it.
/// One rule in one place rather than the same line written twice, which is how
/// the two came to disagree in the first place.
///
/// `node` is `doc.get(id)`, passed rather than looked up again because both
/// callers already hold it.
fn visible(ov: &RenderOverrides, node: &ondin_core::Node, id: NodeId) -> bool {
    ov.get(id)
        .and_then(|o| o.visible)
        .unwrap_or_else(|| node.visible())
}

/// The outline a mask node clips with, in its **parent's** space, with any live
/// gesture taken into account.
///
/// **The mirror of [`ondin_core::Resolved::mask_path`]**, deliberately and arm
/// for arm, and the same relationship `build_of` has with `svg_of`: one reads the
/// committed tree, this one reads it *through* the overrides, so a mask being
/// dragged clips where the pointer is rather than where the last commit left it.
/// The precedence at each field is `Painted`'s — an override if there is one,
/// else the committed value — so the two answer identically when nothing is in
/// flight, which is what `assert_preview_matches_commit` holds them to.
///
/// ⚠️ **This run was sitting on [`visible`] for one commit** — the thirteenth
/// instance of `CLAUDE.md`'s insertion trap, and committed by the session that
/// wrote D566. `visible` was inserted by anchoring an `Edit` on *this* `fn` line,
/// the item below, so `mask_geometry` was left with no doc at all and `visible`
/// carried two. Merged run 21 lines — under the length ranking's 53-line floor,
/// so that sweep is blind to it — and the neighbour grep was not run on this one.
/// `arch-scribe` found it reading D566's brief against the code.
fn mask_geometry(
    doc: &Document,
    res: &Resolved,
    ov: &RenderOverrides,
    id: NodeId,
) -> Option<BezPath> {
    let node = doc.get(id)?;
    let over = ov.get(id);
    if !visible(ov, node, id) {
        return None;
    }
    let kind = over.and_then(|o| o.kind.as_ref()).unwrap_or(node.kind());
    let local = over
        .and_then(|o| o.transform)
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

/// [`mask_geometry`] for a ghost, which carries its derived geometry with it
/// rather than looking it up — the same difference [`paint_ghost`] has from
/// [`paint_node`], for the same reason.
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
    paint: &'a Paint,
    text: Option<&'a TextLayout>,
    /// The outline `Resolved` derived for a `Boolean` node, in its local space.
    ///
    /// Beside `text` and for the same reason: both are geometry the walk must be
    /// *handed* rather than compute. Shaping a text node here would re-run parley
    /// every frame; evaluating a boolean here would re-run the path arithmetic
    /// every frame, which is worse.
    path: Option<&'a BezPath>,
    /// The box a `Boolean` whose arithmetic was **abandoned** draws its
    /// placeholder in, in local space (§15 D298). `None` for every other kind and
    /// for a boolean that is correctly empty, which draws nothing on purpose.
    placeholder: Option<Rect>,
    /// How this node's geometry decides its interior (§15 D239). Read by the fills
    /// and by nothing else here: a stroke traces the outline whatever fills it, the
    /// glyph outlines of a text node are the font's non-zero contours, and a
    /// placeholder is a solid slab by definition.
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

    if !over.and_then(|o| o.visible).unwrap_or(node.visible()) {
        return;
    }

    // Cull anything whose world bounds miss the viewport. For a container this
    // also skips its whole subtree — but only when those bounds actually cover
    // the subtree: a group's are the union of its children, and a *clipping*
    // frame's are the limit of what its children can paint. A frame with
    // clipping off is neither, so its contents are free to sit outside it and
    // must be walked. Skipped inside a moving subtree, where the cached bounds
    // describe where the node *was*.
    // ⚠️ **A `Boolean` belongs on that list and was missing from it** (§15 D536).
    // `[S11.1-L4-05]`: it has children, does not clip and is neither `Group` nor
    // `Root`, so the predicate was `false` and the `if` below short-circuited
    // before ever consulting `ink_bounds` — **every boolean in the document was
    // walked, `paint_shape`d and encoded on every frame, wherever the view was.**
    // Measured in release against a counting painter: 800 booleans parked at
    // x ≥ 10,000 with the viewport at `(-1000,-1000)..(-500,-500)` issued **801**
    // fills and cost 0.106 ms of walk alone, against **1** fill and 0.0116 ms for
    // the same document built from groups. The 0.106 ms is the walk; each of those
    // 801 fills also clones a whole `BezPath` at `paint_shape`, sized by the
    // boolean's own segment count — which is why a page of converted icon artwork
    // is the worst case rather than a contrived one.
    //
    // **And a boolean is the *strongest* member of this list, not a doubtful
    // one.** The binding's criterion is that the bounds cover everything below;
    // the walk stops dead at a boolean (*"a boolean's children are its inputs, not
    // artwork"*, `paint_node`'s own arm below), so its cached ink bounds cover its
    // whole subtree exactly, where a group's merely happen to.
    //
    // ⚠️ **This is CLAUDE.md's named hazard, one crate over from where it was last
    // swept**: *"adding a `NodeKind` is a sweep the compiler only half-finds — grep
    // the `matches!(kind` predicates after it goes quiet."* `[S2.1-MATRIX]` built
    // that matrix for `ondin-core`'s six `_`-armed predicates and `ondin-render`
    // was not in its scope.
    let bounds_cover_subtree = node.children().is_empty()
        || node.clip()
        || matches!(
            node.kind(),
            NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. }
        );
    // A node holding a ghost is never culled: the ghost is a layer that does
    // not exist yet, so the cached bounds cannot know about it. This is not
    // hypothetical — a frame being dragged out hangs off the root, and the
    // root's bounds are the committed artwork's, so the whole walk returned
    // before reaching the ghost whenever the new frame was drawn clear of
    // everything already on the page.
    // **`ink_bounds`, not `world_bounds`** (§5.3a). The cull asks "could this
    // node put anything on screen", and a drop shadow is ink that lands outside
    // the layer's own box — so a shadowed layer scrolled just past the edge of
    // the viewport would be skipped with its shadow still reaching into it, and
    // the shadow would pop in and out as the layer crossed the boundary. The two
    // boxes are equal for any node with no effect beneath it, so this is inert
    // until the first shadow.
    if bounds_cover_subtree
        && !ov.moved(id)
        && ov.ghosts_of(id).next().is_none()
        && let Some(bounds) = res.ink_bounds(id)
        && !intersects(bounds, view)
    {
        return;
    }

    let local = over
        .and_then(|o| o.transform)
        .unwrap_or_else(|| node.transform());
    let world = parent_world * local;
    let opacity = over.and_then(|o| o.opacity).unwrap_or(node.opacity());

    let painted = Painted {
        kind: over.and_then(|o| o.kind.as_ref()).unwrap_or(node.kind()),
        paint: over.and_then(|o| o.paint.as_ref()).unwrap_or(node.paint()),
        // Committed only: a preview cannot change the rule (`absorb` refuses
        // `SetFillRule`), so there is no override to prefer here.
        rule: node.fill_rule().into(),
        text: over
            .and_then(|o| o.text.as_ref())
            .or_else(|| res.text_layout(id)),
        // The preview's outline when a gesture is reshaping this boolean, else the
        // committed one — the same precedence every other field here follows.
        path: ov.boolean_path(id).or_else(|| res.boolean_path(id)),
        // **Committed only, and gated on the preview having nothing to say.** A
        // gesture that is re-evaluating this boolean always leaves an entry — the
        // new outline, or an empty path when the result vanished mid-drag — so
        // `boolean_path` being `None` here is precisely "no preview owns this
        // node". Without the gate a drag would draw the placeholder over the *last
        // committed* operand positions while the operands themselves are under the
        // pointer: a box lagging behind the thing it describes. Failing during a
        // gesture therefore looks the way it always has (nothing) until the release
        // that commits it, which is also the moment the user looks for the shape.
        placeholder: match ov.boolean_path(id) {
            Some(_) => None,
            None => ondin_core::boolean_placeholder(doc, res, id),
        },
    };

    // **The snapped transform is this node's own ink, never its children's.**
    // Both halves matter. Its own, so a frame's background, its clip and its
    // stroke are all the same rounded box and cannot part company. Not its
    // children's, because the rounding is a scale as well as a translate: passed
    // down it would compound, and a child deep in a tree would drift by the sum of
    // its ancestors' roundings rather than by its own half pixel. A child snaps on
    // its own terms against the same grid, which is what keeps it flush with a
    // parent edge it was flush with.
    // **Unless this layer is the one a chrome ghost is glued to** (§15 D272): the
    // offcut is drawn as a second, much larger rectangle, and two boxes of
    // different sizes round differently — so the pair opts out together or the
    // offcut slides against the picture it continues.
    let shape_world = match snappable_box(painted.kind).filter(|_| !ov.unsnapped(id)) {
        Some(size) => snapped_box_world(world, base, size),
        None => world,
    };

    // An artboard is a page: it clips its contents to its own frame — unless
    // the layer says otherwise (`Node::clip`, the inspector's "Clip content").
    // The frame's own outline, which is also what its stroke is drawn along — one
    // construction rather than the rect built again here.
    let clip = match painted.kind {
        NodeKind::Artboard { .. } if node.clip() => geometry::local_path(painted.kind),
        _ => None,
    };
    // **A frame's own stroke is painted after its contents and outside its clip.**
    //
    // Both halves are the whole of what makes frame strokes work (§15 D144). Over the
    // contents, because that is where a border belongs and what makes an inside
    // stroke on a clipping frame read as a frame at all — under them it would be
    // covered by the very children it is meant to enclose. And outside the clip,
    // because the frame's own clip is its own box: a centred stroke would lose its
    // outer half to it and an outside stroke would be erased entirely, which is
    // exactly the drawing that reads as "frames still draw no strokes".
    let frame_stroke = matches!(painted.kind, NodeKind::Artboard { .. })
        && painted.paint.strokes.iter().any(|s| s.visible);
    // The opacity moves to an outer group in that case, so the frame still
    // composites as *one* unit: stroke and contents in separate groups would let a
    // translucent stroke blend against its own contents rather than with them. One
    // extra layer, paid only by a frame that has both a stroke and an opacity.
    // **The effect layer is the outermost of this node's layers**, outside the
    // opacity and outside the clip. Both positions are load-bearing:
    //
    // - Outside the **clip**, so a clipping frame's drop shadow falls outside the
    //   frame instead of being cut off at the very edge it is drawn to escape.
    //   The SVG writer needs a wrapper element for exactly this reason, and the
    //   two are the same decision spelled twice.
    // - Outside the **opacity**, so the shadow is cast from the layer as it
    //   actually appears. A half-transparent layer therefore casts a
    //   half-transparent shadow — the silhouette it is cast from carries the
    //   faded alpha — rather than a solid one from a ghost. (The two orders
    //   happen to agree for a plain fade, since one halves the silhouette and the
    //   other halves the composite; they part company as soon as a `Filters` row
    //   is in the stack, and this is the order that keeps "the shadow follows
    //   what you can see".)
    //
    // Ghosts get theirs in `paint_ghost`; an effect that only exists on a node in
    // the document would leave an Alt-drag copy shadowless.
    let effects = over
        .and_then(|o| o.effects.as_deref())
        .unwrap_or(node.effects());
    // `Some` exactly when a layer was pushed, so the pop below cannot drift from
    // the push: a node with effects but no cached bounds draws **unfiltered**
    // rather than not at all, which is the safer of the two failures and is the
    // state a node has for the frame it is created in.
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
    // **The clip layer closes and reopens around a child that is leaving.**
    //
    // A layer being dragged out of a frame has to draw unclipped — see
    // `RenderOverrides::escape_clip` — and it has to draw *in its own place among
    // its siblings*, because which shape covers which is what the user is steering
    // by. Those two together mean the clip cannot simply be dropped (its siblings
    // still need it) nor the child deferred to the end (that would reorder it), so
    // the layer is popped before the child and pushed again after.
    //
    // One consequence, and it is why this tracks `open` rather than assuming: a
    // frame at less than full opacity composites in two pieces while a child is on
    // its way out, so two of its children overlapping *across* that boundary blend
    // against each other instead of within one group. It takes a semi-transparent
    // clipping frame plus overlapping children plus a drag to see, and the
    // alternative is the layer staying visibly cut in half.
    //
    // **A mask run closes and reopens with it**, always inside it: a mask clips a
    // stretch of this node's children, so its layer belongs under whatever this
    // node pushed for all of them. The pair goes down and comes up in step —
    // mask first out, mask last in — or the pops would unwind in the wrong order.
    let mut open = false;
    let mut mask = MaskRun::default();
    let reopen = |painter: &mut P, open: &mut bool, mask: &mut MaskRun| {
        if needs_layer && !*open {
            painter.push_layer(shape_world, clip.as_ref(), ClipRule::NonZero, inner_opacity);
            *open = true;
        }
        mask.reopen(painter, world);
    };
    // **Ends the mask run and nothing else**, composing an alpha mask's ink over
    // it first. The composite has to happen inside the run's own layer and after
    // everything the run drew, which is exactly here and nowhere else.
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

    // **A boolean's children are its inputs, not artwork.** They are consumed by
    // the operation, so the container draws the result and the walk stops: the
    // operands keep their own geometry and paint in the document — which is what
    // makes the container non-destructive and releasable — and none of it reaches
    // the canvas.
    //
    // Missed on the first pass, and it made the whole feature look broken rather
    // than subtly wrong. The operands were drawn *over* the result, so Subtract,
    // Intersect and Exclude appeared to do nothing at all (the result was there,
    // hidden underneath), and Union looked like it worked while dragging an operand
    // seemed to "carry the original and leave the boolean behind" — the drag was
    // moving a child that should never have been visible.
    if matches!(painted.kind, NodeKind::Boolean { .. }) {
        // Through `close` rather than popping by hand, so a reader does not have to
        // work out whether a mask run could be open here. It cannot — a run only
        // ever begins in the children loop below, which this returns before — and
        // saying that with the same helper is cheaper than saying it in a comment.
        close(painter, &mut open, &mut mask);
        debug_assert!(!outer_opacity, "only a frame opens the outer opacity layer");
        // ⚠️ **And the effect layer, which this arm used to leak** (§15 D436).
        // `close` unwinds
        // the clip/opacity layer and the mask run, and the `debug_assert` above
        // shows the outer opacity layer was audited — the effect layer, pushed
        // eighty lines up and popped at the *end* of the function, is the third
        // one and the one this early return never reached. So a drop shadow on a
        // boolean — an ordinary inspector action — made the boolean **and every
        // layer painted after it** disappear: measured at 11 200 px → 0 and a
        // sibling at 2 304 px → 0, because everything afterwards was drawn inside
        // a layer that was never composited.
        //
        // **Not moved into `close`.** That helper is called from three other
        // places inside the children loop, where the effect layer must stay open
        // — it is the whole node's, not the run's.
        if effect_layer.is_some() {
            painter.pop_layer();
        }
        return;
    }

    // **Children and ghosts in one z-order.** A ghost carries the index its
    // `InsertSubtree` names, and it draws *there* rather than after everything: a
    // shape being dragged out with a create tool appends and so still lands on top,
    // while an Alt-drag copy lands immediately above its original and must preview
    // between that original and whatever sits over it. Ghosts used to be deferred to
    // the end, which was the same answer only while every caller appended.
    let mut ghosts: Vec<&crate::renderer::Ghost> = ov.ghosts_of(id).collect();
    ghosts.sort_by_key(|g| g.index);
    let mut next_ghost = 0usize;
    // `..=len` so anything indexed at or past the end still gets drawn, which is what
    // an appending caller asks for.
    for pos in 0..=node.children().len() {
        while let Some(ghost) = ghosts.get(next_ghost).filter(|g| g.index <= pos) {
            if ov.ghosts_escape_clip() {
                close(painter, &mut open, &mut mask);
            } else {
                reopen(painter, &mut open, &mut mask);
            }
            paint_ghost(doc, ov, &ghost.node, world, base, painter);
            next_ghost += 1;
        }
        let Some(child) = node.children().get(pos) else {
            break;
        };
        // **A mask is not artwork, and this is the whole of what that means
        // here**: the walk does not descend into it, so neither it nor anything
        // inside it reaches the canvas, and what it contributes instead is the
        // clip every sibling after it draws under.
        //
        // Only its own layer is closed, not the parent's: the run ends, the
        // frame's clip does not.
        if let Some(node) = doc.get(*child)
            && node.mask()
        {
            end_run(painter, &mut mask);
            mask.kind = match node.mask_mode() {
                // No geometry means no clip, and no clip means the run above
                // draws unmasked — the reading `mask_path`'s `None` has
                // everywhere else.
                MaskMode::Shape => mask_geometry(doc, res, ov, *child).map(RunKind::Clip),
                // No such test on the mask's **content** for an alpha mask:
                // whether it lays down any ink is not a question geometry
                // answers, and an *empty* alpha mask legitimately erases
                // everything above it. That is the mode being honest rather than
                // a case to guard.
                //
                // ⚠️ **Its `visible` flag is a different question and this arm
                // used to skip it too** (§15 D566). D282's rule opens with the
                // case: *"`None` means masks nothing, never 'clip everything
                // away': a **hidden mask**, an empty text node, an empty group and
                // a cancelled boolean all answer it"* — and only the `Shape` arm
                // honoured it, through `mask_geometry`'s first line. Measured on a
                // 100×100 white mask over a 200×200 red rect: hiding a `Shape`
                // mask gave the 40,000 px D282 asks for and hiding an `Alpha` mask
                // gave **0**, because `end_run` pushes a `DestIn` layer and then
                // draws nothing into it. Worse than invisible — `query::hit_test`
                // still returned the content and `world_bounds` was still the
                // hidden mask's own box, so one document behaved three
                // inconsistent ways at once.
                //
                // The asymmetry D282 does keep is about ink, not about the eye
                // icon: *"an alpha mask with **no ink** legitimately erases the
                // run"*. A mask the user has switched off has not been asked to
                // erase anything.
                MaskMode::Alpha => visible(ov, node, *child).then_some(RunKind::Alpha(*child)),
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
        // `shape_world`, not `world`: the border has to run along the very box the
        // background filled, or snapping the one and not the other would open a
        // half-pixel of daylight between a frame and its own edge.
        paint_strokes(
            doc,
            painter,
            shape_world,
            painted.kind,
            painted.paint,
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
/// `Resolved` caches it in world space, so this is the trip back — a trip rather
/// than a lookup because a backend allocates its buffer from `base * transform *
/// bounds`, and handing it a world rect would apply the world transform twice.
///
/// ⚠️ **Built from what is being *drawn*, never from `Resolved::ink_bounds`**, and
/// that is the whole of §15 D341. `ink_bounds` is the committed document's answer,
/// so sizing the buffer from it clipped the very effect the user was dragging —
/// a scrubbed blur drew into the buffer the *old* radius needed and snapped to its
/// full size the instant the drag committed. This function's doc used to say the
/// cache was stale only "for one frame of a drag" and that fixing it was not worth
/// a second bounds computation. Both halves were wrong: it is stale for the whole
/// drag, and the live answer costs one call to a function `Resolved` already uses.
///
/// So every input is the live one — `painted.kind` and `painted.paint` under a
/// geometry preview, `effects` under an effects preview, `world` under a move —
/// and `overrides::assert_preview_matches_commit` is what says preview and commit
/// ask for the *same* box rather than merely both being plausible.
///
/// **The one input that is still the cache is a container's children**, because
/// the walk has not reached them yet: a `Group`'s ink is its children's ink, with
/// the mask clipping `Resolved` works out, and recomputing that here would be a
/// second copy of a rule with an edge case in it. So a preview that moves a child
/// *inside* a blurred group is a frame behind. Narrower than what it replaces, and
/// stated rather than discovered.
fn effect_bounds(
    res: &Resolved,
    id: NodeId,
    painted: &Painted<'_>,
    world: Affine,
    effects: &[ondin_core::Effect],
) -> Option<Rect> {
    let base = match painted.kind {
        NodeKind::Group | NodeKind::Root => res.inner_ink(id)?,
        // `world_bounds_of_parts` is the function `Resolved` measures a leaf with,
        // taken from loose parts precisely so a preview can call it — **including
        // the stroke expansion**, without which a stroked layer's buffer would be
        // short by half a stroke weight all round.
        //
        // It declines a `Boolean`, whose outline is derived rather than declared;
        // that falls back to the cache, which is the same "no live geometry for
        // this kind" answer the container arm gives.
        _ => ondin_core::geometry::world_bounds_of_parts(
            painted.kind,
            painted.paint,
            world,
            painted.text,
        )
        .or_else(|| res.inner_ink(id))?,
    };
    // The same arithmetic `Resolved` applies over the same base, so the committed
    // path through here lands on the number the cache would have held.
    let ink = ondin_core::effect::escaped(base, ondin_core::effect::stack_escape(effects), world);
    Some(world.inverse().transform_rect_bbox(ink))
}

/// The world box a ghost's own ink covers — [`effect_bounds`]'s answer, for a
/// node `Resolved` has never seen and never will (§15 D567).
///
/// Three arms, and they are `effect_bounds`' three:
///
/// - a **container** has no outline of its own, so its box is its children's,
///   each grown by that child's own stack. This is the arm the old code had no
///   equivalent of at all, and it is the expensive one — `res.inner_ink` is a
///   cache lookup where this is a walk. It is affordable because a ghost forest
///   is what one gesture is dragging, not the document.
/// - a **boolean** carries its evaluated outline on `GhostNode::path`, which is
///   the ghost's share of what `Resolved` keeps for a committed one.
///   `world_bounds_of_parts` declines a boolean for the same reason
///   `geometry::local_path` does.
/// - everything else goes through [`ondin_core::geometry::world_bounds_of_parts`],
///   which is the leaf measurement **with the stroke expansion in it**.
///
/// ⚠️ **A hidden or masking child contributes nothing**, which is the same filter
/// [`ghost_mask_geometry`] applies to a container's operands: a mask is not
/// artwork, so a buffer sized to include one would be sized for ink that is never
/// laid down.
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

/// Draw a ghost subtree. The same walk as [`paint_node`] with the two things a
/// ghost does not have taken out: its children are right here rather than looked
/// up, and there are no cached bounds to cull against — nor any use for them,
/// since a ghost exists precisely because the user is looking at it.
///
/// `doc` is still wanted, for the one thing that is a property of the document
/// rather than of the node: the image table an image fill's framing reads its
/// intrinsic size out of. A ghost carries a `Paint` like any other node, so a
/// dragged copy of a photograph has to frame the same way the original does.
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
    let world = parent_world * ghost.transform;
    // **A ghost snaps exactly as the node it will become does.** It is the same
    // pixel grid and the same rule, and skipping it here is not the harmless
    // omission it looks like: a preview that does not snap draws half a pixel away
    // from what release commits, so a shape would twitch at the instant you let go
    // of it. `assert_preview_matches_commit` is the test that says so.
    // **The same exemption the node arm takes** (§15 D272). A ghost that is about
    // to *become* a node rounds the way that node will, or the shape twitches at
    // the instant of release; the chrome ghost is not becoming anything, and has to
    // stay glued to the layer it continues instead.
    let shape_world = match snappable_box(&ghost.kind).filter(|_| !ov.unsnapped(ghost.id)) {
        Some(size) => snapped_box_world(world, base, size),
        None => world,
    };
    let clip = match &ghost.kind {
        NodeKind::Artboard { .. } if ghost.clip => geometry::local_path(&ghost.kind),
        _ => None,
    };
    // The same two-layer arrangement `paint_node` uses, and for the same reason: a
    // frame dragged out of the page previews with its border, over its contents and
    // outside its own clip.
    let frame_stroke = matches!(ghost.kind, NodeKind::Artboard { .. })
        && ghost.paint.strokes.iter().any(|s| s.visible);
    // **A ghost's effects, measured rather than looked up.** `paint_node` gets its
    // box from `Resolved`, which has never seen this node and never will — a ghost
    // exists precisely because the artwork is not in the document yet. So the box
    // is built here from the ghost's own outline, grown by the stack, which is
    // what the cache would hold once the gesture commits.
    //
    // Without this an Alt-drag copy of a shadowed layer draws flat for the whole
    // drag and gains its shadow at the instant of release — the same class of
    // twitch the snapping note above is about, and just as visible.
    // ⚠️ **Through [`ghost_ink`] rather than `local_path`, and grown the way
    // [`effect_bounds`] grows it** (§15 D567). This read
    // `geometry::local_path(&ghost.kind)?.bounding_box()`, which is wrong in two
    // ways at once and the paragraph above described neither:
    //
    // - `local_path` answers `None` for `Root | Group | Text` and `Boolean` — the
    //   arm is explicit — so the `?` swallowed the whole computation and a ghost
    //   of any of those pushed **no effect layer at all**. Alt-dragging a copy of
    //   a shadowed *group* previewed flat for the whole drag, which is exactly the
    //   twitch this binding exists to prevent, for four of the eleven kinds.
    // - The outline's box is not the ink's box. `world_bounds_of_parts` is what
    //   `Resolved` measures a leaf with **including the stroke expansion**, and a
    //   40pt-stroked rect's ghost buffer came out 20 units short on every side —
    //   half the stroke, which is what `effect_bounds`' own comment says it exists
    //   to include.
    //
    // Measured against `RenderOverrides::from_transaction` of the `InsertSubtree`
    // an Alt-drag commits: a group pushed 1 effect layer where the committed pair
    // pushes 2, and the stroked rect's box was `(-12,-4)–(112,120)` against the
    // committed `(-32,-24)–(132,140)`.
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
            paint: &ghost.paint,
            // **Non-zero, and a ghost has no field for anything else.** A `GhostNode`
            // carries what the scene walk reads off a `Node`, and nothing that builds
            // one can author a rule: a shape tool drags out a default node, and an
            // Alt-drag copy of an `Exclude` previews as its *outline* (`GhostNode::path`
            // is evaluated once when the subtree is captured), not as its operands. If
            // a ghost ever carries an even-odd shape, this needs a field beside `path`.
            rule: ClipRule::NonZero,
            text: ghost.text.as_ref(),
            // Evaluated when the ghost subtree was built, for the reason the
            // committed walk reads a cache here: path arithmetic per frame is worse
            // than shaping per frame. See [`GhostNode::path`].
            path: ghost.path.as_ref(),
            // **Never for a ghost**, and it is the same call the committed walk
            // declines to make while a gesture is in flight: a ghost *is* a gesture,
            // it has no id in the document to ask `boolean_failed` about, and a copy
            // being dragged is not the place to report a defect in the original. It
            // draws nothing, exactly as it did before this existed, and the original
            // underneath carries the mark.
            placeholder: None,
        },
        shape_world,
        painter,
    );
    // **A ghost boolean's children are its inputs too**, and the walk stops at it
    // exactly as `paint_node` does. Missing this was the visible half of the bug: the
    // copy an Alt-drag carried drew the *operands* — every shape the operation was
    // supposed to consume, in full, over a result that was not there — so a
    // subtracted bite looked whole while it was being dragged and appeared at the
    // instant of release. Reported as "the clone shows the original shapes".
    if matches!(ghost.kind, NodeKind::Boolean { .. }) {
        if needs_layer {
            painter.pop_layer();
        }
        // ⚠️ **The committed walk's twin, unwound the same way and for a
        // different reason.** `paint_node`'s boolean arm leaked its effect layer
        // and the boolean plus everything after it vanished; this one leaks two,
        // and it used to be saved by `geometry::local_path` answering `None` for
        // `Boolean`, which made `ghost_effects` always `None` here. That was a
        // fact about a function two modules away, not about this arm — so the
        // unwind was written rather than argued away, against the day a ghost
        // boolean got a path.
        //
        // ⚠️ **That day was §15 D567.** `ghost_ink`'s boolean arm reads
        // `GhostNode::path`, so `ghost_effects` can be `Some` here now and both
        // pops below are load-bearing rather than precautionary. The prediction
        // was right and the sentence that made it is what has to change; this is
        // the comment `arch-scribe` found still asserting the old answer.
        if outer_opacity {
            painter.pop_layer();
        }
        if ghost_effects.is_some() {
            painter.pop_layer();
        }
        return;
    }
    // **A ghost masks its own children**, and it has to: an Alt-drag copy of a
    // masked group is one of the ordinary things to do with a mask, and a preview
    // that showed the whole photograph until the instant of release would be the
    // same twitch the pixel snapping above exists to prevent, several hundred
    // pixels wide. There is no `escape_clip` here — nothing leaves a ghost — so
    // the run is the plain version of `paint_node`'s.
    let mut mask = MaskRun::default();
    // The same two-step end the committed walk uses: an alpha mask's ink goes
    // over the run before the run's layer is popped.
    let end_run = |painter: &mut P, mask: &mut MaskRun, ghost: &GhostNode| {
        if mask.alpha_to_composite().is_some() {
            painter.push_mask_layer();
            paint_ghost(doc, ov, ghost, world, base, painter);
            painter.pop_layer();
        }
        mask.close(painter);
    };
    // The ghost the open run is masked by, since `RunKind::Alpha` can only carry
    // an id and a ghost forest is not looked up by one.
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
                // The same visibility test `paint_node`'s arm makes, for the same
                // reason (§15 D566) — and `alpha_ghost` has to stay `None` too,
                // or `end_run` pushes the `DestIn` layer this arm just declined.
                // `ghost_mask_geometry` already refuses a hidden ghost on the
                // `Shape` side, so a preview and its commit would otherwise
                // disagree exactly where `assert_preview_matches_commit` looks.
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
        // `shape_world`, as in `paint_node`: the border runs along the box the
        // background filled.
        paint_strokes(
            doc,
            painter,
            shape_world,
            &ghost.kind,
            &ghost.paint,
            &frame,
            box_,
        );
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
/// **Two shapes of call, and which one is taken is a property of the run rather
/// than of the node.** Ordinary horizontal type goes out as a single
/// [`ScenePainter::draw_text`] with every glyph in it, exactly as it did before
/// type on a path existed — a rotation of zero takes that arm, so nothing that was
/// fast became slow. A bent run cannot: [`ScenePainter::draw_text`] carries **one**
/// transform for the whole run, which is all either backend's glyph API offers, so
/// each glyph goes out under its own.
///
/// **The split lives here and not at the render boundary**, which is what kept
/// this feature from touching the backends at all. `draw_text` already took a
/// transform per call, so the walk had the whole vocabulary it needed; adding a
/// per-glyph transform to [`TextRun`] would have meant teaching both backends to
/// split a run, twice, identically.
///
/// ⚠️ **A per-glyph transform moves the brush with the glyph, so a bent run under
/// anything but a solid is filled as an outline instead.** A gradient and an image
/// are both authored in the node's local space (see [`ScenePainter`]'s doc), and a
/// glyph drawn under its own transform carries that space with it — every letter
/// would restart the gradient at its own origin, which reads as the fill having
/// come apart rather than as type being bent. `text::run_outline` puts the same
/// ink through `fill_path` in the space the brush was authored in. A solid has no
/// position to lose, which is why it keeps the cheaper arm rather than everything
/// taking the safe one.
fn draw_run(
    painter: &mut impl ScenePainter,
    world: Affine,
    run: &ondin_core::text::GlyphRun,
    brush: &Brush,
    framing: Option<Framing>,
) {
    if run.glyphs.iter().any(|g| g.rot != 0.0) && !matches!(brush, Brush::Solid(_)) {
        // Non-zero, like every other glyph outline the walk fills: the contours
        // are the font's and the font's convention is non-zero, so an even-odd
        // reading would fill the counter of every `o`.
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
        // The placement moves into the transform and the glyph is drawn at its own
        // origin, because a rotation has to be applied *about* that origin — a
        // glyph left at `(x, y)` under a rotated transform would swing around the
        // node's origin instead, i.e. the further along the line a letter sat the
        // further it would be thrown.
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
/// **Built here rather than stored, and filled rather than stroked.** The four
/// styles are ours (parley draws no decoration at all); building them at the
/// render boundary is the same bargain a stroke's dash pattern makes (§6.3), and
/// filling keeps all four one code path — a stroked dash would need the cap
/// substitution `paint_stroke` does and would put a zero-length dash back in play.
///
/// The dash and dot periods are multiples of the band's own **thickness**, so a
/// heavier underline gets proportionally longer dashes instead of turning into a
/// dotted line, and the pattern survives a change of type size with no stored
/// numbers to go stale.
///
/// ## Skip-ink is a set of x-spans, and that is what makes it free here
///
/// `DecorationInk::gaps` says where the glyph ink crosses this band; core computed
/// it once, on the relayout, out of the glyph outlines (`text::skip_ink`, §15
/// D356). Everything below therefore reduces to interval arithmetic on the
/// **kept** spans, and the four styles keep their own character across a gap
/// rather than restarting at one:
///
/// - the dash and dot runs are generated over the whole band and *then* clipped,
///   so the phase is the band's, not the fragment's — three dashes with a gap
///   through the middle stay on the same rhythm as three without;
/// - the ribbon is sampled per fragment but its phase is still measured from
///   `band.x0`, so a wave interrupted by a descender comes back where it would
///   have been.
///
/// A gap-less band takes exactly the path it took before any of this existed: one
/// span covering the whole width.
fn decoration_path(ink: &ondin_core::DecorationInk) -> BezPath {
    use ondin_core::LineStyle;
    let band = ink.band;
    let t = band.height();
    if t <= 0.0 || band.width() <= 0.0 {
        return BezPath::new();
    }
    let kept = kept_spans(band, &ink.gaps);
    match ink.style {
        LineStyle::Solid => bars(band, &kept),
        // A run of rectangles: 3 thicknesses of ink, 2 of gap. The first dash
        // starts at the band's own left edge, so two adjacent runs of the same
        // style line up rather than each restarting its phase.
        LineStyle::Dashed => bars(band, &overlap(&dash_spans(band, t * 3.0, t * 2.0), &kept)),
        // Square dots at twice the thickness, centre to centre — the same
        // "a dot is a zero-length dash" reading the stroke panel uses, with the
        // period preserved.
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

/// What is left of `band`'s width once the skipped spans are taken out of it —
/// the whole width when nothing is skipped.
///
/// `gaps` arrives sorted and merged (core's `crossings` guarantees both), so this
/// is one pass. The `max` is belt and braces for a gap that starts before the
/// band: it cannot happen through `crossings`, which clamps, and it is one
/// comparison against reading a negative-width bar.
///
/// ⚠️ **A plain complement, and deliberately no judgement of its own.** How wide a
/// fragment has to be to be worth drawing, and how much air a gap gets, are decided
/// once in `ondin_core::text::crossings` — including the two rules that back those
/// cosmetics off where a band would otherwise show nothing (§15 D356). This function
/// held the "a bar shorter than the band is thick is not a bar" rule for one commit,
/// and it moved because the *back-off* needs both rules in one place: a fallback that
/// depends on whether a rule two crates away would empty the band is the gate written
/// down twice that this project keeps paying for.
///
/// What is left here is the pattern, which is the render boundary's business: the
/// dash and dot runs are met with these spans and the ribbon is sampled across them.
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

/// The overlap of two sorted, non-overlapping span lists — the dash run met with
/// what skip-ink left of the band.
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

/// A sine ribbon along `from..to` of `band`, appended to `path`: the curve sampled
/// as a polyline, offset half a thickness each way and closed.
///
/// One wavelength is six thicknesses and the amplitude is one — the proportions a
/// spell-check underline uses, which is the shape this style is recognised by. A
/// polyline rather than cubics because the ribbon is a few pixels tall: the
/// flattening error at this scale is far below the rasterizer's own, and two
/// offset curves would need a real offsetting pass to stay parallel.
///
/// ⚠️ **The phase is measured from `band.x0` and not from `from`.** A fragment is
/// what skip-ink left of the band, so a ribbon that restarted its phase at each
/// fragment would step out of line with itself across every descender — and, worse,
/// would put a crest where the gap was cut for a trough. This is also why the
/// ribbon's own reach is `1.5t` from the band's middle rather than the band's own
/// `0.5t`: `text::ink_extent` in core measures the gaps against that number, and
/// `tests::a_wavy_ribbon_stays_inside_the_reach_core_assumes` is what keeps the
/// two from drifting. **Plain backticks, not a link**: the target is inside a
/// `#[cfg(test)]` module, which rustdoc cannot see under any flag (§15 D319), so a
/// `[link]` there is decoration no gate can validate — the same convention
/// `canvas::CanvasRenderer::gpu` states.
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
    // **A frame's fills, in list order, behind its children.** The clip layer is
    // already in place. Its *strokes* are not here — a frame's border is painted
    // over its children and outside its own clip, which only `paint_node` is in a
    // position to do (§15 D144), and that split is the only thing left that makes
    // a frame's paint different from a rectangle's.
    //
    // ⚠️ **This is why a frame cannot simply fall through to the generic tail
    // below** (§15 D400): that tail fills *and then strokes*, in one place,
    // between this node's children and nothing. A frame's two halves happen at two
    // different depths of the walk, so the fill loop is written here and the
    // stroke call stays in `paint_node`. The loop itself is the tail's, and the
    // two are kept side by side deliberately — if one grows a rule the other has
    // to.
    if let NodeKind::Artboard { size } = painted.kind {
        let frame = Rect::new(0.0, 0.0, size.width, size.height);
        let path = frame.to_path(TOLERANCE);
        for fill in painted.paint.fills.iter().filter(|f| f.visible) {
            let framing = framing_of(doc, &fill.brush, frame);
            // A frame's outline is a rectangle: one subpath, so the node's fill
            // rule has nothing to decide and non-zero is the honest constant.
            fill_or_placeholder(
                painter,
                world,
                &path,
                &fill.brush,
                framing,
                ClipRule::NonZero,
            );
        }
        return;
    }

    // Text: draw the glyph runs `Resolved` already shaped (§5.9 — never shape
    // during the scene walk; that would re-run parley for every text node,
    // every frame). Each visible fill is a pass over the same runs, so stacked
    // paints work on type exactly as they do on a shape; an unpainted text node
    // still draws, in black, because ink that vanished when its one fill was
    // hidden would look like the text itself had gone.
    if let NodeKind::Text { .. } = painted.kind {
        let Some(layout) = painted.text else { return };
        let fallback = Brush::Solid(ondin_core::peniko::Color::BLACK);
        let inked = painted.paint.strokes.iter().any(|s| s.visible);
        let mut inks: Vec<&Brush> = painted
            .paint
            .fills
            .iter()
            .filter(|f| f.visible)
            .map(|f| &f.brush)
            .collect();
        // **The black fallback is for a text node with no ink at all**, not for one
        // with no fill. Outlined type is a stroke and no fill, and a fallback that
        // ignored the stroke list would fill every hollow letter in solid black —
        // which is the one drawing the feature exists to avoid.
        if inks.is_empty() && !inked {
            inks.push(&fallback);
        }
        // **`for run { for ink }`, and the inversion is the whole of per-run
        // colour.** `TextRun` was already per-run and both backends already honoured
        // its brush; what was node-level was this loop. A run with a colour of its
        // own draws **once**, in that colour; a run without one draws the node's
        // whole fill stack, exactly as every run did before.
        //
        // The asymmetry is deliberate (§15 D154): a node can carry two stacked
        // translucent fills and a run cannot, because a per-run *list* would need a
        // coalescing story for `Vec<Fill>` and its visibility flags. It is the same
        // trade `Decoration::color` has always made.
        //
        // **This changes paint order across runs, and only there.** Before: every
        // run in fill 1, then every run in fill 2. After: run 1 in fills 1..n, then
        // run 2. Identical wherever glyphs do not overlap, which is every ordinary
        // case; visible only where glyphs from adjacent runs overlap *and* the fills
        // are translucent. Stated rather than fixed — the alternative is two passes
        // and a per-run brush lookup in each.
        for run in &layout.runs {
            let own: Option<Brush> = run.color.map(Brush::Solid);
            // One-element slice rather than a branch around the draw, so the call
            // below is written once. No allocation: the array lives in `own_one`.
            let own_one = own.as_ref().map(|b| [b]);
            let run_inks: &[&Brush] = match &own_one {
                Some(one) => one,
                None => &inks,
            };
            for brush in run_inks {
                // The frame is the whole text box, not the run's advance: an
                // image-filled headline shows one picture behind the words,
                // which is what makes the ink read as cut out of it.
                let framing = framing_of(doc, brush, layout.bounds());
                clipped(painter, world, framing, |p| {
                    draw_run(p, world, run, brush, framing);
                });
            }
        }
        // **Decoration ink is filled here, not shaped here.** `Resolved` reports
        // where each band goes and how thick it is; how the ink is *broken up* —
        // solid, dashed, dotted, wavy — is ours, and is built at the render
        // boundary for the same reason a stroke's dash pattern is (§6.3): the band
        // and its length are both to hand here, where a baked-in pattern would go
        // stale the moment the type size was scrubbed.
        //
        // Drawn after the glyphs, so an opaque underline sits on top of a
        // descender rather than under it — **except where it skips one**, which is
        // the whole of skip-ink: `DecorationInk::gaps` has already taken the
        // crossing ink out of the band (§15 D356, `text::skip_ink`). The order still
        // matters, though, and this comment used to say it had stopped: an underline
        // whose skip-ink is switched off (§15 D357) hands over an empty `gaps` and is
        // painted across the descenders exactly as it was before that feature
        // existed. A strikethrough is painted straight over the letters in every
        // case, which is what `line-through` means.
        // **Type strokes through the same construction a shape's does**, on the
        // outline of its own glyphs (`ondin_core::text::outline`). That is what makes
        // `StrokeAlign::Outside` on text mean outlined type rather than nothing: the
        // model has always been able to say it, and both this walk and the SVG writer
        // used to drop it. The outlines are built here and only when there is a
        // visible stroke to draw with them — see the note on `text::outline`.
        //
        // Drawn after the fills and before the decorations, which is where a shape's
        // strokes sit relative to its fills. A decoration band is separate ink at a
        // separate weight and is deliberately not stroked with the glyphs; stroking it
        // would put a second, differently-shaped rule under every underline.
        if inked {
            let outline = ondin_core::text::outline(layout);
            if !outline.is_empty() {
                // The text box, not the outline's — see [`paint_strokes`].
                paint_strokes(
                    doc,
                    painter,
                    world,
                    painted.kind,
                    painted.paint,
                    &outline,
                    layout.bounds(),
                );
            }
        }
        for ink in &layout.decorations {
            let path = decoration_path(ink);
            // **A band is warped where a glyph is carried.** Type on a path bends
            // the plane, and a decoration runs *along* the line rather than sitting
            // at a point on it, so its ribbon has to follow the curve — the one
            // consumer `TextLayout::warp` exists for (§15 D405). The band is still
            // built flat, just above: the dash pattern, the wave and the skip-ink
            // gaps are all decided in the space they were measured in, and only the
            // finished outline is mapped.
            let path = match &layout.warp {
                Some(warp) => warp.warp(&path),
                None => path,
            };
            // The decoration's own colour, or the text's — the *first* ink, which
            // is the bottom of the fill stack and the one a single-fill node has.
            // A decoration is not repeated per fill: two stacked translucent fills
            // would double its opacity where they only tint the glyphs once.
            //
            // `inks` can now be empty — outlined type has no fill and gets no black
            // stand-in — and a decoration on it falls back to black rather than to
            // nothing, so an underline does not silently disappear with the fill.
            let own = ink.color.map(Brush::Solid);
            let brush = own.as_ref().or(inks.first().copied()).unwrap_or(&fallback);
            let framing = framing_of(doc, brush, layout.bounds());
            // **Glyph outlines are non-zero and a text node's rule does not reach
            // them.** The contours come from the font, whose own convention is
            // non-zero — an even-odd reading would fill the counters of every `o`
            // and `a`. A text node keeps a rule like any other node; it is the
            // *mask* it lends and the hit test that read it, not this.
            clipped(painter, world, framing, |p| {
                p.fill_path(world, &path, brush, framing, ClipRule::NonZero)
            });
        }
        return;
    }

    // **A boolean the arithmetic gave up on draws the missing-picture placeholder**
    // — the same grey ground, rim and cross, off the same `missing_placeholder`
    // (§15 D298). One drawing rather than two, because it is one fact about a
    // layer: it is there, and what it draws is not what it says it is. The layers
    // row makes the same choice with the same warning colour.
    //
    // Over the operands' box, since the outline that would have been its shape is
    // exactly what failed — see `boolean_placeholder`, which is also what the hit
    // test and `Resolved`'s bounds ask, so the drawing and the click target cannot
    // part company.
    //
    // Ahead of the path below rather than folded into it: this is *instead of* the
    // node's own paint, not a fill of it. A boolean with a red fill that cannot be
    // computed has no shape to put the red on.
    if let Some(area) = painted.placeholder {
        paint_missing(painter, world, &area.to_path(TOLERANCE));
        return;
    }

    // A boolean's outline comes in with the node; everything else builds its own
    // from its kind.
    let path = match painted.path {
        Some(derived) => derived.clone(),
        None => match local_path(painted.kind) {
            Some(path) => path,
            None => return,
        },
    };

    // **Every visible fill, then every visible stroke, in list order.** Two
    // stacks rather than one interleaved appearance list: `StrokeAlign::Outside`
    // covers the outlined-text case that would otherwise need a stroke under a
    // fill, and keeping the model's two `Vec`s as the z-order means the panel's
    // rows *are* the stack (`inspector::paint_rows` presents each list reversed,
    // so the topmost row is the last element and therefore the last painted).
    //
    // Fills are skipped on a line, which has no interior to fill.
    //
    // **The frame an image fill is framed in is this path's box**, taken once
    // and shared by the fills and the strokes: it is the shape's own bounds in
    // its own local space, which is exactly what `ImageRef::framing` is
    // documented to want. A boolean uses its derived outline's box, which is the
    // shape a user sees and therefore the one they crop against.
    let frame = path.bounding_box();
    if !matches!(painted.kind, NodeKind::Line { .. }) {
        for fill in painted.paint.fills.iter().filter(|f| f.visible) {
            let framing = framing_of(doc, &fill.brush, frame);
            fill_or_placeholder(painter, world, &path, &fill.brush, framing, painted.rule);
        }
    }

    paint_strokes(
        doc,
        painter,
        world,
        painted.kind,
        painted.paint,
        &path,
        frame,
    );
}

/// Draw every visible stroke of `paint` on `shape`, in list order, building
/// inside/outside alignment out of a centred stroke.
///
/// No 2D backend in the stack has stroke alignment, so the construction is
/// always the same: stroke at **twice** the width — which reaches a full width
/// either side of the outline — and clip away the half you do not want. What is
/// left is exactly a width of paint on the chosen side.
///
/// **One clip per run of same-aligned strokes, not one per stroke.** A clip is a
/// geometric mask, so applying the same one twice is applying it once, and two
/// outside strokes on a shape want the same mask: the layer is pushed for the whole
/// *run* of consecutive strokes that agree about alignment, which is one push/pop
/// where the shape used to pay N — and one complement path built per shape per
/// frame where it used to build one per stroke. Runs rather than a sort, because
/// the list order **is** the z-order (§5.3) and grouping across a differently
/// aligned stroke would reorder the paint. The run's ring is padded for the
/// *widest* reach in it, which is sound because the ring only has to enclose the
/// ink: too large clips nothing, too small cuts bites out of a wide stroke (see
/// [`outer_bounds`]). §15 D146 has what it measures out at, and the finding that the
/// clip was never the dominant cost.
///
/// `shape` is both what gets stroked and what the clip is built from — except for a
/// per-side stroke, where the ink follows one edge and the clip is still the whole
/// shape: inside and outside are questions about the shape, not about the edge, so a
/// top-only outside stroke sits outside the rect rather than outside a line.
///
/// Alignment is honoured only where the kind has an interior to be inside of
/// (`geometry::stroke_align_applies`); an open path draws centred whatever the model
/// says, since a half-clipped stroke on one would just look like a stroke with a
/// bite taken out of it. Asked of the **kind** rather than measured off the path,
/// because a side path is open and the shape it belongs to is not.
///
/// `frame` is what an image brush is framed in, and it is a parameter rather than
/// `shape.bounding_box()` for the two callers where those differ: outlined **type**
/// is stroked along its glyph outlines and framed in its text box, so deriving it
/// here would put a picture at one scale through the letters and another behind
/// them. It is also the shape's box rather than the ink's, so widening a border on
/// a photograph does not re-frame the photograph.
fn paint_strokes<P: ScenePainter>(
    doc: &Document,
    painter: &mut P,
    world: Affine,
    kind: &NodeKind,
    paint: &Paint,
    shape: &BezPath,
    frame: Rect,
) {
    let visible: Vec<&Stroke> = paint.strokes.iter().filter(|s| s.visible).collect();
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
                let framing = framing_of(doc, &s.brush, frame);
                clipped(painter, world, framing, |p| {
                    lay_sides(p, world, kind, s, shape, framing, false)
                });
            }
            continue;
        }
        // "Everything but the shape": the shape plus a rect big enough to hold
        // anything, taken even-odd. Even-odd rather than a reversed winding because
        // the user's own path direction is unknown, and it gets a shape with holes
        // right — the hole *is* outside.
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
            let framing = framing_of(doc, &s.brush, frame);
            clipped(painter, world, framing, |p| {
                lay_sides(p, world, kind, s, shape, framing, true)
            });
        }
        painter.pop_layer();
    }
}

/// One stroke's ink: the whole outline, or one open path per side it occupies.
///
/// `doubled` is the aligned construction asking for twice the width — the half that
/// does not belong has already been clipped away by the caller.
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

/// Lay `s` along `path` at `width`, with its dash pattern resolved for what is
/// actually being drawn.
///
/// The pattern is resolved against the path being stroked and the **nominal** width
/// being laid down — not the doubled one: a `Custom` side's own width decides how
/// long a square dot is, and fit-to-corners measures that side rather than the whole
/// outline.
///
/// **A fitted pattern on a multi-subpath path is laid subpath by subpath**
/// (`geometry::dash_fit_pieces`), because a fit scale is a property of one closed run
/// and one pattern cannot say two things. Every other stroke — which is nearly all of
/// them — goes down the single-pattern route and splits nothing.
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
        brush: &s.brush,
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
/// but for ink so wide that stroking it comes out wrong, by expanding the outline,
/// normalising its windings and filling the result.
///
/// **A very wide stroke leaves a hole in the middle of its own ink.** Reported with
/// a 309pt centred stroke on a 200×120 shape, and measured through the CPU backend
/// on a bare stroke with **no fill at all**, so it is a gap in the stroke's own
/// coverage rather than a fill painted over it:
///
/// - a 200×120 **ellipse** draws correctly to 200pt and is holed from 202pt, the
///   hole's half-width then being exactly `width/2 − 100` — one hundred is the
///   *larger* semi-axis, not the smaller and not the inradius;
/// - the same **rect** draws correctly to 320pt and is holed from 330pt, at
///   `width/2 − 160` — a hundred and sixty is half the width *plus* half the height.
///
/// **The mechanism is the inner offset turning inside out**, which `docs/roadmap.md`
/// predicted and which the numbers confirm — but not at the threshold it predicted.
/// Offsetting a closed contour inward by more than it has room for sends the inner
/// contour out through the far side; the region it then encloses nets to winding
/// **zero**, and a non-zero fill leaves it empty. Measured directly on
/// `kurbo::stroke`'s expansion of the ellipse above: winding 2 at the centre at
/// 120pt, 1 at 200pt, **0 from 220pt** — the same onset the pixels show.
///
/// So this cannot be fixed by expanding with kurbo instead: kurbo inverts the
/// contour in exactly the same place. What expanding buys is a *path*, and a path can
/// be taken apart — which is what the three steps below do, and why they are right
/// is that a stroke this wide has no interior left to keep.
///
/// **Gated, because this gives up the backend's own expander** — and a flo_curves
/// pass — for every hairline in the document. The gate is "the ink is at least as
/// wide as the shape is thin", past which the inward offset has nowhere left to go,
/// and it is deliberately *conservative*: on the ellipse above it fires at 120pt
/// where the bug starts at 202. Firing early costs work on artwork nobody draws by
/// accident; firing late would be the bug back again. It cannot fire late, because a
/// shape's inradius is never more than half its narrowest dimension.
/// `ui::MAX_STROKE_WIDTH` is 10 000, so widths like these are permitted on purpose
/// and "do not do that" is not an available answer.
///
/// **Closed paths only, and read off the path rather than off the node's kind.** An
/// open path encloses nothing, so it has no inward offset to invert — and its
/// bounding box can be flat, which would otherwise send every per-side stroke (each
/// drawn along one open edge, of a rect whose kind *is* closed) down the slow path
/// however thin it was.
///
/// **And solid strokes only.** A dashed stroke is a row of open segments, so none of
/// them has an interior to lose; and the plug below — the outline's own fill — would
/// fill the shape solid, which is the one thing a dash pattern exists not to do.
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
    // **The outline's own interior goes in with it**, and then every contour is wound
    // the same way, which is what turns a non-zero fill into a plain union.
    //
    // Both halves are load-bearing. Wound alike, no contour can cancel another, so
    // the inverted inner offset stops cutting its hole and merely re-fills something
    // already filled. And adding the outline closes the argument: an inner offset is
    // the outline pushed *inward*, so anything it encloses lies inside the outline —
    // and the outline's interior is wholly covered by ink once the gate above has
    // fired, since the gate is exactly "the ink is wider than the shape is thin".
    //
    // Two things that look simpler and are not. Filling the expansion alone
    // reproduces the hole, because kurbo's inner contour inverts exactly where the
    // backend's does. And handing the expansion to flo_curves — either
    // `path_remove_interior_points` or a union against the outline — left the
    // inverted contour standing at some widths and produced almost no ink at all at
    // others: measured, an ellipse still holed at 220pt and a rect at 400, and by
    // 1000pt the union had thrown away everything outside the shape.
    ink.extend(path.iter());
    // **Non-zero, and `wound_alike` is why.** This is a stroke expansion being
    // filled, and the function above it has just forced every subpath to wind the
    // same way *so that* a non-zero fill reads it as one solid band. Handing it the
    // node's rule would undo that in the one place the winding was arranged by hand.
    let ink = wound_alike(&ink);
    painter.fill_path(world, &ink, paint.brush, paint.framing, ClipRule::NonZero);
}

/// `path` with every subpath another subpath's box swallows thrown away.
///
/// A stroke expansion is an outer contour and an inner one per input subpath, and
/// [`lay_stroke`] wants only the outer ones: the inner offset is the outline pushed
/// inward, so it encloses nothing that the outline's own interior does not, and the
/// outline goes in beside it. Dropping them outright is what makes the answer stable
/// at *any* width — an inner offset pushed far enough in becomes a figure-eight, with
/// regions of both windings inside one contour, which [`wound_alike`] cannot correct
/// because a subpath has one signed area and a figure-eight needs two. Measured: a
/// 200×120 rect kept a 59pt ring void through the whole band from 500 to 950pt until
/// these were dropped.
///
/// **Boxes rather than the curves**, which is coarse and is exactly enough. Two
/// contours of one expansion are concentric, so containment is not a close call; and
/// the cost of being wrong either way is bounded, because everything dropped lies
/// inside a region already being filled.
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

/// `path`'s subpaths, every one of them wound positively, so that a **non-zero fill
/// of the result is the union** of the regions they enclose rather than an
/// alternation of solids and holes.
///
/// Only sound where the regions really are meant to be unioned: it cannot express a
/// hole, by construction. [`lay_stroke`] is the one caller, and it has established
/// there is no hole to keep before asking.
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

/// How far past the outline a stroke of `width` can actually reach once mitres are
/// allowed for.
///
/// The doubled stroke an aligned one is built from lays down `width` either side of the
/// outline — but at a **corner** a mitre runs out to `half_width · miter_limit` from the
/// vertex, which for the doubled stroke is `width · limit`. That is not a rounding
/// allowance: at the SVG default limit of 4 a corner throws a spike four times as far
/// as the flat of the stroke goes.
fn miter_reach(width: f64, miter_limit: f64) -> f64 {
    width * miter_limit.max(1.0)
}

/// A box comfortably containing `path` and any stroke on it, for the outside
/// clip's outer ring. Derived from the path rather than a fixed constant so it
/// stays sane at any document scale.
///
/// **`reach` is not optional, and leaving it out was a bug with a very specific
/// look.** The padding used to be the box's own width plus its height and nothing
/// else — generous for an ordinary hairline and *narrower than the ink* once a stroke
/// got wide, because the ring has to clear the stroke rather than the shape. Past that
/// point the complement's own outer edge cut the stroke off, so a wide outside stroke
/// came back with straight rectangular bites taken out of it, at a distance that
/// depended on the shape's size and looked like nothing in the document.
///
/// A boolean is where it shows first, and it is the case that was reported: its outline
/// has sharp vertices wherever two operands' edges cross, so it throws the longest
/// mitres of any shape — a 241pt stroke on a 142pt boolean reaches 964pt at a corner
/// against the 284pt of padding the old formula gave it.
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

    /// **An outside stroke's clip must clear the ink, not the shape.**
    ///
    /// Reported as "a really big stroke on a boolean may yield weird results", with a
    /// photo of a 241pt stroke on a 142pt shape. Most of what that photo shows *is*
    /// mathematically right — a stroke 1.7× the shape's own size covers an enormous
    /// area, and a mitre limit of 4 throws spikes four times further still — but one
    /// part of it was not: the complement rectangle the stroke is clipped against was
    /// padded by the shape's own width plus height and by nothing else, so past a
    /// certain width its own outer edge sat *inside* the stroke and cut straight bites
    /// out of it.
    ///
    /// The numbers from the report: a 142×142 outline gets 284 of padding, while a
    /// 241pt stroke reaches 964 at a mitre. The clip was three and a half times too
    /// small.
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

    /// A mitre reaches `limit` times the half-width from the corner, and the doubled
    /// stroke's half-width is the nominal `width` — so this is `width · limit`, floored
    /// at `width` because a limit below 1 is not a thing kurbo will honour.
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

    /// A band to build decoration paths from: 100 long, 2 thick, its middle at
    /// y 11 — so a thickness is 2, a dash period is 10, and a wavelength is 12.
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

    /// **A gap is a hole in the band, not a shortened band.** The first thing to get
    /// wrong here is to draw up to the gap and stop.
    ///
    /// ⚠️ Flipped by returning `vec![(band.x0, band.x1)]` from `kept_spans`
    /// unconditionally: one bar instead of two, and the length assertion catches it
    /// before the coordinates do.
    #[test]
    fn a_skipped_span_leaves_a_hole_in_a_solid_band() {
        let ink = band(ondin_core::LineStyle::Solid, vec![(40.0, 60.0)]);
        let bars = bars_of(&decoration_path(&ink));
        assert_eq!(bars.len(), 2, "a hole makes two bars: {bars:?}");
        assert_eq!(bars[0], (0.0, 40.0));
        assert_eq!(bars[1], (60.0, 100.0));
        // And a band with nothing in its way is exactly what it always was.
        let whole = bars_of(&decoration_path(&band(
            ondin_core::LineStyle::Solid,
            vec![],
        )));
        assert_eq!(whole, vec![(0.0, 100.0)]);
    }

    /// ⚠️ **A sliver handed to this function is drawn, and that is on purpose.**
    /// Whether a 1.5-wide fragment of a 2-thick band is worth drawing is decided in
    /// `ondin_core::text::crossings`, which absorbs it into the gaps either side
    /// *and* backs that rule off where it would erase the band's only mark. This
    /// function held a `retain` for one commit; it moved because a back-off that has
    /// to know whether a rule in another crate would empty the band is the gate
    /// written down twice this project keeps paying for (§15 D356).
    ///
    /// So this asserts the **boundary**, not the drawing: three gaps in, four bars
    /// out, sliver included. ⚠️ Flipped by putting the `retain` back here, where the
    /// `(18.0, 19.5)` bar disappears — which is the *symptom* to look for if a narrow
    /// band ever loses its rule again, because both places filtering is exactly the
    /// state that produced the bug the tiers fix.
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

    /// **The dash phase belongs to the band, not to the fragment.** A dash run is
    /// generated across the whole band and then met with what skip-ink left, so a
    /// dash the gap cut in half resumes where it would have been rather than
    /// restarting at the gap's edge.
    ///
    /// The numbers: dashes of 6 on, 4 off from x 0, so `20..26` is a dash, and a gap
    /// of `13..22` ends inside it. The bar that comes out is `22..26` — the tail of
    /// that dash.
    ///
    /// ⚠️ Flipped by generating the run per fragment (`dash_spans` over a rect built
    /// from each kept span instead of `overlap(dash_spans(band), kept)`): the bar
    /// becomes `22..28`, a whole dash starting at the gap, and *every* dash after it
    /// moves with it — `32..38, 42..48, …` where the grid says 30, 40. The assertion
    /// on 26 fails; the one on `10..13` does not, which is the point of having both.
    /// It is the **right** edge that catches it — the left edge is the gap boundary
    /// either way, so asserting where the ink resumes proves nothing on its own.
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

    /// **And so does the wave's phase.** Same rule as the dashes and a worse failure
    /// if it is wrong: a ribbon that restarted at each fragment would put a crest
    /// where the gap had been cut for a trough.
    ///
    /// A wavelength is six thicknesses — 12 here — so a fragment starting at x 3
    /// starts a quarter of the way through one, at the crest: `mid + amplitude`,
    /// upper edge `11 + 2 − 1 = 12`.
    ///
    /// ⚠️ Flipped by measuring the phase from `from` instead of `band.x0`, where the
    /// first point comes back at y 10 — the ribbon's own zero — rather than 12.
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

    /// ⚠️ **A wavy ribbon reaches one and a half thicknesses from the band's middle,
    /// and core depends on that number.** `ondin_core::text`'s `ink_extent` measures
    /// an underline's gaps against `1.5t` for this style alone, because the ribbon is
    /// three thicknesses tall where the band is one; if `wave` ever swings wider,
    /// crests start running through the descenders the gaps were cut for, and nothing
    /// on either side of the boundary would say so.
    ///
    /// Asserted **tight in both directions**, so it cannot rot into a claim that is
    /// merely true: a ribbon that reached less would make core's gaps too wide, and
    /// one that reached more would make them too narrow.
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
