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
use ondin_core::variable::{self, CollectionId};
use ondin_core::{
    Brush, CharAttr, Document, ModeId, NodeId, NodeKind, Operation, Resolved, Transaction,
};
use ondin_render::{ImageStore, MAX_RASTER_SIDE, RenderOverrides, VelloCpuRenderer, Viewport};

/// The scores, the findings and the fix arithmetic are core's
/// (`ondin_core::contrast`), so a verb can fix a finding; the measuring is here.
pub use ondin_core::contrast::{
    Finding, Level, Sample, apca, distance, drawn, luminance, nearest, passing_variables, ratio,
    rgb,
};

/// What a check measures.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    pub level: Level,
    /// Also measure shapes under [`NON_TEXT_MAX`] across, at [`Level::non_text`].
    pub non_text: bool,
    /// The ground a page with no colour of its own is measured over; `None` for
    /// [`ondin_core::DEFAULT_CANVAS_BACKGROUND`].
    pub default_ground: Option<Color>,
}

/// The largest a shape's shorter side may be, in world units, for the non-text
/// check to count it as a mark rather than a surface.
pub const NON_TEXT_MAX: f64 = 24.0;

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
            default_ground: None,
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

/// The renderer-backed [`ondin_core::contrast::Probe`] a verb borrows: each page
/// the layers are on prepared and checked for them alone, text and small shapes
/// both, over `default_ground` where a page has no colour of its own.
#[derive(Default)]
pub struct Measurer {
    pub default_ground: Option<Color>,
}

impl ondin_core::contrast::Probe for Measurer {
    fn measure(&mut self, doc: &Document, ids: &[NodeId], level: Level) -> Vec<Finding> {
        let mut pages: Vec<NodeId> = ids.iter().filter_map(|id| doc.page_of(*id)).collect();
        pages.sort();
        pages.dedup();
        let options = Options {
            level,
            non_text: true,
            default_ground: self.default_ground,
        };
        pages
            .into_iter()
            .flat_map(|page| Prepared::new(doc, page).check(options, Some(ids)).findings)
            .collect()
    }
}

/// A page made ready to measure: each mode's document, its layout and its
/// decoded pictures, built once, so measuring a few layers at a time does not
/// redo them for every few.
pub struct Prepared {
    page: NodeId,
    /// The page in each mode checked, with the collection that mode is of —
    /// `None` for the one document of a page no mode changes.
    modes: Vec<(Option<CollectionId>, Document, Resolved, ImageStore)>,
}

impl Prepared {
    /// `doc`'s `page`, in each mode of every collection with two or more that a
    /// variable reads — of its own, or a kit's mapping from it; a colour, or a
    /// visibility or size that makes a layer a subject in one mode only — one
    /// collection at a time, every other as the page sets it, since colours
    /// rarely vary along two.
    pub fn new(doc: &Document, page: NodeId) -> Self {
        let t = doc.variables();
        let mut docs: Vec<(Option<CollectionId>, Document)> = Vec::new();
        for c in t.collections().iter().filter(|c| c.modes.len() > 1) {
            // Its own variables, or a kit's that map from it.
            let maps_here = |v: &&ondin_core::Variable| {
                t.kit_onto(t.kit_collection_of(v).unwrap_or_default())
                    .is_some_and(|o| o.id == c.id)
            };
            if t.of_collection(c.id).next().is_none()
                && !t
                    .iter()
                    .filter(|v| v.kit().is_some())
                    .any(|v| maps_here(&v))
            {
                continue;
            }
            for m in &c.modes {
                docs.push((Some(c.id), in_mode(doc, page, c.id, m.id)));
            }
        }
        if docs.is_empty() {
            docs.push((None, doc.clone()));
        }
        let modes = docs
            .into_iter()
            .map(|(c, d)| {
                let res = Resolved::rebuild(&d);
                let mut images = ImageStore::new();
                images.prepare(&d);
                (c, d, res, images)
            })
            .collect();
        Self { page, modes }
    }

    /// [`check_with`] on the prepared page.
    pub fn check(&self, options: Options, only: Option<&[NodeId]>) -> Report {
        let page = self.page;
        let mut report = Report::default();
        for (cycled, d, res, images) in &self.modes {
            let named = *cycled;
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
            let vars = self.modes.first().map(|(_, d, ..)| d.variables());
            m.and_then(|m| {
                let t = vars?;
                Some((
                    t.collection_index(t.collection_of_mode(m)?.id)?,
                    t.mode_index(m)?,
                ))
            })
            .unwrap_or((usize::MAX, usize::MAX))
        };
        for f in &mut report.findings {
            f.samples.sort_by_key(|s| order(s.mode));
            f.settle(options.level);
        }
        report
    }
}

/// `doc` with every top-level frame of `page` that sets no mode of collection
/// `c` set to `mode`, the modes it sets of others kept.
fn in_mode(doc: &Document, page: NodeId, c: CollectionId, mode: ModeId) -> Document {
    let mut d = doc.clone();
    let t = doc.variables();
    let owner = variable::ModeOwner::Collection(c);
    let ops: Vec<Operation> = doc
        .get(page)
        .map(|p| p.children().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| doc.get(id))
        .filter(|n| matches!(n.kind(), NodeKind::Artboard { .. }) && n.mode_of(t, owner).is_none())
        .map(|n| Operation::SetModes {
            id: n.id(),
            modes: variable::with_mode(t, n.modes(), owner, Some(mode)),
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
    named: Option<CollectionId>,
) -> Report {
    let level = options.level;
    let mut report = Report::default();
    let ground = doc
        .own_ground(page)
        .or(options.default_ground)
        .unwrap_or(ondin_core::DEFAULT_CANVAS_BACKGROUND);
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
        if blends_here(doc, id) {
            skip(&mut report, BLENDED);
            continue;
        }
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
        let mode = named.and_then(|c| doc.mode_at(id, c));
        let name = mode
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
            | NodeKind::Spiral { .. }
            | NodeKind::Arc { .. }
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

/// Why a layer mixed into what is beneath it by a blend mode is skipped: the
/// drawn colour is [`drawn`]'s source-over, which a mode replaces.
const BLENDED: &str = "is blended with what is beneath it";

/// Whether `id` or a layer it is drawn inside has a mode that mixes rather
/// than covers.
fn blends_here(doc: &Document, id: NodeId) -> bool {
    std::iter::successors(doc.get(id), |n| n.parent().and_then(|p| doc.get(p))).any(|n| {
        !matches!(
            n.blend(),
            ondin_core::BlendMode::Normal | ondin_core::BlendMode::PassThrough
        )
    })
}

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
