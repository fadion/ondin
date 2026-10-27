//! Text contrast against what is actually drawn behind it, for `ondin check
//! contrast` and the canvas's contrast check mode.
//!
//! **The backdrop is rendered, not guessed**: each text layer's box is drawn
//! twice through the CPU renderer, once as it is and once with the layer hidden
//! (`RenderOverrides`), both over the page's ground. The pixels the text changes
//! most are under its glyphs; at each, the text's colour composited over the
//! backdrop pixel with the layer's opacity is measured against that backdrop, and
//! the worst sample is the layer's score. So a gradient, a picture or another
//! layer behind the type is measured where the glyphs sit.
//!
//! The thresholds are WCAG 2's: 4.5 : 1, or 3 : 1 for large text (24 px, or
//! 18.66 px from weight 700), and 7 : 1 and 4.5 : 1 at AAA. [`Level::Apca`]
//! scores APCA's Lc instead, held to its Bronze levels by size and weight. The
//! size is each run's, at the layer's world scale, and the strictest run's
//! threshold is the layer's. Every mode of the document is checked, top-level
//! frames that set none drawn in it, and the worst kept.
//!
//! With [`Options::non_text`], small shapes (icons, borders, dividers: under
//! [`NON_TEXT_MAX`] across) are measured the same way against 3 : 1.

use ondin_core::kurbo::Rect;
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ModeId, NodeId, NodeKind, Operation, Resolved, Transaction, Value,
};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};

/// Which standard a check holds text to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Aa,
    Aaa,
    /// APCA's Lc, the absolute value scored, at the Bronze simple-mode levels.
    Apca,
}

impl Level {
    /// The score of `fg`, already composited over `bg`, against `bg`: WCAG 2's
    /// ratio, or APCA's |Lc|.
    pub fn score(self, fg: [f64; 3], bg: [f64; 3]) -> f64 {
        match self {
            Level::Aa | Level::Aaa => ratio(fg, bg),
            Level::Apca => apca(fg, bg).abs(),
        }
    }

    /// What one run of `px` pixels at `weight` needs.
    pub fn needs(self, px: f64, weight: u16) -> f64 {
        let large = px >= 24.0 || (px >= 18.66 && weight >= 700);
        match self {
            Level::Aa if large => 3.0,
            Level::Aa => 4.5,
            Level::Aaa if large => 4.5,
            Level::Aaa => 7.0,
            Level::Apca if px >= 36.0 || (px >= 24.0 && weight >= 700) => 45.0,
            Level::Apca if px >= 24.0 || (px >= 16.0 && weight >= 700) => 60.0,
            Level::Apca => 75.0,
        }
    }

    /// What body text needs: the threshold a run is held to when its size earns
    /// it nothing.
    pub fn body(self) -> f64 {
        self.needs(0.0, 400)
    }

    /// What a non-text mark needs: WCAG 2.1's 3 : 1 (1.4.11), or Lc 30, APCA's
    /// floor for fine non-text detail.
    pub fn non_text(self) -> f64 {
        match self {
            Level::Aa | Level::Aaa => 3.0,
            Level::Apca => 30.0,
        }
    }

    /// The level's name as the bar and the CLI print it.
    pub fn label(self) -> &'static str {
        match self {
            Level::Aa => "AA",
            Level::Aaa => "AAA",
            Level::Apca => "APCA",
        }
    }

    /// A score as the level writes it: `4.6 : 1`, or `Lc 48`.
    pub fn format(self, score: f64) -> String {
        match self {
            Level::Aa | Level::Aaa => format!("{} : 1", one_place(score)),
            Level::Apca => format!("Lc {}", score.floor()),
        }
    }

    /// A threshold as the level writes it: `4.5`, or `60`.
    pub fn format_needs(self, needs: f64) -> String {
        match self {
            Level::Aa | Level::Aaa => one_place(needs),
            Level::Apca => format!("{needs}"),
        }
    }

    /// How far past its threshold [`nearest`] aims, so a fix does not pass by a
    /// rounding error.
    fn margin(self) -> f64 {
        match self {
            Level::Aa | Level::Aaa => 0.08,
            Level::Apca => 1.5,
        }
    }
}

/// `x` with at most one decimal, rounded down so a failing 4.49 never reads as
/// a passing 4.5; a whole number drops its `.0`.
fn one_place(x: f64) -> String {
    let t = (x * 10.0 + 1e-6).floor() / 10.0;
    if t.fract() == 0.0 {
        format!("{t:.0}")
    } else {
        format!("{t:.1}")
    }
}

/// What a check measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub level: Level,
    /// Also measure shapes under [`NON_TEXT_MAX`] across, at [`Level::non_text`].
    pub non_text: bool,
}

/// The largest a shape's shorter side may be, in world units, for the non-text
/// check to count it as a mark rather than a surface.
pub const NON_TEXT_MAX: f64 = 24.0;

/// The worst sample of one layer in one mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The mode the layer was drawn in, `None` in a document with no modes.
    pub mode: Option<ModeId>,
    /// That mode's name, `None` for a document with one or no mode.
    pub name: Option<String>,
    pub score: f64,
    /// The layer's colour at the worst sample, as set, before opacity.
    pub ink: Color,
    /// The layer's and its ancestors' opacity, which the ink is composited at.
    pub opacity: f64,
    /// The opaque colour drawn behind the worst sample.
    pub backdrop: Color,
}

impl Sample {
    /// The score `ink` would get here, composited at this sample's opacity.
    pub fn score_of(&self, level: Level, ink: Color) -> f64 {
        let bg = rgb(self.backdrop);
        level.score(composite(ink, self.opacity, bg), bg)
    }
}

/// One layer's measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: NodeId,
    /// The layer's name, under the names of the layers above it.
    pub trail: Vec<String>,
    /// The worst score found: `x` in `x : 1`, or the Lc under [`Level::Apca`].
    pub ratio: f64,
    /// The score the level asks of this layer.
    pub needs: f64,
    /// Whether the threshold is below body text's, because of the text's size.
    pub large: bool,
    /// A shape measured by the non-text check, not a text layer.
    pub non_text: bool,
    /// The mode the worst score was measured in, `None` for a document with one
    /// or no mode.
    pub mode: Option<String>,
    /// The worst sample in each mode the layer was drawn in, in the document's
    /// mode order; the one [`Self::ratio`] came from is among them.
    pub samples: Vec<Sample>,
}

impl Finding {
    pub fn passes(&self) -> bool {
        self.ratio + 1e-9 >= self.needs
    }

    /// The sample the layer's score came from.
    pub fn worst(&self) -> Option<&Sample> {
        self.samples
            .iter()
            .min_by(|a, b| a.score.total_cmp(&b.score))
    }

    /// The lowest score `ink` would get across every sample, or `None` when
    /// there are none.
    pub fn score_of(&self, level: Level, ink: Color) -> Option<f64> {
        self.samples
            .iter()
            .map(|s| s.score_of(level, ink))
            .min_by(f64::total_cmp)
    }
}

/// A layer the check could not measure, and why.
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
    check_with(
        doc,
        page,
        Options {
            level,
            non_text: false,
        },
        None,
    )
}

/// [`check`] with [`Options`], and limited to the layers in `only` when it is
/// given; a layer there that is not a shown subject on `page` is left out.
pub fn check_with(
    doc: &Document,
    page: NodeId,
    options: Options,
    only: Option<&[NodeId]>,
) -> Report {
    let subjects: Vec<NodeId> = subjects(doc, page, options.non_text)
        .into_iter()
        .filter(|id| only.is_none_or(|o| o.contains(id)))
        .collect();
    let named = doc.variables().modes().len() > 1;
    let modes: Vec<Document> = match doc.variables().modes() {
        [] | [_] => vec![doc.clone()],
        modes => modes.iter().map(|m| in_mode(doc, page, m.id)).collect(),
    };
    let mut report = Report::default();
    for d in &modes {
        let res = Resolved::rebuild(d);
        let mut images = ImageStore::new();
        images.prepare(d);
        let here = check_one(d, &res, &images, page, options, &subjects, named);
        for f in here.findings {
            match report.findings.iter_mut().find(|x| x.id == f.id) {
                Some(x) => {
                    for s in f.samples {
                        if !x.samples.iter().any(|y| y.mode == s.mode) {
                            x.samples.push(s);
                        }
                    }
                    if f.ratio < x.ratio {
                        x.ratio = f.ratio;
                        x.mode = f.mode;
                    }
                    x.needs = x.needs.max(f.needs);
                    x.large &= f.large;
                }
                None => report.findings.push(f),
            }
        }
        for s in here.skipped {
            if !report.skipped.iter().any(|x| x.id == s.id) {
                report.skipped.push(s);
            }
        }
    }
    // A layer measured in one mode and skipped in another is reported measured.
    report
        .skipped
        .retain(|s| !report.findings.iter().any(|f| f.id == s.id));
    let order = |m: Option<ModeId>| {
        m.and_then(|m| doc.variables().mode_index(m))
            .unwrap_or(usize::MAX)
    };
    for f in &mut report.findings {
        f.samples.sort_by_key(|s| order(s.mode));
    }
    report
}

/// `doc` with every top-level frame of `page` that sets no mode set to `mode`.
fn in_mode(doc: &Document, page: NodeId, mode: ModeId) -> Document {
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
    options: Options,
    subjects: &[NodeId],
    named: bool,
) -> Report {
    let level = options.level;
    let mut report = Report::default();
    let ground = doc.ground(page);
    for &id in subjects {
        let Some(node) = doc.get(id) else { continue };
        let skip = |report: &mut Report, why| {
            report.skipped.push(Skipped {
                id,
                trail: trail(doc, id),
                why,
            })
        };
        let paint = doc.resolved_paint(id, node.paint());
        let top_fill = paint
            .fills
            .iter()
            .rev()
            .find(|f| f.visible)
            .map(|f| f.brush.drawn().clone());
        let scale = res.world_transform(id).map_or(1.0, |t| {
            let [a, b, c, d, _, _] = t.as_coeffs();
            (a * d - b * c).abs().sqrt()
        });
        let mut colours = Vec::new();
        let (needs, large, non_text) =
            if let Some(text) = ondin_core::TextRef::of(res.used_kind_of(node)) {
                let runs = text.spans.runs(text.style, text.content.len());
                let mut unsolid = None;
                for (_, style) in &runs {
                    match (style.color, &top_fill) {
                        (Some(c), _) => colours.push(c),
                        (None, Some(Brush::Solid(c))) => colours.push(*c),
                        (None, Some(b)) => unsolid = Some(not_solid(b)),
                        (None, None) => {}
                    }
                }
                if let Some(why) = unsolid {
                    skip(&mut report, why);
                    continue;
                }
                let needs = runs
                    .iter()
                    .map(|(_, s)| level.needs(s.font_size * scale, s.weight))
                    .fold(0.0, f64::max);
                (needs, needs < level.body(), false)
            } else {
                let mut unsolid = None;
                let brushes = paint
                    .fills
                    .iter()
                    .filter(|f| f.visible)
                    .map(|f| f.brush.drawn())
                    .chain(
                        paint
                            .strokes
                            .iter()
                            .filter(|s| s.visible && s.width > 0.0)
                            .map(|s| s.brush.drawn()),
                    );
                for b in brushes {
                    match b {
                        Brush::Solid(c) => colours.push(*c),
                        b => unsolid = Some(not_solid(b)),
                    }
                }
                if let Some(why) = unsolid {
                    skip(&mut report, why);
                    continue;
                }
                (level.non_text(), false, true)
            };
        if colours.is_empty() {
            continue;
        }
        let opacity = chain_opacity(doc, id);
        let Some(bounds) = res
            .world_bounds(id)
            .filter(|b| b.width() + b.height() > 0.0)
        else {
            continue;
        };
        let Some((score, ink, backdrop)) = worst_sample(
            doc, res, images, page, id, bounds, ground, &colours, opacity, level,
        ) else {
            continue;
        };
        let mode = doc.mode_at(id);
        let name = mode
            .filter(|_| named)
            .and_then(|m| doc.variables().mode(m))
            .map(|m| m.name.clone());
        report.findings.push(Finding {
            id,
            trail: trail(doc, id),
            ratio: score,
            needs,
            large,
            non_text,
            mode: name.clone(),
            samples: vec![Sample {
                mode,
                name,
                score,
                ink,
                opacity,
                backdrop,
            }],
        });
    }
    report
}

/// Why a paint that is not one colour cannot be measured, as a skip says it.
fn not_solid(b: &Brush) -> &'static str {
    match b {
        Brush::Image(_) => "picture fill",
        _ => "gradient fill",
    }
}

/// The layers a check measures on `page`, in document order: the shown text
/// layers and, with `non_text`, the shown shapes under [`NON_TEXT_MAX`] across.
/// A boolean's operands are its shape, not marks of their own.
pub fn subjects(doc: &Document, page: NodeId, non_text: bool) -> Vec<NodeId> {
    let res = non_text.then(|| Resolved::rebuild(doc));
    let mut out = Vec::new();
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !doc.shown(id) {
            continue;
        }
        match n.kind() {
            NodeKind::Text { .. } => out.push(id),
            NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Path { .. }
            | NodeKind::Boolean { .. } => {
                let small = res
                    .as_ref()
                    .and_then(|r| r.world_bounds(id))
                    .is_some_and(|b| b.width().min(b.height()) < NON_TEXT_MAX);
                if small {
                    out.push(id);
                }
                continue;
            }
            _ => {}
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

/// The lowest score between a layer colour composited over the backdrop and the
/// backdrop, over the pixels the layer covers most, with the colour and the
/// backdrop it was found at. `None` when the layer draws nothing in its box.
#[allow(clippy::too_many_arguments)]
fn worst_sample(
    doc: &Document,
    res: &Resolved,
    images: &ImageStore,
    page: NodeId,
    id: NodeId,
    bounds: Rect,
    ground: Color,
    colours: &[Color],
    opacity: f64,
    level: Level,
) -> Option<(f64, Color, Color)> {
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
    let mut worst: Option<(f64, Color, [f64; 3])> = None;
    for (d, i) in diffs {
        if d < most * 0.5 {
            continue;
        }
        let bg = over(&without[i]);
        for c in colours {
            let s = level.score(composite(*c, opacity, bg), bg);
            if worst.is_none_or(|(w, ..)| s < w) {
                worst = Some((s, *c, bg));
            }
        }
    }
    worst.map(|(s, c, bg)| {
        (
            s,
            c,
            Color::new([bg[0] as f32, bg[1] as f32, bg[2] as f32, 1.0]),
        )
    })
}

/// `c`, its own alpha times `opacity`, composited over the opaque `bg`.
pub fn composite(c: Color, opacity: f64, bg: [f64; 3]) -> [f64; 3] {
    let [r, g, b, a] = c.components;
    let alpha = f64::from(a) * opacity;
    let fg = [r, g, b].map(f64::from);
    std::array::from_fn(|k| fg[k] * alpha + bg[k] * (1.0 - alpha))
}

/// A colour's straight sRGB channels, alpha dropped.
pub fn rgb(c: Color) -> [f64; 3] {
    let [r, g, b, _] = c.components;
    [r, g, b].map(f64::from)
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

/// APCA-W3 0.0.98G's Lc of `text` on `bg`, straight sRGB: positive for dark text
/// on a light ground, negative for light on dark.
pub fn apca(text: [f64; 3], bg: [f64; 3]) -> f64 {
    let y = |c: [f64; 3]| {
        let y = 0.2126729 * c[0].max(0.0).powf(2.4)
            + 0.7151522 * c[1].max(0.0).powf(2.4)
            + 0.0721750 * c[2].max(0.0).powf(2.4);
        // The soft clamp near black.
        if y < 0.022 {
            y + (0.022 - y).powf(1.414)
        } else {
            y
        }
    };
    let (yt, yb) = (y(text), y(bg));
    if (yb - yt).abs() < 0.0005 {
        return 0.0;
    }
    let out = if yb > yt {
        let sapc = (yb.powf(0.56) - yt.powf(0.57)) * 1.14;
        if sapc < 0.1 { 0.0 } else { sapc - 0.027 }
    } else {
        let sapc = (yb.powf(0.65) - yt.powf(0.62)) * 1.14;
        if sapc > -0.1 { 0.0 } else { sapc + 0.027 }
    };
    out * 100.0
}

/// The colour at `ink`'s hue and chroma whose lightness is nearest `ink`'s and
/// passes `finding` in every sample, with a small margin; chroma gives way only
/// where the gamut ends. `None` when no lightness at that hue passes.
pub fn nearest(finding: &Finding, level: Level, ink: Color) -> Option<Color> {
    let goal = finding.needs + level.margin();
    let passes = |c: Color| finding.score_of(level, c).is_some_and(|s| s + 1e-9 >= goal);
    let [l, a, b] = oklab(rgb(ink));
    let (chroma, hue) = ((a * a + b * b).sqrt(), b.atan2(a));
    let at = |l: f64| {
        let [r, g, b] = in_gamut(l, chroma, hue);
        Color::new([r as f32, g as f32, b as f32, ink.components[3]])
    };
    const STEP: f64 = 0.002;
    let steps = (1.0 / STEP) as usize;
    (1..=steps)
        .flat_map(|i| {
            let d = i as f64 * STEP;
            [l - d, l + d]
        })
        .filter(|l| (0.0..=1.0).contains(l))
        .map(at)
        .find(|c| passes(*c))
}

/// The squared OKLab distance between two colours, for ranking the colours
/// nearest one.
pub fn distance(a: Color, b: Color) -> f64 {
    let (p, q) = (oklab(rgb(a)), oklab(rgb(b)));
    (0..3).map(|k| (p[k] - q[k]).powi(2)).sum()
}

/// The colour variables of `doc` whose value in each sample's mode passes
/// `finding` there, each with its lowest score, nearest `ink` first.
pub fn passing_variables(
    doc: &Document,
    finding: &Finding,
    level: Level,
    ink: Color,
) -> Vec<(ondin_core::VarId, f64)> {
    let vars = doc.variables();
    let mut out: Vec<(ondin_core::VarId, f64, f64)> = vars
        .iter()
        .filter(|v| v.kind == ondin_core::VarKind::Color)
        .filter_map(|v| {
            let mut lowest = f64::INFINITY;
            let mut near = None;
            for s in &finding.samples {
                let Some(Value::Color(c)) = vars.resolve_opt(v.id, s.mode) else {
                    return None;
                };
                lowest = lowest.min(s.score_of(level, c));
                near.get_or_insert(distance(c, ink));
            }
            (lowest + 1e-9 >= finding.needs).then(|| (v.id, lowest, near.unwrap_or(0.0)))
        })
        .collect();
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out.into_iter().map(|(v, s, _)| (v, s)).collect()
}

/// Straight sRGB to OKLab.
fn oklab(c: [f64; 3]) -> [f64; 3] {
    let lin = |x: f64| {
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = c.map(lin);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// OKLab to straight sRGB, unclamped: a channel outside 0..=1 is out of gamut.
fn from_oklab([l, a, b]: [f64; 3]) -> [f64; 3] {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let lin = [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_,
    ];
    lin.map(|x| {
        if x <= 0.0031308 {
            x * 12.92
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// The sRGB colour at OKLCH `l`, `chroma`, `hue`, with the chroma reduced, by
/// bisection, as far as the gamut needs.
fn in_gamut(l: f64, chroma: f64, hue: f64) -> [f64; 3] {
    let at = |c: f64| from_oklab([l, c * hue.cos(), c * hue.sin()]);
    let fits = |x: [f64; 3]| x.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v));
    let first = at(chroma);
    if fits(first) {
        return first.map(|v| v.clamp(0.0, 1.0));
    }
    let (mut lo, mut hi) = (0.0, chroma);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if fits(at(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(lo).map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// APCA's published reference pairs: black on white is Lc 106.04, white on
    /// black −107.88, and #888 on white 63.06.
    #[test]
    fn apca_matches_its_reference_values() {
        let (black, white, grey) = ([0.0; 3], [1.0; 3], [0x88 as f64 / 255.0; 3]);
        assert!((apca(black, white) - 106.04).abs() < 0.05);
        assert!((apca(white, black) + 107.88).abs() < 0.05);
        assert!((apca(grey, white) - 63.06).abs() < 0.05);
    }

    /// OKLab round-trips a colour, so a fix at the same lightness is the colour.
    #[test]
    fn oklab_round_trips() {
        let c = [0.64, 0.63, 0.61];
        let back = from_oklab(oklab(c));
        for k in 0..3 {
            assert!((back[k] - c[k]).abs() < 1e-6, "{back:?}");
        }
    }

    /// A score prints rounded down, so a fail never reads as its threshold.
    #[test]
    fn scores_print_rounded_down() {
        assert_eq!(Level::Aa.format(4.49), "4.4 : 1");
        assert_eq!(Level::Aa.format(12.0), "12 : 1");
        assert_eq!(Level::Apca.format(59.9), "Lc 59");
        assert_eq!(Level::Aa.format_needs(4.5), "4.5");
        assert_eq!(Level::Aa.format_needs(3.0), "3");
    }

    fn finding(ink: Color, backdrop: Color, needs: f64) -> Finding {
        let s = Sample {
            mode: None,
            name: None,
            score: 0.0,
            ink,
            opacity: 1.0,
            backdrop,
        };
        Finding {
            id: NodeId { actor: 1, seq: 1 },
            trail: vec![],
            ratio: 0.0,
            needs,
            large: false,
            non_text: false,
            mode: None,
            samples: vec![s],
        }
    }

    /// The nearest fix keeps the hue, passes with a little to spare, and is the
    /// closest lightness that does: one step lighter fails.
    #[test]
    fn the_nearest_fix_is_just_far_enough() {
        let ink = Color::from_rgb8(0xa3, 0xa1, 0x9c);
        let bg = Color::from_rgb8(0xf4, 0xf2, 0xee);
        let f = finding(ink, bg, 4.5);
        let fix = nearest(&f, Level::Aa, ink).expect("a darker grey passes");
        let s = f.score_of(Level::Aa, fix).unwrap();
        assert!((4.5..4.8).contains(&s), "{s}");
        let [l, ..] = oklab(rgb(fix));
        let lighter = Color::new({
            let [r, g, b] = in_gamut(l + 0.004, 0.0, 0.0);
            [r as f32, g as f32, b as f32, 1.0]
        });
        assert!(f.score_of(Level::Aa, lighter).unwrap() < 4.5 + Level::Aa.margin());
        let ([_, a0, b0], [_, a1, b1]) = (oklab(rgb(ink)), oklab(rgb(fix)));
        assert!((b0.atan2(a0) - b1.atan2(a1)).abs() < 0.05, "same hue");
    }

    /// Against mid grey nothing reaches 7 : 1, so there is no nearest fix.
    #[test]
    fn no_lightness_passes_on_mid_grey_at_seven() {
        let grey = Color::from_rgb8(0x77, 0x77, 0x77);
        let f = finding(Color::from_rgb8(0x60, 0x60, 0x60), grey, 7.0);
        assert_eq!(nearest(&f, Level::Aa, f.samples[0].ink), None);
    }
}
