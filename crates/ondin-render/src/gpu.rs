//! GPU backend: vello + wgpu — the interactive canvas (§6.3).
//!
//! Builds a `vello::Scene` from the document through the shared `scene` module
//! (so the CPU pixel tests validate the walk both backends share), then
//! rasterizes it on the app's wgpu device. Scene construction is pure and
//! unit-tested via a recording painter; on-device rasterization is covered by a
//! headless smoke test gated on adapter availability (`headless.rs`).

use crate::color;
use crate::fx_gpu::{self, FxPipelines, FxPool};
use crate::images::ImageStore;
use crate::renderer::{RenderOverrides, Viewport};
use crate::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use kurbo::Shape;
use kurbo::{Affine, BezPath, Rect, Stroke};
use ondin_core::peniko::{
    BlendMode, Blob, Color, Compose, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
    ImageQuality, ImageSampler, Mix,
};
use ondin_core::{Brush, Document, Resolved};
use vello::{AaConfig, Glyph, RenderParams, Renderer, RendererOptions, Scene};

use wgpu::{Device, Queue, TextureView};

/// Records draw calls into a `vello::Scene`. Coordinates are local; `base` maps
/// world → target pixels, matching the CPU backend.
struct GpuScenePainter<'a> {
    /// The page. Drawing goes here whenever no effect layer is open.
    root: Scene,
    /// One frame per open effect layer, innermost last — see
    /// [`GpuScenePainter::push_effect_layer`].
    fx: Vec<FxFrame>,
    /// The finished effect layers, **innermost first** (pushed as each pops), so
    /// the resolve is a single forward pass: a nested layer's texture is registered
    /// before the scene that draws it is rendered.
    jobs: Vec<EffectJob>,
    /// world → pixel transform applied on top of each node's world transform.
    /// **Current, not the page's**: an open effect layer draws into a buffer whose
    /// origin is its own.
    base: Affine,
    /// The enclosing target's pixel box — the clamp an effect layer's buffer gets.
    limit: Rect,
    /// The **page's** pixel count, which no single effect buffer may exceed
    /// (`effect::buffer_box_within`); unlike `limit` it does not follow the stack.
    budget: f64,
    /// Decoded images by id. vello takes the pixels inline, so resolving one is an
    /// `Arc` clone.
    images: &'a ImageStore,
}

/// One open effect layer on the GPU: where its buffer sits, and the sub-scene
/// everything inside it is recorded into.
struct FxFrame {
    scene: Scene,
    /// Device-space top-left of this layer's buffer, in the enclosing target's pixels.
    origin: (i32, i32),
    size: (u32, u32),
    effects: Vec<ondin_core::Keyed<ondin_core::Effect>>,
    /// The enclosing target's `base` and `limit`, put back when this layer pops.
    saved_base: Affine,
    saved_limit: Rect,
    /// **Buffer** pixels per document unit along each axis — device pixels divided
    /// by [`Self::downscale`].
    scale: (f64, f64),
    /// Device pixels per buffer pixel (`effect::buffer_downscale`); `1.0` unless
    /// the layer is heavily blurred.
    downscale: f64,
    /// How many *ordinary* layers are open inside this one; it tells
    /// [`GpuScenePainter::pop_layer`] which kind of layer it is closing (§15 D338).
    depth: usize,
    /// How many effect layers enclose this one. Siblings share a level and are
    /// rasterized in one pass; see `VelloGpuRenderer::resolve_effects`.
    level: usize,
}

/// A subtree that has to be rasterized, filtered and handed back as an image
/// before the scene that draws it can be rendered.
///
/// **Recorded during the walk and resolved after it**: a `vello::Scene` is a
/// recording, so no pixels exist during the walk to filter. The CPU backend
/// rasterizes at pop; here the layer pops into a job.
pub struct EffectJob {
    /// Everything drawn inside the layer, in the buffer's own coordinates.
    scene: Scene,
    /// No origin here on purpose: where the layer lands is already the transform
    /// on the `draw_image` `pop_layer` put into the enclosing scene.
    size: (u32, u32),
    effects: Vec<ondin_core::Keyed<ondin_core::Effect>>,
    scale: (f64, f64),
    /// How many effect layers enclose this one; `VelloGpuRenderer::resolve_effects`
    /// batches a level at a time.
    level: usize,
    /// The atlas slot the enclosing scene draws.
    ///
    /// Built with a **fake, empty blob**, as `Renderer::register_texture` does:
    /// the blob is only an identity, and `override_image` points it at a real
    /// texture before anything reads it. `Blob::new` takes its id from a global
    /// counter, so two jobs cannot collide.
    image: ImageData,
}

impl GpuScenePainter<'_> {
    /// The backend brush for `brush` with `framing` applied, and the brush
    /// transform to draw it under.
    ///
    /// [`None`] is "nothing to paint" — an image brush whose reference cannot be
    /// resolved yet (`color::brush_to_backend`); skipped, as the CPU backend does.
    ///
    /// **The brush transform is `None` for a solid and for an app-authored
    /// gradient** (its coordinates share the path's local space); a picture's are
    /// its own pixel grid and need the map the framing carries. vello composes it
    /// as `transform * brush_transform`. An imported gradient can carry one too
    /// (§15 D412); `Affine::IDENTITY` is passed as `None`.
    fn resolve(
        &self,
        brush: &Brush,
        framing: Option<ondin_core::Framing>,
    ) -> Option<(vello::peniko::Brush, Option<Affine>)> {
        // **`Atlas`, where the CPU backend asks for `Source`** (§15 D745): vello
        // keeps every picture in one 8192-pixel sheet and silently drops those
        // that do not fit, so the canvas shrinks pictures to show them all. The
        // export has no atlas and must write what the user placed.
        let (mut backend, second) =
            color::brush_to_backend(brush, self.images, color::Resolution::Atlas)?;
        color::apply_framing(&mut backend, framing);
        // `second` is composed under the framing for an image: the store may have
        // answered with a reduced picture, whose grid is not the framing's.
        let bt = match backend {
            vello::peniko::Brush::Image(_) => color::framed_transform(framing, second),
            vello::peniko::Brush::Gradient(_) if second != Affine::IDENTITY => Some(second),
            _ => None,
        };
        Some((backend, bt))
    }

    /// The scene drawing currently goes into — the innermost open effect layer's,
    /// or the page's. `CpuPainter::ctx`'s twin.
    fn scene(&mut self) -> &mut Scene {
        match self.fx.last_mut() {
            Some(f) => &mut f.scene,
            None => &mut self.root,
        }
    }

    /// Record that an ordinary layer has opened inside the innermost effect layer
    /// (see [`FxFrame::depth`]).
    fn opened_ordinary(&mut self) {
        if let Some(f) = self.fx.last_mut() {
            f.depth += 1;
        }
    }
}

/// The transform and shape vello's `push_layer` is handed for one layer — the
/// node's own clip when it has one, and the substitute for "no clip" when it does
/// not.
///
/// **The substitute is the target's own pixel box under the identity** (§15 D612):
/// no transform can shrink it, whereas a `±1e7` shape under `base * transform`
/// shrinks with any ancestor scale below 1 and clips real content. `limit` is that
/// box, kept current through effect layers, inflated by one pixel so the clip's
/// antialiased edge cannot eat the outermost row. `CpuPainter::push_layer` passes
/// `None` straight through, so this substitution is a GPU-only decision the shared
/// walk never sees.
///
/// Not `(base * transform).inverse() * limit`: a degenerate `transform` (a zero
/// scale, which the model permits) makes that non-finite. `push_mask_layer` goes
/// through this too.
fn layer_shape<'a>(
    limit: Rect,
    base: Affine,
    transform: Affine,
    clip: Option<&'a BezPath>,
) -> (Affine, std::borrow::Cow<'a, BezPath>) {
    match clip {
        Some(path) => (base * transform, std::borrow::Cow::Borrowed(path)),
        None => (
            Affine::IDENTITY,
            std::borrow::Cow::Owned(limit.inflate(1.0, 1.0).to_path(0.1)),
        ),
    }
}

impl ScenePainter for GpuScenePainter<'_> {
    /// The store is the whole answer: an unknown id, a link nothing has read and
    /// bytes that would not decode are all "no pixels here" (§15 D179).
    fn has_image(&self, id: &ondin_core::ImageId) -> bool {
        self.images.get(id).is_some()
    }

    fn fill_path(
        &mut self,
        transform: Affine,
        path: &BezPath,
        brush: &Brush,
        framing: Option<ondin_core::Framing>,
        rule: ClipRule,
    ) {
        let Some((backend, bt)) = self.resolve(brush, framing) else {
            return;
        };
        // `base` is read first: `scene()` borrows `self` mutably.
        let at = self.base * transform;
        let fill = match rule {
            ClipRule::NonZero => Fill::NonZero,
            ClipRule::EvenOdd => Fill::EvenOdd,
        };
        self.scene().fill(fill, at, &backend, bt, path);
    }

    fn stroke_path(&mut self, transform: Affine, path: &BezPath, stroke: &StrokePaint<'_>) {
        let mut k = Stroke::new(stroke.width)
            .with_join(stroke.join)
            .with_caps(stroke.cap)
            .with_miter_limit(stroke.miter_limit);
        if !stroke.dashes.is_empty() {
            k = k.with_dashes(stroke.dash_offset, stroke.dashes.iter().copied());
        }
        let Some((backend, bt)) = self.resolve(stroke.brush, stroke.framing) else {
            return;
        };
        let at = self.base * transform;
        self.scene().stroke(&k, at, &backend, bt, path);
    }

    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>) {
        // Type filled with an unresolvable image draws nothing, as a shape does.
        let Some((backend, bt)) = self.resolve(run.brush, run.framing) else {
            return;
        };
        let at = self.base * transform;
        self.scene()
            .draw_glyphs(run.font)
            .font_size(run.font_size)
            .normalized_coords(run.coords)
            .brush(&backend)
            .brush_transform(bt)
            .transform(at)
            .draw(
                Fill::NonZero,
                run.glyphs.iter().map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                }),
            );
    }

    fn push_layer(
        &mut self,
        transform: Affine,
        clip: Option<&BezPath>,
        rule: ClipRule,
        opacity: f32,
    ) {
        self.opened_ordinary();
        let (at, shape) = layer_shape(self.limit, self.base, transform, clip);
        self.scene().push_layer(
            match rule {
                ClipRule::NonZero => Fill::NonZero,
                ClipRule::EvenOdd => Fill::EvenOdd,
            },
            Mix::Normal,
            opacity,
            at,
            shape.as_ref(),
        );
    }

    fn push_mask_layer(&mut self) {
        self.opened_ordinary();
        // vello always wants a shape; this layer clips nothing ([`layer_shape`]).
        let (at, unbounded) = layer_shape(self.limit, self.base, Affine::IDENTITY, None);
        self.scene().push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::DestIn),
            1.0,
            at,
            unbounded.as_ref(),
        );
    }

    /// Redirect drawing into a sub-scene of its own, to be rasterized and
    /// filtered after the walk.
    ///
    /// The geometry is `CpuPainter::push_effect_layer`'s, number for number (the
    /// clamp, the outward `floor`/`ceil`, the scale from the composed device
    /// transform): backends that rounded differently would put a shadow half a
    /// pixel apart.
    fn push_effect_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        effects: &[ondin_core::Keyed<ondin_core::Effect>],
    ) {
        let device = (self.base * transform).transform_rect_bbox(bounds);
        let m = (self.base * transform).as_coeffs();
        let scale = (
            (m[0] * m[0] + m[1] * m[1]).sqrt(),
            (m[2] * m[2] + m[3] * m[3]).sqrt(),
        );
        // The target grown by the escape, not the bare target (§15 D342).
        let clipped = ondin_core::effect::buffer_box_within(
            device,
            self.limit,
            ondin_core::effect::stack_escape(effects),
            scale,
            self.budget,
        );
        let (x0, y0) = (clipped.x0.floor(), clipped.y0.floor());
        // **The buffer is not always at device resolution** (§15 D395): a heavily
        // blurred layer is rasterized coarse (`effect::buffer_downscale`, as in
        // `CpuPainter::push_effect_layer`) and scaled back up, which bounds the
        // convolution as someone zooms in.
        //
        // **The texture-size cap is reached by resampling** (§15 D744):
        // `effect_buffer` grows `k` until the buffer fits, since `fx_gpu` allocates
        // scratch textures at `job.size`. `pack`'s `min(limit)` is not this guard;
        // it only bounds where a layer is placed in the batch texture.
        let k = ondin_core::effect::buffer_downscale(effects, scale);
        let ((w, h), k) =
            crate::effects::effect_buffer((clipped.x1.ceil() - x0, clipped.y1.ceil() - y0), k);
        let saved_base = self.base;
        let saved_limit = self.limit;
        // Shift so the buffer's top-left is the origin and scale into its resolution,
        // so a nested effect layer clamps to this buffer and inherits the coarser
        // scale through `self.base`.
        self.base = Affine::scale(1.0 / k) * Affine::translate((-x0, -y0)) * saved_base;
        self.limit = Rect::new(0.0, 0.0, w as f64, h as f64);
        self.fx.push(FxFrame {
            scene: Scene::new(),
            origin: (x0 as i32, y0 as i32),
            size: (w, h),
            effects: effects.to_vec(),
            saved_base,
            saved_limit,
            // The passes work in buffer units, keeping the kernel `BLUR_SIGMA_BUDGET`
            // wide at any zoom.
            scale: (scale.0 / k, scale.1 / k),
            downscale: k,
            depth: 0,
            level: self.fx.len(),
        });
    }

    fn pop_layer(&mut self) {
        // An ordinary layer opened inside this effect layer closes first.
        match self.fx.last() {
            Some(f) if f.depth > 0 => {
                if let Some(f) = self.fx.last_mut() {
                    f.depth -= 1;
                }
                self.scene().pop_layer();
                return;
            }
            Some(_) => {}
            None => {
                self.root.pop_layer();
                return;
            }
        }
        let frame = self.fx.pop().expect("an effect frame, checked above");
        self.base = frame.saved_base;
        self.limit = frame.saved_limit;
        let image = ImageData {
            // Never read (`override_image` points it at a texture), but vello sizes
            // the atlas slot from the dimensions.
            data: Blob::new(std::sync::Arc::new(&[] as &[u8])),
            format: ImageFormat::Rgba8,
            // vello supports only straight alpha for image data, so `fx_gpu`
            // un-premultiplies on the way out.
            alpha_type: ImageAlphaType::Alpha,
            width: frame.size.0,
            height: frame.size.1,
        };
        // **Nearest while the buffer is at device resolution**: the transform is a
        // whole-pixel translate, and filtering would soften the edges the passes
        // computed. A downscaled buffer is magnified, and nearest would show blocks.
        let magnified = frame.downscale > 1.0;
        let brush = ImageBrush {
            image: image.clone(),
            sampler: ImageSampler {
                quality: if magnified {
                    ImageQuality::Medium
                } else {
                    ImageQuality::Low
                },
                ..Default::default()
            },
        };
        let at = Affine::translate((frame.origin.0 as f64, frame.origin.1 as f64))
            * Affine::scale(frame.downscale);
        self.scene().draw_image(&brush, at);
        self.jobs.push(EffectJob {
            scene: frame.scene,
            size: frame.size,
            effects: frame.effects,
            scale: frame.scale,
            level: frame.level,
            image,
        });
    }
}

/// Build a `vello::Scene` of `page` for `vp` (world → pixel base transform
/// applied), and the effect layers it needs resolved first. Pure; no GPU device
/// required.
///
/// **The jobs come back rather than being drawn**: [`VelloGpuRenderer::render`]
/// rasterizes each, runs `fx_gpu` over it and points its atlas slot at the result.
pub fn build_scene(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    images: &ImageStore,
    page: ondin_core::NodeId,
) -> (Scene, Vec<EffectJob>) {
    let base = scene::device_base(vp);
    let mut painter = GpuScenePainter {
        root: Scene::new(),
        fx: Vec::new(),
        jobs: Vec::new(),
        base,
        limit: Rect::new(0.0, 0.0, vp.pixel_size.0 as f64, vp.pixel_size.1 as f64),
        budget: f64::from(vp.pixel_size.0) * f64::from(vp.pixel_size.1),
        images,
    };
    scene::build(doc, res, vp, overrides, page, &mut painter);
    (painter.root, painter.jobs)
}

/// Which renderer's atlas an effect layer's slot was registered in — the page's,
/// or the one belonging to a numbered pass.
///
/// Not an `Option<usize>`: the page is not "no renderer" when unregistering
/// at the end of the frame (§15 D404).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Holder {
    Page,
    Layer(usize),
}

/// One vello pass' worth of sibling effect layers, packed into a single texture.
struct Chunk {
    size: (u32, u32),
    /// Index into the level's jobs, and where that job sits in the packed texture.
    items: Vec<(usize, (u32, u32))>,
}

/// Shelf-pack a level's layers into as few textures as the device and the page's
/// scale permit — `sizes` is each layer's buffer in walk order, and the answer
/// indexes back into it.
///
/// **Simple rows, not a good packer**: the point is the pass count. Each chunk is
/// a vello resolve pass, and passes are what evict the page's pictures (§15 D344).
///
/// `limit` is the device's maximum texture dimension and only wraps *between*
/// layers. Every size reaching here is already inside
/// `effects::MAX_EFFECT_BUFFER_SIDE` (§15 D744), so the `min(limit)` clamp is dead
/// and the `debug_assert!` says so; a buffer can legitimately exceed the viewport
/// (it is grown by the stack's escape, §15 D342).
///
/// `budget` is the page's own pixel count and stops a level being one enormous
/// scene (§15 D402): vello cannot report overflow of its fixed intermediate
/// buffers, so it surfaces as a hung fine shader and a GPU reset. Passes of at
/// most the page's area stay inside a load the page itself survives.
///
/// A single layer over budget still gets a chunk to itself: only a `LayerBlur`
/// may buy a coarser buffer (`effect::buffer_downscale`).
fn pack(sizes: &[(u32, u32)], limit: u32, budget: u64) -> Vec<Chunk> {
    debug_assert!(
        sizes.iter().all(|s| s.0 <= limit && s.1 <= limit),
        "every layer is inside the device limit before it is packed — \
         `push_effect_layer` caps the buffer at `effects::MAX_EFFECT_BUFFER_SIDE` \
         by resampling (§15 D744). A size past `limit` here means that cap was \
         bypassed, and the `min(limit)` below would hide it: {sizes:?} against {limit}"
    );
    let mut chunks = Vec::new();
    let mut items: Vec<(usize, (u32, u32))> = Vec::new();
    let (mut x, mut y, mut row_h, mut wide) = (0u32, 0u32, 0u32, 0u32);
    for (i, size) in sizes.iter().enumerate() {
        let (w, h) = (size.0.min(limit), size.1.min(limit));
        if x + w > limit {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        // What the chunk's texture would measure with the layer in it.
        let area = u64::from(wide.max(x + w)) * u64::from(y + row_h.max(h));
        if !items.is_empty() && (y + h > limit || area > budget) {
            chunks.push(Chunk {
                size: (wide.max(1), (y + row_h).max(1)),
                items: std::mem::take(&mut items),
            });
            x = 0;
            y = 0;
            row_h = 0;
            wide = 0;
        }
        items.push((i, (x, y)));
        x += w;
        row_h = row_h.max(h);
        wide = wide.max(x);
    }
    chunks.push(Chunk {
        size: (wide.max(1), (y + row_h).max(1)),
        items,
    });
    chunks
}

/// Wraps a `vello::Renderer` for on-device rasterization.
pub struct VelloGpuRenderer {
    renderer: Renderer,
    /// A vello renderer per effect pass, for the layers' offscreen sub-scenes.
    ///
    /// **Not a spare — the page's renderer cannot do this work** (§15 D344).
    /// vello's atlas evicts an image not drawn for two resolve passes, and every
    /// `render_to_texture` is one; sub-scenes rendered on the page's renderer made
    /// an image fill fall out of the atlas from the second frame on. Separated,
    /// each renderer sees vello's one-pass-per-frame rhythm.
    ///
    /// **Built on first use**: a vello `Renderer` is a full set of compute pipelines
    /// and most documents carry no effects.
    layers: Vec<Renderer>,
    /// The largest pass count any frame has demanded (§15 D763). Not
    /// `layers.len()` read directly: that is what is held, this is what was asked
    /// for, and a future reuse scheme would make them diverge. Each pass index
    /// costs a one-off pipeline build (§15 D777).
    pass_high_water: usize,
    /// The effect passes' compute pipelines, built once per device.
    fx: FxPipelines,
    /// Every texture, buffer and bind group the effect path uses, **kept across
    /// layers and frames** (§15 D1003 (7)); see [`FxPool`]. It does not pool vello
    /// `Renderer`s, which stay one per pass as D344 requires (§15 D779).
    pool: FxPool,
}

impl VelloGpuRenderer {
    /// Create a renderer for `device`. Errors surface vello initialization
    /// failures (e.g. shader compilation).
    pub fn new(device: &Device) -> Result<Self, vello::Error> {
        let renderer = Renderer::new(device, RendererOptions::default())?;
        let fx = FxPipelines::new(device);
        Ok(Self {
            renderer,
            layers: Vec::new(),
            pass_high_water: 0,
            fx,
            pool: FxPool::new(),
        })
    }

    /// What the effect passes did in the last finished frame — textures and bind
    /// groups made, submits, layers, and what the pool holds (§15 D1003 (7)).
    ///
    /// A steady frame makes no texture and no bind group and submits once per pass
    /// (`tests/gpu_effects.rs` asserts all three).
    pub fn fx_stats(&self) -> fx_gpu::FxStats {
        self.pool.last_frame()
    }

    /// How many effect passes the busiest frame so far has demanded (§15 D763);
    /// the count of vello `Renderer`s the process holds, each a one-off pipeline
    /// build (§15 D777). `gpu_effects.rs` prints it for fixtures; no real drawing
    /// has been read.
    ///
    /// A bound-and-reuse pool is declined (§15 D779): the cost is a one-off hitch,
    /// and reusing a renderer across passes would bring back D344's blank-image
    /// bug. If the hitch is felt, build renderers off the critical frame instead.
    pub fn pass_high_water(&self) -> usize {
        self.pass_high_water
    }

    /// Rasterize every effect layer, filter it, and point its atlas slot at the
    /// result — everything that must happen before the page's own scene renders.
    ///
    /// **One vello pass per renderer** is a correctness rule (§15 D344): the atlas
    /// evicts an image not drawn for two resolve passes, so each pass takes the
    /// next renderer, and sibling layers are packed into as few textures as
    /// possible (renderers are handed out per pass since §15 D402).
    ///
    /// **Deepest level first**: a nested layer's texture must be registered before
    /// the scene that draws it is rasterized.
    ///
    /// **A slot is registered on the renderer that will draw it** (§15 D404): the
    /// page for level 0, the parent layer's pass for anything deeper. Hence the
    /// passes are planned before any runs.
    ///
    /// Pushes each atlas slot and its renderer onto `registered` as made, so the
    /// caller unregisters them even when a pass fails part-way (`override_image`
    /// keys a map by blob id and every frame mints new ones).
    fn resolve_effects(
        &mut self,
        device: &Device,
        queue: &Queue,
        jobs: Vec<EffectJob>,
        budget: u64,
        registered: &mut Vec<(Holder, ImageData)>,
    ) -> Result<(), vello::Error> {
        if jobs.is_empty() {
            return Ok(());
        }
        let deepest = jobs.iter().map(|j| j.level).max().unwrap_or(0);
        // Shelf packing wraps at the device's maximum texture dimension.
        let limit = device.limits().max_texture_dimension_2d;
        let mut by_level: Vec<Vec<EffectJob>> = (0..=deepest).map(|_| Vec::new()).collect();
        for job in jobs {
            by_level[job.level].push(job);
        }

        // The passes in run order, planned first because a job at level L is drawn
        // by level L-1's passes and must name them. `passes[level]` is that
        // level's range of pass indices.
        let mut plan: Vec<(usize, Vec<Chunk>)> = Vec::new();
        let mut passes: Vec<std::ops::Range<usize>> = vec![0..0; deepest + 1];
        let mut total = 0usize;
        for (level, jobs) in by_level.iter().enumerate().rev() {
            if jobs.is_empty() {
                continue;
            }
            let sizes: Vec<(u32, u32)> = jobs.iter().map(|j| j.size).collect();
            let chunks = pack(&sizes, limit, budget);
            passes[level] = total..total + chunks.len();
            total += chunks.len();
            plan.push((level, chunks));
        }
        // A renderer per pass, built on demand and held for the process's life
        // (§15 D763): the frame that first demands `total` passes pays a one-off
        // pipeline build per new index (§15 D777). The page budget drives the pass
        // count (area ÷ page area) and nothing caps `total`.
        self.pass_high_water = self.pass_high_water.max(total);
        while self.layers.len() < total {
            self.layers
                .push(Renderer::new(device, RendererOptions::default())?);
        }

        let mut pass = 0usize;
        for (level, chunks) in plan {
            let jobs = &by_level[level];
            for chunk in chunks {
                let (w, h) = chunk.size;
                // From the pool, usually larger than the chunk (`fx_gpu::size_class`):
                // vello writes only inside `RenderParams::width`/`height` and every
                // slice lies inside that, so the margin is never read.
                let packed = self.pool.take(device, w, h);
                // Every sibling's sub-scene, shifted to its own slot **and clipped
                // to it**, as one recording.
                //
                // **The clip keeps a sibling's drawing out of its neighbours' slots**
                // (§15 D990): a sub-scene is not bounded by its trimmed buffer (a
                // plain fill, or a layer clipped to its own path, draws wherever its
                // path goes), so an off-screen part would land at negative x in the
                // previous slot. The CPU backend has a buffer per layer.
                let mut batch = Scene::new();
                for &(i, at) in &chunk.items {
                    let to = Affine::translate((at.0 as f64, at.1 as f64));
                    let (w, h) = jobs[i].size;
                    batch.push_clip_layer(
                        Fill::NonZero,
                        to,
                        &Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                    );
                    batch.append(&jobs[i].scene, Some(to));
                    batch.pop_layer();
                }
                // **Transparent, not the page's ground**, or a rectangle of it would
                // be baked into the shadow.
                self.layers[pass].render_to_texture(
                    device,
                    queue,
                    &batch,
                    &packed.view,
                    &RenderParams {
                        base_color: Color::TRANSPARENT,
                        width: w,
                        height: h,
                        antialiasing_method: AaConfig::Area,
                    },
                )?;
                // **The whole chunk is filtered in one encoder and one submit**, into
                // a result texture packed like the source (§15 D1003 (7)).
                //
                // **The result is held to the end of the frame; the source is not.**
                // The consuming pass's atlas copy reads the result through the slot
                // below, so it goes back in `end_frame`; the source is read only by
                // this chunk's submit, so it is released now (queue order makes
                // that safe).
                let result = self.pool.take(device, w, h);
                let items: Vec<fx_gpu::Item<'_>> = chunk
                    .items
                    .iter()
                    .map(|&(i, at)| fx_gpu::Item {
                        slice: fx_gpu::Slice {
                            at,
                            size: jobs[i].size,
                        },
                        dst_at: at,
                        effects: &jobs[i].effects,
                        scale: jobs[i].scale,
                    })
                    .collect();
                fx_gpu::run_batch(
                    device,
                    queue,
                    &self.fx,
                    &mut self.pool,
                    &packed,
                    &result,
                    &items,
                );
                self.pool.release(&packed);
                for &(i, at) in &chunk.items {
                    let job = &jobs[i];
                    // The slot is this layer's rectangle of the result, named by the
                    // copy's origin: vello copies `job.image`'s size from there.
                    let at_texture = |t: &wgpu::Texture| wgpu::TexelCopyTextureInfoBase {
                        texture: t.clone(),
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: at.0,
                            y: at.1,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    };
                    let filtered = result.texture.clone();
                    // Whoever draws the result needs it in their atlas: the page for
                    // level 0, else every pass of the level above (which one holds
                    // the parent is not recorded, and registering on all is cheap).
                    if level == 0 {
                        self.renderer
                            .override_image(&job.image, Some(at_texture(&filtered)));
                        registered.push((Holder::Page, job.image.clone()));
                    } else {
                        for p in passes[level - 1].clone() {
                            self.layers[p].override_image(&job.image, Some(at_texture(&filtered)));
                            registered.push((Holder::Layer(p), job.image.clone()));
                        }
                    }
                }
                pass += 1;
            }
        }
        Ok(())
    }

    /// Build the scene of `page` for `vp` and rasterize it into `target` (an RGBA
    /// texture view sized `vp.pixel_size`). `base_color` clears the target first.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &Device,
        queue: &Queue,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        overrides: &RenderOverrides,
        target: &TextureView,
        base_color: Color,
        images: &ImageStore,
        page: ondin_core::NodeId,
    ) -> Result<(), vello::Error> {
        let (scene, jobs) = build_scene(doc, res, vp, overrides, images, page);
        // The page's pixel count is the budget every effect pass is held to (`pack`).
        let budget = u64::from(vp.pixel_size.0) * u64::from(vp.pixel_size.1);
        let mut registered = Vec::new();
        let out = self
            .resolve_effects(device, queue, jobs, budget, &mut registered)
            .and_then(|()| {
                let params = RenderParams {
                    base_color,
                    width: vp.pixel_size.0,
                    height: vp.pixel_size.1,
                    antialiasing_method: AaConfig::Area,
                };
                self.renderer
                    .render_to_texture(device, queue, &scene, target, &params)
            });
        // **Unregistered on the way out, whichever pass failed**, or the map entry
        // and its texture live as long as the renderer — and from the renderer that
        // holds each slot, since a nested layer's lives in a layer renderer's atlas.
        for (holder, image) in registered {
            match holder {
                Holder::Page => self.renderer.unregister_texture(image),
                Holder::Layer(p) => self.layers[p].unregister_texture(image),
            }
        }
        // Only now do the results go back to the pool: the page's render, the last
        // reader, is submitted and no slot points at them (§15 D1003 (7)).
        self.pool.end_frame();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{layer_shape, pack};
    use kurbo::{Affine, Rect, Shape};

    /// A 4K page.
    const PAGE: u64 = 3840 * 2160;

    /// An unclipped layer covers the whole target under any transform (§15 D612).
    /// Asserted on `layer_shape`'s decision rather than pixels, since the
    /// divergence lives below the shared walk the CPU pixel tests cover. The scales
    /// include `0.001` (the failure), `1000.0` and a degenerate `0.0`.
    ///
    /// flip: `(base * transform, Cow::Owned(±1e7 rect))` → fails at the first
    /// `0.001` case (the substitute misses the target entirely). Returning
    /// `limit` un-inflated stays green: the inflation is not what this pins.
    #[test]
    fn an_unclipped_layer_covers_the_target_under_any_transform() {
        let limit = Rect::new(0.0, 0.0, 800.0, 600.0);
        let base = Affine::translate((-19000.0, 0.0)) * Affine::scale(0.4);
        for k in [0.001, 1.0, 1000.0, 0.0] {
            let (at, shape) = layer_shape(limit, base, Affine::scale(k), None);
            let device = at.transform_rect_bbox(shape.bounding_box());
            assert!(
                device.contains(limit.center())
                    && device.width() >= limit.width()
                    && device.height() >= limit.height(),
                "at scale {k} the substitute for 'no clip' is {device:?}, which does \
                 not cover the {limit:?} target"
            );
        }

        // The control: a real clip stays in the node's own space.
        let path = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1);
        let (at, shape) = layer_shape(limit, base, Affine::scale(2.0), Some(&path));
        assert_eq!(at, base * Affine::scale(2.0), "a clip keeps its own space");
        assert_eq!(shape.bounding_box(), path.bounding_box());
    }

    /// The ordinary frame is still one pass (D344): a few small layers pack into
    /// one texture.
    #[test]
    fn small_layers_still_share_one_chunk() {
        let sizes = [(400, 300), (512, 512), (64, 64)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 1, "three small layers are one vello pass");
        assert_eq!(chunks[0].items.len(), 3);
    }

    /// Three viewport-sized layers are three passes, not one 24 MP scene that
    /// overflows vello's fixed buffers (§15 D402).
    ///
    /// flip: `budget` of `u64::MAX` (or deleting the budget check) → fails at the
    /// chunk count (one 11520×2160 chunk).
    #[test]
    fn viewport_sized_layers_split_into_a_pass_each() {
        let sizes = [(3840, 2160), (3840, 2160), (3840, 2160)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 3, "one pass each, not one enormous scene");
        for c in &chunks {
            assert!(
                u64::from(c.size.0) * u64::from(c.size.1) <= PAGE,
                "chunk {:?} is over the page's own budget",
                c.size
            );
        }
    }

    /// A layer over budget on its own still gets a chunk (only a `LayerBlur` may
    /// buy a coarser buffer, §15 D395). The count has the flip (without the budget
    /// the two pack into one chunk); the item assertions pin not looping or dropping.
    #[test]
    fn one_layer_over_budget_gets_its_own_chunk_anyway() {
        let sizes = [(4000, 3000), (100, 100)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].items, vec![(0, (0, 0))]);
        assert_eq!(chunks[1].items, vec![(1, (0, 0))]);
    }

    /// The device's maximum texture dimension still wraps a shelf, before the budget
    /// does: an unallocatable chunk is a hard error.
    #[test]
    fn the_device_limit_still_wraps_within_a_chunk() {
        let sizes = [(3000, 200), (3000, 200), (3000, 200)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 1, "1.8 MP total is well inside the budget");
        assert_eq!(
            chunks[0].items[2].1,
            (0, 200),
            "the third wraps onto a second shelf rather than passing 8192 wide"
        );
    }
}
