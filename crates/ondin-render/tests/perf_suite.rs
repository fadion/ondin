//! The performance suite: times the generated fixtures (`perf_suite/fixtures.rs`)
//! and any local documents named in `ONDIN_PERF_EXTRA`, and compares each figure
//! with this machine's baseline in `perf/baselines/<machine>.json`. Nothing is
//! asserted. `perf/README.md` has the commands and the variables.

#[path = "perf_suite/fixtures.rs"]
mod fixtures;

use fixtures::Fixture;
use ondin_core::kurbo::Rect;
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, History, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction, io,
    keyed_by_position, subtree_nodes,
};
use ondin_render::gpu::{Region, build_scene, copy_shifted};
use ondin_render::{ImageStore, RenderOverrides, VelloGpuRenderer, Viewport};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (2560, 1440);
/// How long one metric keeps sampling once it has `MIN_RUNS`.
const BUDGET: Duration = Duration::from_secs(4);
const MIN_RUNS: usize = 3;
const MAX_RUNS: usize = 30;

#[test]
#[ignore = "a benchmark: needs a GPU adapter and a release build"]
fn measure() {
    let only: Option<Vec<String>> = std::env::var("ONDIN_PERF_ONLY")
        .ok()
        .map(|s| s.split(',').map(|n| n.trim().to_string()).collect());
    let wanted = |name: &str| only.as_ref().is_none_or(|o| o.iter().any(|n| n == name));
    let record = std::env::var("ONDIN_PERF_RECORD").is_ok_and(|v| v == "1");
    if record && cfg!(debug_assertions) {
        panic!("ONDIN_PERF_RECORD needs --release: a debug build's figures are not a baseline");
    }

    let mut gpu = Gpu::new();
    let path = baseline_path();
    let mut baseline: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    let (commit, dirty) = git_head();
    let mut results = Map::new();

    for &(name, build) in fixtures::ALL {
        if !wanted(name) {
            continue;
        }
        eprintln!("\n== {name} (fixtures v{}) — generating", fixtures::VERSION);
        let mut entry = measure_one(build(), &mut gpu);
        entry.insert("version".into(), json!(fixtures::VERSION));
        results.insert(name.into(), Value::Object(entry));
    }
    for path in std::env::var_os("ONDIN_PERF_EXTRA")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let name = format!(
            "local:{}",
            path.file_stem().unwrap_or_default().to_string_lossy()
        );
        if !wanted(&name) {
            continue;
        }
        eprintln!("\n== {name} — loading {}", path.display());
        let doc = io::load(&std::fs::read(&path).expect("read")).expect("load");
        let Some(leaf) = first_rect(&doc) else {
            eprintln!("{name}: no rect on the first page to edit; skipped");
            continue;
        };
        let fixture = Fixture {
            doc,
            leaf,
            main_leaf: None,
        };
        results.insert(name, Value::Object(measure_one(fixture, &mut gpu)));
    }

    let fixtures = baseline["fixtures"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    report(&results, &fixtures);

    if record {
        let mut fixtures = fixtures;
        for (name, mut entry) in results {
            let e = entry.as_object_mut().expect("an entry");
            e.insert("commit".into(), json!(commit));
            e.insert("dirty".into(), json!(dirty));
            fixtures.insert(name, entry);
        }
        baseline["machine"] = json!(machine());
        baseline["gpu"] = json!(gpu.name);
        baseline["fixtures"] = Value::Object(fixtures);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        let text = serde_json::to_string_pretty(&baseline).expect("json") + "\n";
        std::fs::write(&path, text).expect("write the baseline");
        eprintln!("\nrecorded {}", path.display());
    } else {
        eprintln!("\nnot recorded (ONDIN_PERF_RECORD=1 records)");
    }
}

/// Writes every generated fixture to `target/perf/<name>.ondin`, to open in the app.
#[test]
#[ignore = "writes files; run on demand"]
fn write_fixtures() {
    let dir = repo_root().join("target").join("perf");
    std::fs::create_dir_all(&dir).expect("mkdir");
    for &(name, build) in fixtures::ALL {
        let out = dir.join(format!("{name}.ondin"));
        std::fs::write(&out, io::save(&build().doc).expect("save")).expect("write");
        eprintln!("wrote {}", out.display());
    }
}

/// p10 and p50 of a metric's samples, in milliseconds.
struct Stat {
    p10: f64,
    p50: f64,
    n: usize,
}

impl Stat {
    fn json(&self) -> Value {
        json!({ "p10": round(self.p10), "p50": round(self.p50), "n": self.n })
    }
}

fn round(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

/// Runs `f`, which answers its own duration in ms, until `BUDGET` is spent. The
/// first run warms caches and the GPU's clocks and is not counted.
fn sample(mut f: impl FnMut() -> f64) -> Stat {
    f();
    let start = Instant::now();
    let mut v = Vec::new();
    while v.len() < MIN_RUNS || (v.len() < MAX_RUNS && start.elapsed() < BUDGET) {
        v.push(f());
    }
    v.sort_by(f64::total_cmp);
    Stat {
        p10: v[v.len() / 10],
        p50: v[v.len() / 2],
        n: v.len(),
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: VelloGpuRenderer,
    name: String,
}

impl Gpu {
    fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("a GPU adapter");
        let name = adapter.get_info().name;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("a device");
        let renderer = VelloGpuRenderer::new(&device).expect("vello");
        Self {
            device,
            queue,
            renderer,
            name,
        }
    }

    fn texture(&self) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("perf-suite"),
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

    fn wait(&self) {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
    }
}

/// The canvas with world `(0, 0)` at device pixel `origin`, at `zoom`.
fn viewport(origin: (f64, f64), zoom: f64) -> Viewport {
    let (x0, y0) = (-origin.0 / zoom, -origin.1 / zoom);
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

/// Every figure for one document: the file, the resolve, the first page drawn
/// whole (`fit`) and at 100% on its first frame (`work`), a pan, and an edit.
fn measure_one(fx: Fixture, gpu: &mut Gpu) -> Map<String, Value> {
    let Fixture {
        mut doc,
        leaf,
        main_leaf,
    } = fx;
    let mut m = Map::new();
    let mut stat = |name: &str, s: Stat| {
        eprintln!(
            "  {name:<14} p50 {:>10.3} ms   p10 {:>10.3}   n {}",
            s.p50, s.p10, s.n
        );
        m.insert(name.into(), s.json());
    };

    let pages: Vec<NodeId> = doc.page_ids().collect();
    let layers = subtree_nodes(&doc, &pages).len() - pages.len();
    let bytes = io::save(&doc).expect("save");
    let file_mb = bytes.len() as f64 / (1024.0 * 1024.0);
    eprintln!(
        "  {layers} layers on {} pages, {file_mb:.1} MB",
        pages.len()
    );

    stat(
        "save",
        sample(|| {
            let t = Instant::now();
            std::hint::black_box(io::save(&doc).expect("save"));
            ms(t)
        }),
    );
    stat(
        "load",
        sample(|| {
            let t = Instant::now();
            std::hint::black_box(io::load(&bytes).expect("load"));
            ms(t)
        }),
    );
    stat(
        "resolve",
        sample(|| {
            let t = Instant::now();
            std::hint::black_box(Resolved::rebuild(&doc));
            ms(t)
        }),
    );

    let mut res = Resolved::rebuild(&doc);
    let page = doc.first_page();
    let mut images = ImageStore::interactive();
    images.prepare(&doc);
    // The canvas always walks at level of detail.
    let mut ov = RenderOverrides::default();
    ov.set_level_of_detail(true);
    let ground = Color::from_rgba8(30, 32, 36, 255);

    let content = res
        .ink_bounds(page)
        .unwrap_or(Rect::new(0.0, 0.0, 1000.0, 1000.0));
    let zoom =
        0.9 * (f64::from(SIZE.0) / content.width()).min(f64::from(SIZE.1) / content.height());
    let fit_origin = (
        (f64::from(SIZE.0) / 2.0 - content.center().x * zoom).round(),
        (f64::from(SIZE.1) / 2.0 - content.center().y * zoom).round(),
    );
    let fit = viewport(fit_origin, zoom);
    let first = doc.get(page).expect("the page").children()[0];
    let frame = res.ink_bounds(first).expect("the first frame's ink");
    let work = viewport((40.0 - frame.x0, 40.0 - frame.y0), 1.0);

    for (name, vp) in [("fit", &fit), ("work", &work)] {
        stat(
            &format!("scene_{name}"),
            sample(|| {
                let t = Instant::now();
                std::hint::black_box(build_scene(&doc, &res, vp, &ov, &images, page));
                ms(t)
            }),
        );
    }
    let (mut shown, mut spare) = (gpu.texture(), gpu.texture());
    for (name, vp) in [("fit", &fit), ("work", &work)] {
        stat(
            &format!("frame_{name}"),
            sample(|| {
                let t = Instant::now();
                gpu.renderer
                    .render(
                        &gpu.device,
                        &gpu.queue,
                        &doc,
                        &res,
                        vp,
                        &ov,
                        &shown.create_view(&Default::default()),
                        ground,
                        &images,
                        page,
                    )
                    .expect("render");
                gpu.wait();
                ms(t)
            }),
        );
    }

    // A pan at fit, as the canvas draws one: the exposed strip, the rest shifted.
    gpu.renderer
        .render(
            &gpu.device,
            &gpu.queue,
            &doc,
            &res,
            &fit,
            &ov,
            &shown.create_view(&Default::default()),
            ground,
            &images,
            page,
        )
        .expect("render");
    let (mut origin, mut i, mut refused) = (fit_origin, 0, 0);
    stat(
        "pan_fit",
        sample(|| {
            let shift = if i % 2 == 0 { (7, -3) } else { (-4, 6) };
            i += 1;
            origin = (origin.0 + f64::from(shift.0), origin.1 + f64::from(shift.1));
            let t = Instant::now();
            let drawn = gpu
                .renderer
                .render_regions(
                    &gpu.device,
                    &gpu.queue,
                    &doc,
                    &res,
                    &viewport(origin, zoom),
                    &ov,
                    &spare.create_view(&Default::default()),
                    &Region::exposed(SIZE, shift),
                    ground,
                    &images,
                    page,
                )
                .expect("regions");
            if drawn {
                copy_shifted(&gpu.device, &gpu.queue, &shown, &spare, SIZE, shift);
                std::mem::swap(&mut shown, &mut spare);
            } else {
                refused += 1;
            }
            gpu.wait();
            ms(t)
        }),
    );
    if refused > 0 {
        eprintln!("  ({refused} pans refused and drawn by nothing)");
    }

    // An edit as the app commits one: the component passes, the commit, the
    // resolve brought up to date; then its undo.
    let mut history = History::new();
    let mut ids = IdSource::new(0xfeed);
    let edits = [
        Some(("commit", leaf)),
        main_leaf.map(|l| ("commit_main", l)),
    ];
    for (name, target) in edits.into_iter().flatten() {
        let mut flip = false;
        let mut undo = Vec::new();
        stat(
            name,
            sample(|| {
                flip = !flip;
                let c = if flip { 0x40 } else { 0xc0 };
                let mut tx = Transaction(vec![Operation::SetFills {
                    id: target,
                    fills: keyed_by_position([Fill {
                        brush: Color::from_rgba8(c, 0x80, 0x40, 0xff).into(),
                        visible: true,
                    }]),
                }]);
                let t = Instant::now();
                ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
                let dirty = history.commit(&mut doc, tx).expect("the edit commits");
                res.update(&doc, &dirty);
                let took = ms(t);
                let t = Instant::now();
                let dirty = history.undo(&mut doc).expect("undo").expect("a step");
                res.update(&doc, &dirty);
                undo.push(ms(t));
                took
            }),
        );
        undo.sort_by(f64::total_cmp);
        stat(
            &format!("{name}_undo"),
            Stat {
                p10: undo[undo.len() / 10],
                p50: undo[undo.len() / 2],
                n: undo.len(),
            },
        );
    }

    let mut out = Map::new();
    out.insert("layers".into(), json!(layers));
    out.insert("pages".into(), json!(pages.len()));
    out.insert("file_mb".into(), json!(round(file_mb)));
    out.insert("metrics".into(), Value::Object(m));
    out
}

/// The first rect on the first page, for a document the suite did not build.
fn first_rect(doc: &Document) -> Option<NodeId> {
    subtree_nodes(doc, &[doc.first_page()])
        .into_iter()
        .find(|&id| matches!(doc.get(id).map(|n| n.kind()), Some(NodeKind::Rect { .. })))
}

/// Each metric beside the baseline's, where the baseline was taken from the
/// same version of the fixture.
fn report(results: &Map<String, Value>, baseline: &Map<String, Value>) {
    eprintln!(
        "\n{:<22} {:<15} {:>11} {:>11} {:>8}",
        "fixture", "metric", "p50 ms", "base ms", "Δ"
    );
    for (name, entry) in results {
        let base = baseline.get(name);
        let comparable = base.is_some_and(|b| b["version"] == entry["version"]);
        if base.is_some() && !comparable {
            eprintln!("{name}: the baseline is of another fixture version; not compared");
        }
        for (metric, s) in entry["metrics"].as_object().expect("metrics") {
            let now = s["p50"].as_f64().expect("p50");
            let was = base
                .filter(|_| comparable)
                .and_then(|b| b["metrics"][metric]["p50"].as_f64());
            match was {
                Some(was) => eprintln!(
                    "{name:<22} {metric:<15} {now:>11.3} {was:>11.3} {:>+7.1}%",
                    (now / was - 1.0) * 100.0
                ),
                None => eprintln!("{name:<22} {metric:<15} {now:>11.3} {:>11} {:>8}", "—", ""),
            }
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository")
}

/// This machine's name: `ONDIN_PERF_MACHINE`, or the host name.
fn machine() -> String {
    let raw = ["ONDIN_PERF_MACHINE", "COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "unknown".into());
    raw.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn baseline_path() -> PathBuf {
    repo_root()
        .join("perf")
        .join("baselines")
        .join(format!("{}.json", machine()))
}

/// The commit measured, and whether tracked files differed from it.
fn git_head() -> (String, bool) {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo_root())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    (commit, dirty)
}
