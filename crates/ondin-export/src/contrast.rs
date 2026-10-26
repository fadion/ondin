//! Text contrast against what is actually drawn behind it, for `ondin check
//! contrast` (and, later, the canvas overlay).
//!
//! **The backdrop is rendered, not guessed**: each text layer's box is drawn
//! twice through the CPU renderer, once as it is and once with the layer hidden
//! (`RenderOverrides`), both over the page's ground. The pixels the text changes
//! most are under its glyphs; at each, the text's colour composited over the
//! backdrop pixel with the layer's opacity is measured against that backdrop, and
//! the worst sample is the layer's ratio. So a gradient, a picture or another
//! layer behind the type is measured where the glyphs sit.
//!
//! The thresholds are WCAG 2's: 4.5 : 1, or 3 : 1 for large text (24 px, or
//! 18.66 px from weight 700), and 7 : 1 and 4.5 : 1 at AAA. The size is the
//! smallest run's, at the layer's world scale. Every mode of the document is
//! checked, top-level frames that set none drawn in it, and the worst kept.

use ondin_core::kurbo::Rect;
use ondin_core::peniko::Color;
use ondin_core::{Brush, Document, NodeId, NodeKind, Operation, Resolved, Transaction};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};

/// Which WCAG level a check holds text to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Aa,
    Aaa,
}

/// One text layer's measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: NodeId,
    /// The layer's name, under the names of the layers above it.
    pub trail: Vec<String>,
    /// The worst ratio found, `x` in `x : 1`.
    pub ratio: f64,
    /// The ratio the level asks of this text.
    pub needs: f64,
    /// The mode the worst ratio was measured in, `None` for a document with one
    /// or no mode.
    pub mode: Option<String>,
}

impl Finding {
    pub fn passes(&self) -> bool {
        self.ratio + 1e-9 >= self.needs
    }
}

/// A text layer the check could not measure, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub id: NodeId,
    pub trail: Vec<String>,
    pub why: &'static str,
}

/// What [`check`] found on a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub skipped: Vec<Skipped>,
}

/// How many device pixels per world unit the boxes are drawn at: two, so a
/// small glyph has a core of fully covered pixels to sample.
const SCALE: f64 = 2.0;
/// The largest box drawn, in pixels; a larger one is drawn at a lower scale.
const MAX_PIXELS: f64 = 4_000_000.0;

/// Every shown text layer on `page`, measured in every mode.
pub fn check(doc: &Document, page: NodeId, level: Level) -> Report {
    let modes: Vec<(Option<String>, Document)> = match doc.variables().modes() {
        [] | [_] => vec![(None, doc.clone())],
        modes => modes
            .iter()
            .map(|m| (Some(m.name.clone()), in_mode(doc, page, m.id)))
            .collect(),
    };
    let mut report = Report::default();
    let mut findings: Vec<Finding> = Vec::new();
    for (mode, d) in &modes {
        let res = Resolved::rebuild(d);
        let mut images = ImageStore::new();
        images.prepare(d);
        let here = check_one(d, &res, &images, page, level, mode.clone());
        for f in here.findings {
            match findings.iter_mut().find(|x| x.id == f.id) {
                Some(x) if f.ratio < x.ratio => *x = f,
                Some(_) => {}
                None => findings.push(f),
            }
        }
        for s in here.skipped {
            if !report.skipped.iter().any(|x| x.id == s.id) {
                report.skipped.push(s);
            }
        }
    }
    report.findings = findings;
    report
}

/// `doc` with every top-level frame of `page` that sets no mode set to `mode`.
fn in_mode(doc: &Document, page: NodeId, mode: ondin_core::ModeId) -> Document {
    let mut d = doc.clone();
    let ops: Vec<Operation> = doc
        .get(page)
        .map(|p| p.children().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| {
            doc.get(*id).is_some_and(|n| {
                matches!(n.kind(), NodeKind::Artboard { .. }) && n.mode().is_none()
            })
        })
        .map(|id| Operation::SetMode {
            id,
            mode: Some(mode),
        })
        .collect();
    // A refusal leaves the document as drawn, which is still a mode to check.
    let _ = d.apply(&Transaction(ops));
    d
}

fn check_one(
    doc: &Document,
    res: &Resolved,
    images: &ImageStore,
    page: NodeId,
    level: Level,
    mode: Option<String>,
) -> Report {
    let mut report = Report::default();
    let ground = doc.ground(page);
    for id in shown_text(doc, page) {
        let trail = trail(doc, id);
        let Some(node) = doc.get(id) else { continue };
        let Some(text) = ondin_core::TextRef::of(res.used_kind_of(node)) else {
            continue;
        };
        let runs = text.spans.runs(text.style, text.content.len());
        let fill = doc
            .resolved_paint(id, node.paint())
            .fills
            .iter()
            .rev()
            .find(|f| f.visible)
            .map(|f| f.brush.drawn().clone());
        let mut colours = Vec::new();
        for (_, style) in &runs {
            match (style.color, &fill) {
                (Some(c), _) => colours.push(c),
                (None, Some(Brush::Solid(c))) => colours.push(*c),
                (None, Some(_)) => {
                    report.skipped.push(Skipped {
                        id,
                        trail: trail.clone(),
                        why: "its fill is not one solid colour",
                    });
                    colours.clear();
                    break;
                }
                (None, None) => {}
            }
        }
        if colours.is_empty() {
            continue;
        }
        let opacity = chain_opacity(doc, id);
        let Some(bounds) = res.world_bounds(id).filter(|b| b.area() > 0.0) else {
            continue;
        };
        let Some(ratio) = worst_ratio(
            doc, res, images, page, id, bounds, ground, &colours, opacity,
        ) else {
            continue;
        };
        let scale = res.world_transform(id).map_or(1.0, |t| {
            let [a, b, c, d, _, _] = t.as_coeffs();
            (a * d - b * c).abs().sqrt()
        });
        let large = runs.iter().all(|(_, s)| {
            let px = s.font_size * scale;
            px >= 24.0 || (px >= 18.66 && s.weight >= 700)
        });
        let needs = match (level, large) {
            (Level::Aa, true) => 3.0,
            (Level::Aa, false) => 4.5,
            (Level::Aaa, true) => 4.5,
            (Level::Aaa, false) => 7.0,
        };
        report.findings.push(Finding {
            id,
            trail,
            ratio,
            needs,
            mode: mode.clone(),
        });
    }
    report
}

/// The shown text layers under `page`, in document order.
fn shown_text(doc: &Document, page: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !doc.shown(id) {
            continue;
        }
        if matches!(n.kind(), NodeKind::Text { .. }) {
            out.push(id);
        }
        stack.extend(n.children().iter().rev().copied());
    }
    out
}

/// The names from the page's top level down to `id`.
fn trail(doc: &Document, id: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|i| doc.get(i)) {
        if doc.is_page(n.id()) {
            break;
        }
        out.push(n.name().to_string());
        at = n.parent();
    }
    out.reverse();
    out
}

/// `id`'s opacity times every ancestor's, as the layers composite.
fn chain_opacity(doc: &Document, id: NodeId) -> f64 {
    let mut out = 1.0f64;
    let mut at = Some(id);
    while let Some(i) = at {
        out *= f64::from(doc.drawn_opacity(i));
        at = doc.get(i).and_then(|n| n.parent());
    }
    out
}

/// The lowest ratio between a text colour composited over the backdrop and the
/// backdrop, over the pixels the text covers most. `None` when the text draws
/// nothing in its box.
#[allow(clippy::too_many_arguments)]
fn worst_ratio(
    doc: &Document,
    res: &Resolved,
    images: &ImageStore,
    page: NodeId,
    id: NodeId,
    bounds: Rect,
    ground: Color,
    colours: &[Color],
    opacity: f64,
) -> Option<f64> {
    let view = bounds.inflate(1.0, 1.0);
    let scale = SCALE.min((MAX_PIXELS / view.area()).sqrt()).max(0.05);
    let vp = Viewport {
        view,
        pixel_size: (
            (view.width() * scale).ceil().max(1.0) as u32,
            (view.height() * scale).ceil().max(1.0) as u32,
        ),
    };
    let hidden = RenderOverrides::from_transaction(
        doc,
        res,
        &Transaction(vec![Operation::SetVisible { id, visible: false }]),
    )?;
    let mut cpu = VelloCpuRenderer::new();
    let (with, ..) =
        cpu.render_subtrees_with(doc, res, &vp, images, &[page], &RenderOverrides::default());
    let (without, ..) = cpu.render_subtrees_with(doc, res, &vp, images, &[page], &hidden);
    let over = |px: &[u8]| -> [f64; 3] {
        let a = f64::from(px[3]) / 255.0;
        let g = ground.components;
        std::array::from_fn(|k| f64::from(px[k]) / 255.0 * a + f64::from(g[k]) * (1.0 - a))
    };
    let (with, without) = (with.as_chunks::<4>().0, without.as_chunks::<4>().0);
    let diffs: Vec<(f64, usize)> = with
        .iter()
        .zip(without)
        .enumerate()
        .map(|(i, (p, q))| {
            let (p, q) = (over(p), over(q));
            let d = (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f64::max);
            (d, i)
        })
        .collect();
    let most = diffs.iter().map(|(d, _)| *d).fold(0.0, f64::max);
    if most < 1e-3 {
        return None;
    }
    // The glyphs' cores: pixels the text changes at least half as much as the
    // most it changes any, so antialiased edges do not read as faint type.
    let mut worst = f64::INFINITY;
    for (d, i) in diffs {
        if d < most * 0.5 {
            continue;
        }
        let bg = over(&without[i]);
        for c in colours {
            let [r, g, b, a] = c.components;
            let alpha = f64::from(a) * opacity;
            let fg = [r, g, b].map(f64::from);
            let mixed: [f64; 3] = std::array::from_fn(|k| fg[k] * alpha + bg[k] * (1.0 - alpha));
            worst = worst.min(ratio(mixed, bg));
        }
    }
    worst.is_finite().then_some(worst)
}

/// WCAG 2's contrast ratio between two straight sRGB colours.
pub fn ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// WCAG 2's relative luminance of a straight sRGB colour, channels 0..=1.
pub fn luminance(c: [f64; 3]) -> f64 {
    let lin = |x: f64| {
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}
