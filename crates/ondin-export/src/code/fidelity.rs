//! The fidelity check: the HTML export opened in a headless browser and held to
//! the CPU raster of the same layers, cell by cell, so a layer the page draws
//! differently is named rather than left to drift. [`CodeExport::approximated`]
//! is what the exporter knows it changed; this is what a browser actually drew.
//!
//! The browser is Chrome, Chromium or Edge, found by [`browser`]; nothing is
//! bundled, and with none found the check cannot run. The page is given the
//! bundled Inter as a web font, which the export itself does not ship, so type is
//! compared as the canvas shapes it.
//!
//! Each 8×8 cell's error is the mean absolute channel difference over white, held
//! to [`MAX_CELL_ERROR`] and put down to the topmost layer under its worst pixel.
//! Text is judged by where its ink is ([`misplaced_ink`]) and what colour
//! ([`ink_colour_error`]) instead: a browser draws light type on a dark fill
//! heavier than vello does, a weight difference no cell limit tells from a line
//! set wrong, while a moved or rewrapped line moves its ink.

use super::CodeExport;
use ondin_core::kurbo::Rect;
use ondin_core::{Document, NodeId, NodeKind, Resolved};
use std::path::{Path, PathBuf};

/// The side of a compared cell, in pixels.
pub const CELL: u32 = 8;

/// The most a cell may differ, per channel out of 255. Matching pages peak near
/// 15 on an inline-SVG inner shadow; a misplaced or miscoloured layer is a run
/// of cells far past it.
pub const MAX_CELL_ERROR: f64 = 16.0;

/// The most the whole page may differ on average, per channel out of 255.
pub const MAX_MEAN_ERROR: f64 = 1.5;

/// The share of a text layer's ink, in either image, that may lie more than a
/// pixel from the other's.
pub const MAX_MISPLACED_INK: f64 = 0.1;

/// How far, in pixels, the page's type ink may lie from the canvas's and still
/// be read as type ([`misplaced_ink`]).
pub const INK_REACH: usize = 3;

/// The most a text layer's solid ink may differ in colour between the images,
/// per channel out of 255: a weight difference moves the coverage of an edge,
/// never the colour of a glyph's full pixels.
pub const MAX_INK_COLOUR_ERROR: f64 = 48.0;

/// Straight-alpha RGBA8 and its size.
pub struct Image {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// One layer the page draws differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Drifted {
    pub id: NodeId,
    /// The layer's kind and name, as the export's report words them.
    pub label: String,
    pub how: How,
}

/// How a layer was found to differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum How {
    /// Its worst cell's error, per channel out of 255, and that cell's top-left
    /// pixel in the compared image; for type, its solid ink's colour error and
    /// its box's top-left pixel.
    Cell { error: f64, at: (u32, u32) },
    /// The share of its text's ink that lies off the other image's.
    Ink { misplaced: f64 },
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fidelity {
    /// Every layer past its limit, in paint order.
    pub drifted: Vec<Drifted>,
    /// Cells past their limit that no layer's ink covers.
    pub unclaimed: usize,
    /// The page's mean error away from type, per channel out of 255: type's
    /// antialiasing is judged by where its ink is and what colour, not here.
    pub mean: f64,
}

impl Fidelity {
    /// Whether the page draws as the canvas does.
    pub fn holds(&self) -> bool {
        self.drifted.is_empty() && self.unclaimed == 0 && self.mean <= MAX_MEAN_ERROR
    }
}

/// The browser to screenshot with: `ONDIN_BROWSER`, else Chrome, Chromium or Edge
/// where they are usually installed, else one of their names on `PATH`.
pub fn browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ONDIN_BROWSER") {
        return Some(PathBuf::from(p));
    }
    let fixed = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ];
    if let Some(p) = fixed.iter().map(PathBuf::from).find(|p| p.is_file()) {
        return Some(p);
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
    ];
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// The `@font-face` rules for the bundled Inter, named as [`write_page`] writes
/// its files.
const INTER_FACES: &str = r#"
@font-face { font-family: "Inter"; src: url(inter.ttf); font-weight: 100 900; font-style: normal; }
@font-face { font-family: "Inter"; src: url(inter-italic.ttf); font-weight: 100 900; font-style: italic; }
"#;

/// Write `export`'s files into `dir`, with the bundled Inter as a web font.
pub fn write_page(export: &CodeExport, dir: &Path) -> Result<(), String> {
    let io = |what: &str, e: std::io::Error| format!("cannot write {what}: {e}");
    for f in &export.files {
        let path = dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(&f.path, e))?;
        }
        let mut bytes = f.bytes.clone();
        if f.path == "styles.css" {
            bytes.extend_from_slice(INTER_FACES.as_bytes());
        }
        std::fs::write(&path, &bytes).map_err(|e| io(&f.path, e))?;
    }
    let [upright, italic] = ondin_core::text::bundled_inter();
    for (name, bytes) in [("inter.ttf", upright), ("inter-italic.ttf", italic)] {
        std::fs::write(dir.join(name), bytes).map_err(|e| io(name, e))?;
    }
    Ok(())
}

/// `dir/index.html` screenshotted at `width`×`height` CSS px, one device pixel each.
///
/// Headless Chrome's viewport is narrower and shorter than `--window-size` (at least
/// 500 px wide, less a toolbar's height), and from Chrome 156 nothing outside the
/// viewport is painted, so the window is oversized and the shot cropped to the page.
pub fn screenshot(browser: &Path, dir: &Path, width: u32, height: u32) -> Result<Image, String> {
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
        .map_err(|e| format!("cannot run {}: {e}", browser.display()))?;
    let bytes = std::fs::read(&shot).map_err(|_| {
        format!(
            "no screenshot from the browser:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })?;
    crop(&decode(&bytes)?, width, height)
}

fn decode(bytes: &[u8]) -> Result<Image, String> {
    let bad = |e: &dyn std::fmt::Display| format!("the screenshot does not decode: {e}");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| bad(&e))?;
    let size = reader.output_buffer_size().ok_or_else(|| bad(&"no size"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| bad(&e))?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => return Err(bad(&format!("colour type {other:?}"))),
    };
    Ok(Image {
        rgba,
        width: info.width,
        height: info.height,
    })
}

/// The top-left `width`×`height` of `img`.
fn crop(img: &Image, width: u32, height: u32) -> Result<Image, String> {
    if img.width < width || img.height < height {
        return Err(format!(
            "the screenshot is {}×{}, smaller than the page's {width}×{height}",
            img.width, img.height
        ));
    }
    let row = |y: u32| {
        let at = (y * img.width * 4) as usize;
        &img.rgba[at..at + (width * 4) as usize]
    };
    Ok(Image {
        rgba: (0..height).flat_map(row).copied().collect(),
        width,
        height,
    })
}

/// One channel of a straight-alpha pixel composited over white.
fn over_white(c: u8, a: u8) -> f64 {
    let a = f64::from(a) / 255.0;
    f64::from(c) * a + 255.0 * (1.0 - a)
}

/// One cell's mean channel error and its top-left pixel.
pub type Cell = (f64, (u32, u32));

/// Every cell, row by row, and the whole image's mean channel error.
pub fn cell_errors(a: &Image, b: &Image) -> (Vec<Cell>, f64) {
    let (w, h) = (a.width.min(b.width), a.height.min(b.height));
    let mut total = 0.0;
    let mut cells = Vec::new();
    for cy in (0..h).step_by(CELL as usize) {
        for cx in (0..w).step_by(CELL as usize) {
            let (mut sum, mut n) = (0.0, 0.0);
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
    (cells, total / f64::from((w * h * 3).max(1)))
}

/// The mean channel error over the pixels `typed` leaves out (row-major over the
/// two images' shared size), 0 when it leaves none.
fn untyped_mean(a: &Image, b: &Image, typed: &[bool]) -> f64 {
    let w = a.width.min(b.width);
    let (mut sum, mut n) = (0.0, 0.0);
    for (i, _) in typed.iter().enumerate().filter(|(_, t)| !**t) {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let (ia, ib) = (
            ((y * a.width + x) * 4) as usize,
            ((y * b.width + x) * 4) as usize,
        );
        for c in 0..3 {
            sum += (over_white(a.rgba[ia + c], a.rgba[ia + 3])
                - over_white(b.rgba[ib + c], b.rgba[ib + 3]))
            .abs();
        }
        n += 3.0;
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// `img` composited over white and box-blurred `passes` times with a 3×3 kernel,
/// back as opaque RGBA.
pub fn blurred(img: &Image, passes: usize) -> Image {
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
                let (mut acc, mut n) = ([0.0; 3], 0.0);
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
            .flat_map(|p| p.map(|v| v.round() as u8).into_iter().chain([255]))
            .collect(),
        width: img.width,
        height: img.height,
    }
}

/// Every shown layer under `origins` that has ink, in paint order — a parent
/// before its children, siblings bottom first — with its world ink box.
fn painted(doc: &Document, res: &Resolved, origins: &[NodeId]) -> Vec<(NodeId, Rect)> {
    fn walk(doc: &Document, res: &Resolved, id: NodeId, out: &mut Vec<(NodeId, Rect)>) {
        if !doc.shown(id) {
            return;
        }
        if let Some(b) = res.ink_bounds(id) {
            out.push((id, b));
        }
        for &c in doc.get(id).map(|n| n.children()).unwrap_or_default() {
            walk(doc, res, c, out);
        }
    }
    let mut out = Vec::new();
    for &o in origins {
        walk(doc, res, o, &mut out);
    }
    out
}

/// Export `origins` as HTML, open the page in `browser` from `dir` (an empty
/// folder the caller owns), and hold it to the CPU raster of the same layers.
///
/// `dir` keeps the page, `cpu.png` and `shot.png` for a reader to compare by eye.
pub fn check(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    browser: &Path,
    dir: &Path,
) -> Result<Fidelity, String> {
    let export = super::html::html_of(doc, res, origins);
    check_export(doc, res, origins, &export, browser, dir)
}

/// [`check`] of a page already exported from `origins`.
pub fn check_export(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    export: &CodeExport,
    browser: &Path,
    dir: &Path,
) -> Result<Fidelity, String> {
    write_page(export, dir)?;
    let view = crate::extent(res, origins).ok_or("nothing to compare: the page has no ink")?;
    let cpu = crate::png::raster_of(doc, res, origins, &Default::default())
        .ok_or("nothing to compare: the page has no ink")?;
    let cpu = Image {
        rgba: cpu.rgba,
        width: cpu.width,
        height: cpu.height,
    };
    std::fs::write(
        dir.join("cpu.png"),
        crate::png::encode_rgba8(&cpu.rgba, cpu.width, cpu.height),
    )
    .map_err(|e| format!("cannot write cpu.png: {e}"))?;
    let page = screenshot(browser, dir, cpu.width, cpu.height)?;
    Ok(compare_page(doc, res, origins, view, &cpu, &page))
}

/// The pixel of the cell at `at` where `a` and `b` differ most: what a cell is
/// put down to, since a cell across two layers' edge may hold only one's drift.
fn worst_pixel(a: &Image, b: &Image, at: (u32, u32)) -> (u32, u32) {
    let (w, h) = (a.width.min(b.width), a.height.min(b.height));
    let mut best = (-1.0, at);
    for y in at.1..(at.1 + CELL).min(h) {
        for x in at.0..(at.0 + CELL).min(w) {
            let (ia, ib) = (
                ((y * a.width + x) * 4) as usize,
                ((y * b.width + x) * 4) as usize,
            );
            let e: f64 = (0..3)
                .map(|c| {
                    (over_white(a.rgba[ia + c], a.rgba[ia + 3])
                        - over_white(b.rgba[ib + c], b.rgba[ib + 3]))
                    .abs()
                })
                .sum();
            if e > best.0 {
                best = (e, (x, y));
            }
        }
    }
    best.1
}

/// The share of the ink in `region` (pixels, clamped to the images) that lies
/// more than a pixel from the other image's ink, the worse of the two ways.
///
/// Ink is what differs from the region's commonest colour by more than a quarter
/// of the range, each image read against its own: the type's background, since
/// type covers less of its box than what is behind it does.
pub fn misplaced_ink(a: &Image, b: &Image, region: (u32, u32, u32, u32)) -> f64 {
    let Some(r) = Region::of(a, b, region) else {
        return 0.0;
    };
    let (w, h) = (r.w, r.h);
    let (ia, ib) = r.inks(a, b);
    let (ma, mb) = (ia.mask, ib.mask);
    let near = |m: &[bool], x: usize, y: usize| near(m, w, h, x, y);
    let off = |from: &[bool], to: &[bool]| {
        let (mut ink, mut stray) = (0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                if from[y * w + x] {
                    ink += 1;
                    if !near(to, x, y) {
                        stray += 1;
                    }
                }
            }
        }
        if ink == 0 {
            0.0
        } else {
            stray as f64 / ink as f64
        }
    };
    off(&ma, &mb).max(off(&mb, &ma))
}

/// The largest channel difference between the colours of the solid ink in
/// `region` — the pixels at least three quarters as far from the type's ground
/// as its farthest — of the two images; 0 when either has no ink there.
pub fn ink_colour_error(a: &Image, b: &Image, region: (u32, u32, u32, u32)) -> f64 {
    let Some(r) = Region::of(a, b, region) else {
        return 0.0;
    };
    let (ia, ib) = r.inks(a, b);
    let solid = |img: &Image, ink: &Ink| -> Option<[f64; 3]> {
        let far = ink
            .distance
            .iter()
            .zip(&ink.mask)
            .filter(|(_, m)| **m)
            .map(|(d, _)| *d)
            .fold(0.0, f64::max);
        let (mut sum, mut n) = ([0.0; 3], 0.0);
        for (i, d) in ink.distance.iter().enumerate() {
            if ink.mask[i] && *d >= far * 0.75 {
                let p = r.px(img, i % r.w, i / r.w);
                for c in 0..3 {
                    sum[c] += p[c];
                }
                n += 1.0;
            }
        }
        (n > 0.0).then(|| sum.map(|v| v / n))
    };
    match (solid(a, &ia), solid(b, &ib)) {
        (Some(ca), Some(cb)) => (0..3).map(|c| (ca[c] - cb[c]).abs()).fold(0.0, f64::max),
        _ => 0.0,
    }
}

/// Whether `m` (row-major, `w`×`h`) holds a pixel within one of `(x, y)`.
fn near(m: &[bool], w: usize, h: usize, x: usize, y: usize) -> bool {
    (y.saturating_sub(1)..(y + 2).min(h))
        .any(|yy| (x.saturating_sub(1)..(x + 2).min(w)).any(|xx| m[yy * w + xx]))
}

/// A pixel region of two images, clamped to both.
struct Region {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

/// One image's type ink in a [`Region`]: which pixels are ink and how far each
/// is from the region's commonest colour, the type's ground.
struct Ink {
    mask: Vec<bool>,
    distance: Vec<f64>,
}

impl Region {
    fn of(a: &Image, b: &Image, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> Option<Self> {
        let (x1, y1) = (x1.min(a.width).min(b.width), y1.min(a.height).min(b.height));
        (x1 > x0 && y1 > y0).then(|| Self {
            x0: x0 as usize,
            y0: y0 as usize,
            w: (x1 - x0) as usize,
            h: (y1 - y0) as usize,
        })
    }

    /// The pixel at `(x, y)` in the region, over white.
    fn px(&self, img: &Image, x: usize, y: usize) -> [f64; 3] {
        let at = (((self.y0 + y) * img.width as usize) + self.x0 + x) * 4;
        [0, 1, 2].map(|c| over_white(img.rgba[at + c], img.rgba[at + 3]))
    }

    /// The type ink of the canvas's image `a` and the page's `b`. The page's
    /// counts only within [`INK_REACH`] of the canvas's: a line set off by a
    /// pixel or two is misplaced type, a layer drifting under the box's empty
    /// part is that layer's.
    fn inks(&self, a: &Image, b: &Image) -> (Ink, Ink) {
        let (ia, mut ib) = (self.ink(a), self.ink(b));
        let r = INK_REACH;
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                if ib.mask[i] {
                    ib.mask[i] = (y.saturating_sub(r)..(y + r + 1).min(self.h)).any(|yy| {
                        (x.saturating_sub(r)..(x + r + 1).min(self.w))
                            .any(|xx| ia.mask[yy * self.w + xx])
                    });
                }
            }
        }
        (ia, ib)
    }

    /// Ink is what differs from the region's commonest colour by more than a
    /// quarter of the range, read against the image's own.
    fn ink(&self, img: &Image) -> Ink {
        let mut counts: std::collections::HashMap<[u8; 3], usize> = Default::default();
        for y in 0..self.h {
            for x in 0..self.w {
                *counts
                    .entry(self.px(img, x, y).map(|v| v as u8))
                    .or_default() += 1;
            }
        }
        let ground = counts
            .into_iter()
            .max_by_key(|&(c, n)| (n, c))
            .map_or([255.0; 3], |(c, _)| c.map(f64::from));
        let distance: Vec<f64> = (0..self.h)
            .flat_map(|y| (0..self.w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = self.px(img, x, y);
                (0..3).map(|c| (p[c] - ground[c]).abs()).fold(0.0, f64::max)
            })
            .collect();
        Ink {
            mask: distance.iter().map(|d| *d > 64.0).collect(),
            distance,
        }
    }
}

/// [`check`]'s comparison, once both images are in hand. `view` is the world box
/// `cpu` was framed on. A cell past its limit is put down to the topmost layer
/// whose ink box holds the cell's worst pixel; one put down to text is left to
/// that layer's [`misplaced_ink`].
pub fn compare_page(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    view: Rect,
    cpu: &Image,
    page: &Image,
) -> Fidelity {
    let (cells, _) = cell_errors(cpu, page);
    let layers = painted(doc, res, origins);
    let (sx, sy) = (
        view.width() / f64::from(cpu.width.max(1)),
        view.height() / f64::from(cpu.height.max(1)),
    );
    let is_text = |id: NodeId| matches!(doc.get(id).map(|n| n.kind()), Some(NodeKind::Text { .. }));
    let region_of = |b: Rect| {
        let px = |v: f64, o: f64, s: f64| ((v - o) / s).max(0.0);
        (
            px(b.x0, view.x0, sx).floor() as u32,
            px(b.y0, view.y0, sy).floor() as u32,
            px(b.x1, view.x0, sx).ceil() as u32,
            px(b.y1, view.y0, sy).ceil() as u32,
        )
    };
    // The pixels type owns: within a pixel of a text layer's ink in either image.
    let (w, h) = (
        cpu.width.min(page.width) as usize,
        cpu.height.min(page.height) as usize,
    );
    let mut typed = vec![false; w * h];
    for &(_, b) in layers.iter().filter(|(id, _)| is_text(*id)) {
        let Some(r) = Region::of(cpu, page, region_of(b)) else {
            continue;
        };
        let (ia, ib) = r.inks(cpu, page);
        let (ma, mb) = (ia.mask, ib.mask);
        for y in 0..r.h {
            for x in 0..r.w {
                if near(&ma, r.w, r.h, x, y) || near(&mb, r.w, r.h, x, y) {
                    typed[(r.y0 + y) * w + r.x0 + x] = true;
                }
            }
        }
    }
    let mean = untyped_mean(cpu, page, &typed);
    let label = |id: NodeId| {
        doc.get(id)
            .map(|n| format!("{} {}", super::walk::kind_word(n), super::walk::label(n)))
            .unwrap_or_default()
    };
    let mut drifted: Vec<Drifted> = Vec::new();
    let mut unclaimed = 0;
    for &(error, at) in &cells {
        if error <= MAX_CELL_ERROR {
            continue;
        }
        let (px, py) = worst_pixel(cpu, page, at);
        let point = ondin_core::kurbo::Point::new(
            view.x0 + (f64::from(px) + 0.5) * sx,
            view.y0 + (f64::from(py) + 0.5) * sy,
        );
        // Type's box claims a pixel only where its ink is: a layer drifting under
        // a text box's empty part is the layer's.
        let on_type = typed
            .get(py as usize * w + px as usize)
            .copied()
            .unwrap_or(false);
        let Some(&(id, _)) = layers
            .iter()
            .rev()
            .find(|(id, b)| b.contains(point) && (on_type || !is_text(*id)))
        else {
            unclaimed += 1;
            continue;
        };
        if is_text(id) {
            continue;
        }
        match drifted.iter_mut().find(|d| d.id == id) {
            Some(Drifted {
                how:
                    How::Cell {
                        error: worst,
                        at: worst_at,
                    },
                ..
            }) if error > *worst => (*worst, *worst_at) = (error, at),
            Some(_) => {}
            None => drifted.push(Drifted {
                id,
                label: label(id),
                how: How::Cell { error, at },
            }),
        }
    }
    for &(id, b) in layers.iter().filter(|(id, _)| is_text(*id)) {
        let region = region_of(b);
        let misplaced = misplaced_ink(cpu, page, region);
        let colour = ink_colour_error(cpu, page, region);
        let how = if misplaced > MAX_MISPLACED_INK {
            How::Ink { misplaced }
        } else if colour > MAX_INK_COLOUR_ERROR {
            How::Cell {
                error: colour,
                at: (region.0, region.1),
            }
        } else {
            continue;
        };
        drifted.push(Drifted {
            id,
            label: label(id),
            how,
        });
    }
    let order: std::collections::HashMap<NodeId, usize> = layers
        .iter()
        .enumerate()
        .map(|(k, (id, _))| (*id, k))
        .collect();
    drifted.sort_by_key(|d| order.get(&d.id).copied().unwrap_or(usize::MAX));
    Fidelity {
        drifted,
        unclaimed,
        mean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Brush, Fill, IdSource, Operation, Transaction, keyed_by_position};

    /// Two filled 40×40 rects, "Left" at x 0 and "Right" over it at x 30.
    fn two_rects() -> (Document, Resolved, [NodeId; 2]) {
        let mut ids = IdSource::new(0xF1D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let made = [ids.mint(), ids.mint()];
        for (i, (id, name)) in made.iter().zip(["Left", "Right"]).enumerate() {
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: *id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(40.0, 40.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::translate((30.0 * i as f64, 0.0))),
                    name: Some(name.into()),
                },
                Operation::SetFills {
                    id: *id,
                    fills: keyed_by_position([Fill {
                        brush: Brush::Solid(Color::from_rgba8(200, 40, 40, 255)).into(),
                        visible: true,
                    }]),
                },
            ]))
            .unwrap();
        }
        let res = Resolved::rebuild(&doc);
        (doc, res, made)
    }

    /// A page drawn like the raster holds; one whose top square came out in
    /// another colour names that layer, and only it, the overlap included.
    ///
    /// flip: `compare_page` taking the first layer whose box covers a cell
    /// rather than the topmost → fails at the ids, "Left" named for the overlap.
    #[test]
    fn a_layer_drawn_differently_is_named() {
        let (doc, res, [_, right]) = two_rects();
        let origins = doc.get(doc.first_page()).unwrap().children().to_vec();
        let view = crate::extent(&res, &origins).unwrap();
        let raster = crate::png::raster_of(&doc, &res, &origins, &Default::default()).unwrap();
        let cpu = Image {
            rgba: raster.rgba,
            width: raster.width,
            height: raster.height,
        };
        let mut page = Image {
            rgba: cpu.rgba.clone(),
            width: cpu.width,
            height: cpu.height,
        };
        let held = compare_page(&doc, &res, &origins, view, &cpu, &page);
        assert!(held.holds(), "{held:?}");

        // The top square's pixels, blue instead of red.
        for y in 0..page.height {
            for x in 30..page.width {
                let at = ((y * page.width + x) * 4) as usize;
                if page.rgba[at + 3] > 0 {
                    page.rgba[at..at + 3].copy_from_slice(&[40, 40, 200]);
                }
            }
        }
        let found = compare_page(&doc, &res, &origins, view, &cpu, &page);
        assert_eq!(
            found.drifted.iter().map(|d| d.id).collect::<Vec<_>>(),
            [right],
            "{found:?}"
        );
        assert_eq!(found.drifted[0].label, "layer “Right”");
        assert!(!found.holds());
    }

    const GROUND: [u8; 3] = [240, 240, 236];

    /// A 320×230 frame filled `GROUND` holding a 300×200 red card and, over it, a
    /// fixed 300×200 text box of `content`, 18px, near-black: (doc, res, origins,
    /// [card, text]).
    fn type_page(content: &str) -> (Document, Resolved, Vec<NodeId>, [NodeId; 2]) {
        let mut ids = IdSource::new(0xF1E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, card, text) = (ids.mint(), ids.mint(), ids.mint());
        let fill = |id, [r, g, b]: [u8; 3]| Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(r, g, b, 255)).into(),
                visible: true,
            }]),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(320.0, 230.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Board".into()),
            },
            fill(frame, GROUND),
            Operation::CreateNode {
                id: card,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(300.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: Some("Card".into()),
            },
            fill(card, [250, 250, 250]),
            Operation::CreateNode {
                id: text,
                parent: frame,
                index: 1,
                kind: NodeKind::Text {
                    content: content.into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_size: 18.0,
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: ondin_core::TextSizing::Fixed(Size::new(300.0, 200.0)),
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: Some("Label".into()),
            },
            fill(text, [20, 20, 20]),
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, vec![frame], [card, text])
    }

    /// The CPU raster of `origins`, and its world box.
    fn raster(doc: &Document, res: &Resolved, origins: &[NodeId]) -> (Image, Rect) {
        let r = crate::png::raster_of(doc, res, origins, &Default::default()).unwrap();
        let view = crate::extent(res, origins).unwrap();
        (
            Image {
                rgba: r.rgba,
                width: r.width,
                height: r.height,
            },
            view,
        )
    }

    /// Each pixel on the card with its largest channel distance from the card's
    /// white, for `edit` to change.
    fn on_card(img: &mut Image, mut edit: impl FnMut(&mut [u8], u8)) {
        for y in 12..208 {
            for x in 12..308 {
                let at = ((y * img.width + x) * 4) as usize;
                let p = &mut img.rgba[at..at + 4];
                let d = (0..3).map(|c| p[c].abs_diff(250)).max().unwrap();
                edit(p, d);
            }
        }
    }

    /// Type drawn lighter at its edges — a browser's weight — holds, though the
    /// page's raw mean is past the limit; type in the wrong colour does not.
    ///
    /// flip: `compare_page` taking the mean over every pixel → fails at "a lighter
    /// edge"; dropping `ink_colour_error` → fails at "the wrong colour".
    #[test]
    fn type_is_held_to_its_ink_and_its_colour_not_its_edges() {
        let lines = "Total due on the first of the month\n".repeat(9);
        let (doc, res, origins, [_, text]) = type_page(&lines);
        let (cpu, view) = raster(&doc, &res, &origins);
        let mut lighter = Image {
            rgba: cpu.rgba.clone(),
            width: cpu.width,
            height: cpu.height,
        };
        on_card(&mut lighter, |p, d| {
            if d > 0 && d < 150 {
                p[..3].copy_from_slice(&[250, 250, 250]);
            }
        });
        assert!(
            cell_errors(&cpu, &lighter).1 > MAX_MEAN_ERROR,
            "fixture: the raw mean is past the limit"
        );
        let held = compare_page(&doc, &res, &origins, view, &cpu, &lighter);
        assert!(held.holds(), "a lighter edge: {held:?}");

        let mut red = Image {
            rgba: cpu.rgba.clone(),
            width: cpu.width,
            height: cpu.height,
        };
        on_card(&mut red, |p, d| {
            if d > 0 {
                // The same coverage of red over the card.
                let k = f64::from(d) / 230.0;
                let mix = |ink: f64| (250.0 + (ink - 250.0) * k).round() as u8;
                p[..3].copy_from_slice(&[mix(255.0), mix(0.0), mix(0.0)]);
            }
        });
        let found = compare_page(&doc, &res, &origins, view, &cpu, &red);
        assert_eq!(
            found.drifted.iter().map(|d| d.id).collect::<Vec<_>>(),
            [text],
            "the wrong colour: {found:?}"
        );
    }

    /// A layer drifting under a text box's empty part is that layer's, not the
    /// text's.
    ///
    /// flip: `compare_page` putting a cell down to a text box whatever lies under
    /// its worst pixel → fails at the ids, the label named.
    #[test]
    fn a_layer_under_empty_type_is_named_and_not_the_type() {
        let (doc, res, origins, [card, _]) = type_page("Hi");
        let (cpu, view) = raster(&doc, &res, &origins);
        let mut page = Image {
            rgba: cpu.rgba.clone(),
            width: cpu.width,
            height: cpu.height,
        };
        for y in 150..198 {
            for x in 250..298 {
                let at = ((y * page.width + x) * 4) as usize;
                page.rgba[at..at + 3].copy_from_slice(&[40, 40, 200]);
            }
        }
        let found = compare_page(&doc, &res, &origins, view, &cpu, &page);
        assert_eq!(
            found.drifted.iter().map(|d| d.id).collect::<Vec<_>>(),
            [card],
            "{found:?}"
        );
    }
}
