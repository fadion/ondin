//! The GPU vision pass (`vision::VisionPass`) held to its CPU specification
//! (`vision::apply`), within a level for a tint or a ghost and two for a blur,
//! whose first pass rounds to 8 bits on both sides.
//!
//! `#[ignore]` like the rest of the on-device suite: CI has no adapter. Run with
//! `cargo test -p ondin-render --test vision -- --ignored`.

use ondin_render::vision::{Condition, Effect, VisionPass, apply};

const W: u32 = 64;
const H: u32 = 48;

/// Every hue at several lightnesses and alphas, so each channel's decode and
/// encode branches are reached.
fn fixture() -> Vec<u8> {
    let mut px = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            px[i] = (x * 4) as u8;
            px[i + 1] = (y * 5) as u8;
            px[i + 2] = ((x * 7 + y * 3) % 256) as u8;
            px[i + 3] = (255 - (y * 2)) as u8;
        }
    }
    px
}

fn texture(device: &wgpu::Device, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vision"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    })
}

fn readback(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let bpr = (W * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        let row = (y * bpr) as usize;
        out.extend_from_slice(&data[row..row + (W * 4) as usize]);
    }
    out
}

/// Inside the region every pixel is the CPU's; outside, every pixel is the
/// source's; under every condition at two severities and two zooms. The blur's
/// scratch texture is let go by the condition after it.
#[test]
#[ignore = "requires a GPU adapter"]
fn the_gpu_pass_matches_its_specification() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("a device");
    let px = fixture();
    let src = texture(
        &device,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &src,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(W * 4),
            rows_per_image: Some(H),
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    let dst = texture(
        &device,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
    );
    let mut pass = VisionPass::new(&device);
    let region = [10, 5, 50, 40];
    for c in Condition::ALL {
        for (severity, scale) in [(0.35, 1.0), (1.0, 2.5)] {
            let effect = c.effect(severity);
            pass.run(
                &device,
                &queue,
                &src.create_view(&Default::default()),
                &dst.create_view(&Default::default()),
                (W, H),
                &effect,
                scale,
                region,
            );
            assert!(
                matches!(effect, Effect::Blur { .. }) || !pass.holds_scratch(),
                "{c:?}: the blur's scratch is let go once nothing blurs"
            );
            let got = readback(&device, &queue, &dst);
            let want = apply(&effect, scale, &px, (W, H), region);
            let slack = match effect {
                Effect::Blur { .. } => 2,
                _ => 1,
            };
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                let (x, y) = ((i / 4) as u32 % W, (i / 4) as u32 / W);
                assert!(
                    g.abs_diff(*w) <= slack,
                    "{c:?} {severity} at ({x}, {y}) channel {}: {g}, the specification says {w}",
                    i % 4
                );
            }
        }
    }
}
