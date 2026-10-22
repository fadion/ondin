//! Effects through the **whole GPU path**, on-device: document → scene walk →
//! sub-scene → vello raster → compute passes → atlas → page (§6.4, §15 D339).
//!
//! `tests/fx_gpu.rs` proves the passes match the CPU reference over a buffer this
//! file never builds; `tests/effects.rs` proves the CPU path draws the right
//! picture. This file proves the wiring: the walk opens a layer, the sub-scene
//! rasterizes into it, and the filtered texture lands back in the page.
//!
//! `#[ignore]` like the rest of the on-device suite: CI has no adapter. Run with
//! `cargo test -p ondin-render --test gpu_effects -- --ignored`.

use ondin_core::kurbo::{Affine, Rect, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeKind, Operation, Resolved, Shadow,
    Transaction, keyed_by_position,
};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, VelloGpuRenderer, Viewport};

const SIDE: u32 = 100;

fn viewport() -> Viewport {
    Viewport {
        view: Rect::new(0.0, 0.0, SIDE as f64, SIDE as f64),
        pixel_size: (SIDE, SIDE),
    }
}

/// The same fixture `tests/effects.rs` uses: a white 40×40 square at (30, 30).
fn document(effects: Vec<Effect>, opacity: f32) -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: r,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((30.0, 30.0))),
            name: None,
        },
        Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id: r,
            effects: keyed_by_position(effects),
        },
    ]))
    .unwrap();
    if opacity < 1.0 {
        doc.apply(&Transaction(vec![Operation::SetOpacity { id: r, opacity }]))
            .unwrap();
    }
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: VelloGpuRenderer,
    /// Held for the test's life: devices made side by side hung the suite
    /// intermittently, so the tests take the adapter one at a time.
    _one_at_a_time: Serial,
}

/// The lock every test here holds while it has a device.
static ADAPTER: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// This thread's hold on [`ADAPTER`] and how many devices share it — a test
    /// that makes a second device to compare against must not wait on itself.
    static HELD: std::cell::RefCell<(usize, Option<std::sync::MutexGuard<'static, ()>>)> =
        const { std::cell::RefCell::new((0, None)) };
}

/// One device's share of this thread's hold on [`ADAPTER`].
struct Serial;

impl Serial {
    fn take() -> Self {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if h.0 == 0 {
                // A test that panicked holding it poisons nothing worth refusing.
                h.1 = Some(ADAPTER.lock().unwrap_or_else(|e| e.into_inner()));
            }
            h.0 += 1;
        });
        Serial
    }
}

impl Drop for Serial {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            h.0 -= 1;
            if h.0 == 0 {
                h.1 = None;
            }
        });
    }
}

fn gpu() -> Option<Gpu> {
    let one_at_a_time = Serial::take();
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    let renderer = VelloGpuRenderer::new(&device).expect("vello renderer");
    Some(Gpu {
        device,
        queue,
        renderer,
        _one_at_a_time: one_at_a_time,
    })
}

/// Render through the GPU backend and read the pixels back, straight alpha —
/// the same convention `VelloCpuRenderer::render_to_rgba` returns, so the two can
/// be compared without a conversion in between.
fn render_gpu(g: &mut Gpu, doc: &Document, res: &Resolved) -> Vec<u8> {
    render_gpu_vp(g, doc, res, &viewport())
}

/// `render_gpu` at a viewport of the caller's choosing — for the tests that pan.
fn render_gpu_vp(g: &mut Gpu, doc: &Document, res: &Resolved, vp: &Viewport) -> Vec<u8> {
    render_gpu_store(g, doc, res, &ImageStore::default(), vp)
}

/// `render_gpu_vp` with a picture store — for the tests that draw one.
fn render_gpu_store(
    g: &mut Gpu,
    doc: &Document,
    res: &Resolved,
    store: &ImageStore,
    vp: &Viewport,
) -> Vec<u8> {
    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-effects-target"),
        size: wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    g.renderer
        .render(
            &g.device,
            &g.queue,
            doc,
            res,
            vp,
            &RenderOverrides::default(),
            &view,
            Color::TRANSPARENT,
            store,
        )
        .expect("render");
    let bpr = (SIDE * 4).div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * SIDE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
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
                rows_per_image: Some(SIDE),
            },
        },
        wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    g.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        let row = (y * bpr) as usize;
        out.extend_from_slice(&data[row..row + (SIDE * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

fn render_cpu(doc: &Document, res: &Resolved) -> Vec<u8> {
    VelloCpuRenderer::new()
        .render_to_rgba(doc, res, &viewport(), &ImageStore::default())
        .0
}

fn at(px: &[u8], x: usize, y: usize) -> (u8, u8, u8, u8) {
    let p = (y * SIDE as usize + x) * 4;
    (px[p], px[p + 1], px[p + 2], px[p + 3])
}

fn red_shadow(dy: f64, blur: f64) -> Effect {
    Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, dy),
        blur,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))
}

/// The canvas draws the shadow: it exists, is its own colour rather than the
/// layer's, and is on the side the offset points at (the CPU test's claims, asked
/// of this backend).
#[test]
#[ignore = "requires a GPU adapter"]
fn the_canvas_draws_a_drop_shadow() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (doc, res) = document(vec![red_shadow(12.0, 0.0)], 1.0);
    let px = render_gpu(&mut g, &doc, &res);

    let below = at(&px, 50, 78);
    assert!(
        below.0 > 200 && below.1 < 40 && below.2 < 40 && below.3 > 200,
        "the shadow is drawn below the square, in red: {below:?}"
    );
    let above = at(&px, 50, 35);
    assert_eq!(
        (above.0, above.1, above.2),
        (255, 255, 255),
        "the square itself is untouched white: {above:?}"
    );
    let over_the_top = at(&px, 50, 20);
    assert_eq!(
        over_the_top.3, 0,
        "and nothing is cast on the side the offset leaves: {over_the_top:?}"
    );
}

/// `n` photographs of `w`×`h`, each on its own 25 × 25 rect, laid out along the
/// top of the page.
fn photographs(n: usize, w: u32, h: u32) -> (Document, Resolved, ImageStore) {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let mut store = ImageStore::new();
    let mut ids = ondin_core::IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    for i in 0..n {
        // A distinct id per picture is the whole fixture: vello keys atlas
        // residency on the blob's id, so copies of one photograph are one atlas
        // entry and fit trivially.
        let iid = ImageId(format!("sha256:photo-{i}"));
        let bytes = photo_png(w, h, i as u8);
        store
            .insert(&iid, &bytes, ImageFormat::Png)
            .expect("the fixture decodes");
        let node = ids.mint();
        ops.push(Operation::AddImage {
            id: iid.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format: ImageFormat::Png,
                width: w,
                height: h,
            },
        });
        ops.push(Operation::CreateNode {
            id: node,
            parent: root,
            index: i,
            kind: NodeKind::Rect {
                size: Size::new(25.0, 25.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((5.0 + 30.0 * i as f64, 20.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: node,
            fills: keyed_by_position([Fill {
                brush: image_brush(iid).into(),
                visible: true,
            }]),
        });
    }
    doc.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&doc);
    store.prepare(&doc);
    (doc, res, store)
}

/// An opaque `w`×`h` PNG in a colour of its own, so a missing one is missing
/// rather than hidden behind an identical neighbour.
fn photo_png(w: u32, h: u32, tint: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut e = enc.write_header().expect("png header");
        let px: Vec<u8> = (0..w * h)
            .flat_map(|_| [200u8, 60 + tint * 40, 60, 255])
            .collect();
        e.write_image_data(&px).expect("png data");
    }
    out
}

/// Three ordinary photographs on one page all draw (§15 D745).
///
/// vello keeps every picture in one 8192-pixel sheet and silently leaves the one
/// that will not fit undrawn. Three 4500 × 3000 photographs need three shelves of
/// 3000, so the page's pictures are shrunk by one integer factor (2 here) until the
/// set fits, and full resolution is kept while it fits.
///
/// The two-photograph row is the anti-vacuity control and the more important
/// assertion: a budget that shrank two photographs would pass the three-photograph
/// one and be wrong, so the factor must be exactly 1 there.
///
/// flip: `atlas_pixels` returning `pixels` unchanged → fails at the opaque-pixel
/// count (`left: 1250, right: 1875`). Making `atlas_downscale` return 1 fails
/// earlier, at the factor assertion, and says nothing about the drawing.
#[test]
#[ignore = "requires a GPU adapter"]
fn three_photographs_on_one_page_all_draw() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (doc, res, store) = photographs(3, 4500, 3000);
    assert_eq!(
        store.atlas_factor(),
        2,
        "three of these do not fit at full resolution, so the budget must bite"
    );
    let px = render_gpu_store(&mut g, &doc, &res, &store, &viewport());
    let opaque = px.as_chunks::<4>().0.iter().filter(|p| p[3] > 200).count();
    assert_eq!(
        opaque, 1875,
        "three 25×25 rects at 625 pixels each — this was 1250 before"
    );

    // The control: two of them fit, so nothing is touched.
    let (doc2, res2, store2) = photographs(2, 4500, 3000);
    assert_eq!(
        store2.atlas_factor(),
        1,
        "two fit at full resolution and must be left alone"
    );
    let px2 = render_gpu_store(&mut g, &doc2, &res2, &store2, &viewport());
    assert_eq!(
        px2.as_chunks::<4>().0.iter().filter(|p| p[3] > 200).count(),
        1250,
        "and both of them draw"
    );
}

/// A long thin banner, the shape whose effect buffer goes past the texture limit
/// first.
fn banner(effects: Vec<Effect>) -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: r,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(1600.0, 12.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((0.0, 0.0))),
            name: None,
        },
        Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id: r,
            effects: keyed_by_position(effects),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// A layer whose effect buffer needs more than the texture limit still draws
/// (§15 D744).
///
/// The fixture must be wide and thin: `effect::grow_to_escape` grows nothing on
/// the side a large offset points at, and `buffer_box_within` trims a big square
/// layer by area long before a side reaches 8192. A 1600 × 12 banner at 6× yields
/// a 9672 × 144 buffer, 1480 past the limit and inside the area budget.
///
/// It asserts a frame that completes with ink in it, not exact pixels (the layer
/// is resampled at this size). A panic allocating the scratch is no frame at all.
///
/// flip: restoring `.min(u16::MAX as f64)` in `push_effect_layer` → the process
/// aborts in wgpu validation, so the suite reports a crash rather than a red test.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_layer_needing_more_than_the_texture_limit_still_draws() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (doc, res) = banner(vec![Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, 2.0),
        blur: 4.0,
        spread: 0.0,
        color: Color::from_rgba8(255, 0, 0, 255),
    }))]);
    // 6×: 1600 units across 9600 pixels, and 12 units of banner is 72 down.
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 1600.0, 40.0),
        pixel_size: (9600, 240),
    };
    let px = render_gpu_vp(&mut g, &doc, &res, &vp);

    assert_eq!(px.len(), (SIDE * SIDE * 4) as usize, "a frame came back");
    assert!(
        px.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
        "and it has ink in it"
    );
}

/// An inner shadow on a rect reaches all four edges.
///
/// `tests/fx_gpu.rs` compares each pass against the CPU reference, which cannot
/// see an error both implementations share (both once inverted the silhouette
/// before the passes that read past the buffer's edge), so the picture is asserted
/// absolutely first and the agreement second. The fixture is a rect, whose ink is
/// exactly its bounding box, at the offset the panel starts every shadow on.
///
/// flip: `silhouette`'s inversion back in `fx.wgsl`, plus `inner` back on the
/// silhouette's `Params` and off the tint's (the shader edit alone is a no-op,
/// since `fx_gpu::run` sets `INNER` on `Pass::Tint`) → fails at the first edge
/// assertion; the backend comparison is the control.
#[test]
#[ignore = "requires a GPU adapter"]
fn the_canvas_draws_an_inner_shadow_on_a_rect() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (doc, res) = document(
        vec![Effect::new(EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(0.0, 0.0),
            blur: 8.0,
            spread: 0.0,
            color: Color::from_rgba8(255, 0, 0, 255),
        }))],
        1.0,
    );
    let px = render_gpu(&mut g, &doc, &res);

    for (x, y, side) in [
        (50, 31, "top"),
        (50, 69, "bottom"),
        (31, 50, "left"),
        (69, 50, "right"),
    ] {
        let p = at(&px, x, y);
        assert!(
            p.0 > 200 && p.1 < 200 && p.2 < 200,
            "the {side} edge is tinted by the shadow: {p:?}"
        );
    }
    assert_eq!(
        at(&px, 50, 50),
        (255, 255, 255, 255),
        "and the middle is untouched"
    );
    assert_eq!(
        px,
        render_cpu(&doc, &res),
        "and the two backends draw it identically"
    );
}

/// The same document through both backends agrees byte for byte (§3: one walk,
/// two backends), asked of the feature with a separate implementation on each.
///
/// Exact because of the fixture: the square sits on pixel boundaries in a 1:1
/// viewport, so neither rasterizer antialiases and the effect arithmetic is the
/// only thing that can differ. It does not claim agreement on any document.
#[test]
#[ignore = "requires a GPU adapter"]
fn both_backends_draw_the_same_shadow() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (doc, res) = document(vec![red_shadow(10.0, 6.0)], 1.0);
    let gpu_px = render_gpu(&mut g, &doc, &res);
    let cpu_px = render_cpu(&doc, &res);

    // The shadow must be there, or two empty buffers would agree and prove nothing.
    let body = at(&gpu_px, 50, 78);
    assert!(
        body.0 > 100 && body.3 > 100,
        "the fixture really does draw a shadow at the sample point: {body:?}"
    );

    if gpu_px != cpu_px {
        let mut total = 0u64;
        let mut worst = (0u8, 0usize);
        for (i, (a, b)) in gpu_px.iter().zip(&cpu_px).enumerate() {
            let d = a.abs_diff(*b);
            total += u64::from(d);
            if d > worst.0 {
                worst = (d, i);
            }
        }
        let p = worst.1 / 4;
        panic!(
            "the two backends disagree: mean {:.3} per channel, worst {} at              ({}, {}) — gpu {:?} vs cpu {:?}",
            total as f64 / gpu_px.len() as f64,
            worst.0,
            p % SIDE as usize,
            p / SIDE as usize,
            at(&gpu_px, p % SIDE as usize, p / SIDE as usize),
            at(&cpu_px, p % SIDE as usize, p / SIDE as usize),
        );
    }
}

/// A shadow and an opacity together: the pairing that panicked the CPU backend
/// (§15 D338). The GPU painter counts ordinary layers for the same reason.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_faded_layer_still_casts_its_shadow() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (doc, res) = document(vec![red_shadow(12.0, 0.0)], 0.5);
    let px = render_gpu(&mut g, &doc, &res);
    let below = at(&px, 50, 78);
    assert!(
        below.0 > 200 && below.1 < 40 && below.3 > 90 && below.3 < 170,
        "the shadow is drawn and is faded with the layer: {below:?}"
    );
    let bare = at(&px, 50, 35);
    assert!(
        (100..=160).contains(&bare.3),
        "and the square is half there: {bare:?}"
    );
}

/// A stack with nothing to draw leaves the picture alone: the walk opens no layer
/// for it (`effects::any_ink`), so no atlas slot, texture or blit is made.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_hidden_shadow_draws_nothing_extra() {
    let Some(mut g) = gpu() else {
        return;
    };
    let hidden = Effect {
        kind: EffectKind::DropShadow(Shadow {
            offset: Vec2::new(0.0, 12.0),
            blur: 0.0,
            spread: 0.0,
            color: Color::from_rgba8(255, 0, 0, 255),
        }),
        visible: false,
        vars: Default::default(),
    };
    let (with, res_with) = document(vec![hidden], 1.0);
    let (without, res_without) = document(vec![], 1.0);
    let a = render_gpu(&mut g, &with, &res_with);
    let b = render_gpu(&mut g, &without, &res_without);
    assert_eq!(a, b, "a hidden effect changes no pixel");
}

/// The canvas half of `effects::a_layer_scrolled_off_the_top_still_casts_into_view`
/// (§15 D342). Both backends reach `effect::buffer_box` through their own
/// `push_effect_layer`, so a fix applied to one only would pass there and fail here.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_layer_scrolled_off_the_top_still_casts_into_view() {
    let Some(mut g) = gpu() else {
        return;
    };
    // The square is world y 30…70; the shadow is 20 down with a 6 blur, so its ink
    // runs well past the square's own box.
    let (doc, res) = document(
        vec![Effect::new(EffectKind::DropShadow(Shadow {
            offset: Vec2::new(0.0, 20.0),
            blur: 6.0,
            spread: 0.0,
            color: Color::from_rgba8(255, 0, 0, 255),
        }))],
        1.0,
    );
    // Panned so the square itself is entirely above the view.
    let vp = Viewport {
        view: Rect::new(0.0, 80.0, SIDE as f64, SIDE as f64 + 80.0),
        pixel_size: (SIDE, SIDE),
    };
    let px = render_gpu_vp(&mut g, &doc, &res, &vp);
    let top = at(&px, 50, 1);
    assert!(
        top.3 > 0 && top.0 > 100 && top.1 < 60,
        "the shadow of an off-screen layer still reaches into view, in its own          red: {top:?}"
    );
    // And it is a shadow rather than the layer having been dragged into view.
    let deep = at(&px, 50, 40);
    assert_eq!(deep.3, 0, "with nothing further down: {deep:?}");
}

/// A sibling cut by the view's edge draws nothing into the slot packed beside it
/// (§15 D990).
///
/// Sibling effect layers share one batch texture, one slot each; a layer's buffer
/// is trimmed to the part in view but its sub-scene is not, so a layer hanging off
/// the left edge drew at negative x into the slot packed before it. The CPU backend
/// gives every layer its own buffer, so only the canvas showed it.
///
/// A white square `a` (packed first) sits in view and a red bar `b` hangs 40 units
/// off the left edge; both carry a small shadow.
///
/// flip: appending the sub-scenes without the slot clip → paints `a` red at its
/// centre.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_sibling_cut_by_the_view_draws_nothing_into_its_neighbour() {
    let Some(mut g) = gpu() else {
        return;
    };
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [a, b] = [(); 2].map(|_| ids.mint());
    let rect = |id, index, w: f64, x: f64, c: Color| {
        [
            Operation::CreateNode {
                id,
                parent: root,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(w, 30.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((x, 30.0))),
                name: None,
            },
            Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(c).into(),
                    visible: true,
                }]),
            },
            Operation::SetEffects {
                id,
                effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 2.0),
                    blur: 0.0,
                    spread: 0.0,
                    color: Color::from_rgba8(0, 0, 0, 255),
                }))]),
            },
        ]
    };
    let mut ops = Vec::new();
    ops.extend(rect(a, 0, 30.0, 60.0, Color::WHITE));
    ops.extend(rect(b, 1, 60.0, -40.0, Color::from_rgba8(255, 0, 0, 255)));
    doc.apply(&Transaction(ops)).unwrap();
    let res = Resolved::rebuild(&doc);
    let px = render_gpu(&mut g, &doc, &res);
    assert_eq!(
        at(&px, 75, 45),
        (255, 255, 255, 255),
        "the square is its own white, not its neighbour's red"
    );
    let bar = at(&px, 10, 45);
    assert!(
        bar.0 > 200 && bar.1 < 40,
        "and the bar still draws, in view: {bar:?}"
    );
}

/// An image fill keeps its pixels while effect layers are on the page (§15 D344).
///
/// Every effect layer costs the page renderer an extra `render_to_texture`, and
/// vello's image atlas evicts a resident image not drawn for two of those passes
/// (`EVICT_AFTER_GENERATIONS`); a picture drawn once per frame came back empty
/// from the second frame on.
///
/// Six frames, not one: the first is always right because the picture is new to
/// the atlas.
#[test]
#[ignore = "requires a GPU adapter"]
fn an_image_survives_effect_layers_on_the_page() {
    let Some(mut g) = gpu() else {
        return;
    };
    for effects in [0usize, 1, 2, 4] {
        let (doc, res, store) = picture_beside_effects(effects);
        for frame in 0..6 {
            let px = render_gpu_store(&mut g, &doc, &res, &store, &viewport());
            let mid = at(&px, 25, 25);
            assert_eq!(
                mid,
                (255, 0, 0, 255),
                "{effects} effect layer(s), frame {frame}: the picture is empty"
            );
        }
    }
}

/// A picture inside an effected layer keeps its pixels too: those sub-scenes are
/// rendered on their own renderer, whose atlas has the same eviction rhythm.
#[test]
#[ignore = "requires a GPU adapter"]
fn an_image_inside_an_effect_layer_survives() {
    let Some(mut g) = gpu() else {
        return;
    };
    for others in [0usize, 1, 3] {
        let (doc, res, store) = picture_inside_an_effect_among_others(others);
        for frame in 0..6 {
            let px = render_gpu_store(&mut g, &doc, &res, &store, &viewport());
            let mid = at(&px, 25, 25);
            assert!(
                mid.0 > 200 && mid.3 > 200,
                "{others} other effect layer(s), frame {frame}: the blurred picture                  is empty: {mid:?}"
            );
        }
    }
    let _ = picture_inside_an_effect;
}

/// An effect layer inside an effect layer reaches the page (§15 D404). A nested
/// layer's result is drawn by its parent's sub-scene, rasterized by a layer
/// renderer with an atlas of its own, so `override_image` must reach that renderer
/// and not only the page's.
///
/// The shadow-only and blur-only fixtures are controls: the bug needs one layer to
/// contain another. The comparison is against the CPU backend, which has no atlas,
/// and is exact for the reason `both_backends_draw_the_same_shadow` gives, except
/// three deep, which allows 1: every GPU effect layer is bracketed by a
/// straight→premultiplied→straight round trip (`fx_gpu`'s module docs) that is not
/// the identity at eight bits, the same ±1 `tests/fx_gpu.rs` allows.
///
/// flip: registering every slot on the page renderer → panics inside `render`
/// (vello refuses to draw a slot it has no pixels for) before any comparison; the
/// comparison would catch a slot registered on the wrong renderer.
#[test]
#[ignore = "requires a GPU adapter"]
fn an_effect_layer_inside_an_effect_layer_reaches_the_page() {
    let Some(mut g) = gpu() else {
        return;
    };
    let blur = || vec![Effect::new(EffectKind::LayerBlur { radius: 3.0 })];
    let shadow = || vec![red_shadow(6.0, 4.0)];
    let none = Vec::new();
    for (label, stacks, tolerance) in [
        (
            "a shadow on the outer group",
            vec![shadow(), none.clone()],
            0,
        ),
        ("a blur on the inner group", vec![none.clone(), blur()], 0),
        ("a blur inside a shadow", vec![shadow(), blur()], 0),
        (
            "three deep: a blur inside a shadow inside a blur",
            vec![blur(), shadow(), blur()],
            1,
        ),
    ] {
        let (doc, res) = nested(stacks);
        let gpu_px = render_gpu(&mut g, &doc, &res);
        let cpu_px = render_cpu(&doc, &res);
        // Two empty buffers would agree perfectly.
        let body = at(&gpu_px, 50, 50);
        assert!(
            body.3 > 200,
            "{label}: the fixture draws nothing at all: {body:?}"
        );
        let worst = gpu_px
            .iter()
            .zip(&cpu_px)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= tolerance,
            "{label}: the two backends disagree by {worst}, over a tolerance of {tolerance}"
        );
    }
}

/// A white 40×40 square at (30, 30) wrapped in one group per entry of `stacks`,
/// outermost first, each carrying that entry's effects.
///
/// Nesting depth is the parameter because the rule that a layer's slot goes to the
/// renderers of the level above (§15 D404) is indistinguishable from "the page"
/// in a two-level fixture.
fn nested(stacks: Vec<Vec<Effect>>) -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    let mut parent = root;
    let mut groups = Vec::new();
    for _ in &stacks {
        let g = ids.mint();
        ops.push(Operation::CreateNode {
            id: g,
            parent,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        groups.push(g);
        parent = g;
    }
    let r = ids.mint();
    ops.push(Operation::CreateNode {
        id: r,
        parent,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(40.0, 40.0),
            corner_radii: Default::default(),
        },
        transform: Some(Affine::translate((30.0, 30.0))),
        name: None,
    });
    ops.push(Operation::SetFills {
        id: r,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::WHITE).into(),
            visible: true,
        }]),
    });
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let ops: Vec<Operation> = groups
        .into_iter()
        .zip(stacks)
        .filter(|(_, fx)| !fx.is_empty())
        .map(|(id, effects)| Operation::SetEffects {
            id,
            effects: keyed_by_position(effects),
        })
        .collect();
    if !ops.is_empty() {
        doc.apply(&Transaction(ops)).expect("the effects apply");
    }
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// `n` effect layers at **one** level, each a blurred square of `size`, all of
/// them on the page.
///
/// The counterpart to `nested`, and the case `pack`'s page-area budget is about.
/// The squares overlap on purpose: the packer weighs the sum of the layers' buffer
/// areas against the page's pixel count, not how much of the page they cover.
fn siblings(n: usize, size: f64) -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    let mut groups = Vec::new();
    for i in 0..n {
        let g = ids.mint();
        ops.push(Operation::CreateNode {
            id: g,
            parent: root,
            index: i,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        let r = ids.mint();
        ops.push(Operation::CreateNode {
            id: r,
            parent: g,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(size, size),
                corner_radii: Default::default(),
            },
            // Staggered so no two layers resolve to the identical box.
            transform: Some(Affine::translate((
                4.0 + (i % 4) as f64 * 2.0,
                4.0 + (i / 4) as f64 * 2.0,
            ))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        });
        groups.push(g);
    }
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let ops: Vec<Operation> = groups
        .into_iter()
        .map(|id| Operation::SetEffects {
            id,
            effects: keyed_by_position([Effect::new(EffectKind::LayerBlur { radius: 3.0 })]),
        })
        .collect();
    doc.apply(&Transaction(ops)).expect("the effects apply");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// A 30×30 square filled with a solid red picture at (10, 10), and `n` small
/// blurred squares beside it.
fn picture_beside_effects(n: usize) -> (Document, Resolved, ImageStore) {
    build_picture_doc(n, false)
}

/// The same picture, with the blur on the picture's own layer.
fn picture_inside_an_effect() -> (Document, Resolved, ImageStore) {
    build_picture_doc(0, true)
}

/// The picture inside its own effect layer and other effect layers beside it, so
/// the layers renderer does more than one pass a frame.
fn picture_inside_an_effect_among_others(n: usize) -> (Document, Resolved, ImageStore) {
    build_picture_doc(n, true)
}

fn build_picture_doc(others: usize, on_the_picture: bool) -> (Document, Resolved, ImageStore) {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let mut store = ImageStore::new();
    let iid = ImageId("sha256:probe".into());
    let bytes = solid_png(PICTURE);
    store
        .insert(&iid, &bytes, ImageFormat::Png)
        .expect("the fixture decodes");

    let mut ids = ondin_core::IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let pic = ids.mint();
    let mut ops = vec![
        Operation::AddImage {
            id: iid.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(bytes.clone().into()),
                format: ImageFormat::Png,
                width: PICTURE,
                height: PICTURE,
            },
        },
        Operation::CreateNode {
            id: pic,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(30.0, 30.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
        Operation::SetFills {
            id: pic,
            fills: keyed_by_position([Fill {
                brush: image_brush(iid).into(),
                visible: true,
            }]),
        },
    ];
    if on_the_picture {
        ops.push(Operation::SetEffects {
            id: pic,
            effects: keyed_by_position([Effect::new(EffectKind::LayerBlur { radius: 3.0 })]),
        });
    }
    for i in 0..others {
        let other = ids.mint();
        ops.push(Operation::CreateNode {
            id: other,
            parent: root,
            index: i + 1,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(18.0, 18.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((60.0, 6.0 + 22.0 * i as f64))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: other,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        });
        ops.push(Operation::SetEffects {
            id: other,
            effects: keyed_by_position([Effect::new(EffectKind::LayerBlur { radius: 4.0 })]),
        });
    }
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    (doc, res, store)
}

/// The picture's side, large enough to matter to the atlas (vello's starts at 1024
/// square).
const PICTURE: u32 = 512;

fn solid_png(side: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        let px: Vec<u8> = (0..side * side).flat_map(|_| [255u8, 0, 0, 255]).collect();
        w.write_image_data(&px).expect("png data");
    }
    out
}

/// How many effect passes a nested document demands (§15 D763): prints
/// `VelloGpuRenderer::pass_high_water` for each fixture. Each pass holds one vello
/// renderer, whose build cost is measured by
/// `a_vello_renderer_costs_about_six_milliseconds_to_build`. It asserts only the
/// floor: a document with effect layers demands at least one pass.
///
/// For a nested document the pass count is the nesting depth, one per level. That
/// is not the unbounded case: many effect layers at one level whose combined area
/// exceeds the page make `pack` start further chunks (see
/// `how_many_effect_passes_a_level_of_siblings_demands`), so do not read the depth
/// figure as the bound.
///
/// Run with `cargo test -p ondin-render --test gpu_effects --release -- --ignored
/// --nocapture` to see the number.
#[test]
#[ignore = "needs a GPU adapter"]
fn how_many_effect_passes_a_nested_document_demands() {
    let Some(mut g) = gpu() else {
        return;
    };
    let blur = || vec![Effect::new(EffectKind::LayerBlur { radius: 3.0 })];
    let shadow = || vec![red_shadow(6.0, 4.0)];

    for (label, stacks) in [
        ("one blur", vec![blur()]),
        ("a blur inside a shadow", vec![shadow(), blur()]),
        (
            "three deep: a blur inside a shadow inside a blur",
            vec![blur(), shadow(), blur()],
        ),
    ] {
        let (doc, res) = nested(stacks);
        let _ = render_gpu(&mut g, &doc, &res);
        println!(
            "[S11.2-L4-05] K after {label}: {} pass(es) — {} vello Renderer(s) held",
            g.renderer.pass_high_water(),
            g.renderer.pass_high_water(),
        );
    }
    assert!(
        g.renderer.pass_high_water() >= 1,
        "a document with effect layers demanded no passes at all, so this \
         measurement is of nothing"
    );
}

/// The sibling half of the question above: many effect layers at one level, where
/// `pack`'s page-area budget starts further chunks (§15 D776).
///
/// The pass count is not bounded by nesting depth. At a fixed page it grows with
/// the number of effect layers, as the ratio `total effect-buffer area ÷ page
/// area` taken to a ceiling (32 blurred 45×45 squares: many passes on a 100×100
/// page, one on 400×400). A real document needs many overlapping on-screen effect
/// layers to demand a second pass, and at full-page size the binding constraint is
/// the shelf row (it wraps at the 8192 device limit and `pack` weighs the packed
/// texture's bounding box), not the ink ratio. This is a measurement, not a licence
/// to bound the pool: one renderer per pass is a correctness rule (§15 D344).
///
/// flip: `budget` set to `u64::MAX` in `resolve_effects` (the packer with no page
/// rule) → fails at the first assertion (32 siblings give 1 pass). The second
/// assertion shows the divisor is the page and not a constant.
#[test]
#[ignore = "needs a GPU adapter"]
fn how_many_effect_passes_a_level_of_siblings_demands() {
    let Some(mut small) = gpu() else {
        return;
    };
    let (doc, res) = siblings(32, 45.0);

    let page = |side: u32| Viewport {
        view: Rect::new(0.0, 0.0, f64::from(side), f64::from(side)),
        pixel_size: (side, side),
    };

    let _ = render_gpu_vp(&mut small, &doc, &res, &page(100));
    let crowded = small.renderer.pass_high_water();
    println!("[S11.2-L4-05] 32 sibling effect layers on a 100×100 page: {crowded} pass(es)");

    // A fresh renderer: the count is a high-water mark.
    let Some(mut large) = gpu() else {
        return;
    };
    let _ = render_gpu_vp(&mut large, &doc, &res, &page(400));
    let roomy = large.renderer.pass_high_water();
    println!("[S11.2-L4-05] the same 32 layers on a 400×400 page: {roomy} pass(es)");

    assert!(
        crowded > 1,
        "one level of 32 effect layers demanded {crowded} pass(es), so the pass \
         count does not grow with sibling count and `[S11.2-L4-05]`'s worry is \
         answered by nesting depth alone"
    );
    assert_eq!(
        roomy, 1,
        "the same document on a page sixteen times the area demanded {roomy} \
         pass(es) rather than 1, so the budget `pack` divides by is not the \
         page — which is what makes the crowded figure above a ratio rather \
         than a property of a small fixture"
    );
}

/// What a vello `Renderer` costs to build (§15 D777): on an otherwise-idle adapter
/// the marginal build is about 6–8 ms and the first in the process about 15 ms;
/// the same calls under the parallel on-device suite take about seven times as
/// long. Quote the band with its condition. `VelloGpuRenderer` builds one
/// `Renderer` up front and one more per effect pass, so a document that first
/// demands `K` passes pays about `(K − high-water) × 7 ms` on that frame only.
///
/// There is no wall-clock assertion and nothing of ours to flip (this times a
/// dependency): a bound is either too loose to catch anything or red on a busy
/// harness. The printed figures are the deliverable; the assertion is only that the
/// loop timed what it says.
#[test]
#[ignore = "needs a GPU adapter"]
fn a_vello_renderer_costs_about_six_milliseconds_to_build() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let Ok((device, _queue)) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
    else {
        return;
    };

    let mut marginal = Vec::new();
    for i in 0..6 {
        let t = std::time::Instant::now();
        let built = vello::Renderer::new(&device, vello::RendererOptions::default()).is_ok();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("[S11.2-L4-05] Renderer::new #{i}: {ms:.2} ms (ok={built})");
        assert!(built, "vello refused to build a Renderer on build #{i}");
        // The first is the cold one; the rest are what an extra effect pass costs.
        if i > 0 {
            marginal.push(ms);
        }
    }

    // Deliberately no wall-clock assertion: the cost is contention-sensitive by
    // about seven times under the parallel suite, so a bound is a flaky gate.
    let worst = marginal.iter().copied().fold(f64::MIN, f64::max);
    let best = marginal.iter().copied().fold(f64::MAX, f64::min);
    println!(
        "[S11.2-L4-05] marginal build: {best:.2}–{worst:.2} ms over {} samples \
         (6–8 ms on an idle adapter; ~48–65 under suite contention — §15 D777)",
        marginal.len()
    );
    assert_eq!(
        marginal.len(),
        5,
        "the loop did not time five marginal builds, so the numbers above are \
         of nothing (§15 D777)"
    );
}

/// An `n`×`n` grid of cards, each a group holding a 70×40 white rect, the group
/// carrying one zero-blur drop shadow — and every third group a second, smaller
/// rect with a shadow of its own, so a third of the cards are a nested effect
/// layer. The shape of a component page, since every instance copies its main's
/// shadow (§15 D978).
///
/// The pitch is 80×50, so a 30×30 grid overruns a 1920×1080 view on both axes and
/// a 20×20 one does not.
fn card_grid(n: usize) -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    let mut layered = Vec::new();
    let white = || {
        keyed_by_position([Fill {
            brush: Brush::Solid(Color::WHITE).into(),
            visible: true,
        }])
    };
    for i in 0..n * n {
        let (col, row) = ((i % n) as f64, (i / n) as f64);
        let g = ids.mint();
        ops.push(Operation::CreateNode {
            id: g,
            parent: root,
            index: i,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        let r = ids.mint();
        ops.push(Operation::CreateNode {
            id: r,
            parent: g,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(70.0, 40.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((col * 80.0, row * 50.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: r,
            fills: white(),
        });
        layered.push(g);
        if i % 3 == 0 {
            let inner = ids.mint();
            ops.push(Operation::CreateNode {
                id: inner,
                parent: g,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((col * 80.0 + 10.0, row * 50.0 + 10.0))),
                name: None,
            });
            ops.push(Operation::SetFills {
                id: inner,
                fills: white(),
            });
            layered.push(inner);
        }
    }
    doc.apply(&Transaction(ops)).expect("the grid applies");
    let shadow = Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(0.0, 2.0),
        blur: 0.0,
        spread: 0.0,
        color: Color::from_rgba8(0, 0, 0, 64),
    }));
    let ops: Vec<Operation> = layered
        .into_iter()
        .map(|id| Operation::SetEffects {
            id,
            effects: keyed_by_position([shadow.clone()]),
        })
        .collect();
    doc.apply(&Transaction(ops)).expect("the shadows apply");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// The median of `xs`, which it sorts.
fn median(xs: &mut [f64]) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

/// The probes' page: 1920×1080.
const PAGE_W: u32 = 1920;
const PAGE_H: u32 = 1080;

/// A render target of `w`×`h` and its view.
fn page_target(g: &Gpu, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let target = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-effects-page"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    (target, view)
}

/// The page viewport, scrolled to `x` along the top.
fn page_at(x: f64) -> Viewport {
    Viewport {
        view: Rect::new(x, -20.0, x + PAGE_W as f64, -20.0 + PAGE_H as f64),
        pixel_size: (PAGE_W, PAGE_H),
    }
}

/// One frame into `view`, with no readback and no wait.
fn draw(g: &mut Gpu, doc: &Document, res: &Resolved, vp: &Viewport, view: &wgpu::TextureView) {
    g.renderer
        .render(
            &g.device,
            &g.queue,
            doc,
            res,
            vp,
            &RenderOverrides::default(),
            view,
            Color::TRANSPARENT,
            &ImageStore::default(),
        )
        .expect("render");
}

/// How many effect jobs the walk records for `doc` at `vp`.
fn job_count(doc: &Document, res: &Resolved, vp: &Viewport) -> usize {
    ondin_render::gpu::build_scene(
        doc,
        res,
        vp,
        &RenderOverrides::default(),
        &ImageStore::default(),
    )
    .1
    .len()
}

/// What an on-screen effect layer costs per frame: a card grid at 1920×1080,
/// panned 3.7 px a frame for 60 frames, the median of the last 50, once for the
/// whole frame (`render` and a `poll(Wait)`) and once for the time `render` takes
/// to return (the host side). Run it alone; under the whole suite the figures are
/// several times higher.
///
/// Wall-clock, so no bound is asserted (§15 D777). The property is pinned by
/// counting instead, in `a_steady_frame_makes_nothing_and_submits_once_per_pass`.
#[test]
#[ignore = "needs a GPU adapter"]
fn what_an_on_screen_effect_layer_costs_per_frame() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (_target, view) = page_target(&g, PAGE_W, PAGE_H);
    for n in [10usize, 20, 30] {
        let (doc, res) = card_grid(n);
        let (mut frame, mut host) = (Vec::new(), Vec::new());
        let (mut jobs, mut made) = (0, 0);
        for f in 0..60 {
            let vp = page_at(-20.0 + f as f64 * 3.7);
            jobs = job_count(&doc, &res, &vp);
            let t = std::time::Instant::now();
            draw(&mut g, &doc, &res, &vp, &view);
            let returned = t.elapsed().as_secs_f64() * 1000.0;
            g.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("poll");
            let total = t.elapsed().as_secs_f64() * 1000.0;
            if f >= 10 {
                host.push(returned);
                frame.push(total);
                made += g.renderer.fx_stats().textures_made;
            }
        }
        let stats = g.renderer.fx_stats();
        println!(
            "[X7-L4-01] {n}x{n}: {jobs} effect jobs, median frame {:.2} ms, \
             render() returned {:.2} ms, passes {}, fx submits {}, textures made \
             over the last 50 frames {made}, pool {:.1} MB",
            median(&mut frame),
            median(&mut host),
            g.renderer.pass_high_water(),
            stats.submits,
            stats.pooled_bytes as f64 / 1e6,
        );
        assert_eq!(frame.len(), 50, "the loop timed fifty frames");
    }
}

/// A steady frame makes no texture and no bind group, and submits once per effect
/// pass rather than once per layer (§15 D1003 (7)): the countable form of the
/// probe above.
///
/// The same 20×20 card grid is drawn three times at one viewport. The first frame
/// fills the pool; the second must take no texture; the third must make nothing.
/// The second frame may still make bind groups, which is not a leak: the first
/// frame's empty pool hands out textures in a different order from every later
/// frame, which all start from the same idle set.
///
/// flips, each failing at the predicted assertion:
/// - `FxPool::take` never reusing an idle texture (`.filter(|_| false)` on its
///   lookup) → `second.textures_made`; it also fails the fixture assertion of
///   `a_pooled_texture_carries_nothing_from_the_frame_before`.
/// - the bind-group cache never hitting (`binds.get(&key).filter(|_| false)`) →
///   `bind_groups_made`.
/// - `run_batch` called once per layer in `resolve_effects` → `submits`, leaving
///   `textures_made` and `bind_groups_made` at 0 (the pool absorbs per-layer
///   batching, so the submit count needs its own assertion). Every picture test in
///   this file stays green under it, since no dispatch reads another's `Params`
///   (the queue-order argument `FxPool`'s doc rests on).
#[test]
#[ignore = "needs a GPU adapter"]
fn a_steady_frame_makes_nothing_and_submits_once_per_pass() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (_target, view) = page_target(&g, PAGE_W, PAGE_H);
    let (doc, res) = card_grid(20);
    let vp = page_at(-20.0);
    let jobs = job_count(&doc, &res, &vp);

    draw(&mut g, &doc, &res, &vp, &view);
    let first = g.renderer.fx_stats();
    draw(&mut g, &doc, &res, &vp, &view);
    let second = g.renderer.fx_stats();
    draw(&mut g, &doc, &res, &vp, &view);
    let steady = g.renderer.fx_stats();
    println!(
        "[X7-L4-01] first {first:?}\n[X7-L4-01] second {second:?}\n[X7-L4-01] steady {steady:?}"
    );

    // The fixture reached the state the test is about: a crowd of layers and a
    // first frame that had to allocate.
    assert!(
        jobs > 100,
        "the grid records {jobs} effect jobs, not a crowd"
    );
    assert!(
        first.textures_made > 0,
        "the first frame had nothing to reuse"
    );
    assert_eq!(steady.layers, jobs, "every job reached the passes");

    assert_eq!(
        second.textures_made, 0,
        "the second frame allocated textures the first had already made"
    );
    assert_eq!(
        steady.textures_made, 0,
        "a steady frame allocated textures, so the pool is not reusing them — \
         every frame's fresh fx textures are §15 D992's leading suspect"
    );
    assert_eq!(
        steady.bind_groups_made, 0,
        "a steady frame made bind groups, so they are not cached by texture"
    );
    assert_eq!(
        steady.submits,
        g.renderer.pass_high_water(),
        "the passes submitted {} times for {} layers in {} vello passes — one \
         submit per layer is `[X7-L4-01]`'s cost",
        steady.submits,
        steady.layers,
        g.renderer.pass_high_water()
    );
    assert_eq!(
        steady.pooled_bytes, first.pooled_bytes,
        "the pool grew across two identical frames"
    );
}

/// A texture back from the pool carries nothing of its last user into the picture
/// (§15 D1003 (7)).
///
/// The pool hands out textures larger than asked (`fx_gpu::size_class`) and never
/// clears them, so the margin is harmless only because no pass reads past
/// `Params::size`. A first frame of the 40×40 square with an upward shadow (a
/// 40×46 buffer, class 48×48) leaves ink in rows 40–46 of every texture it used;
/// the second frame, an inner shadow offset up and left (40×40, the same class),
/// runs through those textures and its silhouette reads six rows below each pixel,
/// which along the bottom edge is exactly that margin.
///
/// flip: `silhouette`'s bound taken from the texture
/// (`all(s < vec2<i32>(textureDimensions(src_a)))`) rather than from `inside` →
/// fails at the CPU comparison, and only through reuse (the fresh renderer's
/// control stays green, as a fresh margin is zero).
#[test]
#[ignore = "requires a GPU adapter"]
fn a_pooled_texture_carries_nothing_from_the_frame_before() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (before, before_res) = document(vec![red_shadow(-6.0, 0.0)], 1.0);
    let (doc, res) = document(
        vec![Effect::new(EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(-6.0, -6.0),
            blur: 0.0,
            spread: 0.0,
            color: Color::from_rgba8(255, 0, 0, 255),
        }))],
        1.0,
    );
    let _ = render_gpu(&mut g, &before, &before_res);
    let reused = render_gpu(&mut g, &doc, &res);
    assert_eq!(
        g.renderer.fx_stats().textures_made,
        0,
        "the second frame took fresh textures, so it never met the first \
         frame's leftovers and this test is about nothing"
    );

    let mut fresh_gpu = gpu().expect("a second device on the same adapter");
    let fresh = render_gpu(&mut fresh_gpu, &doc, &res);
    let cpu = render_cpu(&doc, &res);
    assert_eq!(
        fresh, cpu,
        "the control: a fresh renderer draws the reference"
    );
    assert_eq!(
        at(&cpu, 40, 67),
        (255, 0, 0, 255),
        "the reference has its inner shadow along the bottom edge"
    );
    assert_eq!(
        reused, cpu,
        "a renderer whose pool held the frame before drew something else — a pass \
         read a pooled texture past its own buffer"
    );
}

/// The pool lets go of what recent frames did not need (§15 D1003 (7)), the bound
/// that keeps it from growing without limit.
///
/// A 20×20 card grid fills the pool; then the plain square with one shadow is drawn
/// frame after frame. For `fx_gpu::KEEP_FRAMES` frames the pool may keep the grid's
/// textures, and after that it must hold exactly what the square alone needs, which
/// a fresh renderer that has only drawn the square measures.
///
/// `FxPool::end_frame` has two trims, idle textures older than `KEEP_FRAMES` and
/// the pool held to the largest working set in that window. Over this fixture each
/// alone is enough for the final equality, so either looks removable and neither
/// is; both removed → fails at the final equality. The first assertion pins the
/// byte bound on its own (without it the first square frame holds more than the
/// grid's frame did); nothing pins the age trim on its own.
#[test]
#[ignore = "requires a GPU adapter"]
fn the_pool_lets_go_of_what_recent_frames_did_not_need() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (_target, view) = page_target(&g, PAGE_W, PAGE_H);
    let (grid, grid_res) = card_grid(20);
    let (square, square_res) = document(vec![red_shadow(6.0, 0.0)], 1.0);
    let vp = page_at(-20.0);
    draw(&mut g, &grid, &grid_res, &vp, &view);
    let full = g.renderer.fx_stats().pooled_bytes;

    let mut alone = gpu().expect("a second device on the same adapter");
    let (_t, alone_view) = page_target(&alone, PAGE_W, PAGE_H);
    draw(&mut alone, &square, &square_res, &vp, &alone_view);
    let needed = alone.renderer.fx_stats().pooled_bytes;
    assert!(
        needed > 0 && needed < full,
        "the square needs {needed} bytes of pool and the grid {full}, so the \
         fixture cannot tell holding from letting go"
    );

    let mut held = Vec::new();
    for _ in 0..12 {
        draw(&mut g, &square, &square_res, &vp, &view);
        held.push(g.renderer.fx_stats().pooled_bytes);
    }
    println!("[X7-L4-01] grid {full}, square alone {needed}, held frame by frame {held:?}");
    assert!(
        held[0] <= full,
        "on the first frame of the square the pool held {} bytes, more than the \
         {full} the busiest recent frame used — the byte bound is not trimming",
        held[0]
    );
    assert_eq!(
        *held.last().expect("twelve frames"),
        needed,
        "twelve frames after the grid the pool still holds more than the square \
         needs — it is not letting go of textures no recent frame used"
    );
}
