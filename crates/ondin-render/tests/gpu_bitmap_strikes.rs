//! A ClearType face drawn on the GPU has the ink the CPU backend gives it.
//!
//! Calibri carries monochrome bitmap strikes for screen sizes, which vello draws
//! blank through its emoji path; the canvas draws its outlines instead
//! (`outline_only`). Needs Windows' Calibri; elsewhere it passes without drawing.
//!
//! `#[ignore]` like the rest of the on-device suite. Run with
//! `cargo test -p ondin-render --release --test gpu_bitmap_strikes -- --ignored`.

use ondin_core::kurbo::{Affine, Rect};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, IdSource, NodeKind, Operation, Resolved, TextSizing, TextStyle, Transaction,
};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, VelloGpuRenderer, Viewport};

const W: u32 = 640;
const H: u32 = 80;

/// One line of Calibri at a size its strikes cover.
fn calibri_line() -> (Document, Resolved) {
    let mut ids = IdSource::new(0xCA1D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ids.mint(),
        parent: root,
        index: 0,
        kind: NodeKind::Text {
            content: "The quick brown fox jumps over the lazy dog 1234567890.".into(),
            style: Box::new(TextStyle {
                font_family: "Calibri".into(),
                font_size: 22.0,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: Some(Affine::translate((10.0, 20.0))),
        name: None,
    }]))
    .expect("a text layer");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// The GPU frame as RGBA over white.
fn gpu_frame(doc: &Document, res: &Resolved, vp: &Viewport) -> Option<Vec<u8>> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    let mut renderer = VelloGpuRenderer::new(&device).expect("vello renderer");
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bitmap-strikes"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer
        .render(
            &device,
            &queue,
            doc,
            res,
            vp,
            &RenderOverrides::default(),
            &tex.create_view(&Default::default()),
            Color::WHITE,
            &ImageStore::default(),
            doc.first_page(),
        )
        .expect("render");
    let bpr = (W * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bpr * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
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
    Some(out)
}

/// The GPU's dark pixels over white are within a tenth of the CPU's inked ones.
///
/// flip: `GpuScenePainter::draw_text` drawing `run.font` as it is → fails at
/// "as much ink as the CPU" (most letters blank).
#[test]
#[ignore = "on-device GPU; needs Windows' Calibri"]
fn a_cleartype_face_draws_its_letters_on_the_gpu() {
    let Ok(bytes) = std::fs::read("C:/Windows/Fonts/calibri.ttf") else {
        eprintln!("no Calibri here; nothing to draw");
        return;
    };
    assert_eq!(ondin_core::text::register_fonts(bytes), vec!["Calibri"]);
    let (doc, res) = calibri_line();
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, f64::from(W), f64::from(H)),
        pixel_size: (W, H),
    };
    let (cpu, ..) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &vp,
        &ImageStore::default(),
        doc.first_page(),
    );
    let cpu_ink = cpu.chunks(4).filter(|p| p[3] > 128).count();
    assert!(
        cpu_ink > 1000,
        "the fixture: the CPU draws the line ({cpu_ink})"
    );
    let Some(gpu) = gpu_frame(&doc, &res, &vp) else {
        eprintln!("no adapter; nothing to draw");
        return;
    };
    let gpu_ink = gpu.chunks(4).filter(|p| p[0] < 128).count();
    let ratio = gpu_ink as f64 / cpu_ink as f64;
    assert!(
        (0.9..1.1).contains(&ratio),
        "as much ink as the CPU: {gpu_ink} against {cpu_ink}"
    );
}
