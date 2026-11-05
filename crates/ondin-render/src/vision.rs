//! Colour-vision simulation: how a design reads to someone with a colour-vision
//! deficiency, as a view of the canvas, never of the document.
//!
//! Each deficiency is a 3×3 matrix in linear light: Machado, Oliveira and
//! Fernandes' (2009) model at full severity for the three dichromacies, and
//! Rec. 709 luminance for achromatopsia. [`simulate_rgba8`] is the
//! specification; [`VisionPass`] runs it on the GPU over a rendered canvas.
//!
//! Its sRGB decode and encode work on pixels already drawn, not on document
//! colours, so they are not the document-to-backend conversion `color` keeps.

use wgpu::{Device, Queue, TextureView};

/// A colour-vision deficiency the canvas can be shown through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Deficiency {
    /// No long-wavelength (red) cones.
    Protanopia,
    /// No medium-wavelength (green) cones — the most common.
    #[default]
    Deuteranopia,
    /// No short-wavelength (blue) cones.
    Tritanopia,
    /// No colour at all: luminance only.
    Achromatopsia,
}

impl Deficiency {
    pub const ALL: [Deficiency; 4] = [
        Deficiency::Protanopia,
        Deficiency::Deuteranopia,
        Deficiency::Tritanopia,
        Deficiency::Achromatopsia,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Deficiency::Protanopia => "Protanopia",
            Deficiency::Deuteranopia => "Deuteranopia",
            Deficiency::Tritanopia => "Tritanopia",
            Deficiency::Achromatopsia => "Achromatopsia",
        }
    }

    /// What it is, in a few plain words.
    pub fn gloss(self) -> &'static str {
        match self {
            Deficiency::Protanopia => "Red-blind",
            Deficiency::Deuteranopia => "Green-blind",
            Deficiency::Tritanopia => "Blue-blind",
            Deficiency::Achromatopsia => "No colour",
        }
    }

    /// The key a preference stores it under.
    pub fn key(self) -> &'static str {
        match self {
            Deficiency::Protanopia => "protanopia",
            Deficiency::Deuteranopia => "deuteranopia",
            Deficiency::Tritanopia => "tritanopia",
            Deficiency::Achromatopsia => "achromatopsia",
        }
    }

    /// The deficiency a preference names; anything else is the default.
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|d| d.key() == key)
            .unwrap_or_default()
    }

    /// The matrix from linear RGB seen to linear RGB simulated, row-major. Each
    /// row sums to one, so white stays white and grey stays grey.
    pub fn matrix(self) -> [[f32; 3]; 3] {
        match self {
            Deficiency::Protanopia => [
                [0.152_286, 1.052_583, -0.204_868],
                [0.114_503, 0.786_281, 0.099_216],
                [-0.003_882, -0.048_116, 1.051_998],
            ],
            Deficiency::Deuteranopia => [
                [0.367_322, 0.860_646, -0.227_968],
                [0.280_085, 0.672_501, 0.047_413],
                [-0.011_820, 0.042_940, 0.968_881],
            ],
            Deficiency::Tritanopia => [
                [1.255_528, -0.076_749, -0.178_779],
                [-0.078_411, 0.930_809, 0.147_602],
                [0.004_733, 0.691_367, 0.303_900],
            ],
            Deficiency::Achromatopsia => [[0.2126, 0.7152, 0.0722]; 3],
        }
    }
}

/// One straight-alpha sRGB pixel as `d` sees it; alpha passes through.
pub fn simulate_rgba8(d: Deficiency, [r, g, b, a]: [u8; 4]) -> [u8; 4] {
    use crate::color::{linear_to_srgb, srgb_to_linear};
    let lin = [r, g, b].map(|c| srgb_to_linear(f32::from(c) / 255.0));
    let m = d.matrix();
    let out = m.map(|row| {
        let v = row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2];
        (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8
    });
    [out[0], out[1], out[2], a]
}

/// [`simulate_rgba8`] as a compute pass from one canvas texture into another,
/// inside a region of it. Building it compiles the shader, so it is made once.
pub struct VisionPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    params: wgpu::Buffer,
}

/// The pass's uniform: three matrix rows padded to four floats, then the region.
const PARAMS_BYTES: u64 = 64;

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
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("ondin-vision"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("simulate"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ondin-vision-params"),
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

    /// Write `src` into `dst`, both `size` device pixels, simulating `d` inside
    /// `region` (`x0, y0, x1, y1`, the far edges exclusive) and copying the rest.
    /// `dst` is an `Rgba8Unorm` texture with `STORAGE_BINDING`.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        device: &Device,
        queue: &Queue,
        src: &TextureView,
        dst: &TextureView,
        size: (u32, u32),
        d: Deficiency,
        region: [u32; 4],
    ) {
        let m = d.matrix();
        let mut bytes = Vec::with_capacity(PARAMS_BYTES as usize);
        for row in m {
            for v in [row[0], row[1], row[2], 0.0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in region {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.params, 0, &bytes);
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
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ondin-vision"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ondin-vision"),
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

    /// White, black and greys stay as they are under every deficiency: each row
    /// of each matrix sums to one.
    /// flip: a protanopia row off by 0.1 → fails at the greys.
    #[test]
    fn greys_stay_grey() {
        for d in Deficiency::ALL {
            for v in [0u8, 64, 128, 200, 255] {
                let [r, g, b, a] = simulate_rgba8(d, [v, v, v, 77]);
                for c in [r, g, b] {
                    assert!(c.abs_diff(v) <= 1, "{d:?} turned {v} into {r},{g},{b}");
                }
                assert_eq!(a, 77);
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
        let through = |d| gap(simulate_rgba8(d, red), simulate_rgba8(d, green));
        for d in [Deficiency::Protanopia, Deficiency::Deuteranopia] {
            assert!(through(d) < seen * 0.6, "{d:?}: {} of {seen}", through(d));
            assert!(through(d) < through(Deficiency::Tritanopia) * 0.7, "{d:?}");
        }
        let [r, g, b, _] = simulate_rgba8(Deficiency::Achromatopsia, red);
        assert!(r == g && g == b);
    }

    /// The shader's transfer functions carry `color.rs`'s constants, which the
    /// default gates can check without a GPU; mid-grey decodes to 0.214.
    /// flip: `color::SRGB_KNEE` set to 0.04 → fails at "vision.wgsl lacks".
    #[test]
    fn the_shader_transfers_as_color_rs_does() {
        use crate::color::{
            LINEAR_KNEE, SRGB_GAMMA, SRGB_KNEE, SRGB_OFFSET, SRGB_SCALE, SRGB_SLOPE,
        };
        let wgsl = include_str!("vision.wgsl");
        for line in [
            format!("if c <= {SRGB_KNEE} {{"),
            format!("return c / {SRGB_SLOPE};"),
            format!("return pow((c + {SRGB_OFFSET}) / {SRGB_SCALE}, {SRGB_GAMMA});"),
            format!("if c <= {LINEAR_KNEE} {{"),
            format!("return c * {SRGB_SLOPE};"),
            format!("return {SRGB_SCALE} * pow(c, 1.0 / {SRGB_GAMMA}) - {SRGB_OFFSET};"),
        ] {
            assert!(wgsl.contains(&line), "vision.wgsl lacks `{line}`");
        }
        let mid = crate::color::srgb_to_linear(0.5);
        assert!((mid - 0.214).abs() < 1e-3, "{mid}");
        assert!((crate::color::linear_to_srgb(mid) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_preference_key_names_its_deficiency() {
        for d in Deficiency::ALL {
            assert_eq!(Deficiency::from_key(d.key()), d);
        }
        assert_eq!(Deficiency::from_key("nonsense"), Deficiency::default());
    }
}
