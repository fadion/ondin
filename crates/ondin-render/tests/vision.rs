//! The GPU colour-vision pass (`vision::VisionPass`) held to its CPU
//! specification (`vision::simulate_rgba8`), byte for byte within one level.
//!
//! `#[ignore]` like the rest of the on-device suite: CI has no adapter. Run with
//! `cargo test -p ondin-render --test vision -- --ignored`.

use ondin_render::vision::{Deficiency, VisionPass, simulate_rgba8};

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

/// Inside the region every pixel is the CPU's within a level; outside, every
/// pixel is the source's exactly; under every deficiency.
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
    let pass = VisionPass::new(&device);
    let region = [10, 5, 50, 40];
    for d in Deficiency::ALL {
        pass.run(
            &device,
            &queue,
            &src.create_view(&Default::default()),
            &dst.create_view(&Default::default()),
            (W, H),
            d,
            region,
        );
        let out = readback(&device, &queue, &dst);
        for y in 0..H {
            for x in 0..W {
                let i = ((y * W + x) * 4) as usize;
                let seen = [px[i], px[i + 1], px[i + 2], px[i + 3]];
                let got = [out[i], out[i + 1], out[i + 2], out[i + 3]];
                let inside = x >= region[0] && y >= region[1] && x < region[2] && y < region[3];
                let want = if inside {
                    simulate_rgba8(d, seen)
                } else {
                    seen
                };
                for c in 0..4 {
                    assert!(
                        got[c].abs_diff(want[c]) <= 1,
                        "{d:?} at ({x}, {y}): {got:?}, the specification says {want:?}"
                    );
                }
            }
        }
    }
}
