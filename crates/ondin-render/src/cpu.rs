//! CPU backend: `vello_cpu` — deterministic output for export, CI, and headless
//! (`ondin serve`) (§6.3). No GPU. Drives all PNG export, so a raster is
//! reproducible without a device.
//!
//! **Byte-comparable across machines of one architecture, and only because the
//! SIMD level is pinned** — see [`export_settings`]. This header claimed it
//! unconditionally until 2026-08-22, when it turned out that
//! `RenderSettings::default()` detects the level and a **stroked path**
//! rasterizes differently under AVX2 from the way it does under SSE4.2 or the
//! scalar fallback: 17 bytes over 7 pixels of the exported buffer, worst channel
//! delta 9, all of it on near-transparent edge pixels (§15 D304). Every other
//! primitive measured — fills, gradients, blend layers, glyph runs — is
//! level-invariant.
//!
//! The pin is on the **export** path alone. [`rasterize_glyph_run`] keeps the
//! detected level, because it draws the font picker's live preview strips —
//! doubly safe, since glyph runs are level-invariant and those strips are the
//! same pixels whatever the level.
//!
//! ⚠️ **aarch64/NEON is untested.** The measurement is x86_64 only, so whether
//! `Neon` agrees with `Fallback` is unknown rather than known — which is why the
//! pin is `Level::baseline()`, a *per-architecture* answer, and why a golden
//! carried between architectures is not yet something to rely on.
//!
//! ⚠️ **And a golden does not survive a `vello_cpu` bump either — measured, not
//! feared** (§15 D414). Rendering the §11 golden fixture under 0.0.9 and 0.2.0
//! and comparing the raw buffers: **162 bytes over 78 pixels of 571,200 differ,
//! worst channel delta 1, and the alpha channel does not move at all** — so
//! coverage is identical and only colour rounds differently. Every one of those
//! pixels belongs to **one mitered outside stroke**, confirmed by hiding that
//! stroke and re-rendering, which makes the two buffers byte-identical. Fills,
//! the gradient, blend layers, glyph runs, the boolean, and the *other* two
//! strokes in the fixture are unmoved. That is D304's finding arriving by a
//! second road: the stroked path was the only level-variant primitive and is now
//! the only version-variant one, which is a fact about stroke expansion rather
//! than about either variable. **The consequence for §11 is that a checked-in PNG
//! is a golden for one architecture *and one `vello_cpu`*,** and the regeneration
//! note in `tests/goldens.rs` is what has to carry that.

use crate::color;
use crate::images::ImageStore;
use crate::renderer::{RenderOverrides, Viewport};
use crate::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use kurbo::{Affine, BezPath, Rect, Stroke};
use ondin_core::peniko::{
    BlendMode, Brush as BackendBrush, Color, Compose, Fill as VelloFill, Mix,
};
use ondin_core::{Brush, Document, Effect, ImageId, Resolved};
use rustc_hash::FxHashMap;
use vello_cpu::{
    Glyph, Image as CpuImage, ImageSource, Level, Pixmap, RasterizerSettings, RenderContext,
    RenderSettings, Resources,
};

/// The rasterizer settings every export uses — **the SIMD level pinned to
/// [`Level::baseline`]** rather than detected (§11, §15 D304).
///
/// `RenderSettings::default()` takes `Level::try_detect()`, i.e. whatever this
/// CPU happens to have, and a *stroked* path rasterizes differently under AVX2
/// from the way it does under SSE4.2 or the scalar fallback — which would make a
/// checked-in PNG a golden for one machine and a failing test on the next. Every
/// other primitive measured is level-invariant, so this one line is the whole of
/// what §11's PNG goldens were waiting on.
///
/// **`baseline()` rather than the scalar fallback**, which would be the universal
/// one: `Level::fallback()` needs fearless_simd's `force_support_fallback`
/// feature, which neither `vello_cpu` nor `vello_common` re-exports, and its own
/// doc says the fallback branch may still be auto-vectorised against the ambient
/// target — so it removes the *dispatch* variable without guaranteeing the last
/// byte. `baseline()` is target-derived (`Fallback` on default x86_64, `Neon` on
/// aarch64), so goldens are per-architecture. That was the choice made; the
/// alternatives are in D304.
///
/// `num_threads` is the only other field and is never read without
/// `multithreading`, so it is left to `Default`.
///
/// ⚠️ **`render_mode` used to be the third field here and moved out of this struct
/// in `vello_cpu` 0.2.0** (§15 D414) — it is on [`RasterizerSettings`] now, which
/// is a *rasterize*-time argument rather than a context-construction one. That is
/// the version bump D304 warned this entry about arriving, and it arrived by
/// **moving** a field rather than by changing a default, which is the one shape
/// the warning did not name: `..Default::default()` went on compiling and went on
/// meaning something different.
///
/// **Export only.** [`rasterize_glyph_run`] keeps the detected level — it draws
/// the font picker's live preview strips, where speed is the point and nothing is
/// ever compared.
fn export_settings() -> RenderSettings {
    RenderSettings {
        level: Level::baseline(),
        ..Default::default()
    }
}

/// Every rasterizer setting spelled out, so a golden cannot move on an upstream
/// default (§15 D414).
///
/// **This is the lesson of the bump that created it.** §11's parked PNG-goldens
/// entry warned that `RenderMode` and `num_threads` were *defaults rather than
/// pins*, and that the difference only shows at a version bump — the one moment a
/// golden most needs to be trusted. `vello_cpu` 0.2.0 then split the settings in
/// two and **added three live knobs nobody had named**: `composite_mode`,
/// `pixel_format` and `offset`, each of which changes what lands in the pixmap.
/// Spelling all four out costs one struct literal and removes the whole class.
///
/// The values are today's defaults, chosen deliberately rather than inherited:
/// `OptimizeSpeed` because it is what this build compiles anyway (⚠️ the
/// speed/quality `match` is behind `cfg(all(u8_pipeline, f32_pipeline))` and only
/// `u8_pipeline` is on, so the field is *inert* here — D304's finding, re-checked
/// against 0.2.0's `dispatch::single_threaded` and still true); `Replace` because
/// every target this hands to is a fresh [`Pixmap`]; `Rgba8` because
/// [`color::straight_rgba8_from_premul`] reads the buffer as premultiplied RGBA8
/// immediately afterwards, so this one is load-bearing rather than nominal; and a
/// zero `offset` because the context and the pixmap are always built at the same
/// size.
///
/// **Used by every path, not just export**, so the canvas and the exported file
/// cannot disagree about rasterization — which is the property `cpu.rs` mirrors
/// `gpu.rs` to keep.
fn raster_settings() -> RasterizerSettings {
    RasterizerSettings {
        render_mode: vello_cpu::RenderMode::OptimizeSpeed,
        composite_mode: vello_cpu::CompositeMode::Replace,
        pixel_format: vello_cpu::PixelFormat::Rgba8,
        offset: (0, 0),
    }
}

#[derive(Default)]
pub struct VelloCpuRenderer;

impl VelloCpuRenderer {
    pub fn new() -> Self {
        Self
    }

    /// Render `vp` to a straight-alpha RGBA8 buffer, returning `(rgba, w, h)`.
    /// No GPU; used by `ondin-export` PNG. The base transform maps the world-space
    /// `vp.view` onto the pixel grid.
    pub fn render_to_rgba(
        &mut self,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
    ) -> (Vec<u8>, u32, u32) {
        self.render_subtrees_to_rgba(doc, res, vp, images, &[doc.root()])
    }

    /// [`Self::render_to_rgba`] restricted to **these subtrees** — the raster half
    /// of `scene::build_of`, and what an export of a selection needs.
    ///
    /// Kept as a second entry point rather than an extra argument on the first
    /// because `render_to_rgba` has two dozen call sites, almost all of them tests
    /// that mean "the whole page"; threading an `origins` through every one of them
    /// would be churn that says nothing. The whole-page call is now one call of
    /// this, so there is still only one spelling of the walk.
    pub fn render_subtrees_to_rgba(
        &mut self,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
        origins: &[ondin_core::NodeId],
    ) -> (Vec<u8>, u32, u32) {
        let (w, h) = vp.pixel_size;
        let mut ctx = RenderContext::new_with(w as u16, h as u16, export_settings());

        // From `scene`, not rebuilt here: the walk snaps boxes onto the grid this
        // transform defines, so a second copy that drifted would have it rounding
        // to a grid nothing rasterizes on — invisibly, and only at fractional zoom.
        let base = scene::device_base(vp);

        let mut resources = Resources::new();
        {
            let mut painter = CpuPainter {
                images,
                image_memo: FxHashMap::default(),
                root: &mut ctx,
                fx: Vec::new(),
                base,
                resources: &mut resources,
            };
            scene::build_of(
                doc,
                res,
                vp,
                &RenderOverrides::default(),
                origins,
                &mut painter,
            );
        }

        ctx.flush();
        let mut pixmap = Pixmap::new(w as u16, h as u16);
        ctx.render_with(&mut pixmap, &mut resources, raster_settings());

        let straight = color::straight_rgba8_from_premul(pixmap.data_as_u8_slice());
        (straight, w, h)
    }
}

/// The widest or tallest buffer `vello_cpu` can be asked for, in pixels.
///
/// **65408 is `511 × 128`, and both factors are vello's** (§15 D434, D586).
/// `vello_cpu`'s depth buffer holds one bucket per `DEPTH_BUCKET_WIDTH = 128`
/// pixels (`coarse/depth.rs`), and `bucket_span` computes a bucket run's pixel
/// span as `index as u16 * DEPTH_BUCKET_WIDTH` — so the highest bucket index that
/// cannot overflow a `u16` is `511`, and the widest buffer that cannot produce a
/// higher one is `511 × 128`. That also clears `vello_common`'s
/// `snap_to_tile_coordinates`, whose `checked_next_multiple_of(Tile::WIDTH)`
/// needs a side of at most `65532`; 65408 is already a multiple of 4.
///
/// ⚠️ **Not `u16::MAX`, which is the one value the backend cannot take.** That
/// mistake is D434's whole story at the export end, and the same `as u16` had
/// been made again here.
///
/// ⚠️ **Neither of vello's constants is public**, so this cannot be derived at
/// compile time and an upgrade can silently invalidate it. What stands in for a
/// gate is a test that *renders at the cap on both axes* —
/// `ondin-export`'s `a_raster_at_the_cap_renders_on_either_axis`, and
/// `a_strip_too_big_for_the_backend_is_refused_rather_than_truncated` below.
///
/// **It lives here rather than in `ondin-export` because the limit is the CPU
/// backend's**, and this crate owns it; `png::MAX_RASTER_SIDE` is an alias, so
/// there is one number and one argument for it.
pub const MAX_RASTER_SIDE: u32 = 511 * 128;

/// Rasterize one glyph run into a straight-alpha RGBA8 buffer `w`×`h` pixels,
/// **white ink on transparent** — a coverage strip, not a finished drawing.
///
/// This is the font picker's preview row (§5.4a): a family name drawn in the
/// family it names. White-on-transparent rather than themed, because the colour
/// is the *caller's* — a row is dimmer when disabled, brighter when hovered, and
/// inverts with the theme, while the glyphs are the same glyphs. One cached image
/// tinted per draw beats four rasterizations of the same name.
///
/// `transform` maps the run's own coordinates (x along the run, y on the
/// baseline) into this buffer's pixels, so a caller working in points passes the
/// display scale in here.
///
/// 🚨 **`None` for a size this backend cannot hold, rather than a truncated
/// buffer** (§15 D586, `[S21-L1-05]`). `RenderContext` and `Pixmap` are built
/// from `u16` dimensions, and this used to reach them through a bare `as u16` —
/// a *silent wrap*, not a clamp. Measured: `w=8, h=70000` returned 142,848 bytes
/// where the caller's `ColorImage` wanted 2,240,000, and epaint asserted on the
/// mismatch — a panic on the UI thread, from a font's own metrics. The same
/// `as u16` mechanism is `[S8.2-L1-01]`'s in `export/png.rs`, at the other end of
/// the workspace; refusing here is the shape both want. A caller that cannot draw
/// a strip already has a fallback — the font picker draws the name in the UI font.
pub fn rasterize_glyph_run(
    run: &ondin_core::GlyphRun,
    transform: Affine,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || w > MAX_RASTER_SIDE || h > MAX_RASTER_SIDE {
        return None;
    }
    let (w, h) = (w as u16, h as u16);
    let mut ctx = RenderContext::new(w, h);
    let mut resources = Resources::new();
    ctx.set_transform(transform);
    ctx.set_paint(Color::WHITE);
    ctx.glyph_run(&mut resources, &run.font)
        .font_size(run.font_size)
        .normalized_coords(&run.coords)
        .fill_glyphs(run.glyphs.iter().map(|g| Glyph {
            id: g.id,
            x: g.x,
            y: g.y,
        }));
    ctx.flush();
    let mut pixmap = Pixmap::new(w, h);
    ctx.render_with(&mut pixmap, &mut resources, raster_settings());
    Some(color::straight_rgba8_from_premul(pixmap.data_as_u8_slice()))
}

struct CpuPainter<'a> {
    /// Decoded images, keyed by the id a fill refers to.
    images: &'a ImageStore,
    /// `ImageSource`s built this render — see [`CpuPainter::image_source`].
    image_memo: FxHashMap<(ImageId, [u32; 7]), (ImageSource, Affine)>,
    /// The page. Drawing goes here whenever no effect layer is open.
    root: &'a mut RenderContext,
    /// One frame per open effect layer, innermost last — see
    /// [`CpuPainter::push_effect_layer`].
    ///
    /// **Offscreen contexts rather than `vello_cpu`'s own filter layers.** Its
    /// `push_layer` does take a `Filter`, and it would serve for two of the four
    /// effects; the other two have no primitive there at all, and the GPU backend
    /// has none for any of them. One mechanism that all four go through, on both
    /// backends, is worth more than a fast path for half of them — see
    /// `crate::effects`.
    fx: Vec<FxFrame>,
    /// world → pixel transform applied on top of each node's world transform.
    ///
    /// **Current, not the page's**: an open effect layer draws into a buffer whose
    /// origin is its own, so pushing one replaces this and popping restores what
    /// it saved. A backend that left this alone would draw the layer's contents at
    /// the page's coordinates inside a buffer that starts somewhere else — i.e.
    /// mostly off the edge of it.
    base: Affine,
    /// Glyph atlas/scratch resources for text runs.
    resources: &'a mut Resources,
}

/// One open effect layer: where its buffer sits and what to do to it on the way
/// out.
struct FxFrame {
    ctx: RenderContext,
    /// Device-space top-left of `ctx`'s buffer, in the *enclosing* target's
    /// pixels.
    origin: (i32, i32),
    size: (u16, u16),
    effects: Vec<Effect>,
    /// The enclosing target's `base`, put back when this layer pops.
    saved_base: Affine,
    /// **Buffer** pixels per document unit along each axis, for the layer this
    /// belongs to — what turns an authored blur radius into a kernel and an
    /// authored offset into pixels. Device pixels divided by [`Self::downscale`].
    scale: (f64, f64),
    /// How many device pixels one buffer pixel covers (`effect::buffer_downscale`).
    /// `1.0` for every layer that is not heavily blurred, which is most of them.
    downscale: f64,
    /// How many *ordinary* layers are open inside this one.
    ///
    /// The whole of what tells `pop_layer` which kind of layer it is closing —
    /// see [`CpuPainter::opened_ordinary`] for the panic its absence caused.
    depth: usize,
}

impl CpuPainter<'_> {
    /// `vello_cpu`'s paint type is `peniko::Brush` over its own image slot, so
    /// solids and gradients pass straight through once [`color::brush_to_backend`]
    /// has turned the model's brush into a backend one.
    ///
    /// **Returns whether there is anything to paint.** `false` means the brush
    /// resolved to nothing — an image whose reference the store cannot produce
    /// pixels for — and the caller skips the draw entirely, so a shape whose only
    /// fill is a missing image costs no path work at all.
    ///
    /// **The paint transform is context state here, unlike vello's**, so it is
    /// set on every call rather than only for an image: left standing, an
    /// image's framing would go on transforming the next shape's gradient.
    /// `vello_cpu` composes it as `transform * paint_transform`, the same order
    /// vello does, so a framing computed in the node's local space means the same
    /// thing on both backends.
    /// The context drawing currently goes to: the innermost open effect layer, or
    /// the page.
    fn ctx(&mut self) -> &mut RenderContext {
        match self.fx.last_mut() {
            Some(f) => &mut f.ctx,
            None => self.root,
        }
    }

    /// [`Self::ctx`] and the glyph resources together, for the one call that
    /// needs both at once.
    fn split(&mut self) -> (&mut RenderContext, &mut Resources) {
        let ctx = match self.fx.last_mut() {
            Some(f) => &mut f.ctx,
            None => &mut *self.root,
        };
        (ctx, &mut *self.resources)
    }

    /// Record that an ordinary layer has opened inside the innermost effect
    /// layer, so [`ScenePainter::pop_layer`] can tell the two apart.
    ///
    /// ⚠️ **Without this the pops mis-pair, and it is a panic rather than a wrong
    /// picture.** The walk pushes an effect layer *outside* the clip and the
    /// opacity (§6.4), so a layer with a shadow and `opacity < 1` — or a frame
    /// with a shadow and `clip` — opens an ordinary layer inside the effect one.
    /// A `pop_layer` that closed whichever effect frame happened to be open would
    /// close it on that inner pop, leaving the ordinary layer open forever;
    /// `vello_cpu` notices at `flush` and panics with *"some layers haven't been
    /// popped yet"*. Found on 2026-08-24 by mirroring this backend onto the GPU
    /// and having to write the pairing rule down (§15 D338).
    fn opened_ordinary(&mut self) {
        if let Some(f) = self.fx.last_mut() {
            f.depth += 1;
        }
    }

    fn apply_paint(&mut self, brush: &Brush, framing: Option<ondin_core::Framing>) -> bool {
        // **The image arm is taken from the *model's* brush, not the backend's.**
        // `vello_cpu` paints through `ImageSource`, which is a premultiplied
        // `Pixmap` — a whole second buffer, built by walking every pixel. Going
        // via `brush_to_backend` would produce a `peniko::ImageData` and then
        // convert it on every fill; taking the *reference* here instead is what
        // lets the conversion be memoized, since the reference carries the id and
        // the adjustment the memo is keyed on and the backend brush has dropped
        // both.
        if let Brush::Image(img) = brush {
            let Some((source, to_source)) = self.image_source(&img.image) else {
                return false;
            };
            self.ctx().set_paint_transform(
                color::framed_transform(framing, to_source).unwrap_or(Affine::IDENTITY),
            );
            self.ctx().set_paint(CpuImage {
                image: source,
                sampler: color::framed_sampler(img.sampler, framing),
            });
            return true;
        }
        // **Resolved before the context is borrowed**, and the transform is set
        // from the answer rather than cleared: a gradient carries its own now
        // (§15 D412) and a solid answers the identity, so one call keeps the
        // "set on every call" rule this function's doc states without a second
        // arm having to remember it.
        // **`Source`, where the GPU backend asks for `Atlas`** (§15 D745). This
        // path is every PNG export, so it must write the picture the document
        // holds; there is no atlas here to be too small for it.
        let resolved = color::brush_to_backend(brush, self.images, color::Resolution::Source);
        match resolved {
            Some((BackendBrush::Solid(c), t)) => {
                self.ctx().set_paint_transform(t);
                self.ctx().set_paint(c);
            }
            Some((BackendBrush::Gradient(g), t)) => {
                self.ctx().set_paint_transform(t);
                self.ctx().set_paint(g);
            }
            // Unreachable: the image arm returned above, so `brush_to_backend`
            // cannot be handed one. Spelled out rather than wildcarded so that
            // changing either half stops compiling here.
            Some((BackendBrush::Image(_), _)) | None => return false,
        }
        true
    }

    /// The premultiplied pixmap `r` paints from, built at most once per render,
    /// with the map from its pixel grid onto the source's.
    ///
    /// **The memo is per render, not per store, and that is the cheap answer.**
    /// The conversion allocates a second buffer the size of the image and touches
    /// every pixel, so doing it per *fill* would make two layers showing one photo
    /// cost twice — but caching it on the store would mean the GPU canvas carrying
    /// premultiplied buffers it never paints with. A render is exactly the scope
    /// where the work is shared and nothing outlives it.
    ///
    /// **Keyed by the picture *and* what has been done to it**, because that is
    /// what the store answers: two fills showing one photograph at two exposures
    /// are two buffers, and a memo keyed by the id alone would paint the second
    /// with the first's pixels. The adjustment enters the key as its bits, for
    /// the reason `images::AdjustKey` gives.
    fn image_source(&mut self, r: &ondin_core::ImageRef) -> Option<(ImageSource, Affine)> {
        let key = (
            r.id.clone(),
            ondin_core::Adjustment::ALL.map(|k| k.of(&r.adjust).to_bits()),
        );
        if let Some((source, to_source)) = self.image_memo.get(&key) {
            return Some((source.clone(), *to_source));
        }
        let px = self.images.pixels(r)?;
        let source = ImageSource::from_peniko_image_data(&px.data);
        self.image_memo.insert(key, (source.clone(), px.to_source));
        Some((source, px.to_source))
    }
}

impl ScenePainter for CpuPainter<'_> {
    /// The store, exactly as the GPU painter answers it — the placeholder must
    /// appear in `ondin export --png` and on the canvas from one decision.
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
        let base = self.base;
        self.ctx().set_transform(base * transform);
        if !self.apply_paint(brush, framing) {
            return;
        }
        // **Set and restored around the one call**, because this context keeps the
        // rule until something changes it — the same reason `push_layer` below
        // restores it. A fill left in even-odd would silently re-read every later
        // shape on this backend and none on the GPU one, which is the exact class of
        // divergence §3 exists to prevent.
        self.ctx().set_fill_rule(match rule {
            ClipRule::NonZero => VelloFill::NonZero,
            ClipRule::EvenOdd => VelloFill::EvenOdd,
        });
        self.ctx().fill_path(path);
        self.ctx().set_fill_rule(VelloFill::NonZero);
    }

    fn stroke_path(&mut self, transform: Affine, path: &BezPath, stroke: &StrokePaint<'_>) {
        let base = self.base;
        self.ctx().set_transform(base * transform);
        if !self.apply_paint(stroke.brush, stroke.framing) {
            return;
        }
        let mut k = Stroke::new(stroke.width)
            .with_join(stroke.join)
            .with_caps(stroke.cap)
            .with_miter_limit(stroke.miter_limit);
        if !stroke.dashes.is_empty() {
            k = k.with_dashes(stroke.dash_offset, stroke.dashes.iter().copied());
        }
        self.ctx().set_stroke(k);
        self.ctx().stroke_path(path);
    }

    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>) {
        let base = self.base;
        self.ctx().set_transform(base * transform);
        // ⚠️ **The return value, which this used to discard** (§15 D437).
        // `apply_paint`
        // answers `false` for "the brush resolved to nothing" — a fill whose
        // image the store has no pixels for — and `fill_path` and `stroke_path`
        // beside it both return on it. `vello_cpu`'s paint is *context state*, so
        // dropping the answer here did not draw the text in nothing: it drew it
        // in **whatever the last `set_paint` left there**, which is the previous
        // node's colour. Measured as a word rendered in the preceding
        // rectangle's red, 3008 px of it, in `ondin export --png` — and since
        // the CPU backend is the reference every golden agreed. The GPU backend
        // has the guard and its comment states the rule: type filled with an
        // unresolvable image draws nothing, as a shape does (§15 D179).
        if !self.apply_paint(run.brush, run.framing) {
            return;
        }
        // Split rather than two calls: the glyph run needs the context and the
        // scratch resources at once, and they are separate fields.
        let (ctx, resources) = self.split();
        ctx.glyph_run(resources, run.font)
            .font_size(run.font_size)
            .normalized_coords(run.coords)
            .fill_glyphs(run.glyphs.iter().map(|g| Glyph {
                id: g.id,
                x: g.x,
                y: g.y,
            }));
    }

    fn push_layer(
        &mut self,
        transform: Affine,
        clip: Option<&BezPath>,
        rule: ClipRule,
        opacity: f32,
    ) {
        self.opened_ordinary();
        // The clip path is in the node's local space, so the transform has to
        // be in place before the layer is pushed. `vello_cpu` takes the clip's
        // fill rule from the context rather than as an argument, so that has to
        // be set first too — and restored after, or it would leak into the next
        // `fill_path`.
        let base = self.base;
        self.ctx().set_transform(base * transform);
        self.ctx().set_fill_rule(match rule {
            ClipRule::NonZero => VelloFill::NonZero,
            ClipRule::EvenOdd => VelloFill::EvenOdd,
        });
        self.ctx().push_layer(clip, None, Some(opacity), None, None);
        self.ctx().set_fill_rule(VelloFill::NonZero);
    }

    fn push_mask_layer(&mut self) {
        self.opened_ordinary();
        // No transform is set: the mask's own drawing carries its transform on
        // every call, and this layer only says *how* what is drawn in it
        // composites. No clip and no opacity, for the same reason.
        self.ctx().push_layer(
            None,
            Some(BlendMode::new(Mix::Normal, Compose::DestIn)),
            None,
            None,
            None,
        );
    }

    /// Redirect drawing into a buffer of its own, to be filtered on the way out.
    ///
    /// **The buffer is clamped to the enclosing target**, so an effect on a layer
    /// mostly off-screen costs the visible part rather than the whole of it. That
    /// is also the only bound on the allocation: `bounds` comes from the walk and
    /// a large enough blur on a large enough frame is a genuinely large buffer.
    ///
    /// An empty intersection means there is nothing to draw — but the layer is
    /// still pushed, with a 1×1 buffer, because [`Self::pop_layer`] cannot tell
    /// which kind of layer it is closing and the walk owes it a pop either way.
    fn push_effect_layer(&mut self, transform: Affine, bounds: Rect, effects: &[Effect]) {
        let device = (self.base * transform).transform_rect_bbox(bounds);
        let limit = match self.fx.last() {
            Some(f) => Rect::new(0.0, 0.0, f.size.0 as f64, f.size.1 as f64),
            None => Rect::new(
                0.0,
                0.0,
                self.root.width() as f64,
                self.root.height() as f64,
            ),
        };
        // **The target grown by the escape, not the bare target** — the buffer is
        // what the silhouette is read from as well as what gets drawn, so clipping
        // it at the target's edge would cast a shadow from the visible sliver of a
        // half-scrolled layer (§15 D342).
        let m0 = (self.base * transform).as_coeffs();
        let s0 = (
            (m0[0] * m0[0] + m0[1] * m0[1]).sqrt(),
            (m0[2] * m0[2] + m0[3] * m0[3]).sqrt(),
        );
        // **The same budget the GPU backend holds a buffer to** — the page's own
        // pixel count, here the export target's (`effect::buffer_box_within`, §15
        // D402). It bites only where the escape is a large fraction of the target,
        // which an export at its own scale never reaches; it is here so the two
        // backends cannot choose different buffer boxes for one drawing.
        let clipped = ondin_core::effect::buffer_box_within(
            device,
            limit,
            ondin_core::effect::stack_escape(effects),
            s0,
            self.root.width() as f64 * self.root.height() as f64,
        );
        // `ceil`/`floor` outward, so a fractional edge keeps the pixel it touches
        // rather than losing it — the same direction every other conservative box
        // in this crate rounds.
        let (x0, y0) = (clipped.x0.floor(), clipped.y0.floor());

        // The scale the passes work in. Taken from the composed device transform
        // rather than from the viewport, so a layer inside a skewed ancestor blurs
        // by the amount its own pixels are actually stretched.
        let m = (self.base * transform).as_coeffs();
        let scale = (
            (m[0] * m[0] + m[1] * m[1]).sqrt(),
            (m[2] * m[2] + m[3] * m[3]).sqrt(),
        );

        // **The buffer is not always at device resolution** — see
        // `effect::buffer_downscale`. `GpuScenePainter::push_effect_layer` does
        // this arithmetic identically, and identically is the requirement: two
        // backends that chose different buffer resolutions would blur the same
        // drawing by different amounts (§15 D395).
        //
        // 🚨 **`crate::effects::effect_buffer` and not a second copy of the
        // arithmetic** (§15 D744, `[S11.2-L1-01]`). "Identically" was the
        // requirement and two hand-kept copies were how it was met, which is the
        // shape §15 D601 and D615 have each caught at this very pair of sites.
        // The cap was `u16::MAX` here — 127 pixels past [`MAX_RASTER_SIDE`],
        // whose own doc calls that *"the one value the backend cannot take"*
        // twenty lines above this line — and eight times what the GPU can build.
        // It is `MAX_EFFECT_BUFFER_SIDE` for both now, and a layer past it is
        // **resampled** into the cap rather than truncated.
        let k = ondin_core::effect::buffer_downscale(effects, scale);
        let ((w, h), k) =
            crate::effects::effect_buffer((clipped.x1.ceil() - x0, clipped.y1.ceil() - y0), k);
        // Both sides are inside `MAX_EFFECT_BUFFER_SIDE`, which is well inside a
        // `u16`, so this narrowing cannot be the lossy cast the old spelling was.
        let (w, h) = (w as u16, h as u16);

        let ctx = RenderContext::new_with(w, h, export_settings());
        let saved_base = self.base;
        // Everything drawn inside is shifted so the buffer's own top-left is the
        // origin, and scaled into the buffer's own resolution.
        self.base = Affine::scale(1.0 / k) * Affine::translate((-x0, -y0)) * saved_base;
        self.fx.push(FxFrame {
            ctx,
            origin: (x0 as i32, y0 as i32),
            size: (w, h),
            effects: effects.to_vec(),
            saved_base,
            // Buffer pixels per document unit — see the `scale` field.
            scale: (scale.0 / k, scale.1 / k),
            downscale: k,
            depth: 0,
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
                self.ctx().pop_layer();
                return;
            }
            Some(_) => {}
            None => {
                self.root.pop_layer();
                return;
            }
        }
        let mut frame = self.fx.pop().expect("an effect frame, checked above");
        self.base = frame.saved_base;

        // Rasterize the layer, run the passes over it, and paint the result back.
        frame.ctx.flush();
        let mut pixmap = Pixmap::new(frame.size.0, frame.size.1);
        frame
            .ctx
            .render_with(&mut pixmap, &mut *self.resources, raster_settings());
        {
            // `Pixmap` is already premultiplied sRGB, which is exactly what the
            // passes want — so this is a borrow rather than a conversion, and the
            // two backends cannot disagree by rounding differently on the way in.
            let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
            let mut surface = crate::effects::Surface::new(pixmap.data_as_u8_slice_mut(), w, h);
            crate::effects::run(&mut surface, &frame.effects, frame.scale);
        }

        let ctx = self.ctx();
        // Buffer → device: the origin, and the resolution the buffer was
        // rasterized at. The scale is `1.0` unless the layer earned a downscale.
        ctx.set_transform(
            Affine::translate((frame.origin.0 as f64, frame.origin.1 as f64))
                * Affine::scale(frame.downscale),
        );
        ctx.reset_paint_transform();
        ctx.set_paint(CpuImage {
            image: ImageSource::Pixmap(std::sync::Arc::new(pixmap)),
            sampler: Default::default(),
        });
        ctx.fill_rect(&Rect::new(
            0.0,
            0.0,
            frame.size.0 as f64,
            frame.size.1 as f64,
        ));
    }
}

#[cfg(test)]
mod alpha_mask_tests {
    use super::*;
    use kurbo::{Rect, Shape};
    use ondin_core::peniko::{BlendMode, Compose, Mix};

    /// **The mechanism an alpha mask would be built on, asserted before anything
    /// is built on it.**
    ///
    /// `Compose::DestIn` composites the *mask* over an isolated layer holding the
    /// content and keeps the destination where the source has alpha — which is an
    /// alpha mask, per pixel and soft-edged, rather than the hard clip
    /// `ScenePainter::push_layer`'s `clip` argument gives. Both numbers matter:
    /// 128 says the content survives *at the mask's own alpha* instead of being
    /// kept or dropped whole, and the red says the mask's **colour is not read** —
    /// it was drawn black, and black is what a luminance mask would have erased.
    ///
    /// The export path pins the SIMD level; it does not take what the CPU has
    /// (§15 D304). The failure this guards is a one-word revert — `new_with` back
    /// to `new` — which compiles, passes every pixel assertion in the suite on the
    /// machine that made it, and silently unpins every future golden.
    ///
    /// `Level` is `#[non_exhaustive]` and implements no `PartialEq`, so the
    /// variant is read through `Debug`. That is a spelling this test depends on,
    /// and it is safe to depend on: `vello_cpu` is pinned at `=0.2.0` and drags
    /// exactly one `fearless_simd` (0.4.1, re-counted in `Cargo.lock` at the bump —
    /// ⚠️ **`vello_common` is *two*, and `vello_cpu` itself is two**, because
    /// `epaint` 0.35 depends on 0.0.9; only `fearless_simd` unified, which is the
    /// one this sentence is about).
    ///
    /// ⚠️ **Vacuous on a machine whose detected level already is the baseline** —
    /// the two strings agree and nothing is proved. It bites on AVX2, which is
    /// where the difference was measured; the note below says which case ran, so
    /// a green result is not mistaken for a proof.
    #[test]
    fn export_pins_the_simd_level_rather_than_detecting_it() {
        let pinned = format!("{:?}", export_settings().level);
        let baseline = format!("{:?}", Level::baseline());
        let detected = format!("{:?}", Level::try_detect().expect("std is on"));

        assert_eq!(
            pinned, baseline,
            "the export path must pin the level, not take whatever this CPU has"
        );
        if detected == baseline {
            eprintln!(
                "note: this CPU detects {detected}, which IS the baseline — \
                 the test cannot distinguish a pin from a detect here"
            );
        } else {
            eprintln!("note: detected {detected} vs pinned {baseline} — the test bites");
        }
    }

    /// Kept as a test rather than deleted with the probe that produced it because
    /// it is the load-bearing assumption of the feature and it is about a
    /// dependency's behaviour, which no amount of reading our own code confirms.
    #[test]
    fn dest_in_composites_a_soft_alpha_mask_and_ignores_its_colour() {
        let mut ctx = RenderContext::new(40, 40);
        ctx.set_transform(Affine::IDENTITY);
        // Isolation layer, then the content: a red square over the whole canvas.
        ctx.push_layer(None, None, None, None, None);
        ctx.set_paint(BackendBrush::Solid(Color::from_rgba8(220, 30, 40, 255)));
        ctx.fill_path(&Rect::new(0.0, 0.0, 40.0, 40.0).to_path(0.1));
        // The mask: a half-transparent square over the left half only.
        ctx.push_layer(
            None,
            Some(BlendMode::new(Mix::Normal, Compose::DestIn)),
            None,
            None,
            None,
        );
        ctx.set_paint(BackendBrush::Solid(Color::from_rgba8(0, 0, 0, 128)));
        ctx.fill_path(&Rect::new(0.0, 0.0, 20.0, 40.0).to_path(0.1));
        ctx.pop_layer();
        ctx.pop_layer();
        ctx.flush();

        let mut pixmap = Pixmap::new(40, 40);
        let mut resources = Resources::new();
        ctx.render_with(&mut pixmap, &mut resources, raster_settings());
        let data = color::straight_rgba8_from_premul(pixmap.data_as_u8_slice());
        let px = |x: usize, y: usize| {
            let i = (y * 40 + x) * 4;
            [data[i], data[i + 1], data[i + 2], data[i + 3]]
        };
        assert_eq!(
            px(10, 20),
            [219, 30, 40, 128],
            "under a half-alpha mask the content survives at the mask's alpha, \
             and keeps its own colour — the mask was drawn black"
        );
        assert_eq!(
            px(30, 20),
            [0, 0, 0, 0],
            "and outside the mask nothing is left"
        );
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use ondin_core::text;

    /// The strip a picker row draws, at `ppp`: `(rgba, w, h)`.
    fn strip(ppp: f64) -> (Vec<u8>, u32, u32) {
        let preview = text::family_preview("Inter", "Inter", 15.0).expect("Inter is bundled");
        let w = (preview.width * ppp).ceil() as u32 + 2;
        let h = (preview.height * ppp).ceil() as u32 + 2;
        let transform = Affine::translate((1.0, 1.0)) * Affine::scale(ppp);
        (
            rasterize_glyph_run(&preview.run, transform, w, h)
                .expect("Inter's own strip is a size this backend can hold"),
            w,
            h,
        )
    }

    /// **A size this backend cannot hold is refused, not truncated** (§15 D586,
    /// `[S21-L1-05]`).
    ///
    /// `RenderContext` and `Pixmap` take `u16` dimensions and this reached them
    /// through a bare `as u16`, which *wraps*. The caller pairs the buffer with
    /// the untruncated numbers, so `70000 as u16 = 4464` produced 142,848 bytes
    /// against a `ColorImage` expecting 2,240,000 — an assertion inside epaint, on
    /// the UI thread, reachable from a font's own metrics.
    ///
    /// ⚠️ **The measured pair is the assertion**: the buffer a *legal* size
    /// returns is checked against `w × h × 4` in the same test, because "returns
    /// `None` for a big number" is satisfied by a function that returns `None` for
    /// everything. `MAX_RASTER_SIDE × 1` is deliberately included — the bound is
    /// inclusive, and an off-by-one here would be `[S8.2-L1-01]` again.
    ///
    /// 🚨 **The bound is vello's and not `u16::MAX`, and this test is what said
    /// so.** Written first with `u16::try_from`, the control at `65535 × 1`
    /// panicked inside `vello_common/src/util.rs:174` on an `Option::unwrap` —
    /// D434's finding arriving at a second site, exactly as `[S21-L1-05]`
    /// predicted it would. The refusal is against
    /// `MAX_RASTER_SIDE` now (plain backticks, §15 D319: this is a `#[cfg(test)]`
    /// module and `cargo doc` builds without the `test` cfg, so a link here is
    /// checked by nothing), and rendering *at* the cap here is one of the two
    /// things in the workspace that re-measures that constant against the
    /// dependency.
    ///
    /// **Flip run**, `u16::try_from` put back to `as u16`: fails on *"a size past
    /// the backend's u16 must be refused"*, having returned a 142,848-byte buffer
    /// for a 70,000-pixel request.
    #[test]
    fn a_strip_too_big_for_the_backend_is_refused_rather_than_truncated() {
        let preview = text::family_preview("Inter", "Inter", 15.0).expect("Inter is bundled");
        let go = |w, h| rasterize_glyph_run(&preview.run, Affine::IDENTITY, w, h);

        assert!(
            go(8, 70_000).is_none(),
            "a size past the backend's u16 must be refused"
        );
        assert!(go(70_000, 8).is_none(), "on the other axis too");
        assert!(go(8, 0).is_none(), "and an empty buffer is not a strip");

        // The control: the largest legal size still rasterizes, and the buffer it
        // answers with is the one the caller's dimensions describe.
        let big = go(MAX_RASTER_SIDE, 1).expect("the cap is inside the bound, not past it");
        assert_eq!(big.len(), MAX_RASTER_SIDE as usize * 4);
        let ordinary = go(40, 20).expect("an ordinary strip");
        assert_eq!(ordinary.len(), 40 * 20 * 4);
    }

    /// Coverage, and coverage that can be tinted: the row's colour comes from the
    /// theme and its state (hovered, selected, disabled), so the cached strip has
    /// to be white ink whose *alpha* is the drawing. A strip that baked the colour
    /// in would need rasterizing again per state.
    #[test]
    fn a_preview_strip_is_white_ink_with_the_drawing_in_its_alpha() {
        let (rgba, w, h) = strip(1.0);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let inked: Vec<&[u8]> = rgba.chunks(4).filter(|px| px[3] > 0).collect();
        assert!(
            inked.len() > 20,
            "expected the name to leave ink, got {} covered pixels in {w}x{h}",
            inked.len()
        );
        assert!(
            inked
                .iter()
                .all(|px| px[0] == 255 && px[1] == 255 && px[2] == 255),
            "ink must be white so the row can tint it"
        );
    }

    /// **Where the baseline actually lands in the strip**, which is the number the
    /// picker positions every row by: it places the strip so that
    /// `strip.baseline` sits on one shared baseline down the list, so if that
    /// value and the ink disagree the names sit visibly off their row.
    ///
    /// "Inter" has no descender, so every pixel of it must be above the baseline
    /// — and inside the buffer, which is what the strip's one-pixel margin is
    /// there for. (Inter's own side bearings mean the margin is not *load-bearing*
    /// for this string; it is insurance for faces whose ink leans out past the
    /// advance width they were measured by, and this test does not exercise that.)
    #[test]
    fn the_ink_sits_on_the_baseline_the_strip_reports() {
        let ppp = 2.0;
        let preview = text::family_preview("Inter", "Inter", 15.0).expect("Inter is bundled");
        let (rgba, w, h) = strip(ppp);
        let px = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
        let inked_rows: Vec<u32> = (0..h).filter(|y| (0..w).any(|x| px(x, *y) > 0)).collect();
        let (&top, &bottom) = (
            inked_rows.first().expect("some ink"),
            inked_rows.last().unwrap(),
        );

        // The transform `strip` applied, and the baseline the app derives from it.
        let baseline = preview.baseline * ppp + 1.0;
        assert!(
            f64::from(bottom) <= baseline,
            "'Inter' has no descender, so its lowest ink (row {bottom}) must not \
             fall below the baseline at {baseline}"
        );
        assert!(
            f64::from(bottom) > baseline - 2.0,
            "and it must reach it: row {bottom} against a baseline at {baseline}"
        );
        assert!(
            top >= 1,
            "the top of the ink is inside the buffer, not clipped"
        );
        assert!(
            (0..h).all(|y| px(w - 1, y) == 0),
            "and the right edge has room too"
        );
    }

    /// Rasterized in device pixels, not points: at 150% display scaling the strip
    /// is bigger and carries more coverage, rather than the same bitmap stretched.
    #[test]
    fn a_higher_display_scale_rasterizes_a_bigger_strip() {
        let (low, lw, lh) = strip(1.0);
        let (high, hw, hh) = strip(2.0);
        assert!(hw > lw && hh > lh, "{hw}x{hh} should exceed {lw}x{lh}");
        let ink = |rgba: &[u8]| rgba.chunks(4).filter(|px| px[3] > 0).count();
        assert!(
            ink(&high) > ink(&low),
            "2x should carry more coverage: {} vs {}",
            ink(&high),
            ink(&low)
        );
    }
}
