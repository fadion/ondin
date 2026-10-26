//! CPU backend: `vello_cpu` — deterministic output for export, CI, and headless
//! (`ondin serve`) (§6.3). No GPU. Drives all PNG export, so a raster is
//! reproducible without a device.
//!
//! Byte-comparable across machines of one architecture only because the SIMD
//! level is pinned (see [`export_settings`]): a stroked path rasterizes
//! differently under AVX2 than under SSE4.2 or the scalar fallback. Every other
//! primitive is level-invariant. The pin is on the export path alone;
//! [`rasterize_glyph_run`] keeps the detected level.
//!
//! aarch64/NEON is untested, so the pin is `Level::baseline()` (a per-architecture
//! answer) and a golden carried between architectures is not to be relied on.
//! A golden does not survive a `vello_cpu` bump either: a stroked path is also
//! the only version-variant primitive, so a checked-in PNG is a golden for one
//! architecture and one `vello_cpu` (§15 D414).

use crate::color;
use crate::images::ImageStore;
use crate::renderer::{RenderOverrides, Viewport};
use crate::scene::{self, ClipRule, ScenePainter, StrokePaint, TextRun};
use kurbo::{Affine, BezPath, Rect, Stroke};
use ondin_core::peniko::{
    BlendMode, Brush as BackendBrush, Color, Compose, Fill as VelloFill, ImageSampler, Mix,
};
use ondin_core::{Brush, Document, Effect, ImageId, Resolved};
use rustc_hash::FxHashMap;
use vello_cpu::{
    Glyph, Image as CpuImage, ImageSource, Level, Pixmap, RasterizerSettings, RenderContext,
    RenderSettings, Resources,
};

/// The rasterizer settings every export uses, with the SIMD level pinned to
/// [`Level::baseline`] rather than detected (§15 D304).
///
/// `RenderSettings::default()` detects the level, and a stroked path rasterizes
/// differently per level, which would make a checked-in PNG golden for one machine.
/// `baseline()` rather than the scalar fallback because `Level::fallback()` needs a
/// `fearless_simd` feature neither `vello_cpu` nor `vello_common` re-exports, and
/// may still be auto-vectorised; `baseline()` is target-derived, so goldens are
/// per-architecture. `num_threads` is never read without `multithreading` and is
/// left to `Default`.
///
/// Export only: [`rasterize_glyph_run`] keeps the detected level, as its strips are
/// never compared.
fn export_settings() -> RenderSettings {
    RenderSettings {
        level: Level::baseline(),
        ..Default::default()
    }
}

/// Every rasterizer setting spelled out, so a golden cannot move on an upstream
/// default (§15 D414). Used by every path, not just export.
///
/// The values are chosen, not inherited: `OptimizeSpeed` because the speed/quality
/// `match` is behind `cfg(all(u8_pipeline, f32_pipeline))` and only `u8_pipeline`
/// is on, so the field is inert; `Replace` because every target is a fresh
/// [`Pixmap`]; `Rgba8` because [`color::straight_rgba8_from_premul`] reads the
/// buffer as premultiplied RGBA8 (load-bearing); a zero `offset` because the
/// context and the pixmap are always the same size.
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

    /// Render `page` in `vp` to a straight-alpha RGBA8 buffer, returning
    /// `(rgba, w, h)`. No GPU; used by `ondin-export` PNG. The base transform maps
    /// the world-space `vp.view` onto the pixel grid.
    pub fn render_to_rgba(
        &mut self,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
        page: ondin_core::NodeId,
    ) -> (Vec<u8>, u32, u32) {
        self.render_subtrees_to_rgba(doc, res, vp, images, &[page])
    }

    /// [`Self::render_to_rgba`] restricted to these subtrees: the raster half of
    /// `scene::build_of`, and what an export of a selection needs. The whole-page
    /// call is one call of this, so there is one spelling of the walk.
    pub fn render_subtrees_to_rgba(
        &mut self,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
        origins: &[ondin_core::NodeId],
    ) -> (Vec<u8>, u32, u32) {
        self.render_subtrees_with(doc, res, vp, images, origins, &RenderOverrides::default())
    }

    /// [`Self::render_subtrees_to_rgba`] drawn through `overrides`, as the canvas
    /// draws a preview — the contrast check's backdrop is a page with one layer
    /// hidden.
    pub fn render_subtrees_with(
        &mut self,
        doc: &Document,
        res: &Resolved,
        vp: &Viewport,
        images: &ImageStore,
        origins: &[ondin_core::NodeId],
        overrides: &RenderOverrides,
    ) -> (Vec<u8>, u32, u32) {
        let (w, h) = vp.pixel_size;
        let mut ctx = RenderContext::new_with(w as u16, h as u16, export_settings());

        // From `scene`, not rebuilt: the walk snaps boxes onto the grid this
        // transform defines, so a drifted copy would round to a grid nothing uses.
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
            scene::build_of(doc, res, vp, overrides, origins, &mut painter);
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
/// 65408 is `511 × 128`, both factors vello's (§15 D434, D586): the depth buffer
/// holds one bucket per `DEPTH_BUCKET_WIDTH = 128` pixels and `bucket_span`
/// computes `index as u16 * DEPTH_BUCKET_WIDTH`, so 511 is the highest bucket
/// index that cannot overflow a `u16`. It is also a multiple of 4, which
/// `snap_to_tile_coordinates` needs.
///
/// Not `u16::MAX`, the one value the backend cannot take. Neither of vello's
/// constants is public, so an upgrade can silently invalidate this; the stand-in
/// gate is a test that renders at the cap on both axes (`ondin-export`'s
/// `a_raster_at_the_cap_renders_on_either_axis`, and
/// `a_strip_too_big_for_the_backend_is_refused_rather_than_truncated` below).
/// `png::MAX_RASTER_SIDE` is an alias of this.
pub const MAX_RASTER_SIDE: u32 = 511 * 128;

/// Rasterize one glyph run into a straight-alpha RGBA8 buffer `w`×`h` pixels,
/// **white ink on transparent** — a coverage strip, not a finished drawing.
///
/// This is the font picker's preview row (§5.4a). White rather than themed
/// because the colour is the caller's (hover, disabled, theme), so one cached
/// image is tinted per draw.
///
/// `transform` maps the run's own coordinates (x along the run, y on the
/// baseline) into this buffer's pixels, so a caller working in points passes the
/// display scale in here.
///
/// `None` for a size this backend cannot hold, rather than a truncated buffer:
/// `RenderContext` and `Pixmap` take `u16` dimensions and a bare `as u16` wraps,
/// leaving a buffer that does not match the caller's `ColorImage` (§15 D586). The
/// font picker's fallback is to draw the name in the UI font.
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
    /// Offscreen contexts rather than `vello_cpu`'s own filter layers: those cover
    /// only two of the four effects and the GPU backend has none, so one mechanism
    /// on both backends wins (see `crate::effects`).
    fx: Vec<FxFrame>,
    /// world → pixel transform applied on top of each node's world transform.
    ///
    /// Current, not the page's: an open effect layer's buffer has its own origin,
    /// so pushing one replaces this and popping restores the saved value.
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
    effects: Vec<ondin_core::Keyed<Effect>>,
    /// The enclosing target's `base`, put back when this layer pops.
    saved_base: Affine,
    /// Buffer pixels per document unit along each axis: what turns an authored
    /// blur radius into a kernel and an offset into pixels. Device pixels divided
    /// by [`Self::downscale`].
    scale: (f64, f64),
    /// How many device pixels one buffer pixel covers (`effect::buffer_downscale`);
    /// `1.0` unless the layer is heavily blurred.
    downscale: f64,
    /// How many ordinary layers are open inside this one; what tells `pop_layer`
    /// which kind of layer it is closing (see [`CpuPainter::opened_ordinary`]).
    depth: usize,
}

impl CpuPainter<'_> {
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
    /// Without this the pops mis-pair, and `vello_cpu` panics at `flush`. The walk
    /// pushes an effect layer outside the clip and the opacity (§6.4), so a shadow
    /// with `opacity < 1` or `clip` opens an ordinary layer inside the effect one;
    /// closing the effect frame on that inner pop would leave the ordinary layer
    /// open forever (§15 D338).
    fn opened_ordinary(&mut self) {
        if let Some(f) = self.fx.last_mut() {
            f.depth += 1;
        }
    }

    /// Sets the paint (solid, gradient or image) and its transform; returns the
    /// opacity the draw owes as a layer ([`Self::painted`]), or `None` when the
    /// brush resolved to nothing (an image the store has no pixels for) and the
    /// caller skips the draw.
    ///
    /// An image's alpha comes back rather than going on the sampler: `vello_cpu`
    /// panics on an image paint whose sampler alpha is not 1.
    ///
    /// The paint transform is context state here, unlike vello's, so it is set on
    /// every call: left standing, an image's framing would transform the next
    /// shape's gradient. It composes as `transform * paint_transform`, as in vello.
    fn apply_paint(&mut self, brush: &Brush, framing: Option<ondin_core::Framing>) -> Option<f32> {
        // The image arm is taken from the model's brush, not the backend's: the
        // reference carries the id and adjustment the conversion memo is keyed on,
        // which the backend brush has dropped.
        if let Brush::Image(img) = brush {
            let (source, to_source) = self.image_source(&img.image)?;
            self.ctx().set_paint_transform(
                color::framed_transform(framing, to_source).unwrap_or(Affine::IDENTITY),
            );
            self.ctx().set_paint(CpuImage {
                image: source,
                sampler: ImageSampler {
                    alpha: 1.0,
                    ..color::framed_sampler(img.sampler, framing)
                },
            });
            return Some(img.sampler.alpha);
        }
        // The transform is set from the answer rather than cleared: a gradient
        // carries its own (§15 D412) and a solid answers the identity.
        // `Source`, where the GPU backend asks for `Atlas` (§15 D745): every PNG
        // export must write the picture the document holds.
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
            // Unreachable (the image arm returned above); spelled out so that
            // changing either half stops compiling here.
            Some((BackendBrush::Image(_), _)) | None => return None,
        }
        Some(1.0)
    }

    /// Run `draw` at `alpha`: directly when opaque, else inside an opacity layer,
    /// which is how an image paint's alpha reaches the raster here.
    fn painted(&mut self, alpha: f32, draw: impl FnOnce(&mut Self)) {
        if alpha >= 1.0 {
            draw(self);
            return;
        }
        // Pushed and popped on the context directly, inside one draw: the walk's
        // own layer bookkeeping (`opened_ordinary`) never sees it.
        self.ctx().push_layer(None, None, Some(alpha), None, None);
        draw(self);
        self.ctx().pop_layer();
    }

    /// The premultiplied pixmap `r` paints from, built at most once per render,
    /// with the map from its pixel grid onto the source's.
    ///
    /// The memo is per render, not per store: the conversion allocates a buffer
    /// the size of the image, and caching it on the store would make the GPU canvas
    /// carry premultiplied buffers it never paints with.
    ///
    /// Keyed by the picture and its adjustment (as bits, per `images::AdjustKey`),
    /// since two fills of one photograph at two exposures are two buffers.
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
        let Some(alpha) = self.apply_paint(brush, framing) else {
            return;
        };
        // Set and restored around the one call: this context keeps the rule until
        // something changes it, and a fill left in even-odd would re-read every
        // later shape on this backend and not on the GPU one.
        self.ctx().set_fill_rule(match rule {
            ClipRule::NonZero => VelloFill::NonZero,
            ClipRule::EvenOdd => VelloFill::EvenOdd,
        });
        self.painted(alpha, |p| p.ctx().fill_path(path));
        self.ctx().set_fill_rule(VelloFill::NonZero);
    }

    fn stroke_path(&mut self, transform: Affine, path: &BezPath, stroke: &StrokePaint<'_>) {
        let base = self.base;
        self.ctx().set_transform(base * transform);
        let Some(alpha) = self.apply_paint(stroke.brush, stroke.framing) else {
            return;
        };
        let mut k = Stroke::new(stroke.width)
            .with_join(stroke.join)
            .with_caps(stroke.cap)
            .with_miter_limit(stroke.miter_limit);
        if !stroke.dashes.is_empty() {
            k = k.with_dashes(stroke.dash_offset, stroke.dashes.iter().copied());
        }
        self.ctx().set_stroke(k);
        self.painted(alpha, |p| p.ctx().stroke_path(path));
    }

    fn draw_text(&mut self, transform: Affine, run: &TextRun<'_>) {
        let base = self.base;
        self.ctx().set_transform(base * transform);
        // The `None` return must be honoured: paint is context state, so ignoring
        // it would draw the text in whatever the last `set_paint` left (the previous
        // node's colour) rather than nothing (§15 D437, D179).
        let Some(alpha) = self.apply_paint(run.brush, run.framing) else {
            return;
        };
        self.painted(alpha, |p| {
            // Split: the glyph run needs the context and the resources at once.
            let (ctx, resources) = p.split();
            ctx.glyph_run(resources, run.font)
                .font_size(run.font_size)
                .normalized_coords(run.coords)
                .fill_glyphs(run.glyphs.iter().map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                }));
        });
    }

    fn push_layer(
        &mut self,
        transform: Affine,
        clip: Option<&BezPath>,
        rule: ClipRule,
        opacity: f32,
    ) {
        self.opened_ordinary();
        // The clip is in the node's local space, so the transform goes first.
        // `vello_cpu` takes the clip's fill rule from the context; restore it
        // after, or it leaks into the next `fill_path`.
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
        // No transform, clip or opacity: the mask's own drawing carries its
        // transform, and this layer only sets how it composites.
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
    /// The buffer is clamped to the enclosing target, so an effect on a mostly
    /// off-screen layer costs only the visible part; that is also the only bound
    /// on the allocation.
    ///
    /// An empty intersection still pushes a layer (a 1×1 buffer), because the walk
    /// owes [`Self::pop_layer`] a pop either way.
    fn push_effect_layer(
        &mut self,
        transform: Affine,
        bounds: Rect,
        effects: &[ondin_core::Keyed<Effect>],
    ) {
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
        // The target grown by the escape: the silhouette is read from this buffer,
        // so clipping at the target's edge would cast a shadow from the visible
        // sliver of a half-scrolled layer (§15 D342).
        let m0 = (self.base * transform).as_coeffs();
        let s0 = (
            (m0[0] * m0[0] + m0[1] * m0[1]).sqrt(),
            (m0[2] * m0[2] + m0[3] * m0[3]).sqrt(),
        );
        // The same budget the GPU backend holds a buffer to
        // (`effect::buffer_box_within`, §15 D402), so the two backends cannot
        // choose different buffer boxes for one drawing.
        let clipped = ondin_core::effect::buffer_box_within(
            device,
            limit,
            ondin_core::effect::stack_escape(effects),
            s0,
            self.root.width() as f64 * self.root.height() as f64,
        );
        // Rounded outward, so a fractional edge keeps the pixel it touches.
        let (x0, y0) = (clipped.x0.floor(), clipped.y0.floor());

        // The scale the passes work in: from the composed device transform, so a
        // layer in a skewed ancestor blurs by how far its pixels are stretched.
        let m = (self.base * transform).as_coeffs();
        let scale = (
            (m[0] * m[0] + m[1] * m[1]).sqrt(),
            (m[2] * m[2] + m[3] * m[3]).sqrt(),
        );

        // The buffer is not always at device resolution (`effect::buffer_downscale`),
        // and `GpuScenePainter::push_effect_layer` must choose identically or the
        // backends blur by different amounts (§15 D395). Both go through
        // `crate::effects::effect_buffer` rather than hand-kept copies; a layer past
        // `MAX_EFFECT_BUFFER_SIDE` is resampled into the cap, not truncated (§15 D744).
        let k = ondin_core::effect::buffer_downscale(effects, scale);
        let ((w, h), k) =
            crate::effects::effect_buffer((clipped.x1.ceil() - x0, clipped.y1.ceil() - y0), k);
        // Both sides are inside `MAX_EFFECT_BUFFER_SIDE`, well inside a `u16`.
        let (w, h) = (w as u16, h as u16);

        let ctx = RenderContext::new_with(w, h, export_settings());
        let saved_base = self.base;
        // Drawing inside is shifted to the buffer's top-left and scaled to its resolution.
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
        // An ordinary layer opened inside this effect layer closes first.
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
            // `Pixmap` is already premultiplied sRGB, which the passes want: a
            // borrow, not a conversion, so the backends cannot round differently.
            let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
            let mut surface = crate::effects::Surface::new(pixmap.data_as_u8_slice_mut(), w, h);
            crate::effects::run(&mut surface, &frame.effects, frame.scale);
        }

        let ctx = self.ctx();
        // Buffer → device: the origin and the buffer's resolution.
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

    /// The export path pins the SIMD level rather than taking the CPU's (§15 D304);
    /// guards a one-word revert (`new_with` back to `new`) that would silently
    /// unpin every golden.
    ///
    /// `Level` is `#[non_exhaustive]` with no `PartialEq`, so the variant is read
    /// through `Debug`; `vello_cpu` is pinned at `=0.2.0`, so that spelling is stable.
    ///
    /// Vacuous on a machine whose detected level already is the baseline; the
    /// printed note says which case ran.
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

    /// `Compose::DestIn` makes a per-pixel soft alpha mask: the content survives at
    /// the mask's alpha (128) and the mask's colour is not read (drawn black here,
    /// which a luminance mask would have erased). Pins a dependency's behaviour.
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

    /// A size this backend cannot hold is refused, not truncated (§15 D586): a
    /// wrapped `as u16` once returned a buffer that did not match the caller's
    /// `ColorImage`. The legal sizes are checked against `w × h × 4` in the same
    /// test, since `None` for everything would satisfy the refusals;
    /// `MAX_RASTER_SIDE × 1` is included because the bound is inclusive, and
    /// rendering at the cap re-measures that constant against the dependency.
    ///
    /// flip: `u16::try_from` back to `as u16` → fails at "a size past the
    /// backend's u16 must be refused".
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

    /// The strip is white ink whose alpha is the drawing, so a row can tint it per
    /// state without rasterizing again.
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

    /// The ink sits on the baseline the strip reports, which the picker aligns
    /// every row by. "Inter" has no descender, so all ink is above the baseline and
    /// inside the buffer. (The one-pixel margin is insurance for faces whose ink
    /// leans past the advance width; this test does not exercise that.)
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

    /// Rasterized in device pixels: a higher display scale gives a bigger strip with
    /// more coverage, not a stretched bitmap.
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
