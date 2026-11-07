//! The pixel grid's adaptive ink: each grid pixel is the canvas pixel under it,
//! moved [`STEP`] in CIE L* with its hue kept, so the lattice reads over any fill.
//!
//! Above [`DARKEN_ABOVE`] a pixel darkens, by scaling its linear channels; at or
//! below it lightens, by mixing toward white in linear light. [`adapt_rgba8`] is
//! the specification and [`on_line`] the line rule; [`GridInkPass`] runs both on
//! the GPU over a rendered canvas.

use crate::color::{linear_to_srgb, srgb_to_linear};
use wgpu::{Device, Queue, TextureView};

/// How far a grid pixel moves from the pixel under it, in CIE L*.
pub const STEP: f32 = 10.0;
/// The lightness above which a grid pixel darkens rather than lightens: near the
/// middle, so either way has room for a full [`STEP`].
pub const DARKEN_ABOVE: f32 = 45.0;
/// Rec. 709 relative luminance from linear RGB.
pub(crate) const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// CIE's ε (216/24389) and κ (24389/27), where L* leaves its cube root for a line.
pub(crate) const CIE_EPSILON: f32 = 216.0 / 24389.0;
pub(crate) const CIE_KAPPA: f32 = 24389.0 / 27.0;

/// CIE L* (0–100) of relative luminance `y`.
pub fn lightness(y: f32) -> f32 {
    if y > CIE_EPSILON {
        116.0 * y.cbrt() - 16.0
    } else {
        CIE_KAPPA * y
    }
}

/// [`lightness`]'s inverse: the relative luminance of L* `l`, clamped to 0–1.
pub fn luminance_at(l: f32) -> f32 {
    let l = l.clamp(0.0, 100.0);
    if l > CIE_KAPPA * CIE_EPSILON {
        ((l + 16.0) / 116.0).powi(3)
    } else {
        l / CIE_KAPPA
    }
}

/// The grid's ink over one straight-alpha sRGB pixel; alpha passes through.
pub fn adapt_rgba8([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let lin = [r, g, b].map(|c| srgb_to_linear(f32::from(c) / 255.0));
    let y = LUMA[0] * lin[0] + LUMA[1] * lin[1] + LUMA[2] * lin[2];
    let l = lightness(y);
    let out = if l > DARKEN_ABOVE {
        let scale = luminance_at(l - STEP) / y;
        lin.map(|c| c * scale)
    } else {
        let m = (luminance_at(l + STEP) - y) / (1.0 - y);
        lin.map(|c| c + m * (1.0 - c))
    };
    let [r, g, b] = out.map(|c| (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
    [r, g, b, a]
}

/// Whether texel `at` is a grid line on an axis whose lines lie at `origin +
/// k × pitch` texels: the line rounds to the texel holding its position, a half
/// rounding up. `grid_ink.wgsl` runs this rule; the app's egui grid calls it too.
pub fn on_line(origin: f32, pitch: f32, at: u32) -> bool {
    // Lines are at least `VISIBLE_AT_ZOOM` texels apart, so the nearest index is the only candidate.
    let x = at as f32;
    let k = ((x - origin) / pitch).round_ties_even();
    (origin + k * pitch + 0.5).floor() == x
}

/// Where the lattice falls in a canvas texture, in texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lattice {
    /// Per axis, a line's position; any line serves, so keep it near zero.
    pub origin: [f32; 2],
    /// Per axis, texels from one line to the next.
    pub pitch: [f32; 2],
    /// `x0, y0, x1, y1`, the far edges exclusive: the only texels shaded.
    pub region: [u32; 4],
}

impl Lattice {
    /// The texel columns holding a vertical line, inside the region.
    pub fn columns(&self) -> impl Iterator<Item = u32> + '_ {
        (self.region[0]..self.region[2]).filter(|&x| on_line(self.origin[0], self.pitch[0], x))
    }

    /// The texel rows holding a horizontal line, inside the region.
    pub fn rows(&self) -> impl Iterator<Item = u32> + '_ {
        (self.region[1]..self.region[3]).filter(|&y| on_line(self.origin[1], self.pitch[1], y))
    }
}

/// [`adapt_rgba8`] on the lattice's texels as a compute pass from one canvas
/// texture into another, copying every other texel. Building it compiles the
/// shader, so it is made once.
pub struct GridInkPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    params: wgpu::Buffer,
}

/// The pass's uniform: origin and pitch per axis, then the region.
const PARAMS_BYTES: u64 = 32;

impl GridInkPass {
    pub fn new(device: &Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ondin-grid-ink"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grid_ink.wgsl").into()),
        });
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ondin-grid-ink"),
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
            label: Some("ondin-grid-ink"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("ondin-grid-ink"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("shade"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ondin-grid-ink-params"),
            size: PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            layout,
            pipeline,
            params,
        }
    }

    /// Write `src` into `dst`, both `size` texels, with the lattice's lines in
    /// the adaptive ink. `dst` is an `Rgba8Unorm` texture with `STORAGE_BINDING`.
    pub fn run(
        &self,
        device: &Device,
        queue: &Queue,
        src: &TextureView,
        dst: &TextureView,
        size: (u32, u32),
        lattice: &Lattice,
    ) {
        let mut bytes = Vec::with_capacity(PARAMS_BYTES as usize);
        for v in [
            lattice.origin[0],
            lattice.origin[1],
            lattice.pitch[0],
            lattice.pitch[1],
        ] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in lattice.region {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.params, 0, &bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ondin-grid-ink"),
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
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ondin-grid-ink"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ondin-grid-ink"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        }
        queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(v: u32) -> [u8; 4] {
        [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
    }

    fn l_star([r, g, b, _]: [u8; 4]) -> f32 {
        let lin = [r, g, b].map(|c| srgb_to_linear(f32::from(c) / 255.0));
        lightness(LUMA[0] * lin[0] + LUMA[1] * lin[1] + LUMA[2] * lin[2])
    }

    /// The seven reference fills land on the reference ink within one level, and
    /// each a full step in L* from its fill.
    /// flip: lightening by scaling up instead of mixing toward white → fails at #3d7be0.
    #[test]
    fn each_fill_takes_the_reference_ink() {
        for (fill, want) in [
            (0xffffff, 0xe2e2e2),
            (0xd9d9d9, 0xbdbdbd),
            (0x808080, 0x676767),
            (0x3d7be0, 0x3062b5),
            (0xe06b5a, 0xb95749),
            (0x3fa66b, 0x338958),
            (0x1e1e1e, 0x333333),
        ] {
            let got = adapt_rgba8(hex(fill));
            let want = hex(want);
            for c in 0..3 {
                assert!(
                    got[c].abs_diff(want[c]) <= 1,
                    "#{fill:06x}: got {got:?}, the reference is {want:?}"
                );
            }
            let moved = (l_star(got) - l_star(hex(fill))).abs();
            assert!((moved - STEP).abs() < 0.6, "#{fill:06x} moved {moved} L*");
        }
    }

    /// Alpha passes through, and black lightens rather than dividing by zero.
    #[test]
    fn alpha_passes_through_and_black_lightens() {
        let [r, g, b, a] = adapt_rgba8([0, 0, 0, 77]);
        assert_eq!(a, 77);
        assert!(r == g && g == b && r > 0, "{r},{g},{b}");
    }

    /// L* and its inverse agree across the range, either side of the knee.
    #[test]
    fn lightness_and_luminance_invert_each_other() {
        for l in [0.0f32, 3.0, 8.0, 8.5, 30.0, 45.0, 72.0, 100.0] {
            let back = lightness(luminance_at(l));
            assert!((back - l).abs() < 1e-3, "{l} came back {back}");
        }
    }

    /// A line lands on the texel holding its position and a half rounds up; the
    /// nearest-index search finds lines on either side of zero.
    /// flip: `round_ties_even` → `floor` in `on_line` → fails at "just inside".
    #[test]
    fn a_line_lands_on_the_texel_holding_it() {
        let lattice = Lattice {
            origin: [3.5, 0.2],
            pitch: [8.5, 10.0],
            region: [0, 0, 40, 40],
        };
        // 3.5 → 4, 12 → 12, 20.5 → 21, 29 → 29, 37.5 → 38.
        assert_eq!(
            lattice.columns().collect::<Vec<_>>(),
            [4, 12, 21, 29, 38],
            "columns"
        );
        assert_eq!(lattice.rows().collect::<Vec<_>>(), [0, 10, 20, 30], "rows");
        // Lines a little either side of texel 0's left edge both round into it.
        assert!(on_line(9.6, 10.0, 0), "just outside");
        assert!(on_line(0.4, 10.0, 0), "just inside");
    }

    /// The shader carries `color.rs`'s transfer and this module's constants.
    /// flip: `STEP` set to 12 → fails at "grid_ink.wgsl lacks".
    #[test]
    fn the_shader_carries_the_constants_of_its_specification() {
        let wgsl = include_str!("grid_ink.wgsl");
        let mut lines: Vec<String> = crate::color::wgsl_transfer_lines().into();
        lines.extend([
            format!("const STEP: f32 = {STEP:?};"),
            format!("const DARKEN_ABOVE: f32 = {DARKEN_ABOVE:?};"),
            format!(
                "const LUMA: vec3<f32> = vec3<f32>({:?}, {:?}, {:?});",
                LUMA[0], LUMA[1], LUMA[2]
            ),
            format!("const CIE_EPSILON: f32 = {CIE_EPSILON:?};"),
            format!("const CIE_KAPPA: f32 = {CIE_KAPPA:?};"),
        ]);
        for line in lines {
            assert!(wgsl.contains(&line), "grid_ink.wgsl lacks `{line}`");
        }
    }
}
