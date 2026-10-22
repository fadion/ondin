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
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::{
    Color, ColorStop, Extend, Gradient, GradientKind, LinearGradientPosition,
    RadialGradientPosition, SweepGradientPosition,
};
use ondin_core::{
    Brush, CharAttr, Document, Effect, EffectKind, Fill, GradientBrush, IdSource, NodeId, NodeKind,
    Operation, Resolved, Shadow, Stroke, StrokeAlign, StrokeSides, TextAlign, TextSizing,
    TextStyle, Transaction, VerticalAlign, keyed_by_position,
};
use ondin_export::code::html::html_of;
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

/// `dir/index.html` screenshotted at `width`×`height` CSS px, one device pixel each.
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
        .arg(format!("--window-size={width},{height}"))
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
    decode(&bytes)
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

/// Text's limit, after one pass of `blurred`: Chrome and vello antialias glyph
/// edges differently even where every glyph is placed alike. The matching fixture
/// peaks at 7.7. flip: an automatic line height written as `normal` → 12.8, a
/// one-pixel drop of the centred line.
const MAX_TEXT_CELL_ERROR: f64 = 10.0;

/// [`holds`], comparing after `passes` of `blurred` with `limit` per cell.
fn holds_within(test: &str, doc: &Document, res: &Resolved, passes: usize, limit: f64) {
    let origins: Vec<NodeId> = doc.get(doc.root()).expect("root").children().to_vec();
    let export = html_of(doc, res, &origins);
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
    let (cells, mean) = if passes == 0 {
        compare(&cpu, &page)
    } else {
        compare(&blurred(&cpu, passes), &blurred(&page, passes))
    };
    let (worst, at) = cells.first().copied().unwrap_or_default();
    eprintln!("{test}: worst cell {worst:.1}/255 at {at:?}, mean {mean:.2}/255");
    assert!(
        worst <= limit && mean <= MAX_MEAN_ERROR,
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
    holds_within("text", &doc, &res, 1, MAX_TEXT_CELL_ERROR);
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
