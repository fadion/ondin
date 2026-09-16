//! Effects through the **whole GPU path**, on-device: document → scene walk →
//! sub-scene → vello raster → compute passes → atlas → page (§6.4, §15 D339).
//!
//! `tests/fx_gpu.rs` proves the passes match the CPU reference over a buffer this
//! file never builds; `tests/effects.rs` proves the CPU path draws the right
//! picture. Neither says the two are *wired together* — that the walk opens a
//! layer, that the sub-scene rasterizes into it, that the filtered texture lands
//! back in the page at the right place. That wiring is what drew nothing at all
//! until 2026-08-25, and this is what says it draws now.
//!
//! `#[ignore]` like the rest of the on-device suite: CI has no adapter. Run with
//! `cargo test -p ondin-render --test gpu_effects -- --ignored`.

use ondin_core::kurbo::{Affine, Rect, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeKind, Operation, Resolved, Shadow,
    Transaction,
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
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::SetEffects { id: r, effects },
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
}

fn gpu() -> Option<Gpu> {
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

/// **The canvas draws the shadow.** Until 2026-08-25 it drew the layer plainly —
/// `ScenePainter::push_effect_layer`'s default — so the whole of what this asserts
/// is that a stack authored in the panel reaches the screen.
///
/// The three claims are the CPU test's, re-asked of the other backend: the shadow
/// exists, it is its own colour rather than the layer's, and it is on the side the
/// offset points at.
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
/// top of the page — the finding's own fixture for `[S9.2-L1-01]`.
fn photographs(n: usize, w: u32, h: u32) -> (Document, Resolved, ImageStore) {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let mut store = ImageStore::new();
    let mut ids = ondin_core::IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    for i in 0..n {
        // **A distinct id per picture, which is the whole fixture.** vello keys
        // atlas residency on the blob's id, so `n` copies of one photograph are
        // one atlas entry and fit trivially — a test that reused the bytes would
        // be green against every version of this code.
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
            fills: vec![Fill {
                brush: image_brush(iid),
                visible: true,
            }],
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

/// **Three ordinary photographs on one page all draw** — `[S9.2-L1-01]`, §15
/// D745, and the case a moodboard is.
///
/// vello keeps every picture a scene draws in one 8192-pixel sheet and, when the
/// next will not fit, sets its position to `None` and carries on — *"there's
/// nothing we can do"*, in `resolve_pending_images`' own words. No error, no log.
/// Three 4500 × 3000 photographs need three shelves of 3000 and the third has
/// nowhere to go, so the canvas drew **1 250 opaque pixels of 1 875** while the
/// layers row and the Fill swatch showed all three — they read the store, and only
/// the canvas reads the atlas.
///
/// The page's pictures are now shrunk by one integer factor until the set fits,
/// full resolution until it stops fitting. At three photographs the factor is 2
/// and every one of them draws.
///
/// ⚠️ **Distinct bytes per picture, and the doc comment on the fixture says why**:
/// vello keys residency on the blob id, so three copies of one photograph are one
/// atlas entry and would fit however this code behaved.
///
/// ⚠️ **The two-photograph row is the anti-vacuity control and the more important
/// assertion of the two.** The maintainer's decision was *full resolution until it
/// stops fitting* — a budget that shrank two photographs would pass the
/// three-photograph assertion and be wrong. This asserts the factor is exactly 1
/// there.
///
/// ⚠️ **Flipped twice, because the first flip never reached the assertion the
/// test is for.** Making `atlas_downscale` return 1 unconditionally fails on the
/// *factor* assertion four lines above the pixels, so it says nothing about the
/// drawing. The flip that reaches the picture is `atlas_pixels` returning
/// `pixels` unchanged — the budget still computed, the reduction not applied —
/// and it comes back **`left: 1250, right: 1875`**, which is the finding's own
/// measurement to the pixel. **Two flips, because a test with a cheap assertion in
/// front of an expensive one is only as good as the flip that gets past it.**
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
    let opaque = px.chunks_exact(4).filter(|p| p[3] > 200).count();
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
        px2.chunks_exact(4).filter(|p| p[3] > 200).count(),
        1250,
        "and both of them draw"
    );
}

/// A long thin banner, the shape whose effect buffer goes past the texture limit
/// first. `[S11.2-L1-01]`.
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
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        },
        Operation::SetEffects { id: r, effects },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

/// **A layer whose effect buffer needs more than the texture limit still draws**
/// — `[S11.2-L1-01]`, §15 D744, and the case that used to take the canvas down
/// while the CPU backend and the export both drew the same document.
///
/// ⚠️ **Finding the fixture was most of the work and is the part worth reading**,
/// because the finding's own case — *"a drop shadow at `offset = 1000`, 8× zoom"*
/// — **does not reproduce**, and a test written from it passes against the
/// unfixed code. Two things bound the buffer before the cap ever sees it, and
/// both had to be measured rather than read:
///
/// - `effect::grow_to_escape` intersects with the layer's **own ink**, and grows
///   the target on each side by the escape from the *opposite* side (a shadow
///   thrown right lets ink from off-screen *left* reach in). So a large **offset**
///   grows nothing on the side it points at: at `offset = 1000` and 8× the buffer
///   came back exactly 8192 — the viewport, to the pixel.
/// - `buffer_box_within` then bisects the escape down until the box's **area**
///   fits the page's pixel count, so a big square layer is trimmed by area long
///   before either side reaches 8192.
///
/// What clears both is a layer that is **wide and thin**: a 1600 × 12 banner —
/// a rule, a divider, a hero strip — at 6×. Measured: the buffer comes out
/// **9672 × 144**, which is 1480 past the limit on an area of 1.39 M against a
/// 2.3 M budget, so nothing else trims it. Against the old cap this aborts with
/// `wgpu error: Validation Error`. **This is an ordinary drawing, not a hostile
/// one.**
///
/// ⚠️ **It asserts a render that completes and ink that lands, not a picture.**
/// The layer is resampled at this size, so the exact pixels are `blur_coarse`'s
/// business; what is pinned is that the frame *happens*. A panic in `Scratch::new`
/// is not a wrong colour, it is no frame at all.
///
/// ⚠️ **Flipped by restoring `.min(u16::MAX as f64)` in `push_effect_layer`, and
/// the failure does not look like a test failure** — the process aborts inside
/// wgpu's validation, so the suite reports a crash rather than a red test. Worth
/// knowing before somebody reads a broken run as a broken harness.
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
    assert!(px.chunks_exact(4).any(|p| p[3] > 0), "and it has ink in it");
}

/// **An inner shadow on a rect reaches all four edges here too** — the on-device
/// half of `[S10.2-L1-01]`, and the reason it needs asserting on this side at all.
///
/// `tests/fx_gpu.rs` compares each GPU pass against the CPU reference and stayed
/// green through the whole life of the bug, because *both* backends inverted the
/// silhouette before the passes that read past the buffer's edge. A differential
/// test cannot see an error the two implementations share, so this asserts the
/// picture **absolutely** first and the agreement second. The fixture is the one
/// the bug needs: a rect, whose ink is exactly its own bounding box, at the offset
/// the panel starts every shadow on.
///
/// ⚠️ **The flip is three edits, and one of them alone is a no-op** — which is
/// the thing to write down rather than the result. Restoring `silhouette`'s
/// inversion in `fx.wgsl` on its own changes nothing at all, because `INNER` no
/// longer reaches that pass: `fx_gpu::run` sets it on `Pass::Tint`. The flip that
/// bites is the shipped spelling entire — the shader, plus `inner` back on the
/// silhouette's `Params`, minus it from the tint's. Run: this test fails on the
/// **first** edge assertion at `(255, 255, 255, 255)`, and the backend comparison
/// at the end is never reached, so it is the absolute half that carries the test
/// and the differential half that is the control.
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

/// **The same document through both backends agrees, byte for byte.** This is
/// §3's promise — one scene walk, two backends — asked of the one feature that
/// has a *separate implementation* on each side of it.
///
/// **Exact, and the fixture is what earns that.** A GPU rasterizer and a CPU one
/// do not generally agree on an antialiased edge, so a rotated or fractionally
/// placed shape would differ along every outline and this would have to be a
/// tolerance. Here the square sits at (30, 30) at 40×40 in a 1:1 viewport, so
/// every edge lands on a pixel boundary and neither rasterizer has anything to
/// antialias — which leaves the effect arithmetic as the only thing that *can*
/// differ, and that is the thing being tested. Measured: mean 0.000, worst 0.
///
/// What it does not claim is that the two backends agree on any document. It
/// claims that where the rasterizers have nothing to disagree about, the effect
/// passes have nothing either.
#[test]
#[ignore = "requires a GPU adapter"]
fn both_backends_draw_the_same_shadow() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (doc, res) = document(vec![red_shadow(10.0, 6.0)], 1.0);
    let gpu_px = render_gpu(&mut g, &doc, &res);
    let cpu_px = render_cpu(&doc, &res);

    // The fixture is asserted before the claim is: a blurred shadow has to be
    // *there*, or two empty buffers would agree perfectly and prove nothing.
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

/// **A shadow and an opacity together** — the pairing that panicked the CPU
/// backend (§15 D338). The GPU painter counts ordinary layers for the same
/// reason, and nothing but a render says the counting is right.
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

/// **A stack with nothing to draw leaves the picture alone.** The walk opens no
/// layer for it (`effects::any_ink`), so this is really asking that the *absence*
/// of a job is handled — no atlas slot, no texture, no blit.
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
    };
    let (with, res_with) = document(vec![hidden], 1.0);
    let (without, res_without) = document(vec![], 1.0);
    let a = render_gpu(&mut g, &with, &res_with);
    let b = render_gpu(&mut g, &without, &res_without);
    assert_eq!(a, b, "a hidden effect changes no pixel");
}

/// **The canvas half of `effects::a_layer_scrolled_off_the_top_still_casts_into_view`**
/// (§15 D342).
///
/// The clamp that lost the silhouette lives in `effect::buffer_box` and both
/// backends call it, so the CPU test is the one that would fail first — but the
/// canvas is the surface this was reported on, and the GPU path reaches
/// `buffer_box` through its own `push_effect_layer`. A fix applied to one backend
/// and not the other would pass there and fail here.
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

/// **An image fill keeps its pixels while effect layers are on the page**
/// (§15 D344).
///
/// Every effect layer costs the page renderer an extra `render_to_texture`, and
/// vello's image atlas evicts a resident image that has not been *drawn* for two
/// of those passes (`EVICT_AFTER_GENERATIONS`). With the sub-scenes rendered on
/// the page's own renderer, a picture drawn once per frame fell behind that
/// rhythm and came back empty on the second frame onwards — reported from the
/// machine as *"pasting an image into the canvas while there's an effect on a
/// layer makes the image empty"*, with the bounding box and both thumbnails still
/// showing because they read the store rather than the atlas.
///
/// **Six frames, not one.** The first frame is always right: the picture is new
/// to the atlas, so it is uploaded whatever else is going on. Everything this
/// test is about happens from the second.
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

/// **A picture *inside* an effected layer keeps its pixels too**, which is the
/// same question one level down: those sub-scenes are rendered on their own
/// renderer, so its atlas has the rhythm the page's does.
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

/// **An effect layer inside an effect layer**, which panicked vello outright
/// until 2026-09-01 (§15 D404): *"Tried to draw an invalid empty image (id: 2).
/// Maybe it was registered to a different renderer"* — which is exactly what had
/// happened. A nested layer's result is drawn by its **parent's** sub-scene, and
/// that scene is rasterized by a layer renderer with an atlas of its own, while
/// `override_image` was only ever called on the page's.
///
/// ⚠️ **Three fixtures, and the first two are the control.** A shadow alone and a
/// blur alone both worked the whole time — the bug needs one layer to *contain*
/// another — so a test that only asserted the nested case would not say which of
/// the three broke if this ever regresses.
///
/// The comparison is against the CPU backend, which composites layers directly and
/// has no atlas at all, so it cannot share the failure. Exact rather than
/// tolerant, for the reason `both_backends_draw_the_same_shadow` gives: the square
/// is on pixel boundaries, so the rasterizers have nothing to disagree about and
/// the effect wiring is all that is left.
///
/// ⚠️ **Except three deep, which allows one level, and finding that out is what
/// the third fixture is for.** Every effect layer the GPU path opens is bracketed
/// by a straight→premultiplied→straight round trip (`fx_gpu`'s module docs), and
/// at eight bits that is not the identity on a partly transparent pixel. Two
/// layers happen to move nothing in this fixture and three move one channel by 1,
/// which is the same ±1 `tests/fx_gpu.rs` allows for the same reason. The CPU
/// backend stays premultiplied throughout and so has no such boundary.
///
/// ⚠️ **The flip's failure site was not the one predicted.** Registering every
/// slot on the page renderer — the state before D404 — was expected to fail the
/// comparison; it panics inside `render` instead, before any pixel is compared,
/// because vello refuses to draw an atlas slot it has no pixels for. The
/// comparison is what would catch a slot registered on the *wrong* renderer rather
/// than on none.
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
        // The fixture before the claim: two empty buffers agree perfectly.
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
/// **Nesting depth is the parameter** rather than two fixed groups, because the
/// rule D404 restored — a layer's slot goes to the renderers of the level *above*
/// — is only exercised at one depth by a two-level fixture, and "the level above"
/// and "the page" are the same thing there.
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
        fills: vec![Fill {
            brush: Brush::Solid(Color::WHITE),
            visible: true,
        }],
    });
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let ops: Vec<Operation> = groups
        .into_iter()
        .zip(stacks)
        .filter(|(_, fx)| !fx.is_empty())
        .map(|(id, effects)| Operation::SetEffects { id, effects })
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
/// The counterpart to `nested`, and the case `pack`'s page-area budget is
/// actually about: depth is bounded by how deeply a designer nests groups, and
/// total area is not. The squares overlap on purpose — what the packer weighs is
/// the sum of the layers' buffer areas against the page's pixel count, not how
/// much of the page they cover between them.
///
/// Plain backticks throughout: `crates/*/tests/` is a separate crate root and no
/// gate reads a doc link there (D319, D622).
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
            // Staggered by a couple of points so no two layers resolve to the
            // identical box — a packer that deduplicated would otherwise read as
            // a packer that packed.
            transform: Some(Affine::translate((
                4.0 + (i % 4) as f64 * 2.0,
                4.0 + (i / 4) as f64 * 2.0,
            ))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: r,
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        });
        groups.push(g);
    }
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let ops: Vec<Operation> = groups
        .into_iter()
        .map(|id| Operation::SetEffects {
            id,
            effects: vec![Effect::new(EffectKind::LayerBlur { radius: 3.0 })],
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

/// The picture inside its own effect layer **and** other effect layers beside it,
/// so the layers renderer does more than one pass a frame.
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
            fills: vec![Fill {
                brush: image_brush(iid),
                visible: true,
            }],
        },
    ];
    if on_the_picture {
        ops.push(Operation::SetEffects {
            id: pic,
            effects: vec![Effect::new(EffectKind::LayerBlur { radius: 3.0 })],
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
            fills: vec![Fill {
                brush: Brush::Solid(Color::WHITE),
                visible: true,
            }],
        });
        ops.push(Operation::SetEffects {
            id: other,
            effects: vec![Effect::new(EffectKind::LayerBlur { radius: 4.0 })],
        });
    }
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    (doc, res, store)
}

/// The picture's side. **Large enough to matter to the atlas** — vello's starts at
/// 1024 square — so this exercises the packing rather than a corner of it.
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

/// How many effect passes a document actually demands — the `K` in
/// `[S11.2-L4-05]`'s `K x ~5.7 ms`, measured rather than reasoned (§15 D763).
/// The other half of that product is measured too, and it is a **6-8 ms** band on
/// an otherwise-idle adapter rather than a figure — seven times that under suite
/// contention: see `a_vello_renderer_costs_about_six_milliseconds_to_build` below,
/// and §15 D777 for why D344's competing *"84 ms"* does not reproduce.
///
/// That finding is open because its own confidence line says the per-renderer
/// cost is measured and `K` is not: it is derived from `pack`'s budget rule and
/// was never observed. Nothing else in the suite answers it — `tests/render.rs`'s
/// `layers.len()` assertions are the `RecordingPainter`'s CPU-side layer stack, a
/// different quantity with the same name, and `zoom_sweep.rs` prints frame times
/// without touching the pass count.
///
/// This reports `VelloGpuRenderer::pass_high_water` for the deepest fixture this
/// file builds, so the number in the finding stops being a guess. It asserts only
/// the floor that must hold if the plumbing works at all — a document with effect
/// layers demands at least one pass — because the interesting figure is the
/// printed one and a hard upper bound here would be asserting `pack`'s arithmetic
/// against itself.
///
/// Plain backticks throughout: `crates/*/tests/` is a separate crate root and no
/// gate reads a doc link there (D319, D622).
///
/// Run with `cargo test -p ondin-render --test gpu_effects --release -- --ignored
/// --nocapture` to see the number.
///
/// 🚨 **Measured 2026-09-15 on an RTX 4070 Ti, and for a *nested* document `K` is
/// the nesting *depth* — one pass per level.** One blur → **1**
/// pass; a blur inside a shadow → **2**; three deep → **3**. So a realistically
/// nested drawing pays `3 x ~5.7 ms` ≈ 17 ms of pipeline build **once in the life
/// of the process** — about **21 ms** at the 6-8 ms idle-adapter band D777
/// measured, which does not change the reading — and holds three renderers. ⚠️ **That de-ranks the finding only
/// for this case** — the unbounded `K` it feared is driven by `pack`'s area
/// budget, and the area is the sibling count.
///
/// ⚠️ **The sibling case is the one the budget rule is actually about** — many
/// effect layers at *one* level whose combined area exceeds the page, which is
/// where `pack` starts a second chunk. Depth is bounded by how deeply a designer
/// nests groups; area is not. **A number measured for the wrong case reads
/// exactly like a number measured for the right one**, so this says which case it
/// is. 🚨 **It was unmeasured until 2026-09-15 and is not any more** — see
/// `how_many_effect_passes_a_level_of_siblings_demands` below, which finds
/// **11** passes for 32 siblings on a 100×100 page. **Do not read the depth
/// figure above as the answer to the finding**: the two measure different things
/// and only one of them is bounded.
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

/// The **sibling** half of the question above, which `[S11.2-L4-05]` is really
/// about and which §15 D763 left unmeasured: many effect layers at *one* level,
/// where `pack`'s page-area budget starts a second chunk.
///
/// 🚨 **Measured 2026-09-15 on an RTX 4070 Ti, and it partly re-ranks what D763
/// de-ranked** (§15 D776). The same blurred 45×45 squares, one level, counted
/// against three page sizes:
///
/// ```text
///                n=1   n=4   n=8   n=16   n=32
///   100×100       1     2     3      6     11   passes
///   200×200       1     1     1      2      3
///   400×400       1     1     1      1      1
/// ```
///
/// So `K` is **not** bounded by nesting depth: at a fixed page it grows roughly
/// linearly with how many effect layers sit on that page, and nothing caps it.
/// What the table also shows is the divisor — four times the page area is four
/// times the per-chunk capacity, and the counts are that ratio taken to a ceiling
/// — so the quantity is a **ratio**, `total effect-buffer area ÷ page area`, and
/// the tiny fixture page is not what makes the number look bad. **D763's depth
/// figure and this are both true and they answer different questions**; a reader
/// who has only the first will conclude `K ≤ 3`.
///
/// The table is exactly what `pack` computes, which is what says the number is
/// the budget rule rather than something else about the device: `effect::reach`
/// of radius 3 is 4.5, so each buffer is about 54 px square, three fit inside the
/// 100×100 page's 10,000-pixel budget and four do not, and ceil(32/3) is 11.
///
/// ⚠️ **What this does not say is that a real document reaches those numbers, and
/// the arithmetic for one is reasoned from `pack` rather than measured at that
/// size.** At 1920×1080 the budget is 2,073,600 pixels and a 200×200 square
/// blurred at radius 3 has a 209×209 buffer, about 44k, so it takes forty-odd
/// *overlapping* on-screen effect layers to demand a second pass. That is a busy
/// file rather than a casual one, and it is the arithmetic the severity should be
/// argued from — not this fixture's 100×100.
///
/// 🚨 **But at that size the binding constraint is the shelf *row* and not the
/// ink, so re-deriving the figure from the ratio alone gets the right order for
/// the wrong reason.** `pack` weighs the packed texture's *bounding box*, and the
/// shelf wraps at the 8192 device limit: thirty-nine 209-pixel layers fill one row
/// at 8151x209 = 1.70M, inside the budget, and the fortieth wraps the box to
/// 8151x418 = 3.4M, over it at a stroke. The break is at 39 where the ink ratio
/// says 47. **This fixture's break is area-driven and a full-page document's is
/// row-driven** — worth knowing before anybody widens the fixture or its page and
/// concludes the rule changed.
///
/// ⚠️ **And it is a measurement, not a licence to bound the pool.** §15 D344 is
/// why there is one renderer per pass; it is a correctness rule and the symptom
/// it was written for was a pasted image going blank from the second frame on.
///
/// Plain backticks throughout: `crates/*/tests/` is a separate crate root and no
/// gate reads a doc link there (D319, D622).
///
/// **Flip-checked** against the plausible wrong version rather than against
/// nothing — `budget` set to `u64::MAX` in `resolve_effects`, i.e. the pre-D402
/// packer that had no page rule. Predicted to fail on the first assertion, and
/// it does: 32 siblings come back as **1** pass. The second assertion is what
/// the first cannot say — that the divisor is the *page* and not a constant —
/// and a budget pinned to any fixed number leaves it red.
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

    // A *fresh* renderer, because the count is a high-water mark: reusing the
    // one above would report the crowded page's number for the roomy one.
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

/// What a vello `Renderer` actually costs to build, because the record gives two
/// answers fifteen-fold apart for the one quantity `[S11.2-L4-05]`'s severity is
/// computed from (§15 D777).
///
/// 🚨 **§15 D344 says *"a vello `Renderer` costs 84 ms to build (49 with
/// area-only antialiasing)"*; `[S11.2-L4-05]`, D763 and D776 all say
/// *"~5.7 ms"*.** Both are in `decisions.md`, both are live, neither cites the
/// other, and the finding that is still open is ranked on `K × the smaller one`.
/// **A record that disagrees with itself about a number is worse than one that is
/// silent**, because each half reads as settled on its own.
///
/// 🚨 **Measured 2026-09-15 on an RTX 4070 Ti, vello 0.9.0, `--release`, six
/// consecutive builds against one device:**
///
/// ```text
///          run 1      run 2
///   #0   15.05 ms   15.04 ms   <- first in the process
///   #1    6.46 ms    7.49 ms
///   #2    6.24 ms    7.33 ms
///   #3    5.99 ms    7.65 ms
///   #4    5.96 ms    7.86 ms
///   #5    5.62 ms    7.51 ms
/// ```
///
/// ⚠️ **Two runs, minutes apart, because one would have read as a precision this
/// has not got**: the marginal build is **6–8 ms**, and the cold one is 15 ms to
/// within a hundredth both times. Quote the band, not a figure.
///
/// 🚨 **And quote the *condition* with it, because both runs above are of an
/// otherwise-idle adapter and that is not the only case.** Run as part of the
/// whole on-device suite, which the harness runs in parallel, the same six calls
/// come back **121.72 / 65.27 / 49.25 / 49.69 / 49.88 / 48.31 ms** — about seven
/// times the idle figure. `--test-threads=1` over the same suite gives
/// **15.50 / 7.19 / 7.48 / 6.86 / 6.89 / 7.26**, reproducing the isolated numbers
/// exactly, which is the control that makes it **contention and not drift**.
/// ⚠️ **The app does not build renderers from thirteen threads**, so 6–8 ms is
/// the figure the finding wants — but a reader who measures this the easy way
/// will get the other one.
///
/// **So ~5.7 ms is the right order and D344's 84 ms does not reproduce** — not
/// even as the cold case, which is 15 ms. ⚠️ **And it is the same option set**:
/// `RendererOptions::default()` is `AaSupport::all()`, which is precisely the
/// arm D344 priced at 84 rather than the area-only one it priced at 49.
///
/// 🚨 **What this cannot say is *why*, and "vello has moved" is not the answer.**
/// D337 and D339 both name vello 0.9 — on 2026-08-24 and 2026-08-25, D337 in the
/// same breath as the `vello_cpu` 0.0.9 that D414 later replaced — and the
/// workspace has pinned `=0.9.0` ever since, while D344 is 2026-08-25. **The
/// renderer that entry timed is this renderer.** What D344 does not record is the
/// profile, the machine or the version, and this measurement is `--release`: that
/// is the one difference still visible, and it is a candidate rather than a
/// finding. So D777 says the figure **does not reproduce**, which is weaker than
/// calling it stale and is as far as the evidence goes.
///
/// ⚠️ **The marginal renderer is what the finding needs, and it is the 6–8 ms
/// one.** `VelloGpuRenderer` builds one `Renderer` up front and one more per
/// effect pass, so a document that first demands `K` passes pays about
/// `(K − high-water) × 7 ms` on that frame and nothing on any frame after it.
/// **At D776's worst measured `K` of 11 that is about 77 ms on one frame** — a
/// visible hitch, once, and then nothing.
///
/// 🚨 **This carried a drift guard at 40 ms and it had to come out — see the
/// comment at the assertion.** It was chosen as *"about seven times the
/// measurement"*, which is exactly the factor contention turns out to be worth,
/// so it passed every run alone and failed the first time the suite ran. **A
/// wall-clock bound cannot be a gate in a parallel harness.** What is left is the
/// numbers, their conditions, and an assertion that the loop timed what it says
/// it timed. **There is nothing of ours to flip here** — this times a dependency
/// — so the six figures above are the deliverable rather than the pass.
///
/// Plain backticks throughout: `crates/*/tests/` is a separate crate root and no
/// gate reads a doc link there (D319, D622).
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
        // The first is the cold one and is a different question; the rest are
        // what an extra effect pass actually costs.
        if i > 0 {
            marginal.push(ms);
        }
    }

    // 🚨 **There is deliberately no wall-clock assertion here, and the first
    // version of this test had one at 40 ms.** It passed every time it was run
    // alone and went red the first time the whole on-device suite ran, because
    // the harness runs tests in parallel and thirteen other tests were on the
    // same adapter. Measured both ways, same session, minutes apart:
    //
    //   whole suite, default (parallel):  121.72  65.27  49.25  49.69  49.88  48.31
    //   whole suite, --test-threads=1:     15.50   7.19   7.48   6.86   6.89   7.26
    //
    // The second reproduces the isolated figure exactly, which is the control:
    // the cost is **contention-sensitive by about seven times**, not drifting.
    // ⚠️ **So the 6–8 ms band is a measurement of an otherwise-idle adapter**,
    // and the first version of this test asserted it under contention — *a
    // number measured for the wrong case reads exactly like one measured for the
    // right case*, which is the trap D776 was written about, committed into the
    // test that cites it. **A wall-clock bound cannot be a gate in a parallel
    // harness**: it is either loose enough to catch nothing or tight enough to
    // fail on a busy machine. The six numbers are the deliverable; the assertion
    // below is only that the thing being timed happened at all.
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
