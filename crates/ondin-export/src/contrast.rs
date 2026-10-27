//! Text contrast against what is actually drawn behind it, for `ondin check
//! contrast` and the canvas's contrast check mode.
//!
//! **The backdrop is rendered, not guessed**: each text layer's box is drawn
//! through the CPU renderer with the layer at zero opacity (`RenderOverrides`;
//! it keeps its place in any layout), and twice with its ink painted white and
//! then black. Where the two inks differ most are the glyphs' cores, whatever
//! the backdrop's colour. At each core, the colour the layer's ink draws there is
//! measured against the backdrop pixel, and the worst sample is the layer's
//! score. So a gradient, a picture or another layer behind the type is measured
//! where the glyphs sit, and ink the colour of its backdrop scores 1 : 1.
//!
//! The thresholds are WCAG 2's: 4.5 : 1, or 3 : 1 for large text (24 px, or
//! 18.66 px from weight 700), and 7 : 1 and 4.5 : 1 at AAA. [`Level::Apca`]
//! scores APCA's Lc instead, held to its Bronze levels by size and weight. The
//! size is each run's, at the layer's world scale, and the strictest run's
//! threshold is the layer's. Every mode of the document is checked, top-level
//! frames that set none drawn in it, each against its own threshold.
//!
//! With [`Options::non_text`], small shapes (icons, borders, dividers: under
//! [`NON_TEXT_MAX`] across) are measured the same way against 3 : 1.

use ondin_core::kurbo::Rect;
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, CharAttr, Document, ModeId, NodeId, NodeKind, Operation, Resolved, Transaction, Value,
};
use ondin_render::{ImageStore, MAX_RASTER_SIDE, RenderOverrides, VelloCpuRenderer, Viewport};

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
    /// What the layer needs in this mode, where its size may differ.
    pub needs: f64,
    /// The layer's colour drawn at the worst sample, as set, before opacity.
    pub ink: Color,
    /// How much of a fully opaque ink reaches the screen there: the layer's
    /// opacity times its ancestors'.
    pub opacity: f64,
    /// What that ink is laid over before it is faded: the backdrop, unless an
    /// ancestor's opacity fades the layer and the backdrop together.
    pub under: Color,
    /// The opaque colour drawn behind the worst sample.
    pub backdrop: Color,
}

impl Sample {
    /// The score `ink` would get here, drawn as this sample's ink is.
    pub fn score_of(&self, level: Level, ink: Color) -> f64 {
        let bg = rgb(self.backdrop);
        level.score(drawn(ink, self.opacity, rgb(self.under), bg), bg)
    }

    /// Whether `ink` drawn here meets this mode's threshold.
    pub fn passes_with(&self, level: Level, ink: Color) -> bool {
        self.score_of(level, ink) + 1e-9 >= self.needs
    }
}

/// One layer's measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: NodeId,
    /// The layer's name, under the names of the layers above it.
    pub trail: Vec<String>,
    /// The score of [`Self::worst`]: `x` in `x : 1`, or the Lc under
    /// [`Level::Apca`].
    pub ratio: f64,
    /// What [`Self::worst`]'s mode asks of this layer.
    pub needs: f64,
    /// Whether that threshold is below body text's, because of the text's size.
    pub large: bool,
    /// A shape measured by the non-text check, not a text layer.
    pub non_text: bool,
    /// [`Self::worst`]'s mode, `None` for a document with one or no mode.
    pub mode: Option<String>,
    /// The worst sample in each mode the layer was drawn in, in the document's
    /// mode order.
    pub samples: Vec<Sample>,
}

impl Finding {
    /// Whether every mode's sample meets that mode's threshold.
    pub fn passes(&self) -> bool {
        self.ratio + 1e-9 >= self.needs
    }

    /// The sample furthest below its own threshold, or least above it: the one
    /// [`Self::ratio`], [`Self::needs`] and [`Self::mode`] come from.
    pub fn worst(&self) -> Option<&Sample> {
        self.samples
            .iter()
            .rev()
            .max_by(|a, b| (a.needs - a.score).total_cmp(&(b.needs - b.score)))
    }

    /// Whether `ink` meets every sample's threshold.
    pub fn passes_with(&self, level: Level, ink: Color) -> bool {
        self.samples.iter().all(|s| s.passes_with(level, ink))
    }

    /// Take the score, threshold and mode from [`Self::worst`].
    fn settle(&mut self, level: Level) {
        let Some(w) = self.worst().cloned() else {
            return;
        };
        self.ratio = w.score;
        self.needs = w.needs;
        self.large = !self.non_text && w.needs < level.body();
        self.mode = w.name;
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
    Prepared::new(doc, page).check(options, only)
}

/// A page made ready to measure: each mode's document, its layout and its
/// decoded pictures, built once, so measuring a few layers at a time does not
/// redo them for every few.
pub struct Prepared {
    page: NodeId,
    /// Whether the document names its modes (it has several).
    named: bool,
    modes: Vec<(Document, Resolved, ImageStore)>,
}

impl Prepared {
    /// `doc`'s `page`, in each of its modes.
    pub fn new(doc: &Document, page: NodeId) -> Self {
        let docs: Vec<Document> = match doc.variables().modes() {
            [] | [_] => vec![doc.clone()],
            modes => modes.iter().map(|m| in_mode(doc, page, m.id)).collect(),
        };
        let modes = docs
            .into_iter()
            .map(|d| {
                let res = Resolved::rebuild(&d);
                let mut images = ImageStore::new();
                images.prepare(&d);
                (d, res, images)
            })
            .collect();
        Self {
            page,
            named: doc.variables().modes().len() > 1,
            modes,
        }
    }

    /// [`check_with`] on the prepared page.
    pub fn check(&self, options: Options, only: Option<&[NodeId]>) -> Report {
        let (page, named) = (self.page, self.named);
        let mut report = Report::default();
        for (d, res, images) in &self.modes {
            // Chosen in each mode: a bound visibility or size can make a layer a
            // subject in one mode only.
            let subjects: Vec<NodeId> = subjects(d, res, page, options.non_text)
                .into_iter()
                .filter(|id| only.is_none_or(|o| o.contains(id)))
                .collect();
            if subjects.is_empty() {
                continue;
            }
            let here = check_one(d, res, images, page, options, &subjects, named);
            for f in here.findings {
                match report.findings.iter_mut().find(|x| x.id == f.id) {
                    Some(x) => {
                        for s in f.samples {
                            if !x.samples.iter().any(|y| y.mode == s.mode) {
                                x.samples.push(s);
                            }
                        }
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
            let vars = self.modes.first().map(|(d, ..)| d.variables());
            m.and_then(|m| vars?.mode_index(m)).unwrap_or(usize::MAX)
        };
        for f in &mut report.findings {
            f.samples.sort_by_key(|s| order(s.mode));
            f.settle(options.level);
        }
        report
    }
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
        // A run with no colour and no fill to draw in draws black, unless the
        // type is outlined (`scene::paint_shape`).
        let stroked = paint.strokes.iter().any(|s| s.visible);
        let fallback = (top_fill.is_none() && !stroked).then_some(Color::BLACK);
        let mut colours: Vec<Color> = Vec::new();
        let (needs, large, non_text) =
            if let Some(text) = ondin_core::TextRef::of(res.used_kind_of(node)) {
                let runs = text.spans.runs(text.style, text.content.len());
                let mut unsolid = None;
                for (_, style) in &runs {
                    match (style.color, &top_fill) {
                        (Some(c), _) => colours.push(c),
                        (None, Some(Brush::Solid(c))) => colours.push(*c),
                        (None, Some(b)) => unsolid = Some(not_solid(b)),
                        (None, None) => colours.extend(fallback),
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
        let colours = colours.into_iter().fold(Vec::new(), |mut out, c| {
            if !out.contains(&c) {
                out.push(c);
            }
            out
        });
        if colours.is_empty() {
            // Outlined type, or a shape with no paint to see.
            continue;
        }
        let Some(bounds) = res
            .world_bounds(id)
            .filter(|b| b.width() + b.height() > 0.0)
        else {
            continue;
        };
        let measured = match measure(
            doc, res, images, page, id, bounds, ground, &colours, level, non_text,
        ) {
            Ok(Some(m)) => m,
            // Clipped away, covered, or fully transparent: nothing to read.
            Ok(None) => continue,
            Err(why) => {
                skip(&mut report, why);
                continue;
            }
        };
        let mode = doc.mode_at(id);
        let name = mode
            .filter(|_| named)
            .and_then(|m| doc.variables().mode(m))
            .map(|m| m.name.clone());
        report.findings.push(Finding {
            id,
            trail: trail(doc, id),
            ratio: measured.score,
            needs,
            large,
            non_text,
            mode: name.clone(),
            samples: vec![Sample {
                mode,
                name,
                score: measured.score,
                needs,
                ink: measured.ink,
                opacity: measured.opacity,
                under: measured.under,
                backdrop: measured.backdrop,
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
/// A boolean's operands are its shape, not marks of their own, and a mask's
/// paint is never drawn. `res` is `doc`'s.
pub fn subjects(doc: &Document, res: &Resolved, page: NodeId, non_text: bool) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !doc.shown(id) || n.mask() {
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
                    .world_bounds(id)
                    .is_some_and(|b| b.width().min(b.height()) < NON_TEXT_MAX);
                if non_text && small {
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

/// A layer's worst sample, as [`measure`] finds it.
struct Measured {
    score: f64,
    ink: Color,
    opacity: f64,
    under: Color,
    backdrop: Color,
}

/// Why a layer whose paint the preview cannot change is skipped.
const UNPREVIEWABLE: &str = "cannot be drawn apart";

/// The lowest score of the layer's ink, as drawn, against the backdrop, over
/// the pixels the layer covers most; with `any_part`, the score of its best
/// colour at that colour's worst. `Ok(None)` when the layer draws nothing in its
/// box: clipped away, covered, or fully transparent.
///
/// Four renders of the box: the backdrop, with the layer at zero opacity (hiding
/// it would take it out of an auto layout and slide its siblings into its
/// place); the layer's ink painted white, and black, whose difference is its
/// coverage at any backdrop; and, when it has several colours, the layer as it
/// is, to tell which colour each pixel draws.
#[allow(clippy::too_many_arguments)]
fn measure(
    doc: &Document,
    res: &Resolved,
    images: &ImageStore,
    page: NodeId,
    id: NodeId,
    bounds: Rect,
    ground: Color,
    colours: &[Color],
    level: Level,
    any_part: bool,
) -> Result<Option<Measured>, &'static str> {
    let view = bounds.inflate(1.0, 1.0);
    let side = f64::from(MAX_RASTER_SIDE);
    let scale = SCALE
        .min((MAX_PIXELS / view.area()).sqrt())
        .max(0.05)
        .min(side / view.width())
        .min(side / view.height());
    let vp = Viewport {
        view,
        pixel_size: (
            ((view.width() * scale).ceil().max(1.0) as u32).min(MAX_RASTER_SIDE),
            ((view.height() * scale).ceil().max(1.0) as u32).min(MAX_RASTER_SIDE),
        ),
    };
    let preview = |ops: Vec<Operation>| {
        RenderOverrides::from_transaction(doc, res, &Transaction(ops)).ok_or(UNPREVIEWABLE)
    };
    let hidden = preview(vec![Operation::SetOpacity { id, opacity: 0.0 }])?;
    let white = preview(paint_as(doc, id, Color::WHITE))?;
    let black = preview(paint_as(doc, id, Color::BLACK))?;
    let mut cpu = VelloCpuRenderer::new();
    let mut render = |ov: &RenderOverrides| {
        cpu.render_subtrees_with(doc, res, &vp, images, &[page], ov)
            .0
    };
    let without = render(&hidden);
    let (white, black) = (render(&white), render(&black));
    let with = (colours.len() > 1).then(|| render(&RenderOverrides::default()));
    let at = |buf: &[u8], i: usize| -> [f64; 3] {
        let px = &buf[i * 4..i * 4 + 4];
        let a = f64::from(px[3]) / 255.0;
        let g = ground.components;
        std::array::from_fn(|k| f64::from(px[k]) / 255.0 * a + f64::from(g[k]) * (1.0 - a))
    };
    // A pixel's share of the ink: how far white and black ink drawn there apart.
    let share: Vec<f64> = (0..without.len() / 4)
        .map(|i| {
            let (w, b) = (at(&white, i), at(&black, i));
            ((w[0] - b[0]) + (w[1] - b[1]) + (w[2] - b[2])) / 3.0
        })
        .collect();
    let most = share.iter().copied().fold(0.0, f64::max);
    if most < 1e-3 {
        return Ok(None);
    }
    // Each colour's worst sample.
    let mut worst: Vec<Option<Measured>> = colours.iter().map(|_| None).collect();
    for (i, &k) in share.iter().enumerate() {
        // The glyphs' cores: pixels the ink covers at least half as much as any,
        // so antialiased edges do not read as faint type.
        if k < most * 0.5 {
            continue;
        }
        let (bg, b) = (at(&without, i), at(&black, i));
        // The colour this pixel draws: the one whose drawing here best matches it.
        let ink = match &with {
            Some(with) => {
                let seen = at(with, i);
                let miss = |c: &Color| {
                    let a = f64::from(c.components[3]);
                    (0..3)
                        .map(|j| {
                            let c = f64::from(c.components[j]);
                            (bg[j] + a * (b[j] + k * c - bg[j]) - seen[j]).powi(2)
                        })
                        .sum::<f64>()
                };
                *colours
                    .iter()
                    .min_by(|x, y| miss(x).total_cmp(&miss(y)))
                    .expect("several colours")
            }
            None => colours[0],
        };
        // The pixel as if fully covered: an opaque ink `c` draws `base + most · c`.
        let cover = k / most;
        let base: [f64; 3] = std::array::from_fn(|j| bg[j] + (b[j] - bg[j]) / cover);
        let under = if most < 0.999 {
            base.map(|v| (v / (1.0 - most)).clamp(0.0, 1.0))
        } else {
            bg
        };
        let to_colour = |c: [f64; 3]| Color::new([c[0] as f32, c[1] as f32, c[2] as f32, 1.0]);
        let (under, backdrop) = (to_colour(under), to_colour(bg));
        let score = level.score(drawn(ink, most, rgb(under), rgb(backdrop)), rgb(backdrop));
        let slot = colours.iter().position(|c| *c == ink).unwrap_or(0);
        if worst[slot].as_ref().is_none_or(|w| score < w.score) {
            worst[slot] = Some(Measured {
                score,
                ink,
                opacity: most,
                under,
                backdrop,
            });
        }
    }
    // Every run of text must be legible; a mark needs only one part that stands
    // out (a white box with a dark border reads by its border).
    let parts = worst.into_iter().flatten();
    Ok(if any_part {
        parts.max_by(|a, b| a.score.total_cmp(&b.score))
    } else {
        parts.min_by(|a, b| a.score.total_cmp(&b.score))
    })
}

/// The edit that paints every ink of `id` in `colour`, opaque: its visible fills
/// and strokes, and a text's runs, whose own colours are cleared onto the fill.
fn paint_as(doc: &Document, id: NodeId, colour: Color) -> Vec<Operation> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    let brush = || ondin_core::BrushSource::Literal(Brush::Solid(colour));
    let mut ops = Vec::new();
    let mut fills = node.paint().fills.clone();
    for f in fills.iter_mut().filter(|f| f.visible) {
        f.brush = brush();
    }
    if let Some(text) = ondin_core::TextRef::of(node.kind()) {
        let len = text.content.len();
        if text
            .spans
            .runs(text.style, len)
            .iter()
            .any(|(_, s)| s.color.is_some())
        {
            let mut spans = text.spans.clone();
            spans.set(0..len, CharAttr::Color(None), text.style);
            ops.push(Operation::SetTextSpans { id, spans });
        }
        // No visible fill draws black; the probe stands in for it.
        if !fills.iter().any(|f| f.visible) {
            fills = ondin_core::keyed_by_position([ondin_core::Fill {
                brush: brush(),
                visible: true,
            }]);
        }
    } else {
        let mut strokes = node.paint().strokes.clone();
        for s in strokes.iter_mut().filter(|s| s.visible) {
            s.brush = brush();
        }
        ops.push(Operation::SetStrokes { id, strokes });
    }
    ops.push(Operation::SetFills { id, fills });
    ops
}

/// What `ink` draws where an opaque ink reaches the screen at `opacity` over
/// `under`, against the opaque `backdrop` it replaces; `ink`'s own alpha fades
/// it toward that backdrop.
pub fn drawn(ink: Color, opacity: f64, under: [f64; 3], backdrop: [f64; 3]) -> [f64; 3] {
    let a = f64::from(ink.components[3]);
    let c = rgb(ink);
    std::array::from_fn(|k| {
        let full = opacity * c[k] + (1.0 - opacity) * under[k];
        backdrop[k] + a * (full - backdrop[k])
    })
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
/// passes `finding` in every sample, against that sample's threshold, with a
/// small margin; chroma gives way only where the gamut ends. `None` when no
/// lightness at that hue passes.
pub fn nearest(finding: &Finding, level: Level, ink: Color) -> Option<Color> {
    let passes = |c: Color| {
        !finding.samples.is_empty()
            && finding
                .samples
                .iter()
                .all(|s| s.score_of(level, c) + 1e-9 >= s.needs + level.margin())
    };
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

/// The colour variables of `doc` whose value in each sample's mode meets that
/// sample's threshold, each with its lowest score, nearest `ink` first.
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
                if !s.passes_with(level, c) {
                    return None;
                }
                lowest = lowest.min(s.score_of(level, c));
                near.get_or_insert(distance(c, ink));
            }
            lowest
                .is_finite()
                .then(|| (v.id, lowest, near.unwrap_or(0.0)))
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
            needs,
            ink,
            opacity: 1.0,
            under: backdrop,
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

    /// Each mode is held to its own threshold: large type at 3.5 : 1 in one mode
    /// and body type at 4.6 : 1 in another passes, reported from the mode
    /// nearest its threshold.
    ///
    /// flip: `Finding::worst` taking the lowest score → fails at the reported
    /// score.
    #[test]
    fn modes_are_held_to_their_own_thresholds() {
        let (black, white) = (Color::BLACK, Color::WHITE);
        let mut f = finding(black, white, 3.0);
        f.samples[0].score = 3.5;
        f.samples.push(Sample {
            score: 4.6,
            needs: 4.5,
            ..f.samples[0].clone()
        });
        f.settle(Level::Aa);
        assert!(f.passes(), "{f:?}");
        assert_eq!((f.ratio, f.needs), (4.6, 4.5), "the reported score");
        assert!(!f.large);
    }

    /// Against mid grey nothing reaches 7 : 1, so there is no nearest fix.
    #[test]
    fn no_lightness_passes_on_mid_grey_at_seven() {
        let grey = Color::from_rgb8(0x77, 0x77, 0x77);
        let f = finding(Color::from_rgb8(0x60, 0x60, 0x60), grey, 7.0);
        assert_eq!(nearest(&f, Level::Aa, f.samples[0].ink), None);
    }
}
