//! Throwaway on-device sweep for the extreme-zoom watchdog crash — the second
//! timeout §15 D395 recorded and did not fix (roadmap, *Now · Canvas*).
//!
//! `#[ignore]` like `gpu_device.rs`, and **run in release**: a debug scene walk
//! is dominated by our own unoptimised code and says nothing about the GPU.
//!
//! ```text
//! cargo test -p ondin-render --release --test zoom_sweep -- --ignored --nocapture
//! ```
//!
//! `ONDIN_SWEEP_DOC` points it at a real drawing (`.ondin` or `.svg`); with
//! nothing set it builds a synthetic illustration of the same order — a few
//! hundred paths, a couple of blurred groups — because the drawing that crashed
//! is not in the tree.
//!
//! ⚠️ **The sweep is the instrument, not one frame** (D395): a single expensive
//! frame passes, and 600 repeats of it pass; the device dies when a genuinely
//! expensive frame is followed by another. So every line is flushed as it is
//! measured — the interesting one is usually the last one printed before the
//! panic, and the panic's own label names whatever the *next* frame touched.

use std::time::Instant;

use ondin_core::kurbo::{Affine, BezPath, Point, Rect, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeId, NodeKind, Operation, Resolved,
    Stroke, Transaction,
};
use ondin_render::{ImageStore, RenderOverrides, VelloGpuRenderer, Viewport};

/// A deterministic little LCG — the fixture must be the same drawing on every
/// run or two sweeps cannot be compared.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next() * (hi - lo)
    }
}

/// One closed blob of `segs` cubics around `centre`, roughly `r` across.
fn blob(rng: &mut Rng, centre: Point, r: f64, segs: usize) -> BezPath {
    let mut p = BezPath::new();
    let mut pts = Vec::new();
    for i in 0..segs {
        let a = (i as f64) / (segs as f64) * std::f64::consts::TAU;
        let rr = r * rng.range(0.6, 1.4);
        pts.push(centre + Vec2::new(a.cos() * rr, a.sin() * rr));
    }
    p.move_to(pts[0]);
    for i in 0..segs {
        let a = pts[i];
        let b = pts[(i + 1) % segs];
        let d = (b - a) * 0.35;
        p.curve_to(a + d + Vec2::new(-d.y, d.x) * 0.4, b - d, b);
    }
    p.close_path();
    p
}

/// A drawing of the same order as the one that crashed: a few hundred paths,
/// sizes from body-panel to detail, two blurred groups and a shadowed one.
///
/// The size distribution is the part that matters. A zoomed-in frame only costs
/// what survives the viewport cull, so a fixture of uniformly *small* blobs
/// measures the cull rather than the rasterizer — a real illustration has a
/// handful of shapes that cover everything and hundreds that do not.
fn synthetic(paths: usize) -> (Document, Rect) {
    let mut rng = Rng(0x5eed);
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (w, h) = (1400.0, 800.0);

    let ab = ids.mint();
    let mut ops = vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(w, h),
        },
        transform: None,
        name: Some("Artboard".into()),
    }];

    // Six groups, two blurred and one shadowed — the effect batch re-rasterizes
    // whatever is inside these, which is the second half of the cost D395 names.
    let groups: Vec<NodeId> = (0..6).map(|_| ids.mint()).collect();
    for (i, g) in groups.iter().enumerate() {
        ops.push(Operation::CreateNode {
            id: *g,
            parent: ab,
            index: i,
            kind: NodeKind::Group,
            transform: None,
            name: Some(format!("Group {i}")),
        });
        let fx = match i {
            1 => vec![Effect::new(EffectKind::LayerBlur { radius: 55.5 })],
            4 => vec![Effect::new(EffectKind::LayerBlur { radius: 24.0 })],
            2 => vec![Effect::new(EffectKind::DropShadow(ondin_core::Shadow {
                // A shadow's kernel is the one term §15 D395 left growing with the
                // zoom on purpose (only a `LayerBlur` may buy a downscale), so it
                // is the knob this probe most wants.
                blur: std::env::var("ONDIN_SWEEP_SHADOW_BLUR")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(18.0),
                offset: Vec2::new(6.0, 8.0),
                ..Default::default()
            }))],
            _ => vec![],
        };
        if !fx.is_empty() {
            ops.push(Operation::SetEffects {
                id: *g,
                effects: fx,
            });
        }
    }

    let mut in_group = vec![0usize; groups.len()];
    for i in 0..paths {
        let id = ids.mint();
        let gi = i % groups.len();
        let g = groups[gi];
        let index = in_group[gi];
        in_group[gi] += 1;
        // 15% cover most of the drawing, 35% are panel-sized, the rest detail.
        let r = match rng.next() {
            x if x < 0.15 => rng.range(300.0, 700.0),
            x if x < 0.50 => rng.range(60.0, 260.0),
            _ => rng.range(4.0, 40.0),
        };
        let centre = Point::new(rng.range(0.0, w), rng.range(0.0, h));
        let segs = 6 + (rng.range(0.0, 12.0) as usize);
        ops.push(Operation::CreateNode {
            id,
            parent: g,
            index,
            kind: NodeKind::Path {
                path: blob(&mut rng, Point::ZERO, r, segs),
                corner_radii: Vec::new(),
            },
            transform: Some(Affine::translate(centre.to_vec2())),
            name: None,
        });
        ops.push(Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(
                    rng.range(20.0, 240.0) as u8,
                    rng.range(20.0, 240.0) as u8,
                    rng.range(20.0, 240.0) as u8,
                    rng.range(120.0, 255.0) as u8,
                )),
                visible: true,
            }],
        });
        if rng.next() < 0.25 {
            ops.push(Operation::SetStrokes {
                id,
                strokes: vec![Stroke {
                    brush: Brush::Solid(Color::from_rgba8(30, 30, 40, 255)),
                    width: rng.range(0.5, 4.0),
                    ..Default::default()
                }],
            });
        }
    }

    doc.apply(&Transaction(ops)).expect("build fixture");
    (doc, Rect::new(0.0, 0.0, w, h))
}

/// The drawing under test, and the box the sweep zooms around.
fn fixture() -> (Document, Rect) {
    let Ok(path) = std::env::var("ONDIN_SWEEP_DOC") else {
        let n: usize = std::env::var("ONDIN_SWEEP_PATHS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(417);
        eprintln!("fixture: synthetic, {n} paths");
        return synthetic(n);
    };
    let bytes = std::fs::read(&path).expect("read ONDIN_SWEEP_DOC");
    let doc = if path.to_ascii_lowercase().ends_with(".svg") {
        let svg = String::from_utf8(bytes).expect("svg is utf-8");
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let im = ondin_core::svg_in::import(&svg, &mut ids, root, 0, None).expect("import svg");
        eprintln!(
            "fixture: {path}, {} shapes, skipped {:?}, approximated {:?}",
            im.shapes, im.skipped, im.approximated
        );
        doc.apply(&im.tx).expect("apply import");
        doc
    } else {
        eprintln!("fixture: {path}");
        ondin_core::io::load(&bytes).expect("load .ondin")
    };
    let res = Resolved::rebuild(&doc);
    let bounds = res
        .ink_bounds(doc.root())
        .unwrap_or(Rect::new(0.0, 0.0, 1000.0, 1000.0));
    (doc, bounds)
}

/// How many nodes the walk's viewport cull would let through — the sweep's own
/// reading of `scene::paint_node`'s rule, so it is an estimate rather than the
/// walk's own count, and it is here to say whether an expensive frame is
/// expensive because of *many* paths or because of a few enormous ones.
fn visible(doc: &Document, res: &Resolved, view: Rect) -> (usize, usize) {
    let mut total = 0;
    let mut shown = 0;
    for id in ondin_core::subtree_nodes(doc, &[doc.root()]) {
        let Some(node) = doc.get(id) else { continue };
        if !matches!(node.kind(), NodeKind::Path { .. }) {
            continue;
        }
        total += 1;
        if let Some(b) = res.ink_bounds(id)
            && b.x0 < view.x1
            && b.x1 > view.x0
            && b.y0 < view.y1
            && b.y1 > view.y0
        {
            shown += 1;
        }
    }
    (total, shown)
}

/// Rasterize the fixture through the **CPU** backend and write the raw RGBA to
/// `ONDIN_SWEEP_DUMP`, so two builds can be compared byte for byte.
///
/// This is how the escape clamp's cost was measured: dump at a zoom, flip
/// `cpu.rs`'s budget to `f64::INFINITY`, dump again, compare. No GPU, so it says
/// nothing about time — it is only about what the picture loses.
#[test]
#[ignore = "writes a file; run with --ignored"]
fn cpu_dump_for_comparison() {
    let Ok(out) = std::env::var("ONDIN_SWEEP_DUMP") else {
        eprintln!("set ONDIN_SWEEP_DUMP to a path");
        return;
    };
    let zoom: f64 = std::env::var("ONDIN_SWEEP_ZOOMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16.0);
    let (doc, bounds) = fixture();
    let res = Resolved::rebuild(&doc);
    let (pw, ph) = (1280u32, 720u32);
    let (vw, vh) = (f64::from(pw) / zoom, f64::from(ph) / zoom);
    let c = bounds.center();
    let vp = Viewport {
        view: Rect::new(
            c.x - vw / 2.0,
            c.y - vh / 2.0,
            c.x + vw / 2.0,
            c.y + vh / 2.0,
        ),
        pixel_size: (pw, ph),
    };
    let (rgba, w, h) = ondin_render::VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &vp,
        &ondin_render::ImageStore::new(),
    );
    std::fs::write(&out, &rgba).expect("write dump");
    eprintln!("wrote {out}: {w}x{h}, {} bytes, zoom {zoom}", rgba.len());
}

#[test]
#[ignore = "requires a GPU adapter; run with --ignored --release on hardware"]
fn zoom_sweep_frame_times() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };
    eprintln!("adapter: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("request device");

    let (mut doc, bounds) = fixture();
    // Ablation: the same drawing with no effect layer at all, which takes the
    // whole `fx` batch out of the frame and leaves the main scene alone.
    if std::env::var("ONDIN_SWEEP_NO_FX").is_ok() {
        let strip: Vec<NodeId> = ondin_core::subtree_nodes(&doc, &[doc.root()])
            .into_iter()
            .filter(|id| doc.get(*id).is_some_and(|n| !n.effects().is_empty()))
            .collect();
        eprintln!("ablation: effects stripped from {} nodes", strip.len());
        let tx = Transaction(
            strip
                .into_iter()
                .map(|id| Operation::SetEffects {
                    id,
                    effects: Vec::new(),
                })
                .collect(),
        );
        doc.apply(&tx).expect("strip effects");
    }
    let res = Resolved::rebuild(&doc);
    let mut renderer = VelloGpuRenderer::new(&device).expect("create vello renderer");
    let images = ImageStore::new();

    let ceiling: f64 = std::env::var("ONDIN_SWEEP_CEILING_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1200.0);

    let repeat: usize = std::env::var("ONDIN_SWEEP_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let list = |k: &str, def: &[f64]| -> Vec<f64> {
        std::env::var(k).ok().map_or_else(
            || def.to_vec(),
            |v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
        )
    };

    let sizes: Vec<(u32, u32)> = match std::env::var("ONDIN_SWEEP_SIZES") {
        Ok(v) => v
            .split(',')
            .filter_map(|s| {
                let (w, h) = s.trim().split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .collect(),
        Err(_) => list("ONDIN_SWEEP_WIDTHS", &[1920.0, 3840.0])
            .iter()
            .map(|w| (*w as u32, (*w * 1080.0 / 1920.0).round() as u32))
            .collect(),
    };
    let zooms = list("ONDIN_SWEEP_ZOOMS", &[1.0, 2.0, 4.0, 8.0, 16.0, 24.0, 32.0]);
    let (sizes, zooms) = (&sizes[..], &zooms[..]);
    // Three places to stand: the middle of the drawing, and two off-centre, so a
    // frame that is cheap only because it points at empty space shows up as such.
    let default_origins: &[(f64, f64)] = &[(0.5, 0.5), (0.3, 0.35), (0.7, 0.6)];
    let picked: Vec<(f64, f64)> = std::env::var("ONDIN_SWEEP_ORIGINS")
        .ok()
        .map(|v| {
            v.split(',')
                .filter_map(|s| {
                    let (a, b) = s.trim().split_once('/')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_else(|| default_origins.to_vec());
    let origins = &picked[..];

    eprintln!(
        "{:>6} {:>10} {:>10} {:>8} {:>10}",
        "zoom", "pixels", "origin", "paths", "ms"
    );
    for &(pw, ph) in sizes {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ondin-sweep-target"),
            size: wgpu::Extent3d {
                width: pw,
                height: ph,
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

        for &zoom in zooms {
            for &(ox, oy) in origins {
                let vw = pw as f64 / zoom;
                let vh = ph as f64 / zoom;
                let cx = bounds.x0 + bounds.width() * ox;
                let cy = bounds.y0 + bounds.height() * oy;
                let vp = Viewport {
                    view: Rect::new(cx - vw / 2.0, cy - vh / 2.0, cx + vw / 2.0, cy + vh / 2.0),
                    pixel_size: (pw, ph),
                };
                let (total, shown) = visible(&doc, &res, vp.view);

                for _ in 0..repeat {
                    let t0 = Instant::now();
                    let r = renderer.render(
                        &device,
                        &queue,
                        &doc,
                        &res,
                        &vp,
                        &RenderOverrides::default(),
                        &view,
                        Color::WHITE,
                        &images,
                    );
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;

                    eprintln!(
                        "{zoom:>6} {:>10} {:>10} {:>8} {ms:>10.1}{}",
                        format!("{pw}x{ph}"),
                        format!("{ox}/{oy}"),
                        format!("{shown}/{total}"),
                        if r.is_err() { "  RENDER ERROR" } else { "" }
                    );
                    if r.is_err() {
                        return;
                    }
                    // ⚠️ **Stop short of the watchdog rather than tripping it.** Two
                    // seconds of GPU work resets the display driver, which is not a
                    // thing to do to the machine someone is working on — and the
                    // curve either side of the budget is the measurement anyway. The
                    // crash itself is already established (§15 D395).
                    if ms > ceiling {
                        eprintln!("stopping: {ms:.0} ms is within reach of the 2,000 ms watchdog");
                        // ⚠️ **A slow frame and a killed one report the same number.**
                        // Windows resets the device at two seconds and the call
                        // returns just after, so "2,1xx ms" is the watchdog's number
                        // rather than the work's — the only thing that tells them
                        // apart is whether the device is still alive afterwards.
                        let small = Viewport {
                            view: bounds,
                            pixel_size: (256, 256),
                        };
                        let probe = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("ondin-sweep-liveness"),
                            size: wgpu::Extent3d {
                                width: 256,
                                height: 256,
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
                        let pv = probe.create_view(&wgpu::TextureViewDescriptor::default());
                        let t1 = Instant::now();
                        let after = renderer.render(
                            &device,
                            &queue,
                            &doc,
                            &res,
                            &small,
                            &RenderOverrides::default(),
                            &pv,
                            Color::WHITE,
                            &images,
                        );
                        let _ = device.poll(wgpu::PollType::wait_indefinitely());
                        eprintln!(
                            "liveness: a 256x256 frame afterwards took {:.1} ms, {}",
                            t1.elapsed().as_secs_f64() * 1000.0,
                            match after {
                                Ok(()) => "device alive".to_string(),
                                Err(e) => format!("DEVICE GONE: {e:?}"),
                            }
                        );
                        return;
                    }
                }
            }
        }
    }
}
