//! The GPU effect passes (§6.4) — `crate::effects` as compute shaders.
//!
//! The CPU module is the specification and this is the port: every dispatch has a
//! named counterpart in `effects.rs`, in the same order, over the same
//! premultiplied 8-bit values. `tests/fx_gpu.rs` runs both over the same input
//! and compares the bytes.
//!
//! Straight alpha at the two ends, premultiplied in the middle: `vello_cpu`'s
//! `Pixmap` is premultiplied, so the CPU backend hands its buffer to
//! `effects::run` untouched, while vello's GPU output and image atlas are straight
//! alpha. The stack is bracketed by `premul`/`unpremul`. That round trip costs at
//! most one level per channel on a partly transparent pixel, which the on-device
//! comparison allows.

use crate::effects;
use ondin_core::effect::{Effect, EffectKind};
use wgpu::{Device, Queue, Texture, TextureView};

/// The size a workgroup covers, matching `@workgroup_size(8, 8)` in `fx.wgsl`.
const TILE: u32 = 8;

/// Bytes of the `Params` uniform — see the struct at the top of `fx.wgsl`.
///
/// Written out by hand (this crate has no `bytemuck`): the layout is a contract
/// with a shader in another language, and the offsets spelled in `Params::bytes`
/// are where the two are read against each other. 160 is the struct's size
/// rounded to WGSL's 16-byte boundary.
const PARAMS_BYTES: usize = 160;

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
/// Defaulted and overridden field by field at each call site, so a pass cannot
/// depend on a field it does not set.
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
    /// Where the layer's rectangle sits in the texture `Pass::Unpremul` writes —
    /// the batch's destination, which is packed like its source. `(0, 0)` for
    /// every other pass, which writes a scratch slot at its own origin.
    dst_origin: (i32, i32),
    /// Where this dispatch's kernel starts in the batch's one kernel buffer, in
    /// `f32`s. Read by `Pass::Blur` alone.
    k_off: u32,
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
        // offset 136: dst_origin: vec2<i32>
        put(136, self.dst_origin.0.to_le_bytes());
        put(140, self.dst_origin.1.to_le_bytes());
        // offset 144: k_off: u32, then 12 bytes of padding to the struct's
        // 16-byte alignment.
        put(144, self.k_off.to_le_bytes());
        b
    }
}

/// The compute pipelines, built once per device.
///
/// Held by [`crate::VelloGpuRenderer`]: shader compilation is the expensive part.
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
        // One layout for every pass, unused bindings filled in by the caller: a
        // wrong index is then a validation error on the first dispatch rather
        // than a wrong picture on one pass.
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
                // A dynamic offset, so one bind group serves every dispatch over
                // the same three textures, each naming its own `Params`.
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_BYTES as u64),
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
            // For `run_batch`'s copy of a layer with nothing to filter into its
            // rectangle of the batch's result.
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Where one effect layer sits inside the texture it is read from.
///
/// Sibling layers share a packed texture and one vello pass, so "which rectangle
/// is mine" travels as a unit. A layer rendered alone passes `at: (0, 0)` and the
/// texture's own size.
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    pub at: (u32, u32),
    pub size: (u32, u32),
}

/// The smallest side a pooled texture is given — a card's 70×44 buffer and its
/// neighbour's 72×46 are one class, not two.
const MIN_CLASS: u32 = 16;

/// How many frames an idle pooled texture survives without being taken again.
///
/// Also the window [`FxPool::end_frame`] reads its byte bound over: what the pool
/// keeps is at most the largest working set — the bytes of the distinct textures
/// one frame took — among the last this-many frames.
const KEEP_FRAMES: u64 = 8;

/// The side a pooled texture is allocated at for a request of `n` pixels: the
/// next of 16, 24, 32, 48, 64, 96 … — a power of two or one and a half times
/// one — capped at the device's `limit` but never below `n`.
///
/// Two steps per octave rather than one, because the rounding is held in memory
/// across frames: a power of two alone wastes up to 4× in area, this up to 2.25×.
/// Classes exist because a layer's size moves by a pixel or two as the view pans.
pub fn size_class(n: u32, limit: u32) -> u32 {
    let n = n.max(MIN_CLASS);
    let pow = n.next_power_of_two();
    let mid = pow / 4 * 3;
    let class = if mid >= n { mid } else { pow };
    class.min(limit).max(n)
}

/// A texture taken from an [`FxPool`] — the handle, its whole-texture view, and
/// the identity the pool caches bind groups under.
///
/// At least the size asked for, and usually larger ([`size_class`]). Every pass
/// bounds itself by `Params::size` and vello's fine stage stops at
/// `RenderParams::width`/`height`; the margin holds whatever the last user left,
/// so nothing may read it.
#[derive(Clone)]
pub struct Pooled {
    id: u64,
    pub texture: Texture,
    pub view: TextureView,
}

/// One texture the pool owns.
struct Slot {
    tex: Pooled,
    class: (u32, u32),
    /// The frame it was last given back in, for [`KEEP_FRAMES`].
    last_used: u64,
    /// The frame it was last taken in, so a texture taken twice in one frame —
    /// released by one batch and taken by the next — counts once towards that
    /// frame's working set.
    taken_in: Option<u64>,
}

impl Slot {
    fn bytes(&self) -> u64 {
        u64::from(self.class.0) * u64::from(self.class.1) * 4
    }
}

/// What the effect passes did in one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FxStats {
    /// Textures the pool had to allocate because no idle one of the class was
    /// there. **Zero on a steady frame** — the whole point of the pool.
    pub textures_made: usize,
    /// Bind groups made, because none for the same three textures was cached.
    pub bind_groups_made: usize,
    /// `queue.submit`s the passes made: one per [`run_batch`], which is one per
    /// vello effect pass — not one per layer.
    pub submits: usize,
    /// Effect layers handed to [`run_batch`], with ink or without.
    pub layers: usize,
    /// Bytes the pool holds once the frame has given everything back and been
    /// trimmed.
    pub pooled_bytes: u64,
}

/// The effect passes' textures and buffers, kept across layers and frames so a
/// steady frame allocates nothing.
///
/// It holds:
///
/// - **Textures, keyed by size class** ([`size_class`]): a pass's packed source,
///   its packed result and the five scratch slots. Taken with [`Self::take`];
///   given back mid-frame with [`Self::release`] once nothing later in the frame
///   reads it, or all at once by [`Self::end_frame`].
/// - **One uniform buffer and one kernel buffer**, grown and never shrunk; a
///   dispatch names its `Params` by dynamic offset and its kernel by
///   `Params::k_off`.
/// - **Bind groups, keyed by the three textures they bind**; with the buffers
///   shared, a bind group is a function of its textures alone.
///
/// **Lifetimes.** A result is read by vello's atlas copy in the consuming pass,
/// and `render` unregisters every slot after the page's render is submitted, so a
/// result is held to the end of the frame and given back by [`Self::end_frame`]
/// after that. A packed source and the scratch are read only by their own batch's
/// submit, so they go back as soon as it is made. Reuse is then safe by queue
/// order: every later write is in a later submission on the one queue. No fence,
/// no `poll`.
///
/// **Bound.** [`Self::end_frame`] drops any idle texture not taken for
/// [`KEEP_FRAMES`] frames, then, oldest first, enough idle textures that the pool
/// holds no more than the largest working set of the last [`KEEP_FRAMES`] frames
/// (the bytes of the distinct textures one frame took). The bound must be the
/// working set, not the peak bytes taken at one moment: a packed source released
/// mid-frame and not retaken makes the distinct textures outnumber the peak, and
/// the trim then drops one the next frame needs, allocating every frame.
#[derive(Default)]
pub struct FxPool {
    idle: Vec<Slot>,
    busy: Vec<Slot>,
    next_id: u64,
    frame: u64,
    /// This frame's working set so far: the bytes of the distinct textures taken.
    used: u64,
    /// The last [`KEEP_FRAMES`] frames' `used`, newest last.
    worked: std::collections::VecDeque<u64>,
    /// The batch's `Params`, one per dispatch at `stride` bytes apart.
    uniforms: Option<wgpu::Buffer>,
    /// Every blur kernel a batch uses, end to end.
    kernels: Option<wgpu::Buffer>,
    /// `Params::bytes`' size rounded up to the device's uniform offset alignment.
    stride: u64,
    binds: rustc_hash::FxHashMap<(u64, u64, u64), wgpu::BindGroup>,
    cur: FxStats,
    last: FxStats,
}

impl FxPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// A texture of at least `w`×`h`, idle from an earlier layer or frame when one
    /// of the class is there and allocated when not.
    pub fn take(&mut self, device: &Device, w: u32, h: u32) -> Pooled {
        let limit = device.limits().max_texture_dimension_2d;
        let class = (size_class(w, limit), size_class(h, limit));
        // The oldest idle texture of the class, not the first found: bind groups
        // are cached by texture, and `swap_remove` reorders `idle`, so "first
        // found" moves each scratch slot to a different texture every frame and
        // the cache never hits.
        let found = self
            .idle
            .iter()
            .enumerate()
            .filter(|(_, s)| s.class == class)
            .min_by_key(|(_, s)| s.tex.id)
            .map(|(i, _)| i);
        let mut slot = match found {
            Some(i) => self.idle.swap_remove(i),
            None => {
                self.cur.textures_made += 1;
                let texture = fx_texture(device, class.0, class.1, "ondin-fx-pooled");
                let view = texture.create_view(&Default::default());
                self.next_id += 1;
                Slot {
                    tex: Pooled {
                        id: self.next_id,
                        texture,
                        view,
                    },
                    class,
                    last_used: self.frame,
                    taken_in: None,
                }
            }
        };
        if slot.taken_in != Some(self.frame) {
            slot.taken_in = Some(self.frame);
            self.used += slot.bytes();
        }
        let tex = slot.tex.clone();
        self.busy.push(slot);
        tex
    }

    /// Give `t` back before the frame ends, for a later batch of the same frame
    /// to take. **Only once every read of it has been submitted** — see the type's
    /// doc for why queue order is then enough.
    pub fn release(&mut self, t: &Pooled) {
        if let Some(i) = self.busy.iter().position(|s| s.tex.id == t.id) {
            let mut slot = self.busy.swap_remove(i);
            slot.last_used = self.frame;
            self.idle.push(slot);
        }
    }

    /// The frame is over: every texture still taken is given back, the idle ones
    /// are trimmed (see the type's doc), and the frame's counts become
    /// [`Self::last_frame`].
    ///
    /// **Called by `VelloGpuRenderer::render` after it has unregistered the
    /// frame's atlas slots**, on the error path as well as the ordinary one.
    pub fn end_frame(&mut self) {
        for mut slot in self.busy.drain(..) {
            slot.last_used = self.frame;
            self.idle.push(slot);
        }
        self.worked.push_back(self.used);
        while self.worked.len() as u64 > KEEP_FRAMES {
            self.worked.pop_front();
        }
        self.used = 0;
        let frame = self.frame;
        let mut gone: Vec<u64> = Vec::new();
        self.idle.retain(|s| {
            let keep = frame - s.last_used < KEEP_FRAMES;
            if !keep {
                gone.push(s.tex.id);
            }
            keep
        });
        let bound = self.worked.iter().copied().max().unwrap_or(0);
        let mut held: u64 = self.idle.iter().map(Slot::bytes).sum();
        if held > bound {
            // Oldest first, and the larger of two equally old first.
            self.idle
                .sort_by_key(|s| (s.last_used, std::cmp::Reverse(s.bytes())));
            while held > bound && !self.idle.is_empty() {
                let s = self.idle.remove(0);
                held -= s.bytes();
                gone.push(s.tex.id);
            }
        }
        if !gone.is_empty() {
            self.binds.retain(|k, _| {
                !gone.contains(&k.0) && !gone.contains(&k.1) && !gone.contains(&k.2)
            });
        }
        self.frame += 1;
        self.cur.pooled_bytes = held;
        self.last = std::mem::take(&mut self.cur);
    }

    /// What the last finished frame did.
    pub fn last_frame(&self) -> FxStats {
        self.last
    }

    /// Wrap a texture the pool does not own, for [`run`]'s one-off batch. It gets
    /// an identity for the bind-group cache and is never trimmed — which is fine
    /// only because `run`'s pool is dropped when it returns.
    fn adopt(&mut self, texture: &Texture) -> Pooled {
        self.next_id += 1;
        Pooled {
            id: self.next_id,
            texture: texture.clone(),
            view: texture.create_view(&Default::default()),
        }
    }

    /// Grow the two shared buffers to hold `steps` dispatches' `Params` and
    /// `floats` kernel weights. A grown buffer is a new buffer, so every cached
    /// bind group naming the old one goes with it.
    fn reserve(&mut self, device: &Device, steps: usize, floats: usize) {
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        self.stride = (PARAMS_BYTES as u64).div_ceil(align) * align;
        let want_u = (steps as u64 * self.stride).max(self.stride);
        let want_k = (floats as u64 * 4).max(256);
        let grow =
            |buf: &Option<wgpu::Buffer>, want: u64| buf.as_ref().is_none_or(|b| b.size() < want);
        if grow(&self.uniforms, want_u) {
            self.uniforms = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ondin-fx-params"),
                size: want_u.next_power_of_two(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.binds.clear();
        }
        if grow(&self.kernels, want_k) {
            self.kernels = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ondin-fx-kernel"),
                size: want_k.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.binds.clear();
        }
    }

    /// The bind group for a dispatch reading `a` and `b` and writing `dst`, from
    /// the cache when the three have been bound together before.
    fn bind_group(
        &mut self,
        device: &Device,
        fx: &FxPipelines,
        a: &Pooled,
        b: &Pooled,
        dst: &Pooled,
    ) -> wgpu::BindGroup {
        let key = (a.id, b.id, dst.id);
        if let Some(g) = self.binds.get(&key) {
            return g.clone();
        }
        let (Some(uniforms), Some(kernels)) = (&self.uniforms, &self.kernels) else {
            unreachable!("`reserve` runs before any dispatch is encoded");
        };
        let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ondin-fx"),
            layout: &fx.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&a.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&b.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dst.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(PARAMS_BYTES as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: kernels.as_entire_binding(),
                },
            ],
        });
        self.cur.bind_groups_made += 1;
        self.binds.insert(key, g.clone());
        g
    }
}

/// One effect layer of a batch: where it is read from, where its result goes,
/// and what to run over it.
#[derive(Clone, Copy)]
pub struct Item<'a> {
    /// Where the layer sits in the batch's source texture.
    pub slice: Slice,
    /// Where its result's top-left goes in the batch's destination. The canvas
    /// passes `slice.at`, so the destination is packed exactly as the source is
    /// and each atlas slot is pointed at its own rectangle of it.
    pub dst_at: (u32, u32),
    pub effects: &'a [ondin_core::Keyed<Effect>],
    /// Device pixels per document unit along each axis — see [`run`].
    pub scale: (f64, f64),
}

/// Slots in the scratch, named so the sequence below reads as the reference does.
///
/// `a`/`b` carry the layer through the appearance passes and then serve as the
/// shadow's scratch, `graphic` holds the layer as the shadows see it, and
/// `o0`/`o1` accumulate the result. They are taken only for a batch with ink
/// (`effects::any_ink`), once per batch at its largest size.
const A: usize = 0;
const B: usize = 1;
const GRAPHIC: usize = 2;
const O0: usize = 3;
const O1: usize = 4;
const SCRATCH: usize = 5;

/// Run an effect stack over `src` and return the result, in **straight** alpha
/// and ready for vello's atlas.
///
/// `src` is what vello rasterized the subtree into — straight alpha, the size of
/// the layer's clipped device box. `scale` is device pixels per document unit
/// along each axis, so an authored radius becomes a kernel and an authored offset
/// becomes pixels; it is the same pair the CPU backend takes.
///
/// Returns `None` when there is nothing to do, so the caller can draw `src`
/// directly rather than paying for a copy that changes nothing. The result sits
/// at the top-left of a texture **at least** `slice.size` ([`size_class`]).
///
/// A one-off: [`run_batch`] over a pool of its own, dropped on return. The canvas
/// goes through `VelloGpuRenderer::resolve_effects` instead; this is for
/// `tests/fx_gpu.rs`, which compares one stack's bytes against the CPU reference.
pub fn run(
    device: &Device,
    queue: &Queue,
    fx: &FxPipelines,
    src: &Texture,
    slice: Slice,
    effects: &[ondin_core::Keyed<Effect>],
    scale: (f64, f64),
) -> Option<Texture> {
    if !effects::any_ink(effects) {
        return None;
    }
    let mut pool = FxPool::new();
    let src = pool.adopt(src);
    let dst = pool.take(device, slice.size.0, slice.size.1);
    let item = Item {
        slice,
        dst_at: (0, 0),
        effects,
        scale,
    };
    run_batch(device, queue, fx, &mut pool, &src, &dst, &[item]);
    Some(dst.texture)
}

/// Filter every layer of one packed batch from `src` into `dst`, through one
/// encoder and one submit.
///
/// A layer with ink runs its stack through the batch's five scratch slots, taken
/// from `pool` at the batch's largest layer size and given back after the submit;
/// its last pass writes into its rectangle of `dst`. A layer with nothing to draw
/// is copied across unchanged.
///
/// The dispatches are planned first so the batch's `Params` and kernels go into
/// the pool's two shared buffers with one `queue.write_buffer` each. One compute
/// pass serves the whole batch: a compute pass's usage scope is per dispatch, so
/// a texture written by one dispatch and read by the next is legal and ordered.
pub fn run_batch(
    device: &Device,
    queue: &Queue,
    fx: &FxPipelines,
    pool: &mut FxPool,
    src: &Pooled,
    dst: &Pooled,
    items: &[Item<'_>],
) {
    pool.cur.layers += items.len();
    let mut plan = Plan::default();
    let mut copies: Vec<&Item<'_>> = Vec::new();
    let mut largest = (0u32, 0u32);
    for item in items {
        if !effects::any_ink(item.effects) {
            copies.push(item);
            continue;
        }
        let (w, h) = item.slice.size;
        largest = (largest.0.max(w), largest.1.max(h));
        Runner {
            plan: &mut plan,
            w,
            h,
        }
        .item(item);
    }
    if plan.steps.is_empty() && copies.is_empty() {
        return;
    }
    let scratch: Vec<Pooled> = if plan.steps.is_empty() {
        Vec::new()
    } else {
        (0..SCRATCH)
            .map(|_| pool.take(device, largest.0, largest.1))
            .collect()
    };

    pool.reserve(device, plan.steps.len(), plan.kernels.len());
    let stride = pool.stride as usize;
    if let (Some(uniforms), Some(kernels)) = (&pool.uniforms, &pool.kernels) {
        let mut ub = vec![0u8; plan.steps.len() * stride];
        for (i, s) in plan.steps.iter().enumerate() {
            ub[i * stride..i * stride + PARAMS_BYTES]
                .copy_from_slice(&s.params.bytes(s.dom.0, s.dom.1));
        }
        if !ub.is_empty() {
            queue.write_buffer(uniforms, 0, &ub);
        }
        if !plan.kernels.is_empty() {
            let kb: Vec<u8> = plan.kernels.iter().flat_map(|v| v.to_le_bytes()).collect();
            queue.write_buffer(kernels, 0, &kb);
        }
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ondin-fx"),
    });
    for item in copies {
        let Slice { at, size } = item.slice;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: at.0,
                    y: at.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &dst.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: item.dst_at.0,
                    y: item.dst_at.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
    }
    if !plan.steps.is_empty() {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ondin-fx"),
            timestamp_writes: None,
        });
        let texture = |at: At| match at {
            At::Scratch(i) => &scratch[i],
            At::Src => src,
            At::Dst => dst,
        };
        for (i, s) in plan.steps.iter().enumerate() {
            let bind = pool.bind_group(device, fx, texture(s.a), texture(s.b), texture(s.dst));
            cpass.set_pipeline(fx.pipeline(s.pass));
            cpass.set_bind_group(0, &bind, &[(i * stride) as u32]);
            cpass.dispatch_workgroups(s.dom.0.div_ceil(TILE), s.dom.1.div_ceil(TILE), 1);
        }
    }
    queue.submit([encoder.finish()]);
    pool.cur.submits += 1;
    for t in &scratch {
        pool.release(t);
    }
}

/// Where a planned dispatch reads or writes: a scratch slot, the batch's packed
/// source, or its packed destination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum At {
    Scratch(usize),
    Src,
    Dst,
}

/// One dispatch, worked out on the host before anything is encoded.
struct Step {
    pass: Pass,
    a: At,
    b: At,
    dst: At,
    params: Params,
    /// The grid the dispatch covers, which is also `Params::size`.
    dom: (u32, u32),
}

/// A batch's dispatches in order, and every blur kernel they read, end to end.
#[derive(Default)]
struct Plan {
    steps: Vec<Step>,
    kernels: Vec<f32>,
}

impl Runner<'_> {
    /// Plan one layer's stack: ingest from its rectangle of the source, the
    /// reference's steps in the reference's order, and the last pass into its
    /// rectangle of the destination.
    fn item(&mut self, item: &Item<'_>) {
        let Item {
            slice: Slice { at, size: (w, h) },
            dst_at,
            effects,
            scale,
        } = *item;
        let run = self;

        // --- the layer's own appearance, in list order -----------------------
        run.step(
            Pass::Premul,
            At::Src,
            At::Src,
            At::Scratch(A),
            Params {
                src_origin: (at.0 as i32, at.1 as i32),
                ..Default::default()
            },
            (w, h),
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
            // `inner` does not reach this pass: the silhouette is the graphic's
            // alpha for both kinds and the complement happens at `Pass::Tint`, so
            // every pass between reads outside the buffer as transparent.
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
                // One radius per axis, as `effects::spread_alpha` takes it, so a
                // non-uniformly scaled layer stays inside its escape box.
                let r = (sh.spread * scale.0, sh.spread * scale.1);
                // Bounded by the buffer through `effects::spread_steps`, the CPU
                // backend's own helper: the zoom multiplies the authored spread
                // without limit, and the cap is lossless because the shader
                // reads outside the buffer as empty. Keep one copy of the
                // arithmetic; `tests/fx_gpu.rs` runs at scale 1 and would not
                // catch a divergent one.
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
                // The same choice `effects::shadow_layer` makes, so the backends
                // resample on the same grid or not at all.
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

        // Back to straight alpha for vello's atlas, into this layer's rectangle
        // of the batch's destination.
        run.step(
            Pass::Unpremul,
            At::Scratch(out),
            At::Scratch(out),
            At::Dst,
            Params {
                dst_origin: (dst_at.0 as i32, dst_at.1 as i32),
                ..Default::default()
            },
            (w, h),
        );
    }
}

/// Plans one layer's dispatches into a batch's [`Plan`].
struct Runner<'a> {
    plan: &'a mut Plan,
    /// The layer's own buffer size — the grid most of its dispatches cover.
    w: u32,
    h: u32,
}

impl Runner<'_> {
    /// A dispatch whose sources and destination are scratch slots.
    fn slot(&mut self, pass: Pass, a: usize, b: usize, dst: usize, p: Params) {
        let dom = (self.w, self.h);
        self.slot_in(pass, a, b, dst, p, dom);
    }

    /// [`Self::slot`] over a smaller grid than the buffer — the coarse region of a
    /// resampled shadow, in the scratch texture's top-left corner. Nothing reads
    /// outside `dom`.
    fn slot_in(&mut self, pass: Pass, a: usize, b: usize, dst: usize, p: Params, dom: (u32, u32)) {
        self.step(
            pass,
            At::Scratch(a),
            At::Scratch(b),
            At::Scratch(dst),
            p,
            dom,
        );
    }

    /// Any dispatch, over `dom`.
    fn step(&mut self, pass: Pass, a: At, b: At, dst: At, params: Params, dom: (u32, u32)) {
        self.plan.steps.push(Step {
            pass,
            a,
            b,
            dst,
            params,
            dom,
        });
    }

    /// Both axes of a separable gaussian over the whole buffer.
    fn blur(&mut self, cur: &mut usize, alt: &mut usize, sigma_x: f32, sigma_y: f32) {
        let dom = (self.w, self.h);
        self.blur_in(cur, alt, sigma_x, sigma_y, dom);
    }

    /// `effects::blur_coarse`: box-average the silhouette into `k`×`k` blocks,
    /// blur *that* at the coarse deviations, and read it back bilinearly.
    ///
    /// The three dispatches are the reference's three steps; the coarse grid lives
    /// in the corner of a full-size scratch texture, which keeps every slot one size.
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
            // `k as i32` is safe only because `shadow_downscale` caps at
            // `MAX_SHADOW_BLOCK`; a wrapped negative radius draws no shadow.
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
            // The kernel comes from the function the CPU convolves with, so the
            // weights are identical numbers.
            let k = effects::kernel(sigma);
            let radius = ((k.len() - 1) / 2) as i32;
            let k_off = self.plan.kernels.len() as u32;
            self.plan.kernels.extend_from_slice(&k);
            self.slot_in(
                Pass::Blur,
                *cur,
                *cur,
                *alt,
                Params {
                    radius,
                    horizontal,
                    k_off,
                    ..Default::default()
                },
                dom,
            );
            std::mem::swap(cur, alt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::size_class;

    /// The pool's size classes: two per octave, never below the request, and never
    /// past the device limit unless the request itself is.
    ///
    /// flip: `class` as `pow` alone → fails at 17 (32 against 24). Dropping the
    /// `.max(n)` → fails only at the last assertion (6000 for 7000).
    #[test]
    fn a_size_class_is_two_steps_per_octave_and_never_smaller_than_asked() {
        let limit = 8192;
        for (n, class) in [
            (1, 16),
            (16, 16),
            (17, 24),
            (24, 24),
            (25, 32),
            (70, 96),
            (96, 96),
            (97, 128),
            (1080, 1536),
            (1920, 2048),
            (6000, 6144),
            (8192, 8192),
        ] {
            assert_eq!(size_class(n, limit), class, "the class for {n}");
        }
        assert_eq!(size_class(5000, 6000), 6000, "capped at the device limit");
        assert_eq!(
            size_class(7000, 6000),
            7000,
            "but never below the request, which the caller has already sized"
        );
    }
}
