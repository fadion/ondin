//! The GPU effect passes against the CPU reference, on-device (§6.4, §15 D333).
//!
//! **This is the only thing that says the shaders are right.** `fx_gpu` is a port
//! of `effects`, pass for pass, and every way it could be wrong — a transposed
//! matrix row, a kernel indexed from the wrong end, a silhouette shifted the
//! other way, a composite with its operands swapped — produces a picture that
//! looks *plausible*. So each case runs both implementations over the same bytes
//! and compares them.
//!
//! `#[ignore]` like the rest of the on-device suite: CI has no adapter. Run with
//! `cargo test -p ondin-render --test fx_gpu -- --ignored`.

use ondin_core::effect::{Effect, EffectKind, Filters, Shadow};
use ondin_core::kurbo::Vec2;
use ondin_core::peniko::Color;
use ondin_render::effects::{self, Surface};
use ondin_render::fx_gpu::{self, FxPipelines, Slice};

/// This file's fixture is a texture of its own, so the slice is all of it. In the
/// app the layers of one nesting level share a **packed** texture and each reads
/// from its own origin (§15 D344).
fn whole() -> Slice {
    Slice {
        at: (0, 0),
        size: (W, H),
    }
}

const W: u32 = 64;
const H: u32 = 48;
const SCALE: (f64, f64) = (1.0, 1.0);

/// Round half-up to an 8-bit level — the shader's `q`, and the CPU reference's
/// `+ 0.5` cast, written once so the two ends of the comparison agree about the
/// ladder they are both standing on.
fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

fn premultiply(straight: &[u8]) -> Vec<u8> {
    straight
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            [
                q(p[0] as f32 / 255.0 * a),
                q(p[1] as f32 / 255.0 * a),
                q(p[2] as f32 / 255.0 * a),
                p[3],
            ]
        })
        .collect()
}

fn unpremultiply(premul: &[u8]) -> Vec<u8> {
    premul
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            if a <= 0.0 {
                return [0, 0, 0, 0];
            }
            [
                q(p[0] as f32 / 255.0 / a),
                q(p[1] as f32 / 255.0 / a),
                q(p[2] as f32 / 255.0 / a),
                p[3],
            ]
        })
        .collect()
}

/// The fixture: a soft-edged disc in a saturated colour over transparency, with
/// a half-transparent bar across it.
///
/// **Soft edges and partial alpha on purpose.** A hard-edged opaque square would
/// pass under a premultiplied/straight mix-up, under a blur that clamps its edge
/// instead of fading, and under a colour matrix applied on the wrong side of the
/// alpha divide — those are exactly the three mistakes this file exists to catch,
/// and all three only show where alpha is neither 0 nor 255.
fn fixture() -> Vec<u8> {
    let mut px = vec![0u8; (W * H * 4) as usize];
    let (cx, cy, r) = (W as f32 * 0.45, H as f32 * 0.5, 14.0);
    for y in 0..H {
        for x in 0..W {
            let d = (((x as f32 + 0.5 - cx).powi(2)) + ((y as f32 + 0.5 - cy).powi(2))).sqrt();
            // A one-pixel ramp at the rim, so the edge carries every alpha level.
            let cover = (r - d + 0.5).clamp(0.0, 1.0);
            let bar = (14..20).contains(&y);
            let a = if bar { cover * 0.5 } else { cover };
            let i = ((y * W + x) * 4) as usize;
            px[i] = q(0.85);
            px[i + 1] = q(0.32);
            px[i + 2] = q(0.15);
            px[i + 3] = q(a);
        }
    }
    px
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fx: FxPipelines,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    let fx = FxPipelines::new(&device);
    Some(Gpu { device, queue, fx })
}

/// The fixture as a texture the passes can read.
///
/// **Its own descriptor rather than `fx_gpu::fx_texture`**, and the difference is
/// `COPY_DST`: in the app nothing ever writes one of these from the host — vello
/// rasterizes into it through `STORAGE_BINDING` — so widening the shared helper
/// to let a test upload bytes would be the test relaxing a production constraint
/// to suit itself.
fn upload(g: &Gpu, px: &[u8]) -> wgpu::Texture {
    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fixture"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    g.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        px,
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
    tex
}

fn readback(g: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
    // `copy_texture_to_buffer` wants rows on a 256-byte boundary, so the buffer is
    // padded and the padding is dropped on the way out.
    let bpr = (W * 4).div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
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
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        let row = (y * bpr) as usize;
        out.extend_from_slice(&data[row..row + (W * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

/// What the CPU backend would produce for the same stack, in straight alpha.
fn reference(effects_list: &[Effect]) -> Vec<u8> {
    let mut buf = premultiply(&fixture());
    let mut s = Surface::new(&mut buf, W as usize, H as usize);
    effects::run(&mut s, effects_list, SCALE);
    unpremultiply(&buf)
}

/// Compare, and say *where* and *how* rather than only that they differ.
///
/// The report names the worst pixel and the histogram of differences, because
/// the two failure modes look nothing alike: a rounding disagreement is a thin
/// spread of ±1 over the soft edges, and a wrong pass is a large delta over a
/// region with a shape to it.
fn compare(name: &str, got: &[u8], want: &[u8], tolerance: u8) {
    assert_eq!(got.len(), want.len());
    let mut worst = (0u8, 0usize);
    let mut hist = [0usize; 256];
    for (i, (a, b)) in got.iter().zip(want).enumerate() {
        let d = a.abs_diff(*b);
        hist[d as usize] += 1;
        if d > worst.0 {
            worst = (d, i);
        }
    }
    let over: usize = hist[tolerance as usize + 1..].iter().sum();
    if over > 0 {
        let p = worst.1 / 4;
        let (x, y) = (p % W as usize, p / W as usize);
        let g = &got[p * 4..p * 4 + 4];
        let w = &want[p * 4..p * 4 + 4];
        let spread: Vec<String> = hist
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(d, n)| format!("{d}:{n}"))
            .collect();
        panic!(
            "{name}: {over} channels differ by more than {tolerance}.\n  \
             worst {} at ({x}, {y}): gpu {g:?} vs cpu {w:?}\n  \
             differences: {}",
            worst.0,
            spread.join(" ")
        );
    }
}

/// One channel of slack, and the reason it is exactly one.
///
/// The passes quantise identically — the shader's `q` is the reference's `+ 0.5`
/// cast — so every intermediate is the same 8-bit ladder. What is left is the
/// **ingest and egress** this backend needs and the CPU one does not: vello's
/// output is straight alpha where `vello_cpu`'s `Pixmap` is premultiplied, so the
/// GPU path makes a premultiplied copy on the way in and undoes it on the way
/// out, and a straight→premul→straight round trip at eight bits is not the
/// identity on a partly transparent pixel. The test asserts the *shape* of the
/// difference as well as its size: anything beyond ±1 fails, and the histogram in
/// the failure message is what tells a rounding disagreement from a wrong pass.
const TOLERANCE: u8 = 1;

fn check(name: &str, list: Vec<Effect>) {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping {name}");
        return;
    };
    let src = upload(&g, &fixture());
    let out = fx_gpu::run(&g.device, &g.queue, &g.fx, &src, whole(), &list, SCALE)
        .expect("the stack has ink, so it produces a texture");
    let got = readback(&g, &out);
    compare(name, &got, &reference(&list), TOLERANCE);
}

fn shadow(dx: f64, dy: f64, blur: f64, spread: f64, rgba: [u8; 4]) -> Shadow {
    Shadow {
        offset: Vec2::new(dx, dy),
        blur,
        spread,
        color: Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]),
    }
}

#[test]
#[ignore = "requires a GPU adapter"]
fn colour_filters_match_the_reference() {
    check(
        "filters",
        vec![Effect::new(EffectKind::Filters(Filters {
            brightness: 1.4,
            contrast: 0.8,
            saturation: 1.7,
            hue: 42.0,
        }))],
    );
}

#[test]
#[ignore = "requires a GPU adapter"]
fn a_layer_blur_matches_the_reference() {
    check(
        "layer blur",
        vec![Effect::new(EffectKind::LayerBlur { radius: 9.0 })],
    );
}

#[test]
#[ignore = "requires a GPU adapter"]
fn a_drop_shadow_matches_the_reference() {
    check(
        "drop shadow",
        vec![Effect::new(EffectKind::DropShadow(shadow(
            6.0,
            4.0,
            8.0,
            0.0,
            [0, 0, 0, 160],
        )))],
    );
}

/// **A shadow wide enough to be resampled**, which is three more dispatches on
/// this backend than the reference has functions — `coarsen`, a blur over a grid
/// that is not the buffer, and `upsample` — and the only case in this file where
/// the two implementations do work on *different-sized* grids (§15 D403).
///
/// ⚠️ **Asserting the fixture reaches the state first.** A blur under twice the
/// budget takes the ordinary path, and this test would then pass while saying
/// nothing at all — the vacuous-by-fixture failure this project has been bitten
/// by before. σ is 60 here, so `k` is 3.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_resampled_drop_shadow_matches_the_reference() {
    let sigma = ondin_core::effect::deviation(120.0);
    assert_eq!(
        ondin_core::effect::shadow_downscale(sigma),
        3,
        "the fixture must actually take the coarse path"
    );
    check(
        "resampled drop shadow",
        vec![Effect::new(EffectKind::DropShadow(shadow(
            5.0,
            3.0,
            120.0,
            0.0,
            [10, 20, 200, 220],
        )))],
    );
}

/// **The resample's cost is the buffer's, not the block's** — the one property
/// §15 D403 was written to obtain, and the one `[S10.2-L4-02]` found the shader
/// had never had.
///
/// `coarsen` is dispatched over the *coarse* grid, one thread per block, which
/// makes the total work one read per source pixel — while `k ≤ min(w, h)`. Past
/// that the grid is 1×1 and a single thread iterates `k²` times over a buffer of
/// `w·h`, so the pass becomes exactly the full-size spelling D403 says it was
/// not writing. It is the *only* cost here that does not track the picture, which
/// is why this asserts a clock rather than a pixel.
///
/// ⚠️ **A wall-clock assertion, deliberately, and the threshold is a hundred
/// times the measured figure rather than a tight one.** Best of three on this
/// machine, a 4070 Ti, release: **0.45 ms** with the loop bounded by the source
/// and **116.3 ms** without — **258×** — and the 116 does not move when the
/// buffer does, which is the control that says the block is what is being paid
/// for. The bound is what fails; the exact number is not portable and is not
/// asserted.
///
/// ⚠️ The fixture assertions are the anti-vacuity half. `k` must be past
/// `max(W, H)` or the dispatch is not degenerate and this test is timing the
/// ordinary path — which is what `a_resampled_drop_shadow_matches_the_reference`
/// covers, at `k = 3`, and what it would silently become.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_deeply_resampled_shadow_costs_the_buffer_and_not_the_block() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // σ is `blur / 2` at this file's scale and `k` is `⌈σ / 24⌉`, so this is the
    // block factor a 250-unit blur reaches at 256× zoom — inside what the panel
    // and the view both permit.
    let blur = 64_032.0;
    let k = ondin_core::effect::shadow_downscale(ondin_core::effect::deviation(blur));
    assert_eq!(k, 1334, "the fixture must reach the block factor it names");
    assert!(
        k > W.max(H),
        "and past the buffer, or the dispatch is not the degenerate one: {k} against {}",
        W.max(H)
    );
    let list = vec![Effect::new(EffectKind::DropShadow(shadow(
        0.0,
        0.0,
        blur,
        0.0,
        [0, 0, 0, 255],
    )))];
    let src = upload(&g, &fixture());
    let once = || {
        let at = std::time::Instant::now();
        let out = fx_gpu::run(&g.device, &g.queue, &g.fx, &src, whole(), &list, SCALE)
            .expect("the stack has ink");
        // The readback polls to completion, so this times the work rather than
        // the submission.
        let _ = readback(&g, &out);
        at.elapsed()
    };
    once();
    let best = (0..3).map(|_| once()).min().expect("three runs");
    assert!(
        best.as_millis() < 50,
        "one resampled shadow is a frame's worth of work, not a stall: {best:?}"
    );
}

/// The same shadow with a spread, because `coarsen` reads whatever the spread
/// pass left and a dispatch-size mistake in either shows up as an offset picture
/// rather than as a wrong one.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_resampled_shadow_with_a_spread_matches_the_reference() {
    check(
        "resampled drop shadow + spread",
        vec![Effect::new(EffectKind::DropShadow(shadow(
            -4.0,
            6.0,
            100.0,
            3.0,
            [0, 0, 0, 200],
        )))],
    );
}

/// **Spread and offset in one case, and both non-zero.** A shadow with neither is
/// answered correctly by a silhouette pass that ignores its parameters entirely.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_spread_drop_shadow_matches_the_reference() {
    check(
        "drop shadow + spread",
        vec![Effect::new(EffectKind::DropShadow(shadow(
            -5.0,
            7.0,
            5.0,
            3.0,
            [20, 40, 200, 220],
        )))],
    );
}

/// The inner shadow is the one with three steps nothing else has — an inverted
/// silhouette, a spread that runs the other way, and the mask that confines it to
/// the layer.
#[test]
#[ignore = "requires a GPU adapter"]
fn an_inner_shadow_matches_the_reference() {
    check(
        "inner shadow",
        vec![Effect::new(EffectKind::InnerShadow(shadow(
            3.0,
            -4.0,
            6.0,
            2.0,
            [0, 0, 0, 200],
        )))],
    );
}

/// **The order of the two stages, which one fold gets wrong.** An inner shadow
/// *after* a drop shadow must take its silhouette from the layer, not from the
/// layer plus the shadow — and a `Filters` row before both must be inside what
/// the shadows are cast from. Nothing about a single-effect stack can tell those
/// apart.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_whole_stack_matches_the_reference() {
    check(
        "filters + blur + drop + inner",
        vec![
            Effect::new(EffectKind::Filters(Filters {
                brightness: 1.1,
                contrast: 1.3,
                saturation: 0.4,
                hue: -30.0,
            })),
            Effect::new(EffectKind::LayerBlur { radius: 3.0 }),
            Effect::new(EffectKind::DropShadow(shadow(
                7.0,
                7.0,
                6.0,
                1.0,
                [10, 10, 40, 190],
            ))),
            Effect::new(EffectKind::InnerShadow(shadow(
                -3.0,
                -3.0,
                4.0,
                0.0,
                [255, 240, 200, 160],
            ))),
        ],
    );
}

/// Two of a kind, which is what the repeatable list is *for* (§15 D333) and the
/// case a stack keyed by kind would silently collapse.
#[test]
#[ignore = "requires a GPU adapter"]
fn two_drop_shadows_match_the_reference() {
    check(
        "two drop shadows",
        vec![
            Effect::new(EffectKind::DropShadow(shadow(
                10.0,
                10.0,
                10.0,
                0.0,
                [0, 0, 0, 120],
            ))),
            Effect::new(EffectKind::DropShadow(shadow(
                2.0,
                2.0,
                2.0,
                0.0,
                [200, 0, 0, 200],
            ))),
        ],
    );
}

/// **A stack with nothing to draw produces no texture at all**, so the caller
/// draws the layer directly rather than paying for a copy and an atlas upload
/// that change nothing. Hidden entries and a neutral `Filters` row are the two
/// ways to reach that state without an empty list.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_stack_with_no_ink_asks_for_no_texture() {
    let Some(g) = gpu() else {
        return;
    };
    let src = upload(&g, &fixture());
    let list = vec![
        Effect {
            kind: EffectKind::DropShadow(shadow(4.0, 4.0, 4.0, 0.0, [0, 0, 0, 255])),
            visible: false,
        },
        Effect::new(EffectKind::Filters(Filters::default())),
    ];
    assert!(fx_gpu::run(&g.device, &g.queue, &g.fx, &src, whole(), &list, SCALE).is_none());
}
