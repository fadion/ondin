//! A canvas patched by `render_regions` against the full frame, on-device.
//!
//! The canvas keeps its texture across a whole-pixel pan: it copies what is still
//! in view (`copy_shifted`) and draws only what the pan exposed
//! (`Region::exposed`). A preview changing redraws only `scene::damage`
//! (`copy_regions`). This file holds both to drawing the whole frame, over
//! shapes, strokes, shadows and blurs cut by the canvas edge.
//!
//! `#[ignore]` like the rest of the on-device suite. Run with
//! `cargo test -p ondin-render --release --test gpu_regions -- --ignored --nocapture`.

use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeKind, Operation, Resolved, Shadow,
    Stroke, Transaction, keyed_by_position,
};
use ondin_render::gpu::{Region, copy_regions, copy_shifted};
use ondin_render::{ImageStore, RenderOverrides, VelloGpuRenderer, Viewport};

const W: u32 = 320;
const H: u32 = 240;
const GROUND: Color = Color::from_rgba8(30, 32, 36, 255);
/// How many levels vello's antialiasing of an edge moves by when the drawing is
/// rasterized at another whole-pixel offset.
const AA_DRIFT: u8 = 2;
/// How many levels a stroke's edge pixel moves by there: vello's stroke expansion
/// places the edge up to about 0.05 px apart.
const STROKE_DRIFT: u8 = 16;

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
    full_under(g, doc, res, vp, &RenderOverrides::default(), tex);
}

/// `full` drawn under a preview.
fn full_under(
    g: &mut Gpu,
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    ov: &RenderOverrides,
    tex: &wgpu::Texture,
) {
    let view = tex.create_view(&Default::default());
    g.renderer
        .render(
            &g.device,
            &g.queue,
            doc,
            res,
            vp,
            ov,
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

/// A frame patched through a run of previews — a layer moved inside its card, a
/// card moved whole, a recolour in another card, a new shape dragged out on the
/// page, and back to nothing — redrawing only `scene::damage`, is the full frame
/// under each preview at every step.
///
/// To within `AA_DRIFT`, but for one edge column of a stroke: an effect layer
/// lands at another offset in its pass's packed texture when fewer layers share
/// the pass, and vello's stroke expansion can place an edge 0.05 px apart there
/// (`STROKE_DRIFT`). A region left stale is a shape off at full strength.
///
/// flip: `scene::damage` measuring the touched items under `after` alone → fails
/// at the second step, where the card's old place is never redrawn.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_frame_patched_where_a_preview_changed_is_the_full_frame() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
    let (doc, res) = cards();
    let page = doc.first_page();
    let cards: Vec<_> = doc.get(page).expect("the page").children().to_vec();
    let inside = |card: usize, at: usize| doc.get(cards[card]).expect("a card").children()[at];
    let (dot, card, bar) = (inside(1, 1), cards[6], inside(9, 0));
    let local = |id| doc.get(id).expect("a node").transform();
    let new = IdSource::new(10_000).mint();
    let steps: Vec<Transaction> = vec![
        Transaction(vec![Operation::SetTransform {
            id: dot,
            transform: Affine::translate((-30.0, 12.0)) * local(dot),
        }]),
        Transaction(vec![Operation::SetTransform {
            id: card,
            transform: Affine::translate((47.0, -21.0)) * local(card),
        }]),
        Transaction(vec![
            Operation::SetTransform {
                id: card,
                transform: Affine::translate((47.0, -21.0)) * local(card),
            },
            Operation::SetFills {
                id: bar,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(250, 200, 0, 255)).into(),
                    visible: true,
                }]),
            },
        ]),
        Transaction(vec![
            Operation::CreateNode {
                id: new,
                parent: page,
                index: cards.len(),
                kind: NodeKind::Rect {
                    size: Size::new(70.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((150.0, 120.0))),
                name: None,
            },
            Operation::SetFills {
                id: new,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(255, 0, 200, 255)).into(),
                    visible: true,
                }]),
            },
        ]),
        Transaction::default(),
    ];
    let size = (W, H);
    let vp = viewport((7, -3), 0.83, size);
    let shown = target(&g, size);
    let spare = target(&g, size);
    let mut before = RenderOverrides::default();
    full(&mut g, &doc, &res, &vp, &shown);
    for (i, tx) in steps.iter().enumerate() {
        let after = RenderOverrides::from_transaction(&doc, &res, tx).expect("a preview");
        let damage = ondin_render::scene::damage(&doc, &res, &vp, page, &before, &after)
            .expect("bounded damage");
        let regions: Vec<Region> = damage
            .iter()
            .filter_map(|r| Region::covering(*r, 2.0, size))
            .collect();
        let drawn = g
            .renderer
            .render_regions(
                &g.device,
                &g.queue,
                &doc,
                &res,
                &vp,
                &after,
                &spare.create_view(&Default::default()),
                &regions,
                GROUND,
                &ImageStore::default(),
                page,
            )
            .expect("render regions");
        assert!(drawn, "step {i}: not pinned");
        copy_regions(&g.device, &g.queue, &spare, &shown, &regions);
        let reference = target(&g, size);
        full_under(&mut g, &doc, &res, &vp, &after, &reference);
        let (now, want) = (read(&g, &shown, size), read(&g, &reference, size));
        let (worst, count) = diff(&now, &want, |_, _| true);
        let beyond = now
            .as_chunks::<4>()
            .0
            .iter()
            .zip(want.as_chunks::<4>().0)
            .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > AA_DRIFT))
            .count();
        eprintln!(
            "step {i}: {} regions, {count} pixels off by up to {worst}, {beyond} past the drift",
            regions.len()
        );
        assert!(
            worst <= STROKE_DRIFT && beyond <= 16,
            "step {i}: {beyond} pixels are off the full frame by more than {AA_DRIFT}, the worst by {worst}"
        );
        before = after;
    }
}

/// Damage cannot be bounded per item when the page's children depend on each
/// other: a mask among them clips the ones after it, and a reorder changes which
/// covers which.
///
/// flip: `top_level_ink` not looking for a mask among the page's children →
/// fails at "a mask on the page".
#[test]
fn damage_is_unbounded_where_the_pages_children_depend_on_each_other() {
    let (mut doc, res) = cards();
    let page = doc.first_page();
    let cards: Vec<_> = doc.get(page).expect("the page").children().to_vec();
    let vp = viewport((0, 0), 1.0, (W, H));
    let moved = Transaction(vec![Operation::SetTransform {
        id: cards[0],
        transform: Affine::translate((5.0, 5.0)),
    }]);
    let none = RenderOverrides::default();
    let ov = RenderOverrides::from_transaction(&doc, &res, &moved).expect("a preview");
    assert!(
        ondin_render::scene::damage(&doc, &res, &vp, page, &none, &ov).is_some(),
        "fixture: a move is bounded"
    );
    let reorder = Transaction(vec![Operation::Reorder {
        id: cards[0],
        index: 3,
    }]);
    let ov = RenderOverrides::from_transaction(&doc, &res, &reorder).expect("a preview");
    assert!(
        ov.children_of(page).is_some(),
        "fixture: the page is reordered"
    );
    assert!(
        ondin_render::scene::damage(&doc, &res, &vp, page, &none, &ov).is_none(),
        "a reorder of the page"
    );
    let mask = IdSource::new(10_000).mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: mask,
            parent: page,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 50.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetMask {
            id: mask,
            mask: true,
        },
    ]))
    .expect("a mask");
    let res = Resolved::rebuild(&doc);
    let ov = RenderOverrides::from_transaction(&doc, &res, &moved).expect("a preview");
    assert!(
        ondin_render::scene::damage(&doc, &res, &vp, page, &none, &ov).is_none(),
        "a mask on the page"
    );
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

/// A picture still draws after the renderer drew a frame with no picture,
/// gradient or text in it — a pan's band over plain shapes.
///
/// flip: `gpu::hold_atlas` not called in `VelloGpuRenderer::draw` → fails at
/// "after a plain region", where the picture comes back blank.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_picture_survives_a_frame_that_draws_none() {
    use ondin_core::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
    let side = 8u32;
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let px: Vec<u8> = (0..side * side).flat_map(|_| [255u8, 0, 0, 255]).collect();
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&px).expect("png data");
    }
    let iid = ImageId("sha256:picture".into());
    let mut images = ImageStore::new();
    images
        .insert(&iid, &png, ImageFormat::Png)
        .expect("the fixture decodes");
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (pic, plain) = (ids.mint(), ids.mint());
    let square = |id, index, x: f64| Operation::CreateNode {
        id,
        parent: root,
        index,
        kind: NodeKind::Rect {
            size: Size::new(30.0, 30.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((x, 10.0))),
        name: None,
    };
    doc.apply(&Transaction(vec![
        Operation::AddImage {
            id: iid.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(png.clone().into()),
                format: ImageFormat::Png,
                width: side,
                height: side,
            },
        },
        square(pic, 0, 10.0),
        Operation::SetFills {
            id: pic,
            fills: keyed_by_position([Fill {
                brush: image_brush(iid).into(),
                visible: true,
            }]),
        },
        square(plain, 1, 200.0),
        Operation::SetFills {
            id: plain,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(90, 90, 90, 255)).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    let (page, size) = (doc.first_page(), (W, H));
    let vp = viewport((0, 0), 1.0, size);
    let ov = RenderOverrides::default();
    let tex = target(&g, size);
    let view = tex.create_view(&Default::default());
    let draw = |g: &mut Gpu| {
        g.renderer
            .render(
                &g.device, &g.queue, &doc, &res, &vp, &ov, &view, GROUND, &images, page,
            )
            .expect("render");
        let px = read(g, &tex, size);
        let i = ((25 * W + 25) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    assert_eq!(draw(&mut g), [255, 0, 0], "the fixture draws its picture");
    let band = [Region {
        at: (190, 0),
        size: (50, 50),
    }];
    let drawn = g
        .renderer
        .render_regions(
            &g.device, &g.queue, &doc, &res, &vp, &ov, &view, &band, GROUND, &images, page,
        )
        .expect("regions");
    assert!(drawn, "the plain band is drawn, not refused");
    assert_eq!(draw(&mut g), [255, 0, 0], "after a plain region");
}

/// `regions` drawn alone by `render_regions` against the full frame there: the
/// largest channel difference, how many pixels differ, and how many of the full
/// frame's pixels there are not the ground.
fn regions_against_full(
    g: &mut Gpu,
    doc: &Document,
    res: &Resolved,
    vp: &Viewport,
    regions: &[Region],
) -> (u8, usize, usize) {
    let size = vp.pixel_size;
    let patched = target(g, size);
    let drawn = g
        .renderer
        .render_regions(
            &g.device,
            &g.queue,
            doc,
            res,
            vp,
            &RenderOverrides::default(),
            &patched.create_view(&Default::default()),
            regions,
            GROUND,
            &ImageStore::default(),
            doc.first_page(),
        )
        .expect("render regions");
    assert!(drawn, "the regions are drawn, not refused");
    let reference = target(g, size);
    full(g, doc, res, vp, &reference);
    let (now, want) = (read(g, &patched, size), read(g, &reference, size));
    let inside = |x: u32, y: u32| {
        regions.iter().any(|r| {
            (r.at.0..r.at.0 + r.size.0).contains(&x) && (r.at.1..r.at.1 + r.size.1).contains(&y)
        })
    };
    let (worst, count) = diff(&now, &want, inside);
    let ground = GROUND.to_rgba8().to_u8_array();
    let inked = want
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(i, p)| inside(*i as u32 % W, *i as u32 / W) && **p != ground)
        .count();
    (worst, count, inked)
}

/// Ink past a layer's box — a trimmed text's descenders, a mitred star's top
/// point — is drawn in a region that misses the box but not the ink.
///
/// flip: `world_ink_of_parts` returning `world_bounds_of_parts` → fails at
/// "descenders".
#[test]
#[ignore = "requires a GPU adapter"]
fn ink_past_a_box_is_drawn_in_a_region_beside_it() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (words, star) = (ids.mint(), ids.mint());
    let white = keyed_by_position([Fill {
        brush: Brush::Solid(Color::WHITE).into(),
        visible: true,
    }]);
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: words,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "gypsy".into(),
                style: Box::new(ondin_core::TextStyle {
                    font_size: 40.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: ondin_core::BlockStyle {
                    trim: ondin_core::BoxTrim::CapToBaseline,
                    ..Default::default()
                },
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        },
        Operation::SetFills {
            id: words,
            fills: white.clone(),
        },
        Operation::CreateNode {
            id: star,
            parent: root,
            index: 1,
            kind: NodeKind::Star {
                size: Size::new(100.0, 100.0),
                points: 5,
                inner_ratio: 0.38,
                corner_radius: 0.0,
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((180.0, 100.0))),
            name: None,
        },
        Operation::SetStrokes {
            id: star,
            strokes: keyed_by_position([Stroke {
                brush: Color::WHITE.into(),
                width: 12.0,
                ..Stroke::default()
            }]),
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    let vp = viewport((0, 0), 1.0, (W, H));
    let text_box = res.world_bounds(words).expect("a box");
    let star_box = res.world_bounds(star).expect("a box");
    for (what, region) in [
        (
            "descenders",
            Region {
                at: (0, text_box.y1.ceil() as u32 + 3),
                size: (W / 2, 20),
            },
        ),
        (
            "the star's point",
            Region {
                at: (W / 2, star_box.y0.floor() as u32 - 23),
                size: (W / 2, 20),
            },
        ),
    ] {
        let (worst, count, inked) = regions_against_full(&mut g, &doc, &res, &vp, &[region]);
        assert!(inked > 0, "fixture: {what} reach into the region");
        assert!(
            worst <= AA_DRIFT,
            "{what}: {count} pixels are off the full frame, by up to {worst}"
        );
    }
}

/// A child of a shadowed group, clear of a region its shadow falls in, still
/// casts that shadow there.
///
/// flip: `paint_node_pass` culling the children against the bare view → fails
/// at "the child's shadow".
#[test]
#[ignore = "requires a GPU adapter"]
fn a_groups_shadow_is_drawn_in_a_region_clear_of_its_child() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        eprintln!("no adapter; skipping");
        return;
    };
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (group, child) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: child,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        },
        Operation::SetFills {
            id: child,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::WHITE).into(),
                visible: true,
            }]),
        },
        Operation::SetEffects {
            id: group,
            effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 50.0),
                blur: 0.0,
                spread: 0.0,
                color: Color::from_rgba8(255, 200, 0, 255),
                blend: ondin_core::BlendMode::Normal,
            }))]),
        },
    ]))
    .expect("the fixture applies");
    let res = Resolved::rebuild(&doc);
    let vp = viewport((0, 0), 1.0, (W, H));
    // The child spans y 20..40 and its shadow 70..90.
    let region = Region {
        at: (0, 60),
        size: (W / 2, 40),
    };
    let (worst, count, inked) = regions_against_full(&mut g, &doc, &res, &vp, &[region]);
    assert!(inked > 0, "fixture: the shadow lands in the region");
    assert!(
        worst <= AA_DRIFT,
        "the child's shadow: {count} pixels are off the full frame, by up to {worst}"
    );
}

/// A shadowed group inside a scaled one, dragged up past the rest of its page in
/// steps whose damage lies clear of where it was committed, is the full frame
/// at every step: the page, whose cached box is the committed content, is not
/// culled away from the regions with the group in it.
/// flip: `paint_node`'s cull no longer sparing a container `Walk::grown` names →
/// fails at the second step, where the patched frame loses the group.
#[test]
#[ignore = "requires a GPU adapter"]
fn a_group_dragged_past_the_rest_of_its_page_is_still_drawn() {
    let _one = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut g) = gpu() else {
        return;
    };
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (outer, scaled, shadowed, art, below) =
        (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let rect = |w: f64, h: f64| NodeKind::Rect {
        size: Size::new(w, h),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    };
    let white = || {
        keyed_by_position([Fill {
            brush: Brush::Solid(Color::WHITE).into(),
            visible: true,
        }])
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: outer,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((40.0, 60.0))),
            name: None,
        },
        Operation::CreateNode {
            id: scaled,
            parent: outer,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::new([0.75, 0.0, 0.0, 0.75, 0.25, 0.3])),
            name: None,
        },
        Operation::CreateNode {
            id: shadowed,
            parent: scaled,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: art,
            parent: shadowed,
            index: 0,
            kind: rect(14.0, 22.0),
            transform: Some(Affine::translate((6.0, 4.0))),
            name: None,
        },
        Operation::SetFills {
            id: art,
            fills: white(),
        },
        Operation::SetEffects {
            id: shadowed,
            effects: keyed_by_position([shadow((0.0, 0.8), 0.93, 0.0)]),
        },
        // The rest of the page, below the group.
        Operation::CreateNode {
            id: below,
            parent: root,
            index: 1,
            kind: rect(30.0, 20.0),
            transform: Some(Affine::translate((90.0, 90.0))),
            name: None,
        },
        Operation::SetFills {
            id: below,
            fills: white(),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let page = doc.first_page();
    let size = (W, H);
    let vp = viewport((0, 0), 2.0, size);
    let shown = target(&g, size);
    let spare = target(&g, size);
    let mut before = RenderOverrides::default();
    full(&mut g, &doc, &res, &vp, &shown);
    // Each step's old and new places both lie above the committed page's ink.
    for (i, dy) in [-30.0, -45.0, -55.0].into_iter().enumerate() {
        let tx = Transaction(vec![Operation::SetTransform {
            id: outer,
            transform: Affine::translate((40.0, 60.0 + dy)),
        }]);
        let after = RenderOverrides::from_transaction(&doc, &res, &tx).expect("a preview");
        let reference = target(&g, size);
        full_under(&mut g, &doc, &res, &vp, &after, &reference);
        let damage = ondin_render::scene::damage(&doc, &res, &vp, page, &before, &after)
            .expect("bounded damage");
        let regions: Vec<Region> = damage
            .iter()
            .filter_map(|r| Region::covering(*r, 2.0, size))
            .collect();
        let drawn = g
            .renderer
            .render_regions(
                &g.device,
                &g.queue,
                &doc,
                &res,
                &vp,
                &after,
                &spare.create_view(&Default::default()),
                &regions,
                GROUND,
                &ImageStore::default(),
                page,
            )
            .expect("render regions");
        assert!(drawn, "step {i}: not pinned");
        copy_regions(&g.device, &g.queue, &spare, &shown, &regions);
        let (now, want) = (read(&g, &shown, size), read(&g, &reference, size));
        let inked = want
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 200)
            .count();
        assert!(inked > 0, "fixture: step {i} draws the group");
        let (worst, count) = diff(&now, &want, |_, _| true);
        assert!(
            worst <= AA_DRIFT,
            "step {i}: {count} pixels are off the full frame, by up to {worst}"
        );
        before = after;
    }
}
