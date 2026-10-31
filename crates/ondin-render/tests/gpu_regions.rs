//! A panned canvas patched by `render_regions` against the full frame, on-device.
//!
//! The canvas keeps its texture across a whole-pixel pan: it copies what is still
//! in view (`copy_shifted`) and draws only what the pan exposed
//! (`Region::exposed`). This file holds that to drawing the whole frame at the
//! new camera, pixel for pixel, over shapes, strokes, shadows and blurs cut by
//! the canvas edge.
//!
//! `#[ignore]` like the rest of the on-device suite. Run with
//! `cargo test -p ondin-render --release --test gpu_regions -- --ignored --nocapture`.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeKind, Operation, Resolved, Shadow,
    Stroke, Transaction, keyed_by_position,
};
use ondin_render::gpu::{Region, copy_shifted};
use ondin_render::{ImageStore, RenderOverrides, VelloGpuRenderer, Viewport};

const W: u32 = 320;
const H: u32 = 240;
const GROUND: Color = Color::from_rgba8(30, 32, 36, 255);
/// How many levels vello's antialiasing of an edge moves by when the drawing is
/// rasterized at another whole-pixel offset.
const AA_DRIFT: u8 = 2;

/// One test at a time holds the adapter: devices made side by side hung the
/// on-device suite intermittently.
static ADAPTER: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

fn target(g: &Gpu, size: (u32, u32)) -> wgpu::Texture {
    g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-regions-target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
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

fn read(g: &Gpu, tex: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
    let bpr = (size.0 * 4).div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bpr * size.1),
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
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
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
    let mut out = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for y in 0..size.1 {
        let row = (y * bpr) as usize;
        out.extend_from_slice(&data[row..row + (size.0 * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

/// The canvas with world `(0, 0)` at device pixel `origin` and `zoom` device
/// pixels per unit — a whole-pixel origin, as the app's camera keeps.
fn viewport(origin: (i32, i32), zoom: f64, size: (u32, u32)) -> Viewport {
    let x0 = -f64::from(origin.0) / zoom;
    let y0 = -f64::from(origin.1) / zoom;
    Viewport {
        view: Rect::new(
            x0,
            y0,
            x0 + f64::from(size.0) / zoom,
            y0 + f64::from(size.1) / zoom,
        ),
        pixel_size: size,
    }
}

fn shadow(offset: (f64, f64), blur: f64, spread: f64) -> Effect {
    Effect::new(EffectKind::DropShadow(Shadow {
        offset: Vec2::new(offset.0, offset.1),
        blur,
        spread,
        color: Color::from_rgba8(0, 0, 0, 140),
        blend: ondin_core::BlendMode::Normal,
    }))
}

/// A grid of cards: rounded frames holding a stroked rect, an ellipse and a
/// blurred group, with drop shadows — the walk's main paths, at sizes and
/// positions that put edges between pixels at a fractional zoom.
fn cards() -> (Document, Resolved) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut ops = Vec::new();
    let mut fx = Vec::new();
    let fill = |c: Color| {
        keyed_by_position([Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }])
    };
    for i in 0..12 {
        let (col, row) = (i % 4, i / 4);
        let card = ids.mint();
        ops.push(Operation::CreateNode {
            id: card,
            parent: root,
            index: i,
            kind: NodeKind::Artboard {
                size: Size::new(97.3, 71.6),
                corner_radii: RoundedRectRadii::from_single_radius(9.0),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((
                col as f64 * 121.7 - 40.0,
                row as f64 * 93.1 - 30.0,
            ))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: card,
            fills: fill(Color::from_rgba8(240, 240 - i as u8 * 9, 230, 255)),
        });
        fx.push((card, vec![shadow((3.0, 5.0), 10.0 + i as f64, 0.0)]));
        let bar = ids.mint();
        ops.push(Operation::CreateNode {
            id: bar,
            parent: card,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(61.5, 11.25),
                corner_radii: RoundedRectRadii::from_single_radius(2.0),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((8.4, 9.7))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: bar,
            fills: fill(Color::from_rgba8(60, 90, 200, 255)),
        });
        ops.push(Operation::SetStrokes {
            id: bar,
            strokes: keyed_by_position([Stroke {
                brush: Color::from_rgba8(10, 20, 60, 255).into(),
                width: 1.5,
                ..Stroke::default()
            }]),
        });
        let dot = ids.mint();
        ops.push(Operation::CreateNode {
            id: dot,
            parent: card,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(23.0, 23.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((63.0, 38.0)).then_rotate(0.1)),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: dot,
            fills: fill(Color::from_rgba8(220, 60, 40, 255)),
        });
        fx.push((dot, vec![shadow((-2.0, 2.0), 4.0, 2.0)]));
        let g = ids.mint();
        ops.push(Operation::CreateNode {
            id: g,
            parent: card,
            index: 2,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        let blob = ids.mint();
        ops.push(Operation::CreateNode {
            id: blob,
            parent: g,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(30.0, 18.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((10.0, 40.0))),
            name: None,
        });
        ops.push(Operation::SetFills {
            id: blob,
            fills: fill(Color::from_rgba8(30, 160, 90, 255)),
        });
        fx.push((
            g,
            vec![Effect::new(EffectKind::LayerBlur {
                radius: 3.0 + (i % 3) as f64,
            })],
        ));
    }
    doc.apply(&Transaction(ops)).expect("the fixture applies");
    let ops = fx
        .into_iter()
        .map(|(id, effects)| Operation::SetEffects {
            id,
            effects: keyed_by_position(effects),
        })
        .collect();
    doc.apply(&Transaction(ops)).expect("the effects apply");
    let res = Resolved::rebuild(&doc);
    (doc, res)
}

fn full(g: &mut Gpu, doc: &Document, res: &Resolved, vp: &Viewport, tex: &wgpu::Texture) {
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
            GROUND,
            &ImageStore::default(),
            doc.first_page(),
        )
        .expect("render");
}

/// The largest channel difference between two frames over the pixels `keep`
/// selects, and how many of them differ.
fn diff(a: &[u8], b: &[u8], keep: impl Fn(u32, u32) -> bool) -> (u8, usize) {
    let mut worst = 0u8;
    let mut count = 0usize;
    let (a, b) = (a.as_chunks::<4>().0, b.as_chunks::<4>().0);
    for (i, (pa, pb)) in a.iter().zip(b).enumerate() {
        if !keep(i as u32 % W, i as u32 / W) {
            continue;
        }
        let d = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        worst = worst.max(d);
        count += usize::from(d > 0);
    }
    (worst, count)
}

/// A walk of pans — each axis alone, both, back, and one past the canvas — kept
/// in one texture by shift and regions. At every step the exposed regions are the
/// full frame at the new camera and the rest is the last frame moved, bit for bit.
///
/// "The full frame" to within `AA_DRIFT` on a handful of pixels: vello's
/// antialiasing of a curved or rotated edge moves by a level when the same drawing
/// is rasterized at another whole-pixel offset, and an effect layer lands at
/// another offset in its pass's packed texture when fewer layers share the pass.
/// Kept pixels were drawn a pan ago, so against a full frame they differ as two
/// full frames a pan apart do (printed as the control): by a level or two, and on
/// one pan here by 14 along one edge column of a stroke, which vello's stroke
/// expansion places 0.05 px apart at the two offsets.
///
/// flip: `render_regions` culling to the regions' device rectangles instead of
/// their world ones → fails at the exposed assertion on the third pan;
/// `copy_shifted` moving by the negated shift → fails at the exposed assertion on
/// the first, the copy landing over the band it should leave.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_canvas_kept_across_pans_draws_the_full_frame_where_it_draws() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
    let (doc, res) = cards();
    let size = (W, H);
    let zoom = 0.83;
    let mut origin = (7, -3);
    let mut shown = target(&g, size);
    full(&mut g, &doc, &res, &viewport(origin, zoom, size), &shown);
    assert!(
        !g.renderer.pinned(),
        "the fixture's buffers are exact under a pan"
    );
    for shift in [(17, 0), (0, -11), (-23, 31), (40, 40), (-5, -60), (400, 3)] {
        let before = read(&g, &shown, size);
        origin = (origin.0 + shift.0, origin.1 + shift.1);
        let vp = viewport(origin, zoom, size);
        let next = target(&g, size);
        let regions = Region::exposed(size, shift);
        let drawn = g
            .renderer
            .render_regions(
                &g.device,
                &g.queue,
                &doc,
                &res,
                &vp,
                &RenderOverrides::default(),
                &next.create_view(&Default::default()),
                &regions,
                GROUND,
                &ImageStore::default(),
                doc.first_page(),
            )
            .expect("render regions");
        assert!(drawn, "{shift:?}: the regions are not pinned");
        copy_shifted(&g.device, &g.queue, &shown, &next, size, shift);
        let reference = target(&g, size);
        full(&mut g, &doc, &res, &vp, &reference);
        let (now, want) = (read(&g, &next, size), read(&g, &reference, size));
        let exposed = |x: u32, y: u32| {
            regions.iter().any(|r| {
                (r.at.0..r.at.0 + r.size.0).contains(&x) && (r.at.1..r.at.1 + r.size.1).contains(&y)
            })
        };
        let (worst, count) = diff(&now, &want, exposed);
        let area: u32 = regions.iter().map(|r| r.size.0 * r.size.1).sum();
        assert!(
            worst <= AA_DRIFT && count * 100 <= area as usize,
            "{shift:?}: {count} of {area} exposed pixels are off the full frame, by up to {worst}"
        );
        // The kept pixels against the last frame read at their old place.
        let mut moved = vec![0u8; now.len()];
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let (sx, sy) = (x - shift.0, y - shift.1);
                if sx >= 0 && sy >= 0 && sx < W as i32 && sy < H as i32 {
                    let d = ((y * W as i32 + x) * 4) as usize;
                    let s = ((sy * W as i32 + sx) * 4) as usize;
                    moved[d..d + 4].copy_from_slice(&before[s..s + 4]);
                }
            }
        }
        let (worst, count) = diff(&now, &moved, |x, y| !exposed(x, y));
        assert_eq!(
            worst, 0,
            "{shift:?}: {count} kept pixels are not the last frame's"
        );
        let (worst, count) = diff(&now, &want, |_, _| true);
        eprintln!("{shift:?}: against a full frame, {count} pixels differ by up to {worst}");
        shown = next;
    }
}

/// A shadowed layer larger than the canvas has its buffer's escape given up to
/// the page's budget, which depends on where the canvas sits: the frame is
/// pinned, and drawing its regions refuses rather than patch a moved frame.
///
/// flip: `push_effect_layer` not comparing the clipped box against the whole one
/// → fails at the first `pinned()` assertion.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_frame_shaped_by_where_the_canvas_sits_is_pinned() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
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
                size: Size::new(2000.0, 2000.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((-800.0, -800.0))),
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
            effects: keyed_by_position([shadow((0.0, 8.0), 40.0, 0.0)]),
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    let size = (W, H);
    let vp = viewport((0, 0), 1.0, size);
    let tex = target(&g, size);
    full(&mut g, &doc, &res, &vp, &tex);
    assert!(
        g.renderer.pinned(),
        "a budget-trimmed buffer pins the frame"
    );
    let drawn = g
        .renderer
        .render_regions(
            &g.device,
            &g.queue,
            &doc,
            &res,
            &vp,
            &RenderOverrides::default(),
            &tex.create_view(&Default::default()),
            &Region::exposed(size, (10, 0)),
            GROUND,
            &ImageStore::default(),
            doc.first_page(),
        )
        .expect("render regions");
    assert!(!drawn, "the regions of a pinned frame are refused");
}

/// The exposed regions tile what the shift uncovered, without overlap.
#[test]
fn the_exposed_regions_cover_what_the_shift_uncovered() {
    let size = (50u32, 40u32);
    for shift in [
        (0, 0),
        (3, 0),
        (0, -4),
        (-7, 9),
        (12, 12),
        (-49, -39),
        (50, 0),
        (0, -40),
    ] {
        let mut hits = vec![0u8; (size.0 * size.1) as usize];
        for r in Region::exposed(size, shift) {
            for y in r.at.1..r.at.1 + r.size.1 {
                for x in r.at.0..r.at.0 + r.size.0 {
                    hits[(y * size.0 + x) as usize] += 1;
                }
            }
        }
        for y in 0..size.1 as i32 {
            for x in 0..size.0 as i32 {
                let (sx, sy) = (x - shift.0, y - shift.1);
                let kept = sx >= 0 && sy >= 0 && sx < size.0 as i32 && sy < size.1 as i32;
                let want = u8::from(!kept);
                assert_eq!(
                    hits[(y as u32 * size.0 + x as u32) as usize],
                    want,
                    "{shift:?} at {x},{y}"
                );
            }
        }
    }
}
