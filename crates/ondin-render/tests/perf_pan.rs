//! What a pan costs against a full frame, on-device, over a document of the
//! caller's: `ONDIN_PERF_DOC=path/to/file.ondin`, at `ONDIN_PERF_ZOOM` (default
//! 0.07) on a 2560×1440 canvas centred on the page's content, walked at level of
//! detail as the canvas is.
//!
//! Prints timings only; nothing is asserted. Run with
//! `cargo test -p ondin-render --release --test perf_pan -- --ignored --nocapture`.

use ondin_core::kurbo::Rect;
use ondin_core::peniko::Color;
use ondin_core::{Document, Resolved};
use ondin_render::gpu::{Region, copy_regions, copy_shifted};
use ondin_render::{ImageStore, RenderOverrides, VelloGpuRenderer, Viewport};
use std::time::Instant;

const SIZE: (u32, u32) = (2560, 1440);

fn texture(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("perf-pan"),
        size: wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// The canvas at `origin` (world `(0, 0)`'s device pixel) and `zoom`.
fn viewport(origin: (i32, i32), zoom: f64) -> Viewport {
    let (x0, y0) = (-f64::from(origin.0) / zoom, -f64::from(origin.1) / zoom);
    Viewport {
        view: Rect::new(
            x0,
            y0,
            x0 + f64::from(SIZE.0) / zoom,
            y0 + f64::from(SIZE.1) / zoom,
        ),
        pixel_size: SIZE,
    }
}

fn wait(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "requires a GPU adapter and ONDIN_PERF_DOC"]
fn a_pan_against_a_full_frame() {
    let Ok(path) = std::env::var("ONDIN_PERF_DOC") else {
        eprintln!("ONDIN_PERF_DOC unset; skipping");
        return;
    };
    let zoom: f64 = std::env::var("ONDIN_PERF_ZOOM")
        .ok()
        .and_then(|z| z.parse().ok())
        .unwrap_or(0.07);
    let doc: Document = ondin_core::io::load(&std::fs::read(&path).expect("read")).expect("load");
    let res = Resolved::rebuild(&doc);
    let page = doc.first_page();
    let mut images = ImageStore::interactive();
    images.prepare(&doc);
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("device");
    let mut renderer = VelloGpuRenderer::new(&device).expect("vello");
    // The canvas always walks at level of detail.
    let mut ov = RenderOverrides::default();
    ov.set_level_of_detail(true);
    let ground = Color::from_rgba8(30, 32, 36, 255);

    let content = res
        .ink_bounds(page)
        .unwrap_or(Rect::new(0.0, 0.0, 1000.0, 1000.0));
    let mut origin = (
        (f64::from(SIZE.0) / 2.0 - content.center().x * zoom).round() as i32,
        (f64::from(SIZE.1) / 2.0 - content.center().y * zoom).round() as i32,
    );
    let (mut shown, mut spare) = (texture(&device), texture(&device));

    let mut full = Vec::new();
    for _ in 0..12 {
        let t = Instant::now();
        renderer
            .render(
                &device,
                &queue,
                &doc,
                &res,
                &viewport(origin, zoom),
                &ov,
                &shown.create_view(&Default::default()),
                ground,
                &images,
                page,
            )
            .expect("render");
        wait(&device);
        full.push(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!("pinned: {}", renderer.pinned());

    let mut pan = Vec::new();
    let mut refused = 0;
    for i in 0..40 {
        let shift = if i % 2 == 0 { (7, -3) } else { (-4, 6) };
        origin = (origin.0 + shift.0, origin.1 + shift.1);
        let vp = viewport(origin, zoom);
        let t = Instant::now();
        let drawn = renderer
            .render_regions(
                &device,
                &queue,
                &doc,
                &res,
                &vp,
                &ov,
                &spare.create_view(&Default::default()),
                &Region::exposed(SIZE, shift),
                ground,
                &images,
                page,
            )
            .expect("regions");
        if drawn {
            copy_shifted(&device, &queue, &shown, &spare, SIZE, shift);
            std::mem::swap(&mut shown, &mut spare);
        } else {
            refused += 1;
        }
        wait(&device);
        pan.push(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!(
        "full frame {:.1} ms, pan frame {:.1} ms (median), {refused} pans refused",
        median(full),
        median(pan)
    );

    // A drag of the top-level item nearest the middle, as the canvas previews it.
    let vp = viewport(origin, zoom);
    let middle = vp.view.center();
    let Some(&dragged) = doc
        .get(page)
        .expect("the page")
        .children()
        .iter()
        .min_by(|a, b| {
            let d = |id: &ondin_core::NodeId| {
                res.ink_bounds(*id)
                    .map_or(f64::INFINITY, |r| (r.center() - middle).hypot())
            };
            d(a).total_cmp(&d(b))
        })
    else {
        return;
    };
    let from = doc.get(dragged).expect("a node").transform();
    let mut before = ov.clone();
    let (mut patch, mut whole, mut unbounded) = (Vec::new(), Vec::new(), 0);
    for i in 1..=30 {
        let tx = ondin_core::Transaction(vec![ondin_core::Operation::SetTransform {
            id: dragged,
            transform: ondin_core::kurbo::Affine::translate((f64::from(i) * 9.0, 0.0)) * from,
        }]);
        let mut after = RenderOverrides::from_transaction(&doc, &res, &tx).expect("a preview");
        after.set_level_of_detail(true);
        let t = Instant::now();
        let Some(damage) = ondin_render::scene::damage(&doc, &res, &vp, page, &before, &after)
        else {
            unbounded += 1;
            continue;
        };
        let regions: Vec<Region> = damage
            .iter()
            .filter_map(|r| Region::covering(*r, 2.0, SIZE))
            .collect();
        let drawn = renderer
            .render_regions(
                &device,
                &queue,
                &doc,
                &res,
                &vp,
                &after,
                &spare.create_view(&Default::default()),
                &regions,
                ground,
                &images,
                page,
            )
            .expect("regions");
        assert!(drawn, "the drag's regions are not pinned");
        copy_regions(&device, &queue, &spare, &shown, &regions);
        wait(&device);
        patch.push(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        renderer
            .render(
                &device,
                &queue,
                &doc,
                &res,
                &vp,
                &after,
                &spare.create_view(&Default::default()),
                ground,
                &images,
                page,
            )
            .expect("render");
        wait(&device);
        whole.push(t.elapsed().as_secs_f64() * 1e3);
        before = after;
    }
    if !patch.is_empty() {
        eprintln!(
            "drag frame {:.1} ms patched, {:.1} ms whole (median), {unbounded} unbounded",
            median(patch),
            median(whole)
        );
    }

    // The same item committed step by step, redrawn as the canvas redraws an
    // edit: its ink before (`Resolved`) and after (`scene::top_level_ink`).
    let (mut doc, mut res) = (doc, res);
    let (mut patch, mut whole) = (Vec::new(), Vec::new());
    for i in 1..=12 {
        let tx = ondin_core::Transaction(vec![ondin_core::Operation::SetTransform {
            id: dragged,
            transform: ondin_core::kurbo::Affine::translate((f64::from(i) * 9.0, 0.0)) * from,
        }]);
        let was = res.ink_bounds(dragged).expect("ink");
        let dirty = doc.apply(&tx).expect("commits").dirty;
        res.update(&doc, &dirty);
        let t = Instant::now();
        let Some(mut damage) =
            ondin_render::scene::top_level_ink(&doc, &res, &vp, &ov, page, &[dragged])
        else {
            eprintln!("the page holds a mask; no commit is patched");
            return;
        };
        damage.push(ondin_render::scene::device_box(&vp, was));
        let regions: Vec<Region> = damage
            .iter()
            .filter_map(|r| Region::covering(*r, 2.0, SIZE))
            .collect();
        let drawn = renderer
            .render_regions(
                &device,
                &queue,
                &doc,
                &res,
                &vp,
                &ov,
                &spare.create_view(&Default::default()),
                &regions,
                ground,
                &images,
                page,
            )
            .expect("regions");
        assert!(drawn, "the commit's regions are not pinned");
        copy_regions(&device, &queue, &spare, &shown, &regions);
        wait(&device);
        patch.push(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        renderer
            .render(
                &device,
                &queue,
                &doc,
                &res,
                &vp,
                &ov,
                &spare.create_view(&Default::default()),
                ground,
                &images,
                page,
            )
            .expect("render");
        wait(&device);
        whole.push(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!(
        "commit {:.1} ms patched, {:.1} ms whole (median)",
        median(patch),
        median(whole)
    );
}
