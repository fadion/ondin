//! Vision simulation: how a design reads to someone who sees differently, as a
//! view of the canvas, never of the document.
//!
//! A [`Condition`] at a severity gives an [`Effect`]: a [`Tint`], an affine map
//! of linear light (the four colour deficiencies, contrast loss and yellowing),
//! or a pass that samples neighbours (blur, ghosting), whose lengths are in
//! design px and scaled by the zoom. [`apply`] is the specification;
//! [`VisionPass`] runs it on the GPU over a rendered canvas.
//!
//! Its sRGB decode and encode work on pixels already drawn, not on document
//! colours, so they are not the document-to-backend conversion `color` keeps.

use crate::color::{linear_to_srgb, srgb_to_linear};
use wgpu::{Device, Queue, TextureView};

/// The weakest severity a condition is shown at: 0% would be the frame untouched,
/// which ending the mode already shows.
pub const MIN_SEVERITY: f32 = 0.1;

/// Blurred vision's Gaussian σ at full severity, in design px.
pub const BLUR_SIGMA: f32 = 6.0;
/// Ghosting's copy offset at full severity, in design px: right and a little down.
pub const GHOST_OFFSET: [f32; 2] = [8.0, 4.0];
/// How much of the ghost is mixed in at full severity.
pub const GHOST_MIX: f32 = 0.4;
/// How far contrast loss pulls every channel toward mid-grey at full severity.
pub const CONTRAST_LOSS: f32 = 0.8;
/// Yellowing's loss of green and of blue at full severity, as sRGB factors lost,
/// an ageing lens passing less short-wavelength light.
pub const YELLOWING: [f32; 2] = [0.1, 0.76];

/// The most taps a blur takes on each side of a pixel. A wider blur strides,
/// so a large σ at a high zoom costs no more than this.
pub const BLUR_TAPS: u32 = 32;

/// A way of seeing the canvas can be shown through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Condition {
    /// No long-wavelength (red) cones; partial severity is protanomaly.
    Protanopia,
    /// No medium-wavelength (green) cones — the most common.
    #[default]
    Deuteranopia,
    /// No short-wavelength (blue) cones.
    Tritanopia,
    /// No colour at all: luminance only.
    Achromatopsia,
    /// Out of focus: a Gaussian blur.
    Blur,
    /// Faded: every channel pulled toward mid-grey.
    ContrastLoss,
    /// A double image: a fainter copy offset beside the first.
    Ghosting,
    /// An ageing lens: less blue and a little less green.
    Yellowing,
}

impl Condition {
    /// In menu order: the colour deficiencies, then sight.
    pub const ALL: [Condition; 8] = [
        Condition::Protanopia,
        Condition::Deuteranopia,
        Condition::Tritanopia,
        Condition::Achromatopsia,
        Condition::Blur,
        Condition::ContrastLoss,
        Condition::Ghosting,
        Condition::Yellowing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Condition::Protanopia => "Protanopia",
            Condition::Deuteranopia => "Deuteranopia",
            Condition::Tritanopia => "Tritanopia",
            Condition::Achromatopsia => "Achromatopsia",
            Condition::Blur => "Blurred vision",
            Condition::ContrastLoss => "Contrast loss",
            Condition::Ghosting => "Ghosting",
            Condition::Yellowing => "Yellowing",
        }
    }

    /// What it is, in a few plain words.
    pub fn gloss(self) -> &'static str {
        match self {
            Condition::Protanopia => "Red-blind",
            Condition::Deuteranopia => "Green-blind",
            Condition::Tritanopia => "Blue-blind",
            Condition::Achromatopsia => "No colour",
            Condition::Blur => "Out of focus",
            Condition::ContrastLoss => "Faded",
            Condition::Ghosting => "Double image",
            Condition::Yellowing => "Ageing lens",
        }
    }

    /// The key a preference stores it under. The deficiencies keep the keys
    /// they were stored under before the sight conditions joined them.
    pub fn key(self) -> &'static str {
        match self {
            Condition::Protanopia => "protanopia",
            Condition::Deuteranopia => "deuteranopia",
            Condition::Tritanopia => "tritanopia",
            Condition::Achromatopsia => "achromatopsia",
            Condition::Blur => "blur",
            Condition::ContrastLoss => "contrast-loss",
            Condition::Ghosting => "ghosting",
            Condition::Yellowing => "yellowing",
        }
    }

    /// The condition a preference names, if any.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }

    /// One of the four colour-vision deficiencies, rather than a loss of sight.
    pub fn is_colour(self) -> bool {
        matches!(
            self,
            Condition::Protanopia
                | Condition::Deuteranopia
                | Condition::Tritanopia
                | Condition::Achromatopsia
        )
    }

    /// The severity a condition opens at before one is chosen: a deficiency in
    /// full, which is the dichromacy; a loss of sight at half.
    pub fn default_severity(self) -> f32 {
        if self.is_colour() { 1.0 } else { 0.5 }
    }

    /// What the canvas goes through at `severity`, held to
    /// [`MIN_SEVERITY`]`..=1`.
    pub fn effect(self, severity: f32) -> Effect {
        let s = severity.clamp(MIN_SEVERITY, 1.0);
        match self {
            Condition::Protanopia => Effect::Tint(machado(&PROTAN, s)),
            Condition::Deuteranopia => Effect::Tint(machado(&DEUTAN, s)),
            Condition::Tritanopia => Effect::Tint(machado(&TRITAN, s)),
            Condition::Achromatopsia => {
                const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
                Effect::Tint(Tint::IDENTITY.lerp(&Tint([row(LUMA); 3]), s))
            }
            Condition::Blur => Effect::Blur {
                sigma: BLUR_SIGMA * s,
            },
            Condition::ContrastLoss => {
                // Toward the sRGB mid-grey, so faded reads as faded to the eye.
                let k = CONTRAST_LOSS * s;
                let grey = srgb_to_linear(0.5) * k;
                let d = 1.0 - k;
                Effect::Tint(Tint([
                    [d, 0.0, 0.0, grey],
                    [0.0, d, 0.0, grey],
                    [0.0, 0.0, d, grey],
                ]))
            }
            Condition::Ghosting => Effect::Ghost {
                offset: [GHOST_OFFSET[0] * s, GHOST_OFFSET[1] * s],
                // Rises fast, so a mild case already shows a second image.
                mix: GHOST_MIX * (1.0 - (1.0 - s).powi(4)),
            },
            Condition::Yellowing => {
                // An sRGB factor scales linear light by about its 2.2th power.
                let lin = |lost: f32| (1.0 - lost * s).powf(2.2);
                Effect::Tint(Tint([
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, lin(YELLOWING[0]), 0.0, 0.0],
                    [0.0, 0.0, lin(YELLOWING[1]), 0.0],
                ]))
            }
        }
    }
}

/// A matrix row with no offset.
const fn row(r: [f32; 3]) -> [f32; 4] {
    [r[0], r[1], r[2], 0.0]
}

/// An affine map from linear RGB seen to linear RGB simulated: each row's three
/// coefficients, then an offset added after them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint(pub [[f32; 4]; 3]);

impl Tint {
    pub const IDENTITY: Tint = Tint([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    /// `self` at `t = 0`, `to` at `t = 1`, each entry straight between.
    pub fn lerp(&self, to: &Tint, t: f32) -> Tint {
        let mut out = *self;
        for (row, to) in out.0.iter_mut().zip(&to.0) {
            for (v, to) in row.iter_mut().zip(to) {
                *v += (to - *v) * t;
            }
        }
        out
    }

    fn of(m: &[[f32; 3]; 3]) -> Tint {
        Tint(m.map(row))
    }

    fn map(&self, lin: [f32; 3]) -> [f32; 3] {
        self.0
            .map(|r| r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2] + r[3])
    }
}

/// What a condition does to the pixels in its region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Each pixel on its own.
    Tint(Tint),
    /// A separable Gaussian of `sigma` design px.
    Blur { sigma: f32 },
    /// A copy `offset` design px away, `mix` of it over the original.
    Ghost { offset: [f32; 2], mix: f32 },
}

/// Machado, Oliveira and Fernandes' (2009) simulation matrix at severity `s`,
/// straight between the two tabulated tenths it falls between.
fn machado(table: &[[[f32; 3]; 3]; 11], s: f32) -> Tint {
    let x = s.clamp(0.0, 1.0) * 10.0;
    let i = (x.floor() as usize).min(9);
    Tint::of(&table[i]).lerp(&Tint::of(&table[i + 1]), x - i as f32)
}

/// One straight-alpha sRGB pixel through `t`; alpha passes through.
pub fn tint_rgba8(t: &Tint, [r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let lin = [r, g, b].map(|c| srgb_to_linear(f32::from(c) / 255.0));
    let [r, g, b] = t.map(lin).map(encode);
    [r, g, b, a]
}

/// A linear value back to an 8-bit sRGB level, clamped first.
fn encode(v: f32) -> u8 {
    (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

/// A blur's kernel at `sigma` device px, as the GPU takes it: the taps on each
/// side, the stride between them in pixels, and the weight of the tap at each
/// distance from `0` out, summing to one over both sides. `None` for a blur too
/// small to move a pixel.
pub fn blur_kernel(sigma: f32) -> Option<(u32, u32, Vec<f32>)> {
    if sigma < 0.25 {
        return None;
    }
    let reach = (3.0 * sigma).ceil() as u32;
    let stride = reach.div_ceil(BLUR_TAPS).max(1);
    let taps = reach.div_ceil(stride);
    let mut w: Vec<f32> = (0..=taps)
        .map(|i| {
            let d = (i * stride) as f32;
            (-d * d / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
    for v in &mut w {
        *v /= sum;
    }
    Some((taps, stride, w))
}

/// The whole-pixel offset a ghost of `offset` design px lands at, `scale` device
/// px to the design px.
pub fn ghost_offset(offset: [f32; 2], scale: f32) -> [i32; 2] {
    offset.map(|v| (v * scale).round() as i32)
}

/// `effect` over `px`, straight-alpha sRGB rows `size` big, inside `region`
/// (`x0, y0, x1, y1`, far edges exclusive), every pixel outside it copied.
/// `scale` is device px to the design px. A neighbour is read clamped to the
/// region, so art outside it never bleeds in. The specification
/// [`VisionPass`] is held to: a blur is two passes, the first stored at 8 bits
/// as the GPU's scratch texture stores it.
pub fn apply(
    effect: &Effect,
    scale: f32,
    px: &[u8],
    size: (u32, u32),
    region: [u32; 4],
) -> Vec<u8> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let [x0, y0, x1, y1] = region.map(|v| v as i32);
    let inside = |x: i32, y: i32| x >= x0 && y >= y0 && x < x1 && y < y1;
    let at = |x: i32, y: i32| ((y * w + x) * 4) as usize;
    let clamp = |x: i32, y: i32| (x.clamp(x0, (x1 - 1).max(x0)), y.clamp(y0, (y1 - 1).max(y0)));
    let lin = |p: &[u8], i: usize| {
        [
            srgb_to_linear(f32::from(p[i]) / 255.0),
            srgb_to_linear(f32::from(p[i + 1]) / 255.0),
            srgb_to_linear(f32::from(p[i + 2]) / 255.0),
            f32::from(p[i + 3]) / 255.0,
        ]
    };
    let store = |out: &mut [u8], i: usize, c: [f32; 4]| {
        out[i] = encode(c[0]);
        out[i + 1] = encode(c[1]);
        out[i + 2] = encode(c[2]);
        out[i + 3] = (c[3].clamp(0.0, 1.0) * 255.0).round() as u8;
    };
    let mut out = px.to_vec();
    match effect {
        Effect::Tint(t) => {
            for y in 0..h {
                for x in 0..w {
                    if inside(x, y) {
                        let i = at(x, y);
                        let [r, g, b, a] = tint_rgba8(t, [px[i], px[i + 1], px[i + 2], px[i + 3]]);
                        out[i..i + 4].copy_from_slice(&[r, g, b, a]);
                    }
                }
            }
        }
        Effect::Blur { sigma } => {
            let Some((taps, stride, wt)) = blur_kernel(sigma * scale) else {
                return out;
            };
            let pass = |src: &[u8], dir: (i32, i32)| {
                let mut dst = src.to_vec();
                for y in 0..h {
                    for x in 0..w {
                        if !inside(x, y) {
                            continue;
                        }
                        let mut acc = [0.0f32; 4];
                        for k in -(taps as i32)..=taps as i32 {
                            let d = k * stride as i32;
                            let (sx, sy) = clamp(x + dir.0 * d, y + dir.1 * d);
                            let c = lin(src, at(sx, sy));
                            let wk = wt[k.unsigned_abs() as usize];
                            for (a, c) in acc.iter_mut().zip(c) {
                                *a += wk * c;
                            }
                        }
                        store(&mut dst, at(x, y), acc);
                    }
                }
                dst
            };
            let across = pass(px, (1, 0));
            out = pass(&across, (0, 1));
        }
        Effect::Ghost { offset, mix } => {
            let [dx, dy] = ghost_offset(*offset, scale);
            for y in 0..h {
                for x in 0..w {
                    if !inside(x, y) {
                        continue;
                    }
                    let a = lin(px, at(x, y));
                    let (sx, sy) = clamp(x - dx, y - dy);
                    let b = lin(px, at(sx, sy));
                    let c = [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * mix);
                    store(&mut out, at(x, y), c);
                }
            }
        }
    }
    out
}

/// [`apply`] as compute passes from one canvas texture into another, inside a
/// region of it. Building it compiles the shader, so it is made once; it keeps
/// the scratch texture a blur's first pass writes, sized to the last canvas.
pub struct VisionPass {
    layout: wgpu::BindGroupLayout,
    tint: wgpu::ComputePipeline,
    blur: wgpu::ComputePipeline,
    ghost: wgpu::ComputePipeline,
    /// One uniform per pass of a frame: a blur's two directions are submitted
    /// together, so they cannot share one buffer.
    params: [wgpu::Buffer; 2],
    scratch: Option<(wgpu::Texture, TextureView, (u32, u32))>,
}

/// The pass's uniform: three matrix rows (an offset in each fourth), the region,
/// a vector of floats (a blur's direction, a ghost's offset and mix), the blur's
/// taps and stride, then its weights packed four to a vector.
const PARAMS_BYTES: u64 = 16 * 6 + 16 * WEIGHT_VECS;
/// [`BLUR_TAPS`]` + 1` weights, four to a vector.
const WEIGHT_VECS: u64 = (BLUR_TAPS as u64 + 1).div_ceil(4);

impl VisionPass {
    pub fn new(device: &Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ondin-vision"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vision.wgsl").into()),
        });
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ondin-vision"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_BYTES),
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ondin-vision"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ondin-vision"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let buffer = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ondin-vision-params"),
                size: PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            tint: pipeline("tint"),
            blur: pipeline("blur"),
            ghost: pipeline("ghost"),
            layout,
            params: [buffer(), buffer()],
            scratch: None,
        }
    }

    /// Write `src` into `dst`, both `size` device pixels, through `effect` inside
    /// `region` (`x0, y0, x1, y1`, the far edges exclusive) and copying the rest;
    /// `scale` is device px to the design px. `dst` is an `Rgba8Unorm` texture
    /// with `STORAGE_BINDING`.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &Device,
        queue: &Queue,
        src: &TextureView,
        dst: &TextureView,
        size: (u32, u32),
        effect: &Effect,
        scale: f32,
        region: [u32; 4],
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ondin-vision"),
        });
        match effect {
            Effect::Tint(t) => {
                self.write(queue, 0, &Params::tint(t, region));
                self.dispatch(device, &mut encoder, &self.tint, 0, src, dst, size);
            }
            Effect::Ghost { offset, mix } => {
                let [dx, dy] = ghost_offset(*offset, scale);
                self.write(
                    queue,
                    0,
                    &Params::vector(region, [dx as f32, dy as f32, *mix, 0.0]),
                );
                self.dispatch(device, &mut encoder, &self.ghost, 0, src, dst, size);
            }
            Effect::Blur { sigma } => {
                let Some(kernel) = blur_kernel(sigma * scale) else {
                    // Nothing to blur: the copy a tint of the identity makes.
                    self.write(queue, 0, &Params::tint(&Tint::IDENTITY, [0; 4]));
                    self.dispatch(device, &mut encoder, &self.tint, 0, src, dst, size);
                    queue.submit([encoder.finish()]);
                    return;
                };
                let scratch = self.scratch(device, size);
                self.write(queue, 0, &Params::blur(region, (1, 0), &kernel));
                self.write(queue, 1, &Params::blur(region, (0, 1), &kernel));
                self.dispatch(device, &mut encoder, &self.blur, 0, src, &scratch, size);
                self.dispatch(device, &mut encoder, &self.blur, 1, &scratch, dst, size);
            }
        }
        queue.submit([encoder.finish()]);
    }

    fn write(&self, queue: &Queue, slot: usize, params: &Params) {
        queue.write_buffer(&self.params[slot], 0, &params.bytes());
    }

    /// The scratch texture's view, made or remade for `size`.
    fn scratch(&mut self, device: &Device, size: (u32, u32)) -> TextureView {
        if self.scratch.as_ref().is_none_or(|(_, _, s)| *s != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ondin-vision-scratch"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.scratch = Some((texture, view, size));
        }
        self.scratch
            .as_ref()
            .map(|(_, v, _)| v.clone())
            .expect("made above")
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        slot: usize,
        src: &TextureView,
        dst: &TextureView,
        size: (u32, u32),
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ondin-vision"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(dst),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params[slot].as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ondin-vision"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
    }
}

/// One pass's uniform, laid out as `vision.wgsl`'s `Params`.
struct Params {
    rows: [[f32; 4]; 3],
    region: [u32; 4],
    vector: [f32; 4],
    taps: [u32; 4],
    weights: [f32; 4 * WEIGHT_VECS as usize],
}

impl Params {
    fn empty(region: [u32; 4]) -> Self {
        Params {
            rows: Tint::IDENTITY.0,
            region,
            vector: [0.0; 4],
            taps: [0; 4],
            weights: [0.0; 4 * WEIGHT_VECS as usize],
        }
    }

    fn tint(t: &Tint, region: [u32; 4]) -> Self {
        Params {
            rows: t.0,
            ..Self::empty(region)
        }
    }

    fn vector(region: [u32; 4], vector: [f32; 4]) -> Self {
        Params {
            vector,
            ..Self::empty(region)
        }
    }

    fn blur(region: [u32; 4], dir: (i32, i32), (taps, stride, w): &(u32, u32, Vec<f32>)) -> Self {
        let mut p = Self::vector(region, [dir.0 as f32, dir.1 as f32, 0.0, 0.0]);
        p.taps = [*taps, *stride, 0, 0];
        p.weights[..w.len()].copy_from_slice(w);
        p
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PARAMS_BYTES as usize);
        for v in self.rows.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.region {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.vector {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.taps {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.weights {
            out.extend_from_slice(&v.to_le_bytes());
        }
        debug_assert_eq!(out.len() as u64, PARAMS_BYTES);
        out
    }
}

/// Machado et al.'s protanomaly matrices at severity 0.0, 0.1, … 1.0.
const PROTAN: [[[f32; 3]; 3]; 11] = [
    [
        [1.000000, 0.000000, -0.000000],
        [0.000000, 1.000000, 0.000000],
        [-0.000000, -0.000000, 1.000000],
    ],
    [
        [0.856167, 0.182038, -0.038205],
        [0.029342, 0.955115, 0.015544],
        [-0.002880, -0.001563, 1.004443],
    ],
    [
        [0.734766, 0.334872, -0.069637],
        [0.051840, 0.919198, 0.028963],
        [-0.004928, -0.004209, 1.009137],
    ],
    [
        [0.630323, 0.465641, -0.095964],
        [0.069181, 0.890046, 0.040773],
        [-0.006308, -0.007724, 1.014032],
    ],
    [
        [0.539009, 0.579343, -0.118352],
        [0.082546, 0.866121, 0.051332],
        [-0.007136, -0.011959, 1.019095],
    ],
    [
        [0.458064, 0.679578, -0.137642],
        [0.092785, 0.846313, 0.060902],
        [-0.007494, -0.016807, 1.024301],
    ],
    [
        [0.385450, 0.769005, -0.154455],
        [0.100526, 0.829802, 0.069673],
        [-0.007442, -0.022190, 1.029632],
    ],
    [
        [0.319627, 0.849633, -0.169261],
        [0.106241, 0.815969, 0.077790],
        [-0.007025, -0.028051, 1.035076],
    ],
    [
        [0.259411, 0.923008, -0.182420],
        [0.110296, 0.804340, 0.085364],
        [-0.006276, -0.034346, 1.040622],
    ],
    [
        [0.203876, 0.990338, -0.194214],
        [0.112975, 0.794542, 0.092483],
        [-0.005222, -0.041043, 1.046265],
    ],
    [
        [0.152286, 1.052583, -0.204868],
        [0.114503, 0.786281, 0.099216],
        [-0.003882, -0.048116, 1.051998],
    ],
];

/// Machado et al.'s deuteranomaly matrices at severity 0.0, 0.1, … 1.0.
const DEUTAN: [[[f32; 3]; 3]; 11] = [
    [
        [1.000000, 0.000000, -0.000000],
        [0.000000, 1.000000, 0.000000],
        [-0.000000, -0.000000, 1.000000],
    ],
    [
        [0.866435, 0.177704, -0.044139],
        [0.049567, 0.939063, 0.011370],
        [-0.003453, 0.007233, 0.996220],
    ],
    [
        [0.760729, 0.319078, -0.079807],
        [0.090568, 0.889315, 0.020117],
        [-0.006027, 0.013325, 0.992702],
    ],
    [
        [0.675425, 0.433850, -0.109275],
        [0.125303, 0.847755, 0.026942],
        [-0.007950, 0.018572, 0.989378],
    ],
    [
        [0.605511, 0.528560, -0.134071],
        [0.155318, 0.812366, 0.032316],
        [-0.009376, 0.023176, 0.986200],
    ],
    [
        [0.547494, 0.607765, -0.155259],
        [0.181692, 0.781742, 0.036566],
        [-0.010410, 0.027275, 0.983136],
    ],
    [
        [0.498864, 0.674741, -0.173604],
        [0.205199, 0.754872, 0.039929],
        [-0.011131, 0.030969, 0.980162],
    ],
    [
        [0.457771, 0.731899, -0.189670],
        [0.226409, 0.731012, 0.042579],
        [-0.011595, 0.034333, 0.977261],
    ],
    [
        [0.422823, 0.781057, -0.203881],
        [0.245752, 0.709602, 0.044646],
        [-0.011843, 0.037423, 0.974421],
    ],
    [
        [0.392952, 0.823610, -0.216562],
        [0.263559, 0.690210, 0.046232],
        [-0.011910, 0.040281, 0.971630],
    ],
    [
        [0.367322, 0.860646, -0.227968],
        [0.280085, 0.672501, 0.047413],
        [-0.011820, 0.042940, 0.968881],
    ],
];

/// Machado et al.'s tritanomaly matrices at severity 0.0, 0.1, … 1.0.
const TRITAN: [[[f32; 3]; 3]; 11] = [
    [
        [1.000000, 0.000000, -0.000000],
        [0.000000, 1.000000, 0.000000],
        [-0.000000, -0.000000, 1.000000],
    ],
    [
        [0.926670, 0.092514, -0.019184],
        [0.021191, 0.964503, 0.014306],
        [0.008437, 0.054813, 0.936750],
    ],
    [
        [0.895720, 0.133330, -0.029050],
        [0.029997, 0.945400, 0.024603],
        [0.013027, 0.104707, 0.882266],
    ],
    [
        [0.905871, 0.127791, -0.033662],
        [0.026856, 0.941251, 0.031893],
        [0.013410, 0.148296, 0.838294],
    ],
    [
        [0.948035, 0.089490, -0.037526],
        [0.014364, 0.946792, 0.038844],
        [0.010853, 0.193991, 0.795156],
    ],
    [
        [1.017277, 0.027029, -0.044306],
        [-0.006113, 0.958479, 0.047634],
        [0.006379, 0.248708, 0.744913],
    ],
    [
        [1.104996, -0.046633, -0.058363],
        [-0.032137, 0.971635, 0.060503],
        [0.001336, 0.317922, 0.680742],
    ],
    [
        [1.193214, -0.109812, -0.083402],
        [-0.058496, 0.979410, 0.079086],
        [-0.002346, 0.403492, 0.598854],
    ],
    [
        [1.257728, -0.139648, -0.118081],
        [-0.078003, 0.975409, 0.102594],
        [-0.003316, 0.501214, 0.502102],
    ],
    [
        [1.278864, -0.125333, -0.153531],
        [-0.084748, 0.957674, 0.127074],
        [-0.000989, 0.601151, 0.399838],
    ],
    [
        [1.255528, -0.076749, -0.178779],
        [-0.078411, 0.930809, 0.147602],
        [0.004733, 0.691367, 0.303900],
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tint(c: Condition, s: f32) -> Tint {
        match c.effect(s) {
            Effect::Tint(t) => t,
            other => panic!("{c:?} is no tint: {other:?}"),
        }
    }

    /// White, black and greys stay as they are under every deficiency at every
    /// severity: each row of each matrix sums to one.
    /// flip: a protanopia row off by 0.1 → fails at the greys.
    #[test]
    fn greys_stay_grey_under_the_deficiencies() {
        for c in Condition::ALL.into_iter().filter(|c| c.is_colour()) {
            for s in [0.1, 0.35, 0.5, 1.0] {
                for v in [0u8, 64, 128, 200, 255] {
                    let [r, g, b, a] = tint_rgba8(&tint(c, s), [v, v, v, 77]);
                    for ch in [r, g, b] {
                        assert!(ch.abs_diff(v) <= 1, "{c:?} {s} turned {v} into {r},{g},{b}");
                    }
                    assert_eq!(a, 77);
                }
            }
        }
    }

    /// Red and green, which a trichromat tells apart at once, move far closer
    /// for the two red-green dichromacies than for tritanopia; every colour is
    /// grey under achromatopsia.
    #[test]
    fn red_and_green_merge_for_the_red_green_dichromacies() {
        let (red, green) = ([220, 40, 40, 255], [40, 160, 40, 255]);
        let gap =
            |a: [u8; 4], b: [u8; 4]| (0..3).map(|i| f32::from(a[i].abs_diff(b[i]))).sum::<f32>();
        let seen = gap(red, green);
        let through = |c, s| {
            let t = tint(c, s);
            gap(tint_rgba8(&t, red), tint_rgba8(&t, green))
        };
        for c in [Condition::Protanopia, Condition::Deuteranopia] {
            assert!(
                through(c, 1.0) < seen * 0.6,
                "{c:?}: {} of {seen}",
                through(c, 1.0)
            );
            assert!(
                through(c, 1.0) < through(Condition::Tritanopia, 1.0) * 0.7,
                "{c:?}"
            );
            assert!(
                through(c, 0.3) > through(c, 1.0),
                "{c:?}: a mild case merges them less"
            );
        }
        let [r, g, b, _] = tint_rgba8(&tint(Condition::Achromatopsia, 1.0), red);
        assert!(r == g && g == b);
    }

    /// Severity steps through Machado's table: a tabulated tenth is its own
    /// matrix, and a point between two is straight between them.
    /// flip: `machado` reading `table[i + 1]` as `table[i]` → fails at "between".
    #[test]
    fn a_severity_reads_machado_s_table() {
        let at = |s: f32| tint(Condition::Deuteranopia, s).0[0][0];
        assert!((at(1.0) - 0.367_322).abs() < 1e-6, "full: the dichromacy");
        assert!((at(0.5) - 0.547_494).abs() < 1e-6, "a tenth: its own row");
        let mid = (0.547_494 + 0.498_864) / 2.0;
        assert!((at(0.55) - mid).abs() < 1e-5, "between: {}", at(0.55));
        assert_eq!(at(0.0), at(MIN_SEVERITY), "held to the floor");
    }

    /// Contrast loss pulls white and black toward mid-grey and keeps mid-grey;
    /// yellowing keeps red, dims blue most.
    #[test]
    fn contrast_loss_fades_and_yellowing_dims_blue() {
        let fade = tint(Condition::ContrastLoss, 1.0);
        let [w, ..] = tint_rgba8(&fade, [255, 255, 255, 255]);
        let [k, ..] = tint_rgba8(&fade, [0, 0, 0, 255]);
        let [m, ..] = tint_rgba8(&fade, [128, 128, 128, 255]);
        assert!(w < 200 && k > 60, "white {w}, black {k}");
        assert!(m.abs_diff(128) <= 1, "mid-grey stays: {m}");
        let [r, g, b, _] = tint_rgba8(&tint(Condition::Yellowing, 0.5), [255, 255, 255, 255]);
        assert_eq!(r, 255);
        assert!(b < g && g < 255, "{r},{g},{b}");
    }

    /// A blur's weights sum to one over both sides, and a wide one strides
    /// rather than taking more than `BLUR_TAPS` a side.
    #[test]
    fn a_blur_kernel_is_normalised_and_bounded() {
        for sigma in [0.5, 3.0, 24.0, 200.0] {
            let (taps, stride, w) = blur_kernel(sigma).expect("a kernel");
            assert!(taps <= BLUR_TAPS, "{sigma}: {taps} taps");
            assert!(taps * stride >= (3.0 * sigma).ceil() as u32 - stride);
            let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            assert!((sum - 1.0).abs() < 1e-5, "{sigma}: {sum}");
        }
        assert!(blur_kernel(0.1).is_none(), "too small to move a pixel");
    }

    /// A blur and a ghost read nothing from outside the region: a black frame
    /// beside a white field stays black at its edge, and the field outside is
    /// copied untouched.
    /// flip: `apply`'s `clamp` reading past the region → fails at "no bleed".
    #[test]
    fn neighbours_are_read_inside_the_region_only() {
        let (w, h) = (24u32, 12u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        let region = [6, 2, 18, 10];
        for y in region[1]..region[3] {
            for x in region[0]..region[2] {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        for effect in [Condition::Blur.effect(1.0), Condition::Ghosting.effect(1.0)] {
            let out = apply(&effect, 1.0, &px, (w, h), region);
            let i = ((5 * w + 6) * 4) as usize;
            assert_eq!(out[i], 0, "{effect:?}: no bleed at the edge");
            let o = ((5 * w + 2) * 4) as usize;
            assert_eq!(&out[o..o + 4], &px[o..o + 4], "{effect:?}: outside copied");
        }
    }

    /// A ghost mixes in the pixel its offset away, by `mix`.
    #[test]
    fn a_ghost_mixes_in_the_offset_copy() {
        let (w, h) = (20u32, 4u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_mut(4) {
            p[3] = 255;
        }
        // Row 1, so the copy lands inside the region's rows too.
        let white = ((w + 2) * 4) as usize;
        px[white..white + 3].copy_from_slice(&[255, 255, 255]);
        let effect = Effect::Ghost {
            offset: [8.0, 0.0],
            mix: 0.4,
        };
        let out = apply(&effect, 1.0, &px, (w, h), [0, 0, w, h]);
        let ghost = ((w + 10) * 4) as usize;
        let want = encode(0.4);
        assert_eq!(out[ghost], want, "the copy eight px on");
        assert_eq!(out[white], encode(0.6), "the original, a little dimmed");
    }

    /// The shader's transfer functions carry `color.rs`'s constants, which the
    /// default gates can check without a GPU; mid-grey decodes to 0.214.
    /// flip: `color::SRGB_KNEE` set to 0.04 → fails at "vision.wgsl lacks".
    #[test]
    fn the_shader_transfers_as_color_rs_does() {
        let wgsl = include_str!("vision.wgsl");
        for line in crate::color::wgsl_transfer_lines() {
            assert!(wgsl.contains(&line), "vision.wgsl lacks `{line}`");
        }
        let mid = srgb_to_linear(0.5);
        assert!((mid - 0.214).abs() < 1e-3, "{mid}");
        assert!((linear_to_srgb(mid) - 0.5).abs() < 1e-5);
    }

    /// The shader's weight array holds every tap a kernel can have.
    #[test]
    fn the_uniform_holds_every_weight() {
        let wgsl = include_str!("vision.wgsl");
        assert!(
            wgsl.contains(&format!("array<vec4<f32>, {WEIGHT_VECS}>")),
            "vision.wgsl's weights are not {WEIGHT_VECS} vectors"
        );
        assert_eq!(Params::empty([0; 4]).bytes().len() as u64, PARAMS_BYTES);
    }

    #[test]
    fn a_preference_key_names_its_condition() {
        for c in Condition::ALL {
            assert_eq!(Condition::from_key(c.key()), Some(c));
        }
        assert_eq!(Condition::from_key("nonsense"), None);
    }
}
