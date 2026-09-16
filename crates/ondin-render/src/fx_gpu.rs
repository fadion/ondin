//! The GPU effect passes (§6.4) — `crate::effects` as compute shaders.
//!
//! **The CPU module is the specification and this is the port, not the other way
//! round.** Every dispatch here has a named counterpart in `effects.rs`, in the
//! same order, over the same premultiplied 8-bit values; where the two could
//! differ they are made to agree deliberately (see the quantisation note below)
//! rather than left to whichever rounding the driver happens to do. What keeps
//! that honest is `tests/fx_gpu.rs`, which runs both over the same input and
//! compares the bytes.
//!
//! **Straight alpha at the two ends, premultiplied in the middle.** This is the
//! one place the two backends genuinely differ at the seam and it is worth
//! stating rather than discovering: `vello_cpu`'s `Pixmap` is *premultiplied*, so
//! the CPU backend hands its buffer to `effects::run` untouched — while
//! **vello's GPU output is straight**, measured on-device rather than assumed
//! (a 50%-alpha white fill reads back `255,255,255,128`, not `128,…`). Its image
//! atlas takes straight alpha too (`Renderer::register_texture`: *"the texture is
//! assumed to have unpremultiplied alpha"*, and vello 0.9 carries a `TODO` for
//! the premultiplied variant it does not yet support). So the stack is bracketed
//! by `premul`/`unpremul`, and everything between them matches the reference.
//!
//! The round trip through straight 8-bit costs at most one level per channel on
//! a partly transparent pixel, which is why the on-device comparison allows one
//! and asserts on the *shape* of the difference rather than only its size.

use crate::effects;
use ondin_core::effect::{Effect, EffectKind};
use wgpu::{Device, Queue, Texture, TextureView};

/// The size a workgroup covers, matching `@workgroup_size(8, 8)` in `fx.wgsl`.
const TILE: u32 = 8;

/// Bytes of the `Params` uniform — see the struct at the top of `fx.wgsl`.
///
/// Written out by hand rather than through `bytemuck`, which this crate does not
/// depend on: the layout is a contract with a shader in another language, and
/// spelling the offsets here is the only place the two can be read against each
/// other.
const PARAMS_BYTES: usize = 144;

/// Which pass a dispatch runs. One per entry point in `fx.wgsl`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pass {
    Premul,
    Unpremul,
    Copy,
    Matrix,
    Blur,
    Silhouette,
    Spread,
    Tint,
    MaskIn,
    Over,
    Coarsen,
    Upsample,
}

impl Pass {
    fn entry(self) -> &'static str {
        match self {
            Self::Premul => "premul",
            Self::Unpremul => "unpremul",
            // `copy` is a WGSL reserved word.
            Self::Copy => "copy_pass",
            Self::Matrix => "matrix",
            Self::Blur => "blur",
            Self::Silhouette => "silhouette",
            Self::Spread => "spread",
            Self::Tint => "tint",
            Self::MaskIn => "mask_in",
            Self::Over => "over",
            Self::Coarsen => "coarsen",
            Self::Upsample => "upsample",
        }
    }

    const ALL: [Pass; 12] = [
        Self::Premul,
        Self::Unpremul,
        Self::Copy,
        Self::Matrix,
        Self::Blur,
        Self::Silhouette,
        Self::Spread,
        Self::Tint,
        Self::MaskIn,
        Self::Over,
        Self::Coarsen,
        Self::Upsample,
    ];
}

/// Everything a dispatch needs beyond its two source textures.
///
/// Defaulted and then overridden field by field at each call site, so a pass that
/// does not care about the offset cannot accidentally depend on a stale one.
#[derive(Clone, Copy, Default)]
struct Params {
    off: (i32, i32),
    /// Where the layer sits inside the texture the ingest reads from — see
    /// `src_origin` in `fx.wgsl`.
    src_origin: (i32, i32),
    radius: i32,
    horizontal: bool,
    inner: bool,
    grow: bool,
    tint: [f32; 4],
    matrix: [f32; 20],
    /// The domain being read where that is a different grid from the one being
    /// written — `(0, 0)` for every pass but `Coarsen` and `Upsample`.
    src_size: (u32, u32),
}

impl Params {
    fn bytes(&self, w: u32, h: u32) -> [u8; PARAMS_BYTES] {
        let mut b = [0u8; PARAMS_BYTES];
        let mut put = |at: usize, v: [u8; 4]| b[at..at + 4].copy_from_slice(&v);
        // offset 0: size: vec2<u32>
        put(0, w.to_le_bytes());
        put(4, h.to_le_bytes());
        // offset 8: off: vec2<i32>
        put(8, self.off.0.to_le_bytes());
        put(12, self.off.1.to_le_bytes());
        // offset 16: radius: i32, flags: u32
        put(16, self.radius.to_le_bytes());
        let flags =
            u32::from(self.horizontal) | (u32::from(self.inner) << 1) | (u32::from(self.grow) << 2);
        put(20, flags.to_le_bytes());
        // offset 24: src_origin: vec2<i32>, which also pads the vec4 below onto 32.
        put(24, self.src_origin.0.to_le_bytes());
        put(28, self.src_origin.1.to_le_bytes());
        // offset 32: tint: vec4<f32>
        for (i, v) in self.tint.iter().enumerate() {
            put(32 + i * 4, v.to_le_bytes());
        }
        // offset 48: m: array<vec4<f32>, 5>
        for (i, v) in self.matrix.iter().enumerate() {
            put(48 + i * 4, v.to_le_bytes());
        }
        // offset 128: src_size: vec2<u32>
        put(128, self.src_size.0.to_le_bytes());
        put(132, self.src_size.1.to_le_bytes());
        b
    }
}

/// The compute pipelines, built once per device.
///
/// Held by [`crate::VelloGpuRenderer`] rather than rebuilt per frame: shader
/// compilation is the expensive part and there is exactly one device.
pub struct FxPipelines {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<(Pass, wgpu::ComputePipeline)>,
}

impl FxPipelines {
    pub fn new(device: &Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ondin-fx"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fx.wgsl").into()),
        });
        // **One layout for every pass**, with the unused bindings filled in by the
        // caller rather than a layout per shape. Ten pipelines that differ only in
        // entry point are ten places a binding index could drift; one layout means
        // a wrong index is a validation error on the first dispatch instead of a
        // wrong picture on one pass.
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let sampled = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ondin-fx"),
            entries: &[
                entry(0, sampled),
                entry(1, sampled),
                entry(
                    2,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    4,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ondin-fx"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = Pass::ALL
            .iter()
            .map(|pass| {
                let p = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(pass.entry()),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(pass.entry()),
                    compilation_options: Default::default(),
                    cache: None,
                });
                (*pass, p)
            })
            .collect();
        Self { layout, pipelines }
    }

    fn pipeline(&self, pass: Pass) -> &wgpu::ComputePipeline {
        &self
            .pipelines
            .iter()
            .find(|(p, _)| *p == pass)
            .expect("every pass has a pipeline")
            .1
    }
}

/// A texture the passes can read and write, and which vello's atlas can copy.
///
/// `COPY_SRC` is not optional: `Renderer::register_texture` copies the texture
/// into the image atlas at the start of every frame that draws it.
pub fn fx_texture(device: &Device, w: u32, h: u32, label: &str) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            // For `copy_out`, which lifts one layer out of a packed batch.
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Where one effect layer sits inside the texture it is read from.
///
/// A pair rather than two arguments because it is one fact — sibling layers share
/// a packed texture and one vello pass (§15 D344), so "which rectangle is mine"
/// travels as a unit. A layer rendered on its own passes `at: (0, 0)` and the
/// texture's own size.
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    pub at: (u32, u32),
    pub size: (u32, u32),
}

/// Lift one layer's rectangle out of a packed batch into a texture of its own.
///
/// The one caller is the effect layer with nothing to filter — a stack whose
/// entries are all hidden or neutral, which the walk should not have opened a
/// layer for. It cannot be handed the packed texture, which holds every sibling,
/// so it gets a copy of its own slot.
pub fn copy_out(device: &Device, queue: &Queue, packed: &Texture, slice: Slice) -> Texture {
    let Slice { at, size } = slice;
    let out = fx_texture(device, size.0, size.1, "ondin-fx-slice");
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: packed,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: at.0,
                y: at.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &out,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    out
}

/// The scratch a run needs, allocated once and ping-ponged.
///
/// Five buffers rather than one per step: `a`/`b` carry the layer through the
/// appearance passes and then serve as the shadow's scratch, `graphic` holds the
/// layer as the shadows see it, and `o0`/`o1` accumulate the result. At a
/// viewport-sized layer that is five times 8 MB, which is why they are allocated
/// only for a layer that actually has ink to filter (`effects::any_ink`).
struct Scratch {
    views: Vec<TextureView>,
    textures: Vec<Texture>,
}

impl Scratch {
    fn new(device: &Device, w: u32, h: u32) -> Self {
        let textures: Vec<Texture> = ["fx-a", "fx-b", "fx-graphic", "fx-o0", "fx-o1"]
            .iter()
            .map(|l| fx_texture(device, w, h, l))
            .collect();
        let views = textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        Self { views, textures }
    }
}

/// Slots in [`Scratch`], named so the sequence below reads as the reference does.
const A: usize = 0;
const B: usize = 1;
const GRAPHIC: usize = 2;
const O0: usize = 3;
const O1: usize = 4;

/// Run an effect stack over `src` and return the result, in **straight** alpha
/// and ready for vello's atlas.
///
/// `src` is what vello rasterized the subtree into — straight alpha, the size of
/// the layer's clipped device box. `scale` is device pixels per document unit
/// along each axis, so an authored radius becomes a kernel and an authored offset
/// becomes pixels; it is the same pair the CPU backend takes.
///
/// Returns `None` when there is nothing to do, so the caller can draw `src`
/// directly rather than paying for a copy that changes nothing.
pub fn run(
    device: &Device,
    queue: &Queue,
    fx: &FxPipelines,
    src: &Texture,
    slice: Slice,
    effects: &[Effect],
    scale: (f64, f64),
) -> Option<Texture> {
    if !effects::any_ink(effects) {
        return None;
    }
    let Slice { at, size } = slice;
    let (w, h) = size;
    let scratch = Scratch::new(device, w, h);
    let src_view = src.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ondin-fx"),
    });
    // **Outlives the compute pass on purpose.** wgpu refcounts a resource a bind
    // group refers to, so dropping these handles early would probably be safe —
    // "probably" being the whole reason they are held until after the submit
    // instead.
    let mut keep: Vec<wgpu::Buffer> = Vec::new();
    // **One compute pass for the whole stack, and it yields the slot it ended in.**
    // In WebGPU a compute pass's usage scope is per *dispatch*, so a texture
    // written by one and read by the next is legal and ordered — which is what
    // lets a ping-pong sequence run without a pass boundary between every step.
    // The block's value is the answer to "which scratch slot holds the result",
    // which is also what ends the borrow of `scratch` so the texture can be moved
    // out of it below.
    let result = {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ondin-fx"),
            timestamp_writes: None,
        });
        let mut run = Runner {
            device,
            fx,
            cpass: &mut cpass,
            scratch: &scratch,
            w,
            h,
            keep: &mut keep,
        };

        // --- the layer's own appearance, in list order -----------------------
        run.dispatch(
            Pass::Premul,
            &src_view,
            &src_view,
            A,
            Params {
                src_origin: (at.0 as i32, at.1 as i32),
                ..Default::default()
            },
        );
        let mut cur = A;
        let mut alt = B;
        for e in effects.iter().filter(|e| e.visible) {
            match &e.kind {
                EffectKind::Filters(f) if !f.is_neutral() => {
                    let p = Params {
                        matrix: effects::filter_matrix(f),
                        ..Default::default()
                    };
                    run.slot(Pass::Matrix, cur, cur, alt, p);
                    std::mem::swap(&mut cur, &mut alt);
                }
                EffectKind::LayerBlur { radius } if *radius > 0.0 => {
                    let (sx, sy) = effects::device_sigma(*radius, scale.0, scale.1);
                    run.blur(&mut cur, &mut alt, sx, sy);
                }
                _ => {}
            }
        }

        // The layer as the shadows will see it, kept aside before anything is
        // composited over or under it — `effects::run`'s `graphic`.
        run.slot(Pass::Copy, cur, cur, GRAPHIC, Params::default());
        let mut out = O0;
        let mut out_alt = O1;
        run.slot(Pass::Copy, cur, cur, out, Params::default());

        // --- the shadows, cast from that ------------------------------------
        for e in effects.iter().filter(|e| e.visible) {
            let (sh, inner) = match &e.kind {
                EffectKind::DropShadow(sh) => (sh, false),
                EffectKind::InnerShadow(sh) => (sh, true),
                _ => continue,
            };
            if sh.color.components[3] <= 0.0 {
                continue;
            }
            // Rounded on the host, exactly as `effects::shadow_layer` rounds it,
            // so the two backends shift by the same whole number of pixels.
            let off = (
                (sh.offset.x * scale.0).round() as i32,
                (sh.offset.y * scale.1).round() as i32,
            );
            // **`inner` does not reach this pass** (§15 D742, `[S10.2-L1-01]`):
            // the silhouette is the graphic's alpha for both kinds of shadow and
            // the complement happens at `Pass::Tint`, so every pass between the
            // two reads outside the buffer as transparent and means the same
            // thing by it. `effects::shadow_layer` carries the argument.
            run.slot(
                Pass::Silhouette,
                GRAPHIC,
                GRAPHIC,
                A,
                Params {
                    off,
                    ..Default::default()
                },
            );
            let mut s = A;
            let mut s_alt = B;
            if sh.spread != 0.0 {
                // **One radius per axis**, which is `effects::spread_alpha`'s pair
                // and the same argument as `device_sigma`'s two deviations: this
                // was `scale.0.max(scale.1)` on both passes, so a spread on a
                // non-uniformly scaled layer came out scale-ratio times too thick
                // on the short axis and outside the box `EffectKind::escape` had
                // allocated for it (§15 D601, `[S10.2-L1-05]`). Both backends did
                // it, which is why `tests/fx_gpu.rs` — every fixture of which is
                // at `SCALE = (1.0, 1.0)` — agreed and stayed green.
                let r = (sh.spread * scale.0, sh.spread * scale.1);
                // **Bounded by the buffer, through the CPU backend's own helper**
                // (§15 D615, `[S10.2-L4-04]`). `n` is device pixels and the only
                // cap on the authored number is in document units, so the zoom
                // multiplied it without limit: 1 092.7 ms for one shadow at
                // `spread: 10_000` and zoom 256, measured on a real adapter. The
                // cap is lossless — this shader reads outside the buffer as empty
                // too, so a window that already covers the line cannot find
                // anything by growing.
                //
                // ⚠️ **`effects::spread_steps` and not a second copy of the
                // arithmetic**, which is the mistake §15 D601 records at this very
                // pair of sites: both backends computed the radius from
                // `scale.0.max(scale.1)`, both were wrong the same way, and every
                // fixture in `tests/fx_gpu.rs` is at `SCALE = (1.0, 1.0)` so the
                // comparison harness agreed with itself and stayed green.
                let n = effects::spread_steps(r, w as usize, h as usize);
                if n.0 > 0 || n.1 > 0 {
                    // `(r > 0) != inner`: a positive spread grows a drop shadow
                    // and thickens an inner one, which are opposite operations on
                    // the silhouette each is cast from.
                    let grow = (r.0 + r.1 > 0.0) != inner;
                    for horizontal in [true, false] {
                        let n = if horizontal { n.0 } else { n.1 };
                        if n == 0 {
                            continue;
                        }
                        run.slot(
                            Pass::Spread,
                            s,
                            s,
                            s_alt,
                            Params {
                                radius: n,
                                horizontal,
                                grow,
                                ..Default::default()
                            },
                        );
                        std::mem::swap(&mut s, &mut s_alt);
                    }
                }
            }
            if sh.blur > 0.0 {
                let (sx, sy) = effects::device_sigma(sh.blur, scale.0, scale.1);
                // The same choice `effects::shadow_layer` makes, from the same
                // function, so the two backends resample on the same grid or not
                // at all (§15 D403).
                let k = ondin_core::effect::shadow_downscale(sx.max(sy) as f64);
                if k > 1 {
                    run.blur_coarse(&mut s, &mut s_alt, sx / k as f32, sy / k as f32, k);
                } else {
                    run.blur(&mut s, &mut s_alt, sx, sy);
                }
            }
            let [r, g, b, ca] = sh.color.components;
            run.slot(
                Pass::Tint,
                s,
                s,
                s_alt,
                Params {
                    tint: [r, g, b, ca],
                    // The complement, for the reason on `Pass::Silhouette` above.
                    inner,
                    ..Default::default()
                },
            );
            std::mem::swap(&mut s, &mut s_alt);
            if inner {
                run.slot(Pass::MaskIn, s, GRAPHIC, s_alt, Params::default());
                std::mem::swap(&mut s, &mut s_alt);
            }
            // An inner shadow goes *over* the accumulated result; a drop shadow
            // goes behind it, which is the same call with the arguments the other
            // way round.
            let (top, bottom) = if inner { (s, out) } else { (out, s) };
            run.slot(Pass::Over, top, bottom, out_alt, Params::default());
            std::mem::swap(&mut out, &mut out_alt);
        }

        // Back to straight alpha for vello's atlas.
        run.slot(Pass::Unpremul, out, out, out_alt, Params::default());
        out_alt
    };
    queue.submit([encoder.finish()]);
    // The views borrow the textures, so they go first; then the result is moved
    // out and the rest of the scratch is dropped.
    let Scratch {
        views,
        mut textures,
    } = scratch;
    drop(views);
    // `swap_remove` reorders what is left, which does not matter — nothing reads
    // the scratch after this.
    Some(textures.swap_remove(result))
}

/// Issues dispatches against one bind group layout.
struct Runner<'a, 'b> {
    device: &'a Device,
    fx: &'a FxPipelines,
    cpass: &'a mut wgpu::ComputePass<'b>,
    scratch: &'a Scratch,
    w: u32,
    h: u32,
    /// The uniform and kernel buffers each dispatch built, held by the caller so
    /// they outlive the submit.
    keep: &'a mut Vec<wgpu::Buffer>,
}

impl Runner<'_, '_> {
    /// A dispatch whose sources are scratch slots.
    fn slot(&mut self, pass: Pass, a: usize, b: usize, dst: usize, p: Params) {
        let dom = (self.w, self.h);
        let (va, vb) = (&self.scratch.views[a], &self.scratch.views[b]);
        self.dispatch_into(pass, va, vb, dst, p, &[1.0], dom);
    }

    /// [`Self::slot`] over a **smaller grid than the buffer** — the coarse region
    /// a resampled shadow is blurred in, which lives in the scratch texture's
    /// top-left corner. Everything outside `dom` is left as it was, and nothing
    /// reads it.
    fn slot_in(&mut self, pass: Pass, a: usize, b: usize, dst: usize, p: Params, dom: (u32, u32)) {
        let (va, vb) = (&self.scratch.views[a], &self.scratch.views[b]);
        self.dispatch_into(pass, va, vb, dst, p, &[1.0], dom);
    }

    /// A dispatch reading a texture from outside the scratch.
    fn dispatch(&mut self, pass: Pass, a: &TextureView, b: &TextureView, dst: usize, p: Params) {
        let dom = (self.w, self.h);
        self.dispatch_into(pass, a, b, dst, p, &[1.0], dom);
    }

    /// Both axes of a separable gaussian over the whole buffer.
    fn blur(&mut self, cur: &mut usize, alt: &mut usize, sigma_x: f32, sigma_y: f32) {
        let dom = (self.w, self.h);
        self.blur_in(cur, alt, sigma_x, sigma_y, dom);
    }

    /// `effects::blur_coarse`: box-average the silhouette into `k`×`k` blocks,
    /// blur *that* at the coarse deviations, and read it back bilinearly.
    ///
    /// The three dispatches are the reference's three steps in the same order and
    /// over the same numbers; what is different is only that the coarse grid lives
    /// in the corner of a full-size scratch texture rather than in a buffer of its
    /// own, which keeps every slot here one size (§15 D403).
    fn blur_coarse(
        &mut self,
        cur: &mut usize,
        alt: &mut usize,
        sigma_x: f32,
        sigma_y: f32,
        k: u32,
    ) {
        let (fw, fh) = (self.w, self.h);
        let (cw, ch) = effects::coarse_size(fw as usize, fh as usize, k as usize);
        let coarse = (cw as u32, ch as u32);
        self.slot_in(
            Pass::Coarsen,
            *cur,
            *cur,
            *alt,
            // ⚠️ **`k as i32` is safe because `shadow_downscale` caps at
            // `MAX_SHADOW_BLOCK`** (§15 D454). It was not: above `i32::MAX`
            // (σ ≈ 5.2e10) this wrapped **negative**, `fx.wgsl`'s `coarsen` ran
            // neither of its loops, and the store was `acc / f32(radius * radius)`
            // — so the GPU drew **no shadow at all** where the CPU drew a wrong
            // one. `[S10.2-L1-03]`, and the one arm of it that is silent rather
            // than slow.
            Params {
                radius: k as i32,
                src_size: (fw, fh),
                ..Default::default()
            },
            coarse,
        );
        std::mem::swap(cur, alt);
        self.blur_in(cur, alt, sigma_x, sigma_y, coarse);
        self.slot_in(
            Pass::Upsample,
            *cur,
            *cur,
            *alt,
            Params {
                radius: k as i32,
                src_size: coarse,
                ..Default::default()
            },
            (fw, fh),
        );
        std::mem::swap(cur, alt);
    }

    /// Both axes of a separable gaussian over `dom`, leaving the result in `cur`.
    fn blur_in(
        &mut self,
        cur: &mut usize,
        alt: &mut usize,
        sigma_x: f32,
        sigma_y: f32,
        dom: (u32, u32),
    ) {
        for (sigma, horizontal) in [(sigma_x, true), (sigma_y, false)] {
            if sigma <= 0.0 {
                continue;
            }
            // The host builds the kernel, from the same function the CPU
            // convolves with — so the weights are identical numbers rather than
            // two implementations of the same formula.
            let k = effects::kernel(sigma);
            let radius = ((k.len() - 1) / 2) as i32;
            let (va, vb) = (&self.scratch.views[*cur], &self.scratch.views[*cur]);
            self.dispatch_into(
                Pass::Blur,
                va,
                vb,
                *alt,
                Params {
                    radius,
                    horizontal,
                    ..Default::default()
                },
                &k,
                dom,
            );
            std::mem::swap(cur, alt);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_into(
        &mut self,
        pass: Pass,
        a: &TextureView,
        b: &TextureView,
        dst: usize,
        p: Params,
        kernel: &[f32],
        dom: (u32, u32),
    ) {
        let params = self.buffer(
            "ondin-fx-params",
            wgpu::BufferUsages::UNIFORM,
            &p.bytes(dom.0, dom.1),
        );
        let mut kb = Vec::with_capacity(kernel.len() * 4);
        for v in kernel {
            kb.extend_from_slice(&v.to_le_bytes());
        }
        let kbuf = self.buffer("ondin-fx-kernel", wgpu::BufferUsages::STORAGE, &kb);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(pass.entry()),
            layout: &self.fx.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.scratch.views[dst]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: kbuf.as_entire_binding(),
                },
            ],
        });
        self.cpass.set_pipeline(self.fx.pipeline(pass));
        self.cpass.set_bind_group(0, &bind, &[]);
        self.cpass
            .dispatch_workgroups(dom.0.div_ceil(TILE), dom.1.div_ceil(TILE), 1);
        self.keep.push(params);
        self.keep.push(kbuf);
    }

    /// A small mapped-at-creation buffer holding `data`.
    ///
    /// Both callers hand over whole `f32`s, so the multiple-of-four a uniform and
    /// a storage binding both require holds by construction — asserted rather than
    /// rounded up, because a size that had to be padded would mean the layout and
    /// the shader had already disagreed.
    fn buffer(&self, label: &str, usage: wgpu::BufferUsages, data: &[u8]) -> wgpu::Buffer {
        assert!(
            !data.is_empty() && data.len().is_multiple_of(4),
            "{label}: {} bytes is not a whole number of f32s",
            data.len()
        );
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: data.len() as u64,
            usage,
            mapped_at_creation: true,
        });
        buf.slice(..).get_mapped_range_mut().copy_from_slice(data);
        buf.unmap();
        buf
    }
}
