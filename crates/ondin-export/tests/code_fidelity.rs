//! Code export's fidelity bar: each fixture's page, screenshotted by a headless
//! Chrome, against the CPU renderer's raster of the same layers. Anything the
//! export does not report as approximated must match.
//!
//! The browser is `ONDIN_BROWSER`, else Chrome, Chromium or Edge where they are
//! usually installed. With none found the comparisons are skipped with a line on
//! stderr — unless `CI` is set, where a missing browser fails the run.
//!
//! The comparison composites both images over white and takes the mean absolute
//! channel error per 8×8 cell, so an edge antialiased differently by the two
//! rasterizers is a few low cells while a misplaced or miscoloured layer is a run
//! of high ones.

use ondin_core::container::{
    AlignItems, Display, Flex, FlexDirection, Grid, Insets, JustifyContent, LayoutItem, LengthPct,
    Track, TrackBreadth, TrackSize,
};
use ondin_core::kurbo::{Affine, Join, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::{
    Color, ColorStop, Extend, Gradient, GradientKind, LinearGradientPosition,
    RadialGradientPosition, SweepGradientPosition,
};
use ondin_core::variant::{self, PropKind, PropValue};
use ondin_core::{
    Brush, CharAttr, Document, Effect, EffectKind, Fill, GradientBrush, IdSource, NodeId, NodeKind,
    Operation, Resolved, Shadow, Stroke, StrokeAlign, StrokeSides, TextAlign, TextSizing,
    TextStyle, Transaction, VerticalAlign, keyed_by_position,
};
use ondin_export::code::CodeExport;
use ondin_export::code::html::{html_of, html_of_components};
use std::path::{Path, PathBuf};

/// The cell size and the two limits. Matching pages peak at 14.6 (an inline-SVG
/// inner shadow) and 12.1 (rotated edges). flip: a blur written as its radius →
/// 23.6 in `effects`; a sweep started at +x → 126 in `gradients`; a centred stroke
/// written as an outside one → 115 in `strokes`. A flat colour error under 16/255
/// over a small layer passes.
const CELL: u32 = 8;
const MAX_CELL_ERROR: f64 = 16.0;
const MAX_MEAN_ERROR: f64 = 1.5;

/// The bundled Inter, both styles, for the page under test.
const INTER_FACES: &str = r#"
@font-face { font-family: "Inter"; src: url(inter.ttf); font-weight: 100 900; font-style: normal; }
@font-face { font-family: "Inter"; src: url(inter-italic.ttf); font-weight: 100 900; font-style: italic; }
"#;

/// A document under construction: a root and helpers that apply one op each.
struct Doc {
    doc: Document,
    ids: IdSource,
}

impl Doc {
    fn new() -> Self {
        let mut ids = IdSource::new(0xC0DE);
        let root = ids.mint();
        Doc {
            doc: Document::new(root),
            ids,
        }
    }

    fn apply(&mut self, ops: Vec<Operation>) {
        self.doc
            .apply(&Transaction(ops))
            .expect("fixture op applies");
    }

    fn add(&mut self, parent: Option<NodeId>, kind: NodeKind, at: Affine, name: &str) -> NodeId {
        let id = self.ids.mint();
        let parent = parent.unwrap_or(self.doc.root());
        let index = self.doc.get(parent).map_or(0, |p| p.children().len());
        self.apply(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(at),
            name: Some(name.into()),
        }]);
        id
    }

    fn frame(
        &mut self,
        parent: Option<NodeId>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        name: &str,
    ) -> NodeId {
        let id = self.add(
            parent,
            NodeKind::Artboard {
                size: Size::new(w, h),
            },
            Affine::translate((x, y)),
            name,
        );
        self.fill(id, solid(240, 240, 236, 255));
        id
    }

    fn rect(&mut self, parent: NodeId, x: f64, y: f64, w: f64, h: f64, r: f64) -> NodeId {
        self.add(
            Some(parent),
            NodeKind::Rect {
                size: Size::new(w, h),
                corner_radii: RoundedRectRadii::from_single_radius(r),
            },
            Affine::translate((x, y)),
            "Rect",
        )
    }

    fn fill(&mut self, id: NodeId, brush: Brush) {
        self.fills(id, vec![brush]);
    }

    fn fills(&mut self, id: NodeId, brushes: Vec<Brush>) {
        self.apply(vec![Operation::SetFills {
            id,
            fills: keyed_by_position(brushes.into_iter().map(|b| Fill {
                brush: b.into(),
                visible: true,
            })),
        }]);
    }

    fn stroke(&mut self, id: NodeId, width: f64, align: StrokeAlign, sides: StrokeSides) {
        self.apply(vec![Operation::SetStrokes {
            id,
            strokes: keyed_by_position([Stroke {
                brush: solid(20, 20, 30, 255).into(),
                width,
                width_var: None,
                join: ondin_core::kurbo::Join::Miter,
                cap: ondin_core::kurbo::Cap::Butt,
                dashes: vec![],
                dash_offset: 0.0,
                sides,
                miter_limit: 4.0,
                dash_fit: false,
                align,
                visible: true,
            }]),
        }]);
    }

    /// [`Self::stroke`] on all sides, with `join` at its corners.
    fn stroke_joined(&mut self, id: NodeId, width: f64, align: StrokeAlign, join: Join) {
        self.stroke(id, width, align, StrokeSides::All);
        let mut strokes = self.doc.get(id).expect("stroked").paint().strokes.clone();
        strokes[0].value.join = join;
        self.apply(vec![Operation::SetStrokes { id, strokes }]);
    }

    fn effects(&mut self, id: NodeId, kinds: Vec<EffectKind>) {
        self.apply(vec![Operation::SetEffects {
            id,
            effects: keyed_by_position(kinds.into_iter().map(Effect::new)),
        }]);
    }

    fn done(self) -> (Document, Resolved) {
        let res = Resolved::rebuild(&self.doc);
        (self.doc, res)
    }
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Brush {
    Brush::Solid(Color::from_rgba8(r, g, b, a))
}

fn ramp(kind: GradientKind, extend: Extend) -> Brush {
    Brush::Gradient(GradientBrush {
        gradient: Gradient {
            kind,
            extend,
            stops: [
                ColorStop {
                    offset: 0.0,
                    color: Color::from_rgba8(230, 60, 40, 255).into(),
                },
                ColorStop {
                    offset: 0.5,
                    color: Color::from_rgba8(250, 210, 60, 255).into(),
                },
                ColorStop {
                    offset: 1.0,
                    color: Color::from_rgba8(40, 90, 220, 255).into(),
                },
            ]
            .as_slice()
            .into(),
            ..Default::default()
        },
        transform: Affine::IDENTITY,
        opacity: 1.0,
    })
}

// ---------------------------------------------------------------------------
// The browser
// ---------------------------------------------------------------------------

fn browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ONDIN_BROWSER") {
        return Some(PathBuf::from(p));
    }
    let fixed = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    if let Some(p) = fixed.iter().map(PathBuf::from).find(|p| p.is_file()) {
        return Some(p);
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ];
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// The browser, or `None` after saying why the comparison is skipped. Fails on CI.
fn browser_or_skip(test: &str) -> Option<PathBuf> {
    let b = browser();
    if b.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test}: no headless browser found (set ONDIN_BROWSER)"
        );
        eprintln!("{test}: skipped, no headless browser found (set ONDIN_BROWSER)");
    }
    b
}

/// A fresh folder under the system temp dir for one test.
fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ondin-code-fidelity-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Straight-alpha RGBA8 and its size.
struct Image {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn decode(bytes: &[u8]) -> Image {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => panic!("unexpected screenshot colour type {other:?}"),
    };
    Image {
        rgba,
        width: info.width,
        height: info.height,
    }
}

/// The top-left `width`×`height` of `img`.
fn crop(img: &Image, width: u32, height: u32) -> Image {
    assert!(
        img.width >= width && img.height >= height,
        "the screenshot is {}×{}, smaller than the page's {width}×{height}",
        img.width,
        img.height
    );
    let row = |y: u32| {
        let at = (y * img.width * 4) as usize;
        &img.rgba[at..at + (width * 4) as usize]
    };
    Image {
        rgba: (0..height).flat_map(row).copied().collect(),
        width,
        height,
    }
}

/// `dir/index.html` screenshotted at `width`×`height` CSS px, one device pixel each.
///
/// Headless Chrome's viewport is narrower and shorter than `--window-size` (at least 500 px
/// wide, less a toolbar's height), and from Chrome 156 nothing outside the viewport is
/// painted, so the window is oversized and the shot cropped to the page.
fn screenshot(browser: &Path, dir: &Path, width: u32, height: u32) -> Image {
    let shot = dir.join("shot.png");
    let url = format!(
        "file:///{}",
        dir.join("index.html")
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    let out = std::process::Command::new(browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--force-device-scale-factor=1")
        .arg("--force-color-profile=srgb")
        .arg("--font-render-hinting=none")
        .arg("--disable-lcd-text")
        // Without it the shot can come before the web font has loaded, and the
        // text is not drawn at all.
        .arg("--virtual-time-budget=10000")
        .arg("--default-background-color=00000000")
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("--window-size={},{}", width.max(500), height + 200))
        .arg(format!("--screenshot={}", shot.display()))
        .arg(url)
        .output()
        .expect("the browser runs");
    let bytes = std::fs::read(&shot).unwrap_or_else(|_| {
        panic!(
            "no screenshot from the browser:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    crop(&decode(&bytes), width, height)
}

/// One channel of a straight-alpha pixel composited over white.
fn over_white(c: u8, a: u8) -> f64 {
    let a = f64::from(a) / 255.0;
    f64::from(c) * a + 255.0 * (1.0 - a)
}

/// One cell's mean channel error and its top-left pixel.
type Cell = (f64, (u32, u32));

/// The worst cells, worst first, and the whole image's mean channel error.
fn compare(a: &Image, b: &Image) -> (Vec<Cell>, f64) {
    let (w, h) = (a.width.min(b.width), a.height.min(b.height));
    let mut total = 0.0;
    let mut cells = Vec::new();
    for cy in (0..h).step_by(CELL as usize) {
        for cx in (0..w).step_by(CELL as usize) {
            let mut sum = 0.0;
            let mut n = 0.0;
            for y in cy..(cy + CELL).min(h) {
                for x in cx..(cx + CELL).min(w) {
                    let ia = ((y * a.width + x) * 4) as usize;
                    let ib = ((y * b.width + x) * 4) as usize;
                    for c in 0..3 {
                        let va = over_white(a.rgba[ia + c], a.rgba[ia + 3]);
                        let vb = over_white(b.rgba[ib + c], b.rgba[ib + 3]);
                        sum += (va - vb).abs();
                    }
                    n += 3.0;
                }
            }
            total += sum;
            cells.push((sum / n, (cx, cy)));
        }
    }
    cells.sort_by(|x, y| y.0.total_cmp(&x.0));
    cells.truncate(5);
    (cells, total / f64::from(w * h * 3))
}

/// `img` composited over white and box-blurred `passes` times with a 3×3 kernel,
/// back as opaque RGBA.
fn blurred(img: &Image, passes: usize) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut px: Vec<[f64; 3]> = img
        .rgba
        .chunks(4)
        .map(|p| [0, 1, 2].map(|c| over_white(p[c], p[3])))
        .collect();
    for _ in 0..passes {
        let src = px.clone();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0; 3];
                let mut n = 0.0;
                for yy in y.saturating_sub(1)..(y + 2).min(h) {
                    for xx in x.saturating_sub(1)..(x + 2).min(w) {
                        let s = src[yy * w + xx];
                        for c in 0..3 {
                            acc[c] += s[c];
                        }
                        n += 1.0;
                    }
                }
                px[y * w + x] = acc.map(|v| v / n);
            }
        }
    }
    Image {
        rgba: px
            .iter()
            .flat_map(|p| {
                [
                    p[0].round() as u8,
                    p[1].round() as u8,
                    p[2].round() as u8,
                    255,
                ]
            })
            .collect(),
        width: img.width,
        height: img.height,
    }
}

/// Export the whole document, screenshot it, and hold it to the canvas.
fn holds(test: &str, doc: &Document, res: &Resolved) {
    holds_within(test, doc, res, 0, MAX_CELL_ERROR);
}

/// Text's limit, after two passes of `blurred`: Chrome and vello antialias glyph
/// edges differently even where every glyph is placed alike, and each platform's
/// Chrome differently again — Linux peaks at 9.1 and macOS at 19.1 on fixtures
/// Windows holds under 6.0. Only Windows is tight enough for a one-pixel drop of a
/// line to show; elsewhere `MAX_TEXT_MEAN_ERROR` is what still catches a layout
/// shift. flip (Windows): an automatic line height written as `normal` → 9.8.
const MAX_TEXT_CELL_ERROR: f64 = if cfg!(target_os = "macos") {
    22.0
} else if cfg!(target_os = "linux") {
    12.0
} else {
    8.0
};

/// The mean limit for a blurred comparison, which only text takes. macOS Chrome's
/// glyph smoothing adds about 0.8 to every text fixture's mean, which carries
/// `decorations` (1.2 on Windows) past `MAX_MEAN_ERROR`.
const MAX_TEXT_MEAN_ERROR: f64 = if cfg!(target_os = "macos") {
    2.5
} else {
    MAX_MEAN_ERROR
};

/// [`holds`], comparing after `passes` of `blurred` with `limit` per cell.
fn holds_within(test: &str, doc: &Document, res: &Resolved, passes: usize, limit: f64) {
    holds_as(test, doc, res, passes, limit, html_of);
}

/// [`holds_within`] for the page `export` writes.
fn holds_as(
    test: &str,
    doc: &Document,
    res: &Resolved,
    passes: usize,
    limit: f64,
    export: fn(&Document, &Resolved, &[NodeId]) -> CodeExport,
) {
    let origins: Vec<NodeId> = doc.get(doc.root()).expect("root").children().to_vec();
    let export = export(doc, res, &origins);
    assert!(
        export.approximated.is_empty(),
        "{test}: the fixture is meant to be exact, but the export reports {:?}",
        export.approximated
    );
    let Some(browser) = browser_or_skip(test) else {
        return;
    };
    let dir = scratch(test);
    for f in &export.files {
        let path = dir.join(&f.path);
        std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("folder");
        let mut bytes = f.bytes.clone();
        // The export ships no fonts; the page is given the face the CPU renderer
        // shapes with, so text is compared as well as boxes.
        if f.path == "styles.css" {
            bytes.extend_from_slice(INTER_FACES.as_bytes());
        }
        std::fs::write(&path, &bytes).expect("export file");
    }
    for (name, from) in [
        ("inter.ttf", "Inter-Variable.ttf"),
        ("inter-italic.ttf", "Inter-Italic-Variable.ttf"),
    ] {
        let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ondin-core/assets/fonts");
        std::fs::copy(fonts.join(from), dir.join(name)).expect("bundled font");
    }
    let cpu = ondin_export::png::raster_of(doc, res, &origins, &Default::default())
        .expect("the fixture has ink");
    let cpu = Image {
        rgba: cpu.rgba,
        width: cpu.width,
        height: cpu.height,
    };
    std::fs::write(
        dir.join("cpu.png"),
        ondin_export::png::encode_rgba8(&cpu.rgba, cpu.width, cpu.height),
    )
    .expect("cpu png");
    let page = screenshot(&browser, &dir, cpu.width, cpu.height);
    assert_eq!(
        (page.width, page.height),
        (cpu.width, cpu.height),
        "{test}: the screenshot is not the page's size"
    );
    let ((cells, mean), mean_limit) = if passes == 0 {
        (compare(&cpu, &page), MAX_MEAN_ERROR)
    } else {
        (
            compare(&blurred(&cpu, passes), &blurred(&page, passes)),
            MAX_TEXT_MEAN_ERROR,
        )
    };
    let (worst, at) = cells.first().copied().unwrap_or_default();
    eprintln!("{test}: worst cell {worst:.1}/255 at {at:?}, mean {mean:.2}/255");
    assert!(
        worst <= limit && mean <= mean_limit,
        "{test}: the page differs from the canvas — worst cells {:?}, mean {mean:.2}/255 \
         (see {})",
        cells
            .iter()
            .map(|(e, at)| format!("{e:.1} at {at:?}"))
            .collect::<Vec<_>>(),
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Fills, corner radii, opacity and z-order on plain shapes in a frame.
#[test]
fn shapes_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 220.0, 160.0, "Board");
    let a = d.rect(f, 20.0, 20.0, 100.0, 70.0, 12.0);
    d.fill(a, solid(220, 50, 60, 255));
    let b = d.add(
        Some(f),
        NodeKind::Ellipse {
            size: Size::new(90.0, 60.0),
        },
        Affine::translate((90.0, 60.0)),
        "Dot",
    );
    d.fill(b, solid(40, 110, 230, 255));
    let c = d.rect(f, 60.0, 40.0, 120.0, 40.0, 0.0);
    d.fills(c, vec![solid(20, 160, 90, 255), solid(255, 255, 255, 128)]);
    d.apply(vec![Operation::SetOpacity {
        id: c,
        opacity: 0.6,
    }]);
    let (doc, res) = d.done();
    holds("shapes", &doc, &res);
}

/// Inside, centre and outside strokes, square and rounded, and a per-side border.
#[test]
fn strokes_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 140.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        let x = 20.0 + i as f64 * 80.0;
        let sq = d.rect(f, x, 20.0, 50.0, 40.0, 0.0);
        d.fill(sq, solid(250, 200, 80, 255));
        d.stroke(sq, 6.0, align, StrokeSides::All);
        let rd = d.rect(f, x, 80.0, 50.0, 40.0, 14.0);
        d.fill(rd, solid(120, 200, 250, 255));
        d.stroke(rd, 6.0, align, StrokeSides::All);
    }
    let (doc, res) = d.done();
    holds("strokes", &doc, &res);
}

/// Per-side widths: borders on a square rect, SVG on a rounded one.
#[test]
fn a_per_side_stroke_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 220.0, 100.0, "Board");
    let sides = StrokeSides::Custom {
        top: 8.0,
        right: 0.0,
        bottom: 2.0,
        left: 4.0,
    };
    for (x, r) in [(20.0, 0.0), (120.0, 10.0)] {
        let rect = d.rect(f, x, 20.0, 80.0, 60.0, r);
        d.fill(rect, solid(250, 230, 200, 255));
        d.stroke(rect, 4.0, StrokeAlign::Inside, sides);
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(
        css.contains("border-top: 8px solid"),
        "the square one is CSS:\n{css}"
    );
    assert_eq!(
        export.positioned,
        vec!["layer “Rect”: a per-side stroke CSS borders cannot draw, written as inline SVG"]
    );
    holds("per-side", &doc, &res);
}

/// A frame's stroke paints over its children, at each alignment: on a clipping
/// frame with a full-bleed child, and on one that does not clip, whose child
/// reaches past its edges.
///
/// flip: `paint` writing an inside or unclipped stroke on the frame itself → the
/// child covers it, 143.3 along the inside card's edges.
#[test]
fn a_frame_stroke_draws_over_its_children() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 420.0, 260.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        for (j, clip) in [true, false].into_iter().enumerate() {
            let (x, y) = (20.0 + i as f64 * 135.0, 20.0 + j as f64 * 125.0);
            let card = d.frame(Some(board), x, y, 110.0, 90.0, "Card");
            d.apply(vec![Operation::SetClip { id: card, clip }]);
            d.stroke(card, 10.0, align, StrokeSides::All);
            let bleed = if clip { 0.0 } else { 8.0 };
            let child = d.rect(
                card,
                -bleed,
                -bleed,
                110.0 + 2.0 * bleed,
                90.0 + 2.0 * bleed,
                0.0,
            );
            d.fill(child, solid(90, 170, 240, 255));
        }
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[board]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(
        css.contains("outline: 5px solid") && css.contains("outline-offset: -5px"),
        "the clipping centred card splits its stroke at the edge:\n{css}"
    );
    holds("frame-stroke", &doc, &res);
}

/// A stroked circle at each alignment is CSS; a stroked ellipse that is not a
/// circle is inline SVG, an outline's offset radii not being its parallel curve.
///
/// flip: `stroke_blocker` letting every ellipse through → `positioned` is empty.
#[test]
fn stroked_ellipses_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 300.0, 260.0, "Board");
    for (i, align) in [
        StrokeAlign::Inside,
        StrokeAlign::Center,
        StrokeAlign::Outside,
    ]
    .into_iter()
    .enumerate()
    {
        let y = 20.0 + i as f64 * 80.0;
        let circle = d.add(
            Some(f),
            NodeKind::Ellipse {
                size: Size::new(50.0, 50.0),
            },
            Affine::translate((20.0, y)),
            "Dot",
        );
        d.fill(circle, solid(250, 200, 80, 255));
        d.stroke(circle, 8.0, align, StrokeSides::All);
        let oval = d.add(
            Some(f),
            NodeKind::Ellipse {
                size: Size::new(180.0, 50.0),
            },
            Affine::translate((100.0, y)),
            "Oval",
        );
        d.fill(oval, solid(120, 200, 250, 255));
        d.stroke(oval, 10.0, align, StrokeSides::All);
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains(".dot {"), "the circles are CSS:\n{css}");
    assert_eq!(
        export.positioned,
        vec!["layer “Oval”: a stroked ellipse that is not a circle, written as inline SVG"]
    );
    holds("ellipses", &doc, &res);
}

/// Round and bevel joins on a rect with one rounded corner and three square ones
/// are drawn as joined; the outline's square corners are always mitred.
///
/// flip: `stroke_blocker` testing the join only when every corner is square →
/// 134.6 at the square corners.
#[test]
fn a_join_on_mixed_corners_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 140.0, "Board");
    for (i, join) in [Join::Round, Join::Bevel].into_iter().enumerate() {
        let r = d.add(
            Some(f),
            NodeKind::Rect {
                size: Size::new(120.0, 80.0),
                corner_radii: RoundedRectRadii::new(20.0, 0.0, 0.0, 0.0),
            },
            Affine::translate((30.0 + i as f64 * 160.0, 30.0)),
            "Tab",
        );
        d.fill(r, solid(250, 230, 200, 255));
        d.stroke_joined(r, 12.0, StrokeAlign::Outside, join);
    }
    let (doc, res) = d.done();
    holds("mixed-joins", &doc, &res);
}

/// Linear, repeating, concentric radial and sweep gradients.
#[test]
fn gradients_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 340.0, 100.0, "Board");
    let a = d.rect(f, 10.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        a,
        ramp(
            GradientKind::Linear(LinearGradientPosition {
                start: Point::new(5.0, 10.0),
                end: Point::new(60.0, 70.0),
            }),
            Extend::Pad,
        ),
    );
    let b = d.rect(f, 90.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        b,
        ramp(
            GradientKind::Linear(LinearGradientPosition {
                start: Point::new(0.0, 20.0),
                end: Point::new(0.0, 45.0),
            }),
            Extend::Repeat,
        ),
    );
    let c = d.rect(f, 170.0, 10.0, 70.0, 80.0, 0.0);
    d.fill(
        c,
        ramp(
            GradientKind::Radial(RadialGradientPosition {
                start_center: Point::new(30.0, 40.0),
                start_radius: 0.0,
                end_center: Point::new(30.0, 40.0),
                end_radius: 45.0,
            }),
            Extend::Pad,
        ),
    );
    let e = d.rect(f, 250.0, 10.0, 80.0, 80.0, 0.0);
    d.fill(
        e,
        ramp(
            GradientKind::Sweep(SweepGradientPosition {
                center: Point::new(40.0, 40.0),
                start_angle: 0.0,
                end_angle: std::f32::consts::TAU,
            }),
            Extend::Pad,
        ),
    );
    let (doc, res) = d.done();
    holds("gradients", &doc, &res);
}

/// A rotated rect, and a rotated frame whose child turns with it.
#[test]
fn rotation_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 180.0, "Board");
    let r = d.add(
        Some(f),
        NodeKind::Rect {
            size: Size::new(80.0, 40.0),
            corner_radii: RoundedRectRadii::default(),
        },
        Affine::translate((60.0, 40.0)) * Affine::rotate(0.4),
        "Tilted",
    );
    d.fill(r, solid(200, 60, 160, 255));
    let inner = d.add(
        Some(f),
        NodeKind::Artboard {
            size: Size::new(70.0, 60.0),
        },
        Affine::translate((140.0, 90.0)) * Affine::rotate(-0.3),
        "Inner",
    );
    d.fill(inner, solid(60, 70, 90, 255));
    let dot = d.rect(inner, 10.0, 10.0, 30.0, 20.0, 4.0);
    d.fill(dot, solid(250, 250, 250, 255));
    let (doc, res) = d.done();
    holds("rotation", &doc, &res);
}

/// A layer blur and a drop shadow, one on a translucent fill.
#[test]
fn effects_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 120.0, "Board");
    let a = d.rect(f, 20.0, 30.0, 60.0, 60.0, 8.0);
    d.fill(a, solid(40, 120, 220, 255));
    d.effects(a, vec![EffectKind::LayerBlur { radius: 8.0 }]);
    let b = d.rect(f, 100.0, 30.0, 60.0, 60.0, 8.0);
    d.fill(b, solid(250, 250, 250, 255));
    let shadow = |a| {
        EffectKind::DropShadow(Shadow {
            offset: Vec2::new(4.0, 6.0),
            blur: 10.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, a),
        })
    };
    d.effects(b, vec![shadow(140)]);
    let c = d.rect(f, 180.0, 30.0, 60.0, 60.0, 30.0);
    d.fill(c, solid(240, 80, 60, 120));
    d.effects(c, vec![shadow(200)]);
    let (doc, res) = d.done();
    holds("effects", &doc, &res);
}

/// A flex row with gap, padding, a growing item and centred cross alignment,
/// and a two-column grid.
#[test]
fn layout_matches_the_canvas() {
    let mut d = Doc::new();
    let row = d.frame(None, 0.0, 0.0, 300.0, 80.0, "Row");
    d.apply(vec![Operation::SetDisplay {
        id: row,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Center,
            row_gap: 0.0,
            column_gap: 12.0,
            padding: [10.0, 16.0, 10.0, 16.0],
            ..Default::default()
        })),
    }]);
    for (i, h) in [30.0, 50.0, 40.0].into_iter().enumerate() {
        let r = d.rect(row, 0.0, 0.0, 50.0, h, 6.0);
        d.fill(r, solid(60 + 60 * i as u8, 100, 200, 255));
        if i == 1 {
            d.apply(vec![Operation::SetLayoutItem {
                id: r,
                item: LayoutItem {
                    grow: 1.0,
                    ..Default::default()
                },
            }]);
        }
    }
    let grid = d.frame(None, 0.0, 100.0, 200.0, 120.0, "Grid");
    d.apply(vec![Operation::SetDisplay {
        id: grid,
        display: Some(Display::Grid(Grid {
            columns: vec![
                Track::Size(TrackSize::Breadth(TrackBreadth::Fr(1.0))),
                Track::Size(TrackSize::Breadth(TrackBreadth::Fr(2.0))),
            ],
            rows: vec![Track::Repeat {
                repeat: 2,
                tracks: vec![TrackSize::Breadth(TrackBreadth::Px(40.0))],
            }],
            row_gap: 8.0,
            column_gap: 8.0,
            padding: [12.0; 4],
            ..Default::default()
        })),
    }]);
    for i in 0..4 {
        let r = d.rect(grid, 0.0, 0.0, 30.0, 30.0, 0.0);
        d.fill(r, solid(230, 90 + 30 * i as u8, 60, 255));
    }
    let (doc, res) = d.done();
    holds("layout", &doc, &res);
}

/// A hugging group with a column layout inside a frame, and children pinned by
/// insets — one to the bottom right, one stretched across.
#[test]
fn insets_and_hugging_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 160.0, "Board");
    let pin = d.rect(f, 0.0, 0.0, 40.0, 30.0, 0.0);
    d.fill(pin, solid(220, 60, 60, 255));
    d.apply(vec![Operation::SetInsets {
        id: pin,
        insets: Insets {
            right: Some(LengthPct::Px(12.0)),
            bottom: Some(LengthPct::Px(10.0)),
            ..Default::default()
        },
    }]);
    let bar = d.rect(f, 0.0, 10.0, 40.0, 16.0, 8.0);
    d.fill(bar, solid(60, 60, 220, 255));
    d.apply(vec![Operation::SetInsets {
        id: bar,
        insets: Insets {
            left: Some(LengthPct::Px(20.0)),
            right: Some(LengthPct::Percent(25.0)),
            ..Default::default()
        },
    }]);
    let stack = d.add(
        Some(f),
        NodeKind::Group,
        Affine::translate((20.0, 40.0)),
        "Stack",
    );
    d.apply(vec![Operation::SetDisplay {
        id: stack,
        display: Some(Display::Flex(Flex {
            direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: 6.0,
            ..Default::default()
        })),
    }]);
    for w in [60.0, 30.0, 45.0] {
        let r = d.rect(stack, 0.0, 0.0, w, 14.0, 0.0);
        d.fill(r, solid(40, 160, 110, 255));
    }
    let (doc, res) = d.done();
    holds("insets", &doc, &res);
}

/// What CSS cannot draw goes to inline SVG and still matches: a star, a path and
/// an inner shadow, beside a group with no layout.
#[test]
fn pictures_match_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 260.0, 120.0, "Board");
    let star = d.add(
        Some(f),
        NodeKind::Star {
            size: Size::new(70.0, 70.0),
            points: 5,
            inner_ratio: 0.5,
        },
        Affine::translate((20.0, 20.0)),
        "Star",
    );
    d.fill(star, solid(250, 180, 30, 255));
    let mut p = ondin_core::kurbo::BezPath::new();
    p.move_to((0.0, 0.0));
    p.curve_to((40.0, -20.0), (60.0, 60.0), (80.0, 40.0));
    p.line_to((20.0, 60.0));
    p.close_path();
    let path = d.add(
        Some(f),
        NodeKind::Path {
            path: p,
            corner_radii: vec![],
        },
        Affine::translate((100.0, 30.0)),
        "Blob",
    );
    d.fill(path, solid(80, 160, 240, 255));
    let g = d.add(
        Some(f),
        NodeKind::Group,
        Affine::translate((190.0, 20.0)),
        "Pair",
    );
    let a = d.rect(g, 0.0, 0.0, 50.0, 40.0, 6.0);
    d.fill(a, solid(240, 240, 240, 255));
    d.effects(
        a,
        vec![EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(0.0, 3.0),
            blur: 6.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, 160),
        })],
    );
    let b = d.rect(g, 10.0, 50.0, 40.0, 30.0, 0.0);
    d.fill(b, solid(30, 30, 30, 255));
    let (doc, res) = d.done();
    holds("pictures", &doc, &res);
}

/// A picture pinned by insets sits where layout puts it, not at its stored
/// transform.
///
/// flip: `css_element` placing an out-of-flow child by `Node::transform` → the
/// star is drawn at the frame's top-left.
#[test]
fn a_pinned_picture_sits_where_layout_puts_it() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 240.0, 160.0, "Board");
    let star = d.add(
        Some(f),
        NodeKind::Star {
            size: Size::new(50.0, 50.0),
            points: 5,
            inner_ratio: 0.5,
        },
        Affine::IDENTITY,
        "Star",
    );
    d.fill(star, solid(250, 180, 30, 255));
    d.apply(vec![Operation::SetInsets {
        id: star,
        insets: Insets {
            right: Some(LengthPct::Px(12.0)),
            bottom: Some(LengthPct::Px(10.0)),
            ..Default::default()
        },
    }]);
    let (doc, res) = d.done();
    assert_eq!(
        doc.get(star).unwrap().transform(),
        Affine::IDENTITY,
        "the stored transform is not where the star is drawn"
    );
    holds("pinned-picture", &doc, &res);
}

/// A text node with its own size and colour and a bold run, one wrapping at a
/// width with a second paragraph, and one centred in a fixed box.
#[test]
fn text_matches_the_canvas() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 300.0, 200.0, "Board");
    let text = |content: &str, sizing, style: TextStyle| NodeKind::Text {
        content: content.into(),
        style: Box::new(style),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    let style = TextStyle {
        font_size: 20.0,
        line_height: Some(ondin_core::Length::Em(1.25)),
        ..Default::default()
    };
    let mut title = text("Hello, layers", TextSizing::Auto, style.clone());
    if let NodeKind::Text { spans, style, .. } = &mut title {
        spans.set(7..13, CharAttr::Weight(700), style);
    }
    let t = d.add(Some(f), title, Affine::translate((16.0, 12.0)), "Title");
    d.fill(t, solid(30, 40, 60, 255));
    let body = text(
        "Two short lines that wrap at the box.\nAnd a second paragraph.",
        TextSizing::AutoHeight(150.0),
        TextStyle {
            font_size: 13.0,
            line_height: Some(ondin_core::Length::Px(18.0)),
            ..Default::default()
        },
    );
    d.add(Some(f), body, Affine::translate((16.0, 50.0)), "Body");
    let mut centred = text(
        "Centred",
        TextSizing::Fixed(Size::new(110.0, 60.0)),
        TextStyle {
            font_size: 15.0,
            italic: true,
            ..Default::default()
        },
    );
    if let NodeKind::Text {
        paragraph, block, ..
    } = &mut centred
    {
        paragraph.align = TextAlign::Center;
        block.vertical_align = VerticalAlign::Middle;
    }
    let c = d.add(Some(f), centred, Affine::translate((180.0, 120.0)), "Badge");
    d.fill(c, solid(200, 40, 90, 255));
    let (doc, res) = d.done();
    holds_within("text", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// A text node of `content` in `style`, edited by `edit` before it is placed.
fn text_node(
    content: &str,
    sizing: TextSizing,
    style: TextStyle,
    edit: impl FnOnce(&mut NodeKind),
) -> NodeKind {
    let mut kind = NodeKind::Text {
        content: content.into(),
        style: Box::new(style),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    edit(&mut kind);
    kind
}

/// The 18px Inter the text fixtures below are set in, on a 24px line.
fn body_style() -> TextStyle {
    TextStyle {
        font_size: 18.0,
        line_height: Some(ondin_core::Length::Px(24.0)),
        ..Default::default()
    }
}

/// Underlines and strikethroughs: the font's own, a thick coloured one that does
/// not skip ink, one moved off the baseline, both on one run, one on a middle run,
/// and a node's underline that one run takes off.
///
/// flip: `Walk::run` taking the node's underline for every run → 31.2 on the
/// runs whose underline differs from their node's.
#[test]
fn text_decorations_match_the_canvas() {
    use ondin_core::Decoration;
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 230.0, "Board");
    let under = |deco: Decoration| TextStyle {
        underline: Some(deco),
        ..body_style()
    };
    let red = Color::from_rgba8(220, 30, 40, 255);
    let lines = [
        ("Underlined gyp", under(Decoration::default())),
        (
            "Thick red, through gyp",
            under(Decoration {
                thickness: Some(ondin_core::Length::Px(4.0)),
                color: Some(red),
                skip_ink: false,
                ..Default::default()
            }),
        ),
        (
            "Moved down",
            under(Decoration {
                offset: Some(ondin_core::Length::Px(-6.0)),
                ..Default::default()
            }),
        ),
        (
            "Struck through",
            TextStyle {
                strikethrough: Some(Decoration::default()),
                ..body_style()
            },
        ),
        (
            "Both at once",
            TextStyle {
                underline: Some(Decoration::default()),
                strikethrough: Some(Decoration {
                    color: Some(red),
                    ..Default::default()
                }),
                ..body_style()
            },
        ),
    ];
    for (i, (content, style)) in lines.into_iter().enumerate() {
        let t = text_node(content, TextSizing::Auto, style, |_| {});
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 12.0 + i as f64 * 32.0)),
            "Line",
        );
    }
    let middle = text_node("plain marked plain", TextSizing::Auto, body_style(), |k| {
        if let NodeKind::Text { spans, style, .. } = k {
            spans.set(
                6..12,
                CharAttr::Underline(Some(Decoration::default())),
                style,
            );
        }
    });
    d.add(Some(f), middle, Affine::translate((16.0, 172.0)), "Middle");
    let off = text_node(
        "all but this one",
        TextSizing::Auto,
        under(Decoration::default()),
        |k| {
            if let NodeKind::Text { spans, style, .. } = k {
                spans.set(8..12, CharAttr::Underline(None), style);
            }
        },
    );
    d.add(Some(f), off, Affine::translate((16.0, 200.0)), "Off");
    let (doc, res) = d.done();
    holds_within("decorations", &doc, &res, 2, MAX_DECORATION_CELL_ERROR);
}

/// Decorations' limit, over text's: Chrome snaps a band to whole device pixels
/// where the canvas antialiases its fractional thickness and position, so a
/// 1.3px underline is a crisp 1px one on the page. The matching fixture peaks at
/// 9.7, and at 14.1 under macOS Chrome's glyph smoothing (see `MAX_TEXT_CELL_ERROR`).
const MAX_DECORATION_CELL_ERROR: f64 = if cfg!(target_os = "macos") {
    22.0
} else {
    12.0
};

/// A run's baseline shift moves its ink and leaves the lines where they are, a
/// whole node's shift moves all of it, and case is the canvas's: Title lowercases
/// the rest of each word and starts one after an apostrophe, Upper keeps `ß`.
///
/// flip: `Walk::run` writing a shift as `vertical-align` → 82.8, the shifted
/// run's line box growing and the second line dropping.
#[test]
fn baseline_shift_and_case_match_the_canvas() {
    use ondin_core::TextCase;
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 200.0, "Board");
    let shifted = text_node("x2 and\nmore below", TextSizing::Auto, body_style(), |k| {
        if let NodeKind::Text { spans, style, .. } = k {
            spans.set(
                1..2,
                CharAttr::BaselineShift(ondin_core::Length::Px(8.0)),
                style,
            );
        }
    });
    d.add(Some(f), shifted, Affine::translate((16.0, 12.0)), "Power");
    let whole = text_node(
        "Raised whole",
        TextSizing::Auto,
        TextStyle {
            baseline_shift: ondin_core::Length::Px(6.0),
            ..body_style()
        },
        |_| {},
    );
    d.add(Some(f), whole, Affine::translate((160.0, 12.0)), "Raised");
    for (i, (content, case)) in [
        ("HELLO WORLD it's", TextCase::Title),
        ("straße", TextCase::Upper),
        ("QUIET Words", TextCase::Lower),
    ]
    .into_iter()
    .enumerate()
    {
        let t = text_node(
            content,
            TextSizing::Auto,
            TextStyle {
                case,
                ..body_style()
            },
            |_| {},
        );
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 90.0 + i as f64 * 32.0)),
            "Cased",
        );
    }
    let (doc, res) = d.done();
    holds_within("shift-and-case", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// Paragraph breaking and alignment: no wrap, a justified paragraph with its last
/// line centred, break-all, a long word broken anywhere, and an RTL paragraph's
/// start edge on the right.
///
/// flip: `Walk::text` leaving `wrap` unread → 85.4, the no-wrap line wrapping on
/// the page.
#[test]
fn paragraph_breaking_matches_the_canvas() {
    use ondin_core::{JustifyLast, OverflowWrap, TextDirection, WordBreak, WrapMode};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 420.0, 330.0, "Board");
    let paragraph = |edit: fn(&mut ondin_core::ParagraphStyle)| {
        move |k: &mut NodeKind| {
            if let NodeKind::Text { paragraph, .. } = k {
                edit(paragraph);
            }
        }
    };
    let cases = [
        (
            "A line that would wrap at the box",
            150.0,
            paragraph(|p| p.wrap = WrapMode::NoWrap),
        ),
        (
            "Justified text whose last line sits in the middle",
            180.0,
            paragraph(|p| {
                p.align = TextAlign::Justify;
                p.justify_last = JustifyLast::Center;
            }),
        ),
        (
            "Break all of these words",
            120.0,
            paragraph(|p| p.word_break = WordBreak::BreakAll),
        ),
        (
            "Antidisestablishmentarianism",
            120.0,
            paragraph(|p| p.overflow_wrap = OverflowWrap::Anywhere),
        ),
        (
            "Starts on the right",
            180.0,
            paragraph(|p| p.direction = TextDirection::Rtl),
        ),
    ];
    for (i, (content, width, edit)) in cases.into_iter().enumerate() {
        let (x, y) = (16.0 + (i % 2) as f64 * 200.0, 12.0 + (i / 2) as f64 * 100.0);
        let t = text_node(content, TextSizing::AutoHeight(width), body_style(), edit);
        d.add(Some(f), t, Affine::translate((x, y)), "Para");
    }
    let (doc, res) = d.done();
    holds_within("breaking", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// Letter and word spacing, a first-line indent, a start indent, space between
/// paragraphs, and a font feature.
///
/// flip: `face` dropping letter spacing → 71.7.
#[test]
fn text_spacing_and_features_match_the_canvas() {
    use ondin_core::{FeatureSetting, Tag};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 200.0, "Board");
    let spaced = text_node(
        "Spaced out words",
        TextSizing::Auto,
        TextStyle {
            letter_spacing: ondin_core::Length::Px(2.0),
            word_spacing: ondin_core::Length::Px(6.0),
            ..body_style()
        },
        |_| {},
    );
    d.add(Some(f), spaced, Affine::translate((16.0, 12.0)), "Spaced");
    let indented = text_node(
        "First paragraph runs on.\nSecond one.",
        TextSizing::AutoHeight(220.0),
        body_style(),
        |k| {
            if let NodeKind::Text { paragraph, .. } = k {
                paragraph.indent = ondin_core::Length::Px(20.0);
                paragraph.indent_start = ondin_core::Length::Px(10.0);
                paragraph.spacing = ondin_core::Length::Px(8.0);
            }
        },
    );
    d.add(
        Some(f),
        indented,
        Affine::translate((16.0, 50.0)),
        "Indented",
    );
    let figures = text_node(
        "1111 0000",
        TextSizing::Auto,
        TextStyle {
            features: vec![FeatureSetting {
                tag: Tag::new(*b"tnum"),
                value: 1,
            }],
            ..body_style()
        },
        |_| {},
    );
    d.add(
        Some(f),
        figures,
        Affine::translate((16.0, 150.0)),
        "Figures",
    );
    let (doc, res) = d.done();
    holds_within("spacing", &doc, &res, 2, MAX_TEXT_CELL_ERROR);
}

/// What the page cannot draw as the canvas does is reported: a wavy underline, a
/// strikethrough moved off the font's position, an ellipsis that fires, optical
/// margins and right-to-left content.
#[test]
fn text_the_page_cannot_match_is_reported() {
    use ondin_core::{Decoration, LineStyle, TextOverflow};
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 320.0, 300.0, "Board");
    // The layer's name, its content, its style and an edit of its kind.
    type Case = (&'static str, &'static str, TextStyle, fn(&mut NodeKind));
    let cases: [Case; 5] = [
        (
            "Wavy",
            "wavy",
            TextStyle {
                underline: Some(Decoration {
                    style: LineStyle::Wavy,
                    ..Default::default()
                }),
                ..body_style()
            },
            |_| {},
        ),
        (
            "Moved",
            "moved strike",
            TextStyle {
                strikethrough: Some(Decoration {
                    offset: Some(ondin_core::Length::Px(3.0)),
                    ..Default::default()
                }),
                ..body_style()
            },
            |_| {},
        ),
        (
            "Clipped",
            "A long line that will not fit",
            body_style(),
            |k| {
                if let NodeKind::Text { block, sizing, .. } = k {
                    block.overflow = TextOverflow::Ellipsis;
                    *sizing = TextSizing::Fixed(Size::new(150.0, 24.0));
                }
            },
        ),
        ("Optical", "Optical", body_style(), |k| {
            if let NodeKind::Text { paragraph, .. } = k {
                paragraph.optical_margins = true;
            }
        }),
        ("Hebrew", "שלום", body_style(), |_| {}),
    ];
    for (i, (name, content, style, edit)) in cases.into_iter().enumerate() {
        let t = text_node(content, TextSizing::Auto, style, edit);
        d.add(
            Some(f),
            t,
            Affine::translate((16.0, 12.0 + i as f64 * 40.0)),
            name,
        );
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec![
            "text “Wavy”: a dashed, dotted or wavy underline",
            "text “Moved”: a strikethrough moved off the font's position",
            "text “Clipped”: an ellipsis where the text overflows",
            "text “Optical”: optical margin alignment",
            "text “Hebrew”: right-to-left text",
        ]
    );
}

/// Variables as `var()` in two modes: one frame in the default, one set to Dark
/// with a Light frame inside. A fill bound through an alias (at full and half
/// alpha), and bound radius, gap, opacity, width, stroke width and shadow blur.
///
/// flip: `Tokens::css` declaring aliases on `:root` only → 136.7 in the dark
/// frame, whose alias keeps the light colour; its default block on `:root` alone
/// → 136.7 at the light frame nested in the dark one.
#[test]
fn variables_match_the_canvas_in_every_mode() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    d.apply(vec![
        Operation::AddMode {
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
    ]);
    let var = |d: &mut Doc, name: &str, kind, a: Cell, b: Cell| {
        let id = d.ids.mint_var();
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(light, a), (dark, b)].into_iter().collect(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
        id
    };
    let brand = var(
        &mut d,
        "brand/500",
        VarKind::Color,
        Cell::Color(Color::from_rgba8(230, 70, 50, 255)),
        Cell::Color(Color::from_rgba8(60, 120, 240, 255)),
    );
    let accent = var(
        &mut d,
        "accent",
        VarKind::Color,
        Cell::Alias(brand),
        Cell::Alias(brand),
    );
    let radius = var(
        &mut d,
        "radius",
        VarKind::Number,
        Cell::Number(6.0),
        Cell::Number(18.0),
    );
    let gap = var(
        &mut d,
        "gap",
        VarKind::Number,
        Cell::Number(8.0),
        Cell::Number(24.0),
    );
    let fade = var(
        &mut d,
        "fade",
        VarKind::Number,
        Cell::Number(100.0),
        Cell::Number(40.0),
    );
    let num = |d: &mut Doc, name: &str, a: f64, b: f64| {
        var(d, name, VarKind::Number, Cell::Number(a), Cell::Number(b))
    };
    let wide = num(&mut d, "wide", 40.0, 70.0);
    let line = num(&mut d, "line", 2.0, 6.0);
    let soft = num(&mut d, "soft", 4.0, 12.0);

    for (i, mode) in [None, Some(dark)].into_iter().enumerate() {
        let f = d.frame(None, 0.0, i as f64 * 90.0, 260.0, 80.0, "Board");
        d.apply(vec![
            Operation::SetMode { id: f, mode },
            Operation::SetDisplay {
                id: f,
                display: Some(Display::Flex(Flex {
                    align_items: AlignItems::Center,
                    padding: [10.0; 4],
                    ..Default::default()
                })),
            },
            Operation::SetSpacingVars {
                id: f,
                vars: [Some(gap), None, None, None, None, None],
            },
        ]);
        for j in 0..3 {
            let r = d.rect(f, 0.0, 0.0, 50.0, 50.0, 0.0);
            d.apply(vec![
                Operation::SetFills {
                    id: r,
                    fills: keyed_by_position([Fill {
                        brush: BrushSource::Bound {
                            var: accent,
                            alpha: if j == 1 { 0.5 } else { 1.0 },
                        },
                        visible: true,
                    }]),
                },
                Operation::SetCornerVars {
                    id: r,
                    vars: [Some(radius), Some(radius), None, Some(radius)],
                },
            ]);
            if j == 2 {
                d.apply(vec![
                    Operation::SetOpacityVar {
                        id: r,
                        var: Some(fade),
                    },
                    Operation::SetSizeVars {
                        id: r,
                        vars: [Some(wide), None],
                    },
                ]);
            }
            if j == 0 {
                d.stroke(r, 0.0, StrokeAlign::Center, StrokeSides::All);
                let mut strokes = d.doc.get(r).unwrap().paint().strokes.clone();
                strokes[0].value.width_var = Some(line);
                d.apply(vec![Operation::SetStrokes { id: r, strokes }]);
            }
            if j == 1 {
                let mut shadow = Effect::new(EffectKind::DropShadow(Shadow {
                    offset: Vec2::new(0.0, 3.0),
                    blur: 0.0,
                    spread: 0.0,
                    color: Color::from_rgba8(0, 0, 0, 120),
                }));
                shadow.vars[2] = Some(soft);
                d.apply(vec![Operation::SetEffects {
                    id: r,
                    effects: keyed_by_position([shadow]),
                }]);
            }
        }
        // The default mode set inside another: back to the light accent.
        if mode.is_some() {
            let inner = d.add(
                Some(f),
                NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                },
                Affine::IDENTITY,
                "Light again",
            );
            d.apply(vec![
                Operation::SetMode {
                    id: inner,
                    mode: Some(light),
                },
                Operation::SetFills {
                    id: inner,
                    fills: keyed_by_position([Fill {
                        brush: BrushSource::Bound {
                            var: accent,
                            alpha: 1.0,
                        },
                        visible: true,
                    }]),
                },
            ]);
        }
    }
    let (doc, res) = d.done();
    holds("variables", &doc, &res);
    let export = html_of(&doc, &res, doc.get(doc.root()).unwrap().children());
    let tokens =
        String::from_utf8(export.file("tokens.css").expect("tokens.css").to_vec()).unwrap();
    assert!(
        tokens.contains(
            "[data-mode=\"dark\"] {\n  --brand-500: #3c78f0;\n  --accent: var(--brand-500);"
        ),
        "the alias is redeclared in the dark block:\n{tokens}"
    );
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    for read in [
        "outline: max(0px, calc(var(--line) * 1px)) solid #14141e",
        "outline-offset: calc(max(0px, calc(var(--line) * 1px)) * -0.5)",
        "color-mix(in srgb, var(--accent) 50%, transparent)",
        "drop-shadow(0 3px calc(max(0px, calc(var(--soft) * 1px)) * 0.5)",
        "opacity: calc(var(--fade) / 100)",
        "width: max(0px, calc(var(--wide) * 1px))",
        "min-width: max(0px, calc(var(--wide) * 1px))",
        "column-gap: calc(var(--gap) * 1px)",
        "min(max(0px, calc(var(--radius) * 1px)), 25px)",
        "min(max(0px, calc(var(--radius) * 1px)), calc(min(max(0px, calc(var(--wide) * 1px)), 50px) / 2))",
    ] {
        assert!(css.contains(read), "styles.css lacks `{read}`:\n{css}");
    }
}

/// A 24×16 picture with a hard quadrant in each corner over a smooth ramp, so a
/// mirror, a turn or a misplaced crop all move something visible.
fn picture_png() -> Vec<u8> {
    let (w, h) = (24u32, 16u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let px = match (x < 6, y < 4, x >= w - 6, y >= h - 4) {
                (true, true, _, _) => [220, 40, 40, 255],
                (_, true, true, _) => [40, 160, 60, 255],
                (true, _, _, true) => [40, 70, 220, 255],
                (_, _, true, true) => [240, 200, 40, 255],
                _ => [(x * 10) as u8, (y * 15) as u8, 160, 255],
            };
            rgba.extend_from_slice(&px);
        }
    }
    ondin_export::png::encode_rgba8(&rgba, w, h)
}

/// Image fills as backgrounds: each of the four fits, a turned, mirrored picture
/// whose file is baked upright, and a crop under per-side borders.
///
/// flip: Fill written as `contain` → 155 at the first rect; the bake skipping the
/// mirror → 149 at the fifth; `background-origin` left at `padding-box` → 50.8 at
/// the bordered one.
#[test]
fn image_fills_match_the_canvas() {
    use ondin_core::{
        ImageBrush, ImageEntry, ImageFit, ImageFormat, ImageId, ImageOrient, ImageRef, ImageSource,
    };
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 502.0, 100.0, "Board");
    let fills = |r: ImageRef| {
        vec![Brush::Image(ImageBrush {
            image: r,
            sampler: Default::default(),
        })]
    };
    let crop = ImageRef {
        fit: ImageFit::Crop,
        crop: ondin_core::kurbo::Rect::new(0.1, 0.2, 0.6, 0.9),
        ..ImageRef::new(id.clone())
    };
    let base = ImageRef::new(id);
    let cases = [
        ImageRef {
            fit: ImageFit::Fill,
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Fit,
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Crop,
            crop: ondin_core::kurbo::Rect::new(0.1, 0.2, 0.6, 0.9),
            ..base.clone()
        },
        ImageRef {
            fit: ImageFit::Tile,
            tile_scale: 1.5,
            ..base.clone()
        },
        ImageRef {
            orient: ImageOrient {
                quarters: 1,
                mirrored: true,
            },
            ..base.clone()
        },
    ];
    for (i, r) in cases.into_iter().enumerate() {
        let rect = d.rect(f, 10.0 + i as f64 * 82.0, 10.0, 72.0, 80.0, 0.0);
        d.fills(rect, fills(r));
    }
    // Borders do not move the picture: it is framed on the whole box.
    let bordered = d.rect(f, 420.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(bordered, fills(crop.clone()));
    d.stroke(
        bordered,
        4.0,
        StrokeAlign::Inside,
        StrokeSides::Custom {
            top: 10.0,
            right: 0.0,
            bottom: 2.0,
            left: 6.0,
        },
    );
    let (doc, res) = d.done();
    holds("images", &doc, &res);
    let export = html_of(&doc, &res, &[f]);
    assert!(
        export.positioned.is_empty(),
        "all CSS: {:?}",
        export.positioned
    );
    assert!(export.file("assets/photo.png").is_some());
    assert!(
        export.file("assets/photo-r1m.png").is_some(),
        "the turned file is baked"
    );
}

/// A GIF ships as a PNG of its first frame, the frame the canvas draws, since a
/// browser would animate the GIF; and two pictures sampled differently on one
/// layer go to SVG, `image-rendering` being the element's.
#[test]
fn a_gif_is_baked_and_mixed_sampling_is_svg() {
    use ondin_core::peniko::{ImageQuality, ImageSampler};
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let frame = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif)
        .encode_frame(image::Frame::new(frame))
        .expect("a gif");
    let mut d = Doc::new();
    let id = ImageId("anim".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(gif.into()),
            format: ImageFormat::Gif,
            width: 4,
            height: 4,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let fill = |quality| {
        Brush::Image(ImageBrush {
            image: ImageRef::new(id.clone()),
            sampler: ImageSampler {
                quality,
                ..Default::default()
            },
        })
    };
    let one = d.rect(f, 10.0, 10.0, 30.0, 30.0, 0.0);
    d.fills(one, vec![fill(ImageQuality::Low)]);
    let two = d.rect(f, 50.0, 10.0, 30.0, 30.0, 0.0);
    d.fills(two, vec![fill(ImageQuality::Low), fill(ImageQuality::High)]);
    d.apply(vec![Operation::SetName {
        id: two,
        name: "Mixed".into(),
    }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let png = export
        .file("assets/anim.png")
        .expect("the GIF is written as PNG");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(export.file("assets/anim.gif").is_none());
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains("image-rendering: pixelated"), "{css}");
    assert_eq!(
        export.positioned,
        vec!["layer “Mixed”: pictures sampled two ways, written as inline SVG"]
    );
}

/// A crop past the picture's edge, which the canvas fills with the edge pixels,
/// has neither a CSS nor an SVG form: it is a PNG, and reported.
///
/// flip: the SVG writer not reporting `crops_past_picture` → `approximated` is
/// empty and the layer goes out as an SVG that repeats the picture.
#[test]
fn a_crop_past_the_picture_is_a_reported_png() {
    use ondin_core::{
        ImageBrush, ImageEntry, ImageFit, ImageFormat, ImageId, ImageRef, ImageSource,
    };
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 10.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(
        r,
        vec![Brush::Image(ImageBrush {
            image: ImageRef {
                fit: ImageFit::Crop,
                crop: ondin_core::kurbo::Rect::new(-0.2, 0.0, 0.8, 1.0),
                ..ImageRef::new(id)
            },
            sampler: Default::default(),
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec![
            "layer “Rect”: a crop past the picture's edge, repeated where the canvas stretches it, so it is a PNG"
        ]
    );
    assert!(export.file("assets/rect.png").is_some());
}

/// A picture fill under 100% goes to SVG, CSS backgrounds having no opacity of
/// their own. Not compared in a browser: the CPU renderer cannot draw an image
/// paint with alpha (`vello_common` leaves it unimplemented).
#[test]
fn a_faint_picture_fill_is_written_as_svg() {
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 10.0, 10.0, 72.0, 80.0, 0.0);
    d.fills(
        r,
        vec![Brush::Image(ImageBrush {
            image: ImageRef::new(id),
            sampler: ondin_core::peniko::ImageSampler {
                alpha: 0.5,
                ..Default::default()
            },
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.positioned,
        vec!["layer “Rect”: a picture fill under 100%, written as inline SVG"]
    );
}

/// Inline SVGs link the pictures they draw, so layers sharing one share its file
/// rather than each embedding it; React imports it, Vue links it from the file.
///
/// flip: `Walk::picture` linking nothing → the page embeds two base64 copies.
#[test]
fn a_picture_in_inline_svg_is_one_linked_file() {
    use ondin_core::{ImageBrush, ImageEntry, ImageFormat, ImageId, ImageRef, ImageSource};
    let mut d = Doc::new();
    let id = ImageId("photo".into());
    d.apply(vec![Operation::AddImage {
        id: id.clone(),
        entry: ImageEntry {
            source: ImageSource::Embedded(picture_png().into()),
            format: ImageFormat::Png,
            width: 24,
            height: 16,
        },
    }]);
    let f = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    for x in [10.0, 100.0] {
        let r = d.rect(f, x, 10.0, 72.0, 80.0, 0.0);
        d.fills(
            r,
            vec![Brush::Image(ImageBrush {
                image: ImageRef::new(id.clone()),
                sampler: ondin_core::peniko::ImageSampler {
                    alpha: 0.5,
                    ..Default::default()
                },
            })],
        );
    }
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(!html.contains("base64"), "{html}");
    assert_eq!(
        html.matches("href=\"assets/photo.png\"").count(),
        2,
        "{html}"
    );
    assert!(export.file("assets/photo.png").is_some());

    let react = ondin_export::code::react::react_of(&doc, &res, &[f]);
    let app = String::from_utf8(react.file("App.tsx").unwrap().to_vec()).unwrap();
    assert!(
        app.contains("import photo_png from \"./assets/photo.png\";"),
        "{app}"
    );
    assert!(app.contains("href={photo_png}"), "{app}");
    let vue = ondin_export::code::vue::vue_of(&doc, &res, &[f]);
    let app = String::from_utf8(vue.file("App.vue").unwrap().to_vec()).unwrap();
    assert!(app.contains("href=\"./assets/photo.png\""), "{app}");
}

/// A text's bound colour and a single side's bound width are `var()` too, not the
/// default mode's numbers.
#[test]
fn bound_text_colour_and_side_width_are_written_as_var() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let mode = d.ids.mint_mode();
    d.apply(vec![Operation::AddMode {
        mode: Mode {
            id: mode,
            name: "Base".into(),
        },
        index: 0,
        cells: vec![],
    }]);
    let (ink, edge) = (d.ids.mint_var(), d.ids.mint_var());
    for (id, name, kind, cell) in [
        (ink, "ink", VarKind::Color, Cell::Color(Color::BLACK)),
        (edge, "edge", VarKind::Number, Cell::Number(3.0)),
    ] {
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: [(mode, cell)].into_iter().collect(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
    }
    let f = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    let t = d.add(
        Some(f),
        NodeKind::Text {
            content: "Ink".into(),
            style: Box::default(),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        Affine::translate((10.0, 10.0)),
        "Label",
    );
    d.apply(vec![Operation::SetFills {
        id: t,
        fills: keyed_by_position([Fill {
            brush: BrushSource::Bound {
                var: ink,
                alpha: 1.0,
            },
            visible: true,
        }]),
    }]);
    let r = d.rect(f, 10.0, 50.0, 80.0, 30.0, 0.0);
    d.stroke(r, 0.0, StrokeAlign::Inside, StrokeSides::Top);
    let mut strokes = d.doc.get(r).unwrap().paint().strokes.clone();
    strokes[0].value.width_var = Some(edge);
    d.apply(vec![Operation::SetStrokes { id: r, strokes }]);
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    assert!(css.contains("color: var(--ink)"), "{css}");
    assert!(
        css.contains("border-top: max(0px, calc(var(--edge) * 1px)) solid"),
        "{css}"
    );
}

/// A feature the SVG writer approximates goes to a PNG and is reported, not
/// passed off as exact.
#[test]
fn an_approximated_layer_is_reported() {
    let mut d = Doc::new();
    let f = d.frame(None, 0.0, 0.0, 100.0, 100.0, "Board");
    let r = d.rect(f, 20.0, 20.0, 60.0, 60.0, 0.0);
    d.fill(r, solid(200, 200, 200, 255));
    d.effects(
        r,
        vec![EffectKind::DropShadow(Shadow {
            offset: Vec2::new(2.0, 2.0),
            blur: 4.0,
            spread: 6.0,
            color: Color::BLACK,
        })],
    );
    let (doc, res) = d.done();
    let export = html_of(&doc, &res, &[f]);
    assert_eq!(
        export.approximated,
        vec!["layer “Rect”: a shadow's spread, drawn with a square kernel, so it is a PNG"]
    );
    assert!(
        export.file("assets/rect.png").is_some(),
        "the PNG is in the export"
    );
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert!(
        html.contains(r#"<img class="rect" src="assets/rect.png""#),
        "{html}"
    );
}

// ---------------------------------------------------------------------------
// Components: the React and Vue walk, drawn as plain HTML
// ---------------------------------------------------------------------------

impl Doc {
    /// A card — a frame holding an icon and a title, `painted` after it is made —
    /// under `parent`, linked to `main`'s layers when it is given.
    fn card(
        &mut self,
        parent: NodeId,
        at: (f64, f64),
        main: Option<[NodeId; 3]>,
        painted: impl Fn(&mut Doc, [NodeId; 3]),
    ) -> [NodeId; 3] {
        let frame = self.frame(Some(parent), at.0, at.1, 160.0, 60.0, "Card");
        let icon = self.rect(frame, 12.0, 18.0, 24.0, 24.0, 4.0);
        self.fill(icon, solid(220, 60, 60, 255));
        let title = text_node(
            "Hello",
            TextSizing::Auto,
            TextStyle {
                font_size: 20.0,
                line_height: Some(ondin_core::Length::Px(24.0)),
                ..Default::default()
            },
            |_| {},
        );
        let title = self.add(Some(frame), title, Affine::translate((48.0, 18.0)), "Title");
        let card = [frame, icon, title];
        painted(self, card);
        match main {
            None => self.apply(vec![Operation::SetComponent {
                id: frame,
                component: true,
            }]),
            Some(m) => {
                // The two trees are alike, `painted`'s layers included.
                let mut links = Vec::new();
                let mut pairs = vec![(frame, m[0])];
                while let Some((copy, source)) = pairs.pop() {
                    links.push(Operation::SetLink {
                        id: copy,
                        link: Some(source),
                    });
                    let children = |id| self.doc.get(id).unwrap().children().to_vec();
                    pairs.extend(children(copy).into_iter().zip(children(source)));
                }
                self.apply(links);
            }
        }
        card
    }

    /// `main` given a property per `(name, kind, bound layer)`.
    fn properties(&mut self, main: NodeId, props: &[(&str, PropKind, NodeId)]) {
        for (name, kind, bound) in props {
            let (tx, _) =
                variant::define(&self.doc, &mut self.ids, main, name, *kind, vec![*bound])
                    .expect("a property");
            self.doc.apply(&tx).expect("the property applies");
        }
    }

    /// The instance at `root` set to `values`, in its properties' order.
    fn set_properties(&mut self, root: NodeId, values: Vec<PropValue>) {
        let props = variant::instance_properties(&self.doc, root);
        for (p, v) in props.iter().zip(values) {
            let ops = variant::set_property(&self.doc, &[root], p, &v);
            self.apply(ops);
        }
    }
}

/// A main card with a boolean and a text property, and two uses of it: one
/// setting both, one setting neither. The control for the component fixtures:
/// both are uses, and the page matches.
///
/// flip: `html_of_components` dropping a use's arguments → 125.3 at the icon
/// the set use hides.
#[test]
fn component_uses_match_the_canvas() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 380.0, 180.0, "Board");
    let main = d.card(board, (10.0, 10.0), None, |_, _| {});
    d.properties(
        main[0],
        &[
            ("Show icon", PropKind::Boolean, main[1]),
            ("Title", PropKind::Text, main[2]),
        ],
    );
    let set = d.card(board, (10.0, 100.0), Some(main), |_, _| {});
    d.set_properties(
        set[0],
        vec![PropValue::Boolean(false), PropValue::Text("World".into())],
    );
    d.card(board, (200.0, 100.0), Some(main), |_, _| {});
    let (doc, res) = d.done();
    let export = html_of_components(&doc, &res, &[board]);
    assert!(export.inlined.is_empty(), "{:?}", export.inlined);
    let html = String::from_utf8(export.file("index.html").unwrap().to_vec()).unwrap();
    assert_eq!(html.matches("Card--card").count(), 2, "two uses: {html}");
    holds_components("component-uses", &doc, &res);
}

/// [`holds`] for [`html_of_components`], after three passes of `blurred`: a
/// component's text lands at whatever x its use puts it, and at some offsets
/// glyph antialiasing alone passes the two-pass limit (8.9 on the default use
/// in `component_uses_match_the_canvas`).
fn holds_components(test: &str, doc: &Document, res: &Resolved) {
    holds_as(test, doc, res, 3, MAX_TEXT_CELL_ERROR, html_of_components);
}

/// A boolean property bound to a dot inside a masked group, which is drawn as a
/// picture: a use cannot pass it into the picture, so the instance hiding the
/// dot is its layers.
///
/// flip: `Walk::unwritable` checking only the trees' roots → `inlined` is empty;
/// past that assertion, the use draws the main's dot at 112.0.
#[test]
fn a_property_drawn_as_a_picture_is_not_frozen_at_the_mains_value() {
    let mut d = Doc::new();
    let board = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    // The card's fourth layer: a group masked by a circle, holding a dot.
    let badge = |d: &mut Doc, [frame, ..]: [NodeId; 3]| {
        let g = d.add(
            Some(frame),
            NodeKind::Group,
            Affine::translate((120.0, 14.0)),
            "Badge",
        );
        let mask = d.add(
            Some(g),
            NodeKind::Ellipse {
                size: Size::new(32.0, 32.0),
            },
            Affine::IDENTITY,
            "Mask",
        );
        d.fill(mask, solid(0, 0, 0, 255));
        d.apply(vec![Operation::SetMask {
            id: mask,
            mask: true,
        }]);
        let dot = d.rect(g, 0.0, 0.0, 32.0, 32.0, 0.0);
        d.fill(dot, solid(40, 120, 220, 255));
    };
    let dot_of = |d: &Doc, frame: NodeId| {
        let g = d.doc.get(frame).unwrap().children()[2];
        d.doc.get(g).unwrap().children()[1]
    };
    // The main past the board's clipping edge: only the instance is compared.
    let main = d.card(board, (500.0, 10.0), None, badge);
    let dot = dot_of(&d, main[0]);
    d.properties(main[0], &[("Show dot", PropKind::Boolean, dot)]);
    let set = d.card(board, (10.0, 10.0), Some(main), badge);
    d.set_properties(set[0], vec![PropValue::Boolean(false)]);
    assert!(!d.doc.get(dot_of(&d, set[0])).unwrap().visible());
    let (doc, res) = d.done();
    let export = html_of_components(&doc, &res, &[board]);
    assert_eq!(
        export.inlined,
        vec!["instance “Card”: its main draws a property's layer as a picture"]
    );
    holds_components("picture-prop", &doc, &res);
}

/// A use under another mode than its main's, whose main draws a picture filled
/// from a colour variable and a stroke whose width variable is 0 in the main's
/// mode: the canvas draws both in the use's mode, so the instance is its layers.
///
/// flip: `Walk::why_inline` ignoring the use's mode → nothing is inlined; past
/// that assertion, the star is red and the stroke missing, at 127.4.
#[test]
fn a_use_in_another_mode_draws_that_modes_values() {
    use ondin_core::BrushSource;
    use ondin_core::variable::{Cell, Mode, VarKind, Variable};
    let mut d = Doc::new();
    let (light, dark) = (d.ids.mint_mode(), d.ids.mint_mode());
    for (i, (mode, name)) in [(light, "Light"), (dark, "Dark")].into_iter().enumerate() {
        d.apply(vec![Operation::AddMode {
            mode: Mode {
                id: mode,
                name: name.into(),
            },
            index: i,
            cells: vec![],
        }]);
    }
    let (ink, line) = (d.ids.mint_var(), d.ids.mint_var());
    for (id, name, kind, cells) in [
        (
            ink,
            "ink",
            VarKind::Color,
            [
                (light, Cell::Color(Color::from_rgba8(220, 40, 40, 255))),
                (dark, Cell::Color(Color::from_rgba8(40, 80, 220, 255))),
            ],
        ),
        (
            line,
            "line",
            VarKind::Number,
            [(light, Cell::Number(0.0)), (dark, Cell::Number(4.0))],
        ),
    ] {
        d.apply(vec![Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind,
                cells: cells.into_iter().collect(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]);
    }
    let board = d.frame(None, 0.0, 0.0, 200.0, 100.0, "Board");
    let painted = |d: &mut Doc, [frame, _, _]: [NodeId; 3]| {
        d.stroke(frame, 0.0, StrokeAlign::Inside, StrokeSides::All);
        let mut strokes = d.doc.get(frame).unwrap().paint().strokes.clone();
        strokes[0].value.width_var = Some(line);
        d.apply(vec![Operation::SetStrokes { id: frame, strokes }]);
        // A picture filled from `ink`.
        let star = d.add(
            Some(frame),
            NodeKind::Star {
                size: Size::new(24.0, 24.0),
                points: 5,
                inner_ratio: 0.5,
            },
            Affine::translate((120.0, 18.0)),
            "Star",
        );
        d.apply(vec![Operation::SetFills {
            id: star,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: ink,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        }]);
    };
    // The main past the board's clipping edge: only the instance is compared.
    let main = d.card(board, (500.0, 10.0), None, painted);
    let used = d.card(board, (10.0, 10.0), Some(main), painted);
    d.apply(vec![Operation::SetMode {
        id: used[0],
        mode: Some(dark),
    }]);
    let (doc, res) = d.done();
    assert_eq!(html_of_components(&doc, &res, &[board]).inlined.len(), 1);
    holds_components("use-mode", &doc, &res);
}
