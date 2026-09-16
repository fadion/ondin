//! GPU backend: vello + wgpu — the interactive canvas (§6.3).
//!
//! Builds a `vello::Scene` from the document through the shared `scene` module
//! (so the CPU and GPU backends draw identically — the CPU pixel tests validate
//! that logic), then rasterizes it on a wgpu device. The app (M5) owns the
//! `wgpu::Device`/`Queue`; this type wraps `vello::Renderer` and the scene build.
//!
//! Scene construction is pure (no device) and unit-tested via a recording
//! painter; on-device rasterization is exercised by a headless smoke test gated
//! on adapter availability (`headless.rs`).

use crate::color;
use crate::fx_gpu::{self, FxPipelines};
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
    /// The finished effect layers, **innermost first** because a frame is pushed
    /// here as it pops. That order is what makes the resolve below a single
    /// forward pass: a nested layer's texture is registered before the scene that
    /// draws it is rendered.
    jobs: Vec<EffectJob>,
    /// world → pixel transform applied on top of each node's world transform.
    ///
    /// **Current, not the page's**, exactly as the CPU backend's is: an open
    /// effect layer draws into a buffer whose origin is its own.
    base: Affine,
    /// The enclosing target's pixel box — the clamp an effect layer's buffer gets,
    /// so a shadow on a layer mostly off-screen costs the visible part.
    limit: Rect,
    /// The **page's** pixel count, which no single effect buffer may exceed
    /// (`effect::buffer_box_within`). Unlike `limit` this does not follow the
    /// stack: a nested layer is bounded by what the page can render, not by what
    /// its parent happens to measure.
    budget: f64,
    /// Decoded images, keyed by the id a fill refers to. vello takes the pixels
    /// inline, so resolving one is an `Arc` clone and needs no memo of its own —
    /// unlike the CPU backend, which premultiplies.
    images: &'a ImageStore,
}

/// One open effect layer on the GPU: where its buffer sits, and the sub-scene
/// everything inside it is recorded into.
struct FxFrame {
    scene: Scene,
    /// Device-space top-left of this layer's buffer, in the *enclosing* target's
    /// pixels.
    origin: (i32, i32),
    size: (u32, u32),
    effects: Vec<ondin_core::Effect>,
    /// The enclosing target's `base` and `limit`, put back when this layer pops.
    saved_base: Affine,
    saved_limit: Rect,
    /// **Buffer** pixels per document unit along each axis, for this layer —
    /// device pixels divided by [`Self::downscale`].
    scale: (f64, f64),
    /// How many device pixels one buffer pixel covers (`effect::buffer_downscale`).
    /// `1.0` for every layer that is not heavily blurred, which is most of them.
    downscale: f64,
    /// How many *ordinary* layers are open inside this one — the whole of what
    /// tells [`GpuScenePainter::pop_layer`] which kind of layer it is closing. The
    /// CPU backend lacked this and panicked; §15 D338 is the story.
    depth: usize,
    /// How many effect layers enclose this one. Siblings share a level and are
    /// rasterized in one pass; see `VelloGpuRenderer::resolve_effects`.
    level: usize,
}

/// A subtree that has to be rasterized, filtered and handed back as an image
/// before the scene that draws it can be rendered.
///
/// **Recorded during the walk and resolved after it**, which is the whole shape
/// of effects on this backend: a `vello::Scene` is a recording rather than a
/// surface, so there is no "now" during the walk at which pixels exist to filter.
/// The CPU backend can rasterize its offscreen buffer the moment the layer pops;
/// here the layer pops into a job.
pub struct EffectJob {
    /// Everything drawn inside the layer, in the buffer's own coordinates.
    scene: Scene,
    /// No origin here, and that is not an omission: **where the layer lands is
    /// already recorded**, as the transform on the `draw_image` `pop_layer` put
    /// into the enclosing scene. A second copy would be a number that could
    /// disagree with the drawing it describes.
    size: (u32, u32),
    effects: Vec<ondin_core::Effect>,
    scale: (f64, f64),
    /// How many effect layers enclose this one — see
    /// [`VelloGpuRenderer::resolve_effects`], which batches a level at a time.
    level: usize,
    /// The atlas slot the enclosing scene draws.
    ///
    /// Built here with a **fake, empty blob** and no pixels behind it, exactly as
    /// `Renderer::register_texture` builds one — the blob is only ever an
    /// identity, and `override_image` points that identity at a real texture
    /// before anything reads it. `Blob::new` takes its id from a global counter,
    /// so two jobs cannot collide on one atlas slot however alike they look.
    image: ImageData,
}

impl GpuScenePainter<'_> {
    /// The backend brush for `brush` with `framing` applied, and the brush
    /// transform to draw it under.
    ///
    /// [`None`] is "nothing to paint" — an image brush whose reference cannot be
    /// resolved yet (`color::brush_to_backend`). Skipped rather than drawn
    /// transparent, which is what the CPU backend does with the same answer.
    ///
    /// **The brush transform is `None` for a solid, and for a gradient the app
    /// itself authored.** Such a gradient's coordinates are in the same local
    /// space as the path, so the shape transform already places them; a picture's
    /// are its own pixel grid and need the map the framing carries. vello
    /// composes it as `transform * brush_transform`, which is why the framing's
    /// own space is the node's local one and not the target's.
    ///
    /// ⚠️ **An *imported* gradient can carry one too** (§15 D412) — a squash that
    /// `peniko::RadialGradientPosition`'s two circles cannot express — so the
    /// gradient arm is no longer unconditionally `None`. `Affine::IDENTITY` is
    /// passed as `None` rather than as itself: vello takes an `Option` here, and
    /// an identity brush transform is what "no brush transform" means.
    fn resolve(
        &self,
        brush: &Brush,
        framing: Option<ondin_core::Framing>,
    ) -> Option<(vello::peniko::Brush, Option<Affine>)> {
        // **`Atlas`, where the CPU backend asks for `Source`** (§15 D745). vello
        // keeps every picture a scene draws in one 8192-pixel sheet and drops the
        // ones that will not fit without saying so, so the canvas shrinks the
        // page's pictures to make them all appear. The export has no atlas and
        // must write what the user placed.
        let (mut backend, second) =
            color::brush_to_backend(brush, self.images, color::Resolution::Atlas)?;
        color::apply_framing(&mut backend, framing);
        // **`second` composed under the framing for an image, not dropped.** The
        // store may have answered with a reduced picture while an adjustment is
        // still moving, and its pixel grid is not the one the framing maps from.
        // For a gradient the same slot is already the whole answer.
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

    /// Record that an ordinary layer has opened inside the innermost effect
    /// layer. See [`FxFrame::depth`] and §15 D338 — this is the CPU backend's
    /// panic, not repeated.
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
/// 🚨 **The substitute used to be `±1e7` under `base * transform`, which is `±1e7`
/// in the *node's own local units*** (§15 D612, `[S11.2-L1-04]`). Any ancestor
/// scale below 1 shrank it in world terms and turned an unclipped layer into a
/// clipped one: at `scale(0.001)` an "unbounded" layer covered ±1e4 world units,
/// and content past that was cut away. Measured in release on a group at
/// `Affine::scale(0.001)` holding one 100×100-unit rect at world (20000, 100) —
/// ordinary artwork at an ordinary place — **CPU 400 green pixels, GPU 0**. The
/// layer was in the export and absent from the canvas.
///
/// **Invariant 11 broken below the shared walk.** `CpuPainter::push_layer` passes
/// `clip` straight through, so `None` reaches `vello_cpu` as no clip at all; this
/// backend substitutes, and a substitute is a decision the document→scene walk
/// never sees. The module header's *"the CPU pixel tests validate that logic"* is
/// true of the walk and cannot be true of this.
///
/// **The answer is the target's own pixel box under the identity**, which is what
/// "no clip" means for a render target and is the one shape no transform can
/// shrink. `limit` is exactly that box and the painter already keeps it current
/// through effect layers, so a nested buffer gets its own. Inflated by one pixel
/// so the clip's antialiased edge cannot eat the target's outermost row.
///
/// ⚠️ **Not `(base * transform).inverse() * limit`**, which the finding sketched
/// first: a degenerate `transform` — a zero scale, which this document model
/// permits — makes that inversion non-finite, and a non-finite clip path is
/// **G1**'s shape. The identity needs no inverse and cannot be degenerate.
///
/// ⚠️ **The two GPU sites did not agree with each other either.**
/// `push_mask_layer` used the same `±1e7` under `base` alone — ±1e7 in *world*
/// units, a different and much wider bound — so "unbounded" meant two things one
/// function apart. They go through this now.
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
        // `base` is read before `scene()` borrows `self` — the accessor is
        // `&mut self`, so the two cannot be in one expression.
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
        // The same unbounded shape an unclipped layer uses above ([`layer_shape`]):
        // vello always wants one, and this layer clips nothing — the mask's own
        // drawing is the only thing that will land in it.
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
    /// The geometry is `CpuPainter::push_effect_layer`'s, number for number — the
    /// clamp to the enclosing target, the outward `floor`/`ceil`, and the scale
    /// taken from the *composed device transform* rather than the viewport so a
    /// layer inside a skewed ancestor blurs by the amount its own pixels are
    /// actually stretched. Two backends that rounded a buffer differently would
    /// put the same shadow half a pixel apart.
    fn push_effect_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        effects: &[ondin_core::Effect],
    ) {
        let device = (self.base * transform).transform_rect_bbox(bounds);
        let m = (self.base * transform).as_coeffs();
        let scale = (
            (m[0] * m[0] + m[1] * m[1]).sqrt(),
            (m[2] * m[2] + m[3] * m[3]).sqrt(),
        );
        // The target grown by the escape, not the bare target — see
        // `effect::buffer_box` and §15 D342.
        let clipped = ondin_core::effect::buffer_box_within(
            device,
            self.limit,
            ondin_core::effect::stack_escape(effects),
            scale,
            self.budget,
        );
        let (x0, y0) = (clipped.x0.floor(), clipped.y0.floor());
        // **The buffer is not always at device resolution** — see
        // `effect::buffer_downscale`, and `CpuPainter::push_effect_layer`, which
        // does this arithmetic identically. A heavily blurred layer is rasterized
        // coarse and scaled back up on the way out, which is exact for a picture
        // the blur has already band-limited and is what stops the convolution
        // growing without bound as someone zooms in (§15 D395).
        //
        // 🚨 **The cap is the *buffer's*, and it is reached by resampling**
        // (§15 D744, `[S11.2-L1-01]`). This said `.min(u16::MAX as f64)`, which is
        // eight times `max_texture_dimension_2d`: an ordinary **wide, thin** layer
        // — a 1600 × 12 banner at 6×, buffer 9672 × 144 — asks for more than the
        // limit, `fx_gpu` allocates its scratch textures at exactly that, and
        // **wgpu validation panicked** where the CPU backend drew the same document
        // correctly. ⚠️ **Not the finding's own drop shadow at `offset = 1000`**,
        // which measures 8192 exactly; `gpu_effects.rs` has why. `pack`'s `min(limit)`
        // reads like the guard for this and is not — it bounds where a layer is
        // *placed* in the batch texture, and the size handed downstream is
        // `job.size`, untouched.
        let k = ondin_core::effect::buffer_downscale(effects, scale);
        let ((w, h), k) =
            crate::effects::effect_buffer((clipped.x1.ceil() - x0, clipped.y1.ceil() - y0), k);
        let saved_base = self.base;
        let saved_limit = self.limit;
        // Everything drawn inside is shifted so the buffer's own top-left is the
        // origin *and scaled into the buffer's own resolution*, so a nested effect
        // layer clamps to this buffer rather than to the page and inherits the
        // coarser scale through `self.base` without knowing about it.
        self.base = Affine::scale(1.0 / k) * Affine::translate((-x0, -y0)) * saved_base;
        self.limit = Rect::new(0.0, 0.0, w as f64, h as f64);
        self.fx.push(FxFrame {
            scene: Scene::new(),
            origin: (x0 as i32, y0 as i32),
            size: (w, h),
            effects: effects.to_vec(),
            saved_base,
            saved_limit,
            // The passes work in the buffer's units, which is the point: this is
            // what makes the kernel `BLUR_SIGMA_BUDGET` wide however far in the
            // view is zoomed.
            scale: (scale.0 / k, scale.1 / k),
            downscale: k,
            depth: 0,
            level: self.fx.len(),
        });
    }

    fn pop_layer(&mut self) {
        // An ordinary layer opened inside this effect layer closes first — see
        // `opened_ordinary`.
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
            // Never read: `override_image` points this identity at a texture. The
            // dimensions are, though — vello sizes the atlas slot from them.
            data: Blob::new(std::sync::Arc::new(&[] as &[u8])),
            format: ImageFormat::Rgba8,
            // vello 0.9 supports only straight alpha for image data (its own
            // `register_texture` says so, and its scene encoder carries a `TODO`
            // for the premultiplied variant), which is why `fx_gpu` un-premultiplies
            // on the way out.
            alpha_type: ImageAlphaType::Alpha,
            width: frame.size.0,
            height: frame.size.1,
        };
        // **Nearest while the buffer is at device resolution.** The transform is
        // then a whole-pixel translate, so a filtered sample would be reading
        // neighbours it should not and softening the very edges the passes just
        // computed. A *downscaled* buffer is the opposite case — it is being
        // magnified, and nearest would show the coarse grid as blocks in a picture
        // whose whole justification is that it has no detail that small.
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

/// Build a `vello::Scene` for `vp` (world → pixel base transform applied), and
/// the effect layers it needs resolved first. Pure; no GPU device required.
///
/// **The jobs come back rather than being drawn**, because a `vello::Scene` is a
/// recording: there is no point during the walk at which an effect layer's pixels
/// exist to be filtered. [`VelloGpuRenderer::render`] rasterizes each job, runs
/// `fx_gpu` over it and points the job's atlas slot at the result.
pub fn build_scene(
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    overrides: &RenderOverrides,
    images: &ImageStore,
) -> (Scene, Vec<EffectJob>) {
    // From `scene`, not rebuilt here — see the note in `cpu::render_to_rgba`.
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
    scene::build(doc, res, vp, overrides, &mut painter);
    (painter.root, painter.jobs)
}

/// Which renderer's atlas an effect layer's slot was registered in — the page's,
/// or the one belonging to a numbered pass.
///
/// **Not an `Option<usize>`**, because the two are read at the end of the frame to
/// unregister and the page is not "no renderer" (§15 D404).
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

/// Shelf-pack a level's layers into as few textures as the device allows **and
/// as the page's own scale permits** — `sizes` is each layer's buffer, in the
/// order the walk opened them, and the answer indexes back into it.
///
/// **Simple rows, not a good packer.** The point is the *pass count*, not the
/// pixels: a level almost always fits in one texture, and a tighter packing would
/// save memory that is freed at the end of the frame anyway. What matters is that
/// the result is one chunk whenever it can be, because each chunk is a vello
/// resolve pass and passes are what evict the page's pictures (§15 D344).
///
/// `limit` is the device's maximum texture dimension, and it only ever wraps
/// *between* layers. ⚠️ **This said "a layer larger than that cannot exist — the
/// walk clamps every buffer to the target, which is the viewport", and that was
/// wrong in the half that matters**: `push_effect_layer` clamps to the target
/// **grown by the stack's escape** (`effect::buffer_box`, §15 D342), so a blurred
/// layer's buffer is legitimately *larger* than the viewport on every side. It was
/// measured at 2069 device pixels tall against a 1980-pixel viewport while §15
/// D402 was being chased. Nothing depends on the false half — `min(limit)` is
/// unconditional — but it is the second time a comment on this function has
/// asserted the thing that needed checking (D395 records the first).
///
/// 🚨 **And *"nothing depends on the false half"* was the third time** (§15 D744,
/// `[S11.2-L1-01]`). `min(limit)` is unconditional and it is applied to the wrong
/// value: it bounds where a layer is **placed**, and the size handed downstream is
/// `job.size`, untouched — so `Slice { size: job.size }` and the atlas
/// `ImageData` both described a texture larger than the chunk holding them, and
/// `fx_gpu` allocated its scratch at that size and panicked in wgpu validation.
/// The repair is not here: `push_effect_layer` caps the buffer where it is
/// **created**, at `effects::MAX_EFFECT_BUFFER_SIDE`, by resampling. **So the
/// clamp below is now genuinely dead** — every size reaching this function is
/// already inside the cap, which is at most `limit` — and the `debug_assert!` is
/// what says so out loud rather than a fourth comment claiming it.
///
/// `budget` is the page's own pixel count, and it is what stops a level being
/// **one enormous scene**. Three viewport-sized layers packed side by side ask
/// vello for a target twice the area of the page carrying the same paths, which is
/// past the capacity of its fixed-size intermediate buffers — and vello 0.9 cannot
/// report that: `render_to_texture` neither reads the bump allocators back nor
/// returns an error, so the overflow surfaces as a *hung* fine shader, a GPU the
/// watchdog resets after two seconds, and a panic in whatever the next frame
/// touches first (§15 D402). Splitting the level into passes of at most the page's
/// area keeps every scene inside a load the page itself is measured to survive —
/// 20,000 paths at 3840×2160 in 30 ms, against a batch that died at 2,500.
///
/// A single layer over budget still gets a chunk to itself: the alternative is a
/// buffer coarser than its own drawing, and only a `LayerBlur` may buy that
/// (`effect::buffer_downscale`).
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
        // What this chunk's texture would measure with the layer in it — the
        // packing is rows, so both terms can grow.
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
    /// A **second** vello renderer, for the effect layers' offscreen sub-scenes.
    ///
    /// ⚠️ **Not a spare — the page's renderer cannot do this work** (§15 D344).
    /// vello's image atlas evicts a resident image that has not been *drawn* for
    /// two resolve passes (`EVICT_AFTER_GENERATIONS`), and every
    /// `render_to_texture` is one pass. Rendering the sub-scenes on the page's own
    /// renderer therefore spent two or more passes a frame on scenes that do not
    /// contain the page's pictures, and an image fill drawn once a frame fell out
    /// of the atlas from the **second** frame onwards. Reported as *"pasting an
    /// image into the canvas while there's an effect on a layer makes the image
    /// empty"* — with the bounding box, the layers-row thumbnail and the Fill
    /// swatch all still there, because those read the image *store* and only the
    /// canvas reads the atlas.
    ///
    /// Separated, each renderer sees the one-pass-per-frame rhythm vello is tuned
    /// for: the page's atlas holds the page's pictures, and this one's holds
    /// whatever is drawn inside effect layers.
    ///
    /// **Built on first use**, because most documents carry no effects and a vello
    /// `Renderer` is a full set of compute pipelines.
    layers: Vec<Renderer>,
    /// The largest pass count any frame has demanded — i.e. how many entries
    /// [`Self::layers`] has grown to, and how many vello `Renderer`s this process
    /// is holding (§15 D763).
    ///
    /// **Recorded because `[S11.2-L4-05]` cannot be ranked without it.** The
    /// per-renderer cost is measured — a **6–8 ms** band of pipeline build on an
    /// otherwise-idle adapter, once ever per pass index, and 15 ms for the first
    /// `Renderer` in the process (§15 D777); ⚠️ **about seven times that when the
    /// on-device suite is contending for the adapter**, 48–65 ms, which is the
    /// number a reader who measures this the easy way will get — while the
    /// *number of them a real document demands* is reasoned
    /// from `pack`'s budget rule and has never been observed. A finding whose
    /// severity turns on an unobserved quantity is exactly `[S4.1-L4-08]`'s shape
    /// — it ranked itself on one call where its own evidence named fourteen — and
    /// the repair there was to go and count.
    ///
    /// 🚨 **Do not read *"is measured"* as settled without reading D777, because
    /// the record carried two answers fifteen-fold apart for this one number.**
    /// §15 D344 says a vello `Renderer` costs *"84 ms to build (49 with area-only
    /// antialiasing)"*; the finding, D763 and D776 all say *"~5.7 ms"*. Both live,
    /// neither citing the other. D777 timed it: **84 does not reproduce**, against
    /// the same `AaSupport::all()` default and the same pinned vello 0.9.0 D344
    /// was written under — and D344 records no profile, machine or version to
    /// adjudicate it by, which is why it stood for three weeks.
    ///
    /// ⚠️ **Not `layers.len()` read directly**, though today they are equal: the
    /// vector is what is *held* and this is what was *asked for*, and any future
    /// bound or reuse scheme (the finding's own suggestion) makes them diverge —
    /// at which point the number worth knowing is still this one.
    pass_high_water: usize,
    /// The effect passes' compute pipelines, built once — shader compilation is
    /// the expensive part and there is one device.
    fx: FxPipelines,
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
        })
    }

    /// How many effect passes the busiest frame so far has demanded — the `K` in
    /// `[S11.2-L4-05]`'s `K × ~5.7 ms`, whose per-renderer half is a measured
    /// **6–8 ms** on an idle adapter (§15 D777), and the number that finding needs
    /// and does not have (§15 D763).
    ///
    /// **Read this on a real drawing before ranking that finding.** Nothing in the
    /// suite answers it: `tests/render.rs`'s `layers.len()` assertions are the
    /// `RecordingPainter`'s CPU-side layer *stack*, a different thing entirely;
    /// `tests/gpu_device.rs` never reaches an effect pass; and `zoom_sweep.rs`
    /// prints frame times without touching either this or `Renderer::new`.
    /// ⚠️ **`gpu_effects.rs` prints it for *fixtures* now** — D763's nested
    /// document and D776's siblings — **and times `Renderer::new` itself (D777).**
    /// What is still unread is a real drawing, which is what this sentence asks
    /// for.
    ///
    /// 🚨 **The bound-and-reuse pool the finding proposes is ruled on and
    /// declined** (§15 D779). The arithmetic after D776 and D777 is a **one-off
    /// hitch**: `K` grows with the ratio of total effect-buffer area to page area
    /// rather than without limit in any direction a drawing goes, and at 1920×1080
    /// it takes on the order of **forty overlapping on-screen effect layers** to
    /// demand a second pass. At the worst `K` ever measured — 11, on a 100×100
    /// fixture page — the bill is about 77 ms, once, in a process's life.
    ///
    /// ⚠️ **And a pool has to argue against §15 D344 rather than around it.** That
    /// entry is why there is one renderer per pass; it is a **correctness** rule,
    /// and the symptom it was written for was a pasted image going blank from the
    /// second frame onwards. Trading a one-off hitch for a re-run of that bug is
    /// the wrong way round. **If the hitch is ever felt, the first thing to try is
    /// building renderers off the critical frame, not reusing one across passes** —
    /// that leaves D344's rule intact — and it wants an on-device test that
    /// reproduces D344's symptom *before* anything is reused, not after.
    pub fn pass_high_water(&self) -> usize {
        self.pass_high_water
    }

    /// Rasterize every effect layer, filter it, and point its atlas slot at the
    /// result — everything that has to happen before the page's own scene is
    /// rendered.
    ///
    /// ⚠️ **One vello pass per *renderer*, and that is a correctness rule rather
    /// than a saving** (§15 D344). vello's image atlas evicts a resident image
    /// that has not been *drawn* for two resolve passes, and every
    /// `render_to_texture` is one — so a renderer that does N passes a frame loses
    /// any picture that appears in only one of them. Sibling effect layers are
    /// therefore packed into as few textures as possible and rasterized together,
    /// and every pass takes the next renderer, which leaves each of them at the
    /// one-pass-per-frame rhythm vello is tuned for.
    ///
    /// **The unit was the nesting level until §15 D402**, on the reasoning that a
    /// level is one pass; the packer's page-area budget made that false — a level
    /// of viewport-sized layers is now deliberately split across passes — so the
    /// renderers are handed out per pass instead. That is the same rule stated
    /// against what it was always about, and it is *why* the budget is safe to
    /// impose: without it, splitting a level would put D344's bug back.
    ///
    /// **Deepest level first.** A nested layer's texture has to be registered
    /// before the scene that *draws* it is rasterized; `pop_layer` records a frame
    /// as it closes, so the levels only have to be walked in descending order.
    ///
    /// ⚠️ **A slot has to be registered on the renderer that will *draw* it, which
    /// for a nested layer is not the page's** (§15 D404). An effect layer's result
    /// is drawn by the scene it popped back into: the page for a layer at level 0,
    /// and the **parent layer's own sub-scene** for anything deeper — and that
    /// sub-scene belongs to a level-above pass with an atlas of its own. Overriding
    /// only the page's, which is what this did until 2026-09-01, made vello panic
    /// with *"Tried to draw an invalid empty image"* the moment one effect layer
    /// contained another. That is why the passes are **planned before any of them
    /// runs**: a level's consumers are the passes of the level above, which have
    /// not happened yet.
    ///
    /// Returns each atlas slot *and the renderer holding it*, so the caller can
    /// unregister them after the frame: `override_image` keys the engine's map by
    /// blob id and every frame mints new ones, so leaving them in would grow that
    /// map without bound.
    fn resolve_effects(
        &mut self,
        device: &Device,
        queue: &Queue,
        jobs: Vec<EffectJob>,
        budget: u64,
    ) -> Result<Vec<(Holder, ImageData)>, vello::Error> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let deepest = jobs.iter().map(|j| j.level).max().unwrap_or(0);
        // The packed texture may not exceed what the device can allocate. Shelf
        // packing wraps at this width and the rows stack under it.
        let limit = device.limits().max_texture_dimension_2d;
        let mut registered = Vec::with_capacity(jobs.len());
        let mut by_level: Vec<Vec<EffectJob>> = (0..=deepest).map(|_| Vec::new()).collect();
        for job in jobs {
            by_level[job.level].push(job);
        }

        // The passes, in the order they will run — deepest level first — worked
        // out before any of them does, because a job at level L is drawn by level
        // L-1's passes and therefore has to name them while they are still in the
        // future. `passes[level]` is that level's stretch of pass indices.
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
        // A renderer per pass rather than one shared: see the ⚠️ above. Built on
        // demand, so the ordinary frame builds exactly one.
        //
        // 🚨 **`layers.len()` is the high-water mark of `total` and nothing ever
        // reads it** — which is the whole of `[S11.2-L4-05]` and the reason that
        // finding is still open (§15 D763). Each entry is a full vello `Renderer`,
        // **6–8 ms** of pipeline build **once ever** per pass index — the loop
        // below is the line §15 D777 times, six builds against one device — and
        // the `Vec` has no `clear`, `truncate` or `pop` anywhere in this file, so
        // it is held for the life of the process. ⚠️ **That band is an
        // otherwise-idle adapter**; under contention it is seven times more, which
        // is why D777 carries no wall-clock assertion.
        //
        // ⚠️ **So the frame that first demands `total` passes pays about
        // `(total − high-water) × 7 ms` and every frame after it pays nothing**,
        // which is the correction D763 makes to how the finding reads: the cost is
        // per pass *index*, once ever, not per frame. At D776's worst measured
        // `K` of 11 that is ~77 ms on one frame — a visible hitch, once.
        // 🚨 **D777 is also why the number above is not 84**: D344 says a
        // `Renderer` costs *"84 ms to build (49 with area-only antialiasing)"* and
        // it does not reproduce, on the same default options and the same pinned
        // vello. **Read D777 before quoting either figure.**
        //
        // ⚠️ **`K` is measured now, for fixtures** — D763's depth and D776's
        // siblings — so what the case rests on is no longer that it is unknown but
        // that nothing bounds it.
        //
        // ⚠️ **D402's page budget does not cap this — it is what drives it.**
        // `budget` is the page's own pixel count and `pack` starts a new chunk
        // whenever the accumulating texture passes it, so bounding each pass's
        // *area* makes the *count* grow with total-effect-area ÷ page-area.
        // `MAX_EFFECT_BUFFER_SIDE` (D744) bounds one layer's side, not how many
        // layers a document has. **Nothing anywhere caps `total`.**
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
                let packed = fx_gpu::fx_texture(device, w, h, "ondin-fx-batch");
                let view = packed.create_view(&Default::default());
                // Every sibling's sub-scene, each shifted to its own slot, as one
                // recording.
                let mut batch = Scene::new();
                for &(i, at) in &chunk.items {
                    batch.append(
                        &jobs[i].scene,
                        Some(Affine::translate((at.0 as f64, at.1 as f64))),
                    );
                }
                // **Transparent, not the page's ground.** Each layer is composited
                // over whatever is behind it once it comes back, so a cleared
                // background here would be a rectangle of it baked into the shadow.
                self.layers[pass].render_to_texture(
                    device,
                    queue,
                    &batch,
                    &view,
                    &RenderParams {
                        base_color: Color::TRANSPARENT,
                        width: w,
                        height: h,
                        antialiasing_method: AaConfig::Area,
                    },
                )?;
                for &(i, at) in &chunk.items {
                    let job = &jobs[i];
                    // `None` is a stack whose entries are all hidden or neutral,
                    // which the walk should not have opened a layer for. It cannot
                    // fall back to the packed texture — that holds every sibling —
                    // so it copies this slot out on its own.
                    let slice = fx_gpu::Slice { at, size: job.size };
                    let filtered = fx_gpu::run(
                        device,
                        queue,
                        &self.fx,
                        &packed,
                        slice,
                        &job.effects,
                        job.scale,
                    )
                    .unwrap_or_else(|| fx_gpu::copy_out(device, queue, &packed, slice));
                    let at_texture = |t: &wgpu::Texture| wgpu::TexelCopyTextureInfoBase {
                        texture: t.clone(),
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    };
                    // Whoever draws this layer's result needs it in *their* atlas:
                    // the page for a top-level layer, and every pass of the level
                    // above for a nested one — which of those passes holds the
                    // parent's chunk is not recorded, and registering on all of
                    // them costs an atlas entry rather than a search.
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
        Ok(registered)
    }

    /// Build the scene for `vp` and rasterize it into `target` (an RGBA texture
    /// view sized `vp.pixel_size`). `base_color` clears the target first.
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
    ) -> Result<(), vello::Error> {
        let (scene, jobs) = build_scene(doc, res, vp, overrides, images);
        // The page's own pixel count is the budget every effect pass is held to
        // (`pack`): a scene the size of the page, carrying at most the page's
        // paths, is a load this renderer is measured to survive.
        let budget = u64::from(vp.pixel_size.0) * u64::from(vp.pixel_size.1);
        let registered = self.resolve_effects(device, queue, jobs, budget)?;
        let params = RenderParams {
            base_color,
            width: vp.pixel_size.0,
            height: vp.pixel_size.1,
            antialiasing_method: AaConfig::Area,
        };
        let out = self
            .renderer
            .render_to_texture(device, queue, &scene, target, &params);
        // **Unregistered on the way out, including when the render failed.** The
        // slots were minted for this frame alone; leaving them behind would keep
        // both the map entry and the texture it holds alive for the life of the
        // renderer, which for a viewport-sized layer is 8 MB a frame. **From the
        // renderer that holds each one** — a nested layer's slot lives in a layer
        // renderer's atlas, not the page's (§15 D404).
        for (holder, image) in registered {
            match holder {
                Holder::Page => self.renderer.unregister_texture(image),
                Holder::Layer(p) => self.layers[p].unregister_texture(image),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{layer_shape, pack};
    use kurbo::{Affine, Rect, Shape};

    /// 4K, which is the viewport every measurement behind §15 D402 was taken at.
    const PAGE: u64 = 3840 * 2160;

    /// **An unclipped layer covers the whole target whatever transform it is
    /// under** (§15 D612, `[S11.2-L1-04]`).
    ///
    /// 🚨 **It did not, and the failure is silent by construction.** vello needs a
    /// shape for every layer, so this backend substituted `±1e7` and drew it under
    /// `base * transform` — i.e. `±1e7` in the **node's own local units**. An
    /// ancestor `scale(0.001)` shrank the "unbounded" layer to ±1e4 world units and
    /// cut away everything past it: measured in release, **CPU 400 green pixels,
    /// GPU 0**, for one 100×100-unit rect at world (20000, 100). The layer was in
    /// the export and absent from the canvas.
    ///
    /// **Asserted on the decision rather than on pixels, and that is the point.**
    /// The divergence lives *below* the shared document→scene walk, where the CPU
    /// pixel tests cannot reach it, and the GPU comparison harness needs a device
    /// this machine does not have. `layer_shape` is §15 D269's move applied here:
    /// the choice is lifted out of `push_layer` so a test can ask it directly, and
    /// what it asks is exactly the invariant — *the substitute does not depend on
    /// the node's transform*.
    ///
    /// **The scales are the ones that break it, not a sample.** `0.001` is the
    /// measured failure; `1000.0` is the mirror, where the old code would have
    /// covered ±1e10 and clipped nothing but is still not the target; and a
    /// **degenerate** `scale(0.0)` is in the list because the finding's own first
    /// fix sketch — invert `base * transform` and map the target back — is
    /// non-finite there, which is **G1**'s shape. The identity has no inverse to
    /// take.
    ///
    /// ⚠️ **Flip-check, run, against the plausible wrong version rather than
    /// against nothing**: `(base * transform, Cow::Owned(±1e7 rect))`, which is
    /// what shipped. It fails at the first `0.001` case — the predicted site — and
    /// the message is starker than the prediction was: the substitute lands at
    /// `x0: -23000 … x1: -15000`, so it does not merely fall short of the 800-wide
    /// target, it **misses it entirely**. The written-up prediction said "a
    /// device-space width of 20000", which was arithmetic done in the head and
    /// wrong; `base` translates as well as scales, and that is exactly the term a
    /// reasoned prediction drops. Returning `Cow::Owned(limit)` un-inflated leaves
    /// both assertions green, so the inflation is not what this test is about and
    /// its own comment says why.
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

        // The control: a real clip is still drawn in the node's own space, or the
        // fix would be "stop clipping", which is a different bug.
        let path = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1);
        let (at, shape) = layer_shape(limit, base, Affine::scale(2.0), Some(&path));
        assert_eq!(at, base * Affine::scale(2.0), "a clip keeps its own space");
        assert_eq!(shape.bounding_box(), path.bounding_box());
    }

    /// The ordinary frame is still **one pass**, which is the property D344 needs
    /// and the budget must not cost: a few small layers pack into one texture.
    ///
    /// Flip: raising the sizes to viewport-scale is what splits them, which is the
    /// test below.
    #[test]
    fn small_layers_still_share_one_chunk() {
        let sizes = [(400, 300), (512, 512), (64, 64)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 1, "three small layers are one vello pass");
        assert_eq!(chunks[0].items.len(), 3);
    }

    /// Three viewport-sized layers are **three passes, not one 24 MP scene**.
    ///
    /// This is the case that killed the device: packed side by side they made a
    /// target far larger than the page, vello's fixed buffers overflowed, its fine
    /// shader hung and Windows reset the GPU at two seconds (§15 D402).
    ///
    /// ⚠️ **Flip-checked against the version that was actually there** — `budget`
    /// of `u64::MAX`, i.e. the old `pack(sizes, limit)` — which returns **one**
    /// chunk of 11520×2160 and fails on the length. Deleting the budget check
    /// altogether does the same thing, so the two flips agree here; the assertion
    /// that has teeth is the count, not the sizes.
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

    /// A layer that is over budget on its own still gets a chunk, rather than the
    /// packer looping or dropping it. There is nothing else to do with it: the only
    /// way to shrink a buffer is to rasterize it coarser, and only a `LayerBlur`
    /// may buy that (`effect::buffer_downscale`, §15 D395).
    ///
    /// Two different claims here and only one of them has a flip. The **count**
    /// falls out of the budget — without it these two pack into one 4100×3000
    /// chunk — while *not looping and not dropping* is what the item assertions
    /// pin, and no plausible one-line change makes those fail without failing the
    /// count first.
    #[test]
    fn one_layer_over_budget_gets_its_own_chunk_anyway() {
        let sizes = [(4000, 3000), (100, 100)];
        let chunks = pack(&sizes, 8192, PAGE);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].items, vec![(0, (0, 0))]);
        assert_eq!(chunks[1].items, vec![(1, (0, 0))]);
    }

    /// The device's maximum texture dimension still wraps, and it wraps *before*
    /// the budget does — a chunk that cannot be allocated is a hard error where one
    /// over budget is only slow.
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
