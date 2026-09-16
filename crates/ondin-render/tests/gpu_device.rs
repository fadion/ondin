//! Opportunistic on-device GPU smoke test. `#[ignore]` so it never runs in the
//! default suite (CI has no GPU); run with `cargo test --ignored` on hardware.
//! It proves the vello + wgpu wiring rasterizes the shared scene, **and reads the
//! pixel back**, which is what §4's invariant 7 asks of it: *"a pin-time
//! verification test asserts a known sRGB fill produces the expected output pixel
//! through each backend"* (§15 D627, `[A1-L6-05]`).
//!
//! ⚠️ **This paragraph used to say pixel correctness was "covered
//! deterministically by the CPU tests, which share `scene.rs`", and stop there.**
//! That is true of the **walk** — genuinely one function — and it is not true of
//! the **colour handoff**, which is exactly what a solid fill pins and is exactly
//! where the two backends differ: §15 D339 records, measured, that vello's GPU
//! output is straight alpha where `vello_cpu`'s `Pixmap` is premultiplied. *An
//! excuse that covers the shared half of a seam reads as covering the seam.*
//!
//! ⚠️ **Still `#[ignore]`d**, so this needs `--ignored` on hardware and the
//! default suite is unchanged. That is the same trade as before and is why the
//! invariant is verified *at pin time* rather than continuously; what changed is
//! that running it now asserts something.
//!
//! (Plain backticks in this file — a `tests/` target is its own crate root, has
//! no `deny`, and `cargo doc` never reads it. §15 D319, and D622 for the counts.)

use ondin_core::Brush;
use ondin_core::kurbo::{Rect, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction};
use ondin_render::{RenderOverrides, VelloGpuRenderer, Viewport};

#[test]
#[ignore = "requires a GPU adapter; run with --ignored on hardware"]
fn gpu_renders_fixture_on_device() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no GPU adapter available; skipping on-device test");
        return;
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("request device");

    let mut renderer = VelloGpuRenderer::new(&device).expect("create vello renderer");

    // Simple fixture: a red rect filling a 100x100 artboard.
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: rect,
        fills: vec![Fill {
            brush: Brush::Solid(Color::from_rgba8(220, 30, 40, 255)),
            visible: true,
        }],
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ondin-gpu-test-target"),
        size: wgpu::Extent3d {
            width: 100,
            height: 100,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    renderer
        .render(
            &device,
            &queue,
            &doc,
            &res,
            &vp,
            &RenderOverrides::default(),
            &view,
            Color::WHITE,
            &ondin_render::ImageStore::new(),
        )
        .expect("gpu render should succeed");
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    // 🚨 **The pixel, which invariant 7 promises and this test did not read**
    // (§15 D627, `[A1-L6-05]`). The invariant says *"a pin-time verification test
    // asserts a known sRGB fill produces the expected output pixel **through each
    // backend**"*, and only `ondin-export/tests/png.rs`'s
    // `color_boundary_produces_expected_pixel` did — the CPU one. This test built
    // the fixture, rendered it and asserted `.expect("gpu render should
    // succeed")`: it never mapped the buffer.
    //
    // ⚠️ **The module doc's excuse covers the walk and not the handoff.** It says
    // *"pixel-level correctness is covered deterministically by the CPU tests,
    // which share `scene.rs`"* — true of the **walk**, which is genuinely one
    // function, and not of the **colour handoff**, which is not: §15 D339 records,
    // measured, that vello's GPU output is straight alpha where `vello_cpu`'s
    // `Pixmap` is premultiplied. That is precisely the axis a solid fill pins.
    //
    // **±2 per channel, the same tolerance the CPU boundary test uses**, because
    // the quantity under test is *which colour arrived*, not the last bit of a
    // rasterizer's rounding.
    let px = read_centre_pixel(&device, &queue, &texture);
    let want = [220u8, 30, 40, 255];
    for (i, (got, want)) in px.iter().zip(want.iter()).enumerate() {
        assert!(
            got.abs_diff(*want) <= 2,
            "channel {i}: the GPU backend put {px:?} where the fill is {want:?}"
        );
    }
}

/// The one pixel at the middle of a 100×100 target, as `Rgba8Unorm` bytes.
///
/// **A copy of `gpu_effects.rs`'s `readback` narrowed to one pixel**, and
/// deliberately not shared with it: that one is built around that file's `Gpu`
/// harness and its `SIDE`, and threading this test's loose `device`/`queue`
/// through it would be a refactor of a file this change has no business touching.
/// The 256-byte row alignment is the part worth copying carefully — it is what
/// makes a naive `bytes_per_row` read the wrong row.
fn read_centre_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> [u8; 4] {
    const SIDE: u32 = 100;
    let bpr = (SIDE * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ondin-gpu-test-readback"),
        size: (bpr * SIDE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(SIDE),
            },
        },
        wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
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
    let at = (SIDE / 2 * bpr + SIDE / 2 * 4) as usize;
    let out = [data[at], data[at + 1], data[at + 2], data[at + 3]];
    drop(data);
    buf.unmap();
    out
}
