//! Contrast scores and the fix for a failing layer: the levels and their
//! thresholds, what a measurement found ([`Finding`], one [`Sample`] per mode),
//! WCAG 2's ratio and APCA's Lc, the nearest passing colour, and the edit that
//! puts a fix on a layer ([`fix_transaction`]).
//!
//! **The measuring is not here**: a backdrop is rendered, which core cannot do,
//! so a caller lends a [`Probe`] (`ondin_export::contrast`'s is the one).

use crate::peniko::Color;
use crate::variable::BrushSource;
use crate::{Brush, CharAttr, Document, Fill, ModeId, NodeId, Operation, Transaction, Value};

/// What measures layers' contrast against what is drawn behind them, which needs
/// a renderer core does not have.
pub trait Probe {
    /// A finding for each of `ids` it could measure, held to `level`, and why it
    /// could not measure the others it considered.
    fn measure(&mut self, doc: &Document, ids: &[NodeId], level: Level) -> Measured;
}

/// What a [`Probe`] made of the layers it was asked about. A layer in neither
/// list is not one it measures: not a text layer or a small shape, or not shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measured {
    pub findings: Vec<Finding>,
    /// Each layer it considered and could not measure, with why.
    pub skipped: Vec<(NodeId, &'static str)>,
}

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
    pub fn settle(&mut self, level: Level) {
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
) -> Vec<(crate::VarId, f64)> {
    let vars = doc.variables();
    let mut out: Vec<(crate::VarId, f64, f64)> = vars
        .iter()
        .filter(|v| v.kind == crate::VarKind::Color)
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

/// A fix: a colour the failing paints take, or a variable they are bound to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fix {
    Colour(Color),
    Var(crate::VarId),
}

/// `brush`'s colour as it draws in `mode`: a bound one's variable read there.
fn colour_in(doc: &Document, brush: &BrushSource, mode: Option<ModeId>) -> Option<Color> {
    match brush {
        BrushSource::Literal(Brush::Solid(c)) => Some(*c),
        BrushSource::Literal(_) => None,
        BrushSource::Bound { var, alpha } => match doc.variables().resolve_opt(*var, mode) {
            Some(Value::Color(c)) => Some(c.multiply_alpha(*alpha)),
            _ => None,
        },
    }
}

/// Whether `brush` fails `f`: a bound colour is judged by its own value in each
/// sample's mode, so a variable that adapts to every mode's backdrop passes.
fn brush_fails(doc: &Document, f: &Finding, level: Level, brush: &BrushSource) -> bool {
    f.samples
        .iter()
        .any(|s| colour_in(doc, brush, s.mode).is_some_and(|c| !s.passes_with(level, c)))
}

/// The edit `fix` makes to `id`: the runs and paints whose colour fails `f` take
/// the new colour, or the variable. A run's own colour is replaced in the spans; a
/// run drawn in the fill changes the fill. Binding a variable goes through the
/// fill (a run's colour cannot be bound), so a failing run's own colour is
/// cleared onto it, and a passing run that drew in the fill keeps its colour. A
/// mark changes only the paints its measured ink came from. `new_fill` keys a
/// fill added to a text that had no visible one. `None` when nothing fails.
pub fn fix_transaction(
    doc: &Document,
    id: NodeId,
    f: &Finding,
    level: Level,
    fix: Fix,
    new_fill: crate::ItemId,
) -> Option<Transaction> {
    let node = doc.get(id)?;
    let fails = |c: Color| !f.passes_with(level, c);
    let paint = doc.resolved_paint(id, node.paint());
    let solid = |b: &Brush| match b {
        Brush::Solid(c) => Some(*c),
        _ => None,
    };
    let replace = |old: &BrushSource| -> BrushSource {
        match fix {
            Fix::Colour(c) => BrushSource::Literal(Brush::Solid(c)),
            Fix::Var(var) => BrushSource::Bound {
                var,
                alpha: match old {
                    BrushSource::Literal(Brush::Solid(c)) => c.components[3],
                    BrushSource::Bound { alpha, .. } => *alpha,
                    BrushSource::Literal(_) => 1.0,
                },
            },
        }
    };
    let mut ops = Vec::new();
    if let Some(text) = crate::TextRef::of(node.kind()) {
        let top = node.paint().fills.iter().rposition(|x| x.visible);
        let stroked = node.paint().strokes.iter().any(|s| s.visible);
        // What a run with no colour of its own draws: the top fill, else black,
        // or nothing for outlined type.
        let top_colour = match top {
            Some(i) => paint.fills.get(i).and_then(|p| solid(p.brush.drawn())),
            None if stroked => Some(Color::TRANSPARENT),
            None => Some(Color::BLACK),
        };
        let runs = text.spans.runs(text.style, text.content.len());
        let mut spans = text.spans.clone();
        let (mut spans_changed, mut onto_fill) = (false, false);
        let fill_fails = runs.iter().any(|(_, s)| s.color.is_none())
            && match top {
                Some(i) => brush_fails(doc, f, level, &node.paint().fills[i].brush),
                None => !stroked && fails(Color::BLACK),
            };
        for (range, style) in &runs {
            if let Some(c) = style.color
                && fails(c)
            {
                let attr = match fix {
                    Fix::Colour(n) => CharAttr::Color(Some(n)),
                    Fix::Var(_) => {
                        onto_fill = true;
                        CharAttr::Color(None)
                    }
                };
                spans.set(range.clone(), attr, text.style);
                spans_changed = true;
            }
        }
        if onto_fill && !fill_fails {
            // The fill is about to change under the runs that drew in it and passed.
            for (range, style) in &runs {
                if style.color.is_none()
                    && let Some(c) = top_colour
                {
                    spans.set(range.clone(), CharAttr::Color(Some(c)), text.style);
                }
            }
        }
        if spans_changed {
            ops.push(Operation::SetTextSpans { id, spans });
        }
        if fill_fails || onto_fill {
            let mut fills = node.paint().fills.clone();
            match top {
                Some(i) => {
                    let b = replace(&fills[i].brush);
                    fills[i].brush = b;
                }
                // On top of the hidden fills, which stay as they were.
                None => fills.push(crate::Keyed::new(
                    new_fill,
                    Fill {
                        brush: replace(&BrushSource::Literal(Brush::Solid(Color::BLACK))),
                        visible: true,
                    },
                )),
            }
            ops.push(Operation::SetFills { id, fills });
        }
    } else {
        // Only the paints the measured ink came from: a white box with a dark
        // border that fails is fixed by its border, its fill left white.
        let changes = |brush: &BrushSource| {
            f.samples
                .iter()
                .any(|s| colour_in(doc, brush, s.mode) == Some(s.ink))
                && brush_fails(doc, f, level, brush)
        };
        let mut fills = node.paint().fills.clone();
        let mut fills_changed = false;
        for fl in fills.iter_mut() {
            if fl.visible && changes(&fl.brush) {
                let b = replace(&fl.brush);
                fl.brush = b;
                fills_changed = true;
            }
        }
        let mut strokes = node.paint().strokes.clone();
        let mut strokes_changed = false;
        for st in strokes.iter_mut() {
            if st.visible && changes(&st.brush) {
                let b = replace(&st.brush);
                st.brush = b;
                strokes_changed = true;
            }
        }
        if fills_changed {
            ops.push(Operation::SetFills { id, fills });
        }
        if strokes_changed {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    (!ops.is_empty()).then_some(Transaction(ops))
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

    fn solid(c: Color) -> Vec<crate::Keyed<Fill>> {
        crate::keyed_by_position([Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }])
    }

    /// A page holding a white frame and in it a text `Title`, "Billed yearly",
    /// filled black: the document, its id source, the frame and the text.
    fn titled() -> (Document, crate::IdSource, NodeId, NodeId) {
        use crate::kurbo::{Affine, Size};
        let mut ids = crate::IdSource::new(0xC0);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let (card, title) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: card,
                parent: page,
                index: 0,
                kind: crate::NodeKind::Artboard {
                    size: Size::new(300.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Card".into()),
            },
            Operation::SetFills {
                id: card,
                fills: solid(Color::WHITE),
            },
            Operation::CreateNode {
                id: title,
                parent: card,
                index: 0,
                kind: crate::NodeKind::Text {
                    content: "Billed yearly".into(),
                    style: Box::default(),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: crate::TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((20.0, 20.0))),
                name: Some("Title".into()),
            },
            Operation::SetFills {
                id: title,
                fills: solid(Color::BLACK),
            },
        ]))
        .unwrap();
        (doc, ids, card, title)
    }

    /// Modes Light and Dark, and a colour variable `fg` black in Light and white
    /// in Dark: the modes and the variable.
    fn light_and_dark(
        doc: &mut Document,
        ids: &mut crate::IdSource,
    ) -> (ModeId, ModeId, crate::VarId) {
        let (light, dark, fg) = (ids.mint_mode(), ids.mint_mode(), ids.mint_var());
        let mode = |id, name: &str, index| Operation::AddMode {
            collection: crate::variable::CollectionId::FIRST,
            mode: crate::Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        };
        doc.apply(&Transaction(vec![
            mode(light, "Light", 0),
            mode(dark, "Dark", 1),
            Operation::AddVariable {
                var: crate::Variable {
                    id: fg,
                    name: "fg".into(),
                    kind: crate::VarKind::Color,
                    cells: [
                        (light, crate::Cell::Color(Color::BLACK)),
                        (dark, crate::Cell::Color(Color::WHITE)),
                    ]
                    .into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
        (light, dark, fg)
    }

    /// A finding for `id` whose one sample, in no mode, scores `score` with `ink`
    /// on white.
    fn on_white(id: NodeId, ink: Color, score: f64, needs: f64, non_text: bool) -> Finding {
        Finding {
            id,
            trail: vec![],
            ratio: score,
            needs,
            large: false,
            non_text,
            mode: None,
            samples: vec![Sample {
                mode: None,
                name: None,
                score,
                needs,
                ink,
                opacity: 1.0,
                under: Color::WHITE,
                backdrop: Color::WHITE,
            }],
        }
    }

    /// `doc` with `tx` applied.
    fn after(doc: &Document, tx: &Transaction) -> Document {
        let mut after = doc.clone();
        after.apply(tx).unwrap();
        after
    }

    /// A text's runs, each with its colour.
    fn run_colours(doc: &Document, id: NodeId) -> Vec<Option<Color>> {
        let t = crate::TextRef::of(doc.get(id).unwrap().kind()).unwrap();
        t.spans
            .runs(t.style, t.content.len())
            .into_iter()
            .map(|r| r.1.color)
            .collect()
    }

    /// A text whose first six bytes run grey.
    fn grey_run(doc: &mut Document, id: NodeId, grey: Color) {
        let mut spans = crate::CharSpans::default();
        spans.set(0..6, CharAttr::Color(Some(grey)), &Default::default());
        doc.apply(&Transaction(vec![Operation::SetTextSpans { id, spans }]))
            .unwrap();
    }

    /// A variable fix on a run with its own colour clears the run onto the fill
    /// and binds the fill, since a run's colour cannot be bound; a passing run
    /// that drew in the fill keeps its look.
    #[test]
    fn a_variable_fix_binds_through_the_fill() {
        let (mut doc, mut ids, _, title) = titled();
        let (_, _, fg) = light_and_dark(&mut doc, &mut ids);
        let grey = Color::from_rgba8(170, 170, 170, 255);
        grey_run(&mut doc, title, grey);
        let finding = on_white(title, grey, 2.3, 4.5, false);
        let key = ids.mint_item();
        let tx =
            fix_transaction(&doc, title, &finding, Level::Aa, Fix::Var(fg), key).expect("a fix");
        let doc = after(&doc, &tx);
        let runs = run_colours(&doc, title);
        assert_eq!(runs[0], None, "the failing run falls onto the fill");
        assert_eq!(runs[1], Some(Color::BLACK), "the passing run keeps black");
        assert_eq!(
            doc.get(title).unwrap().paint().fills[0].brush.var(),
            Some(fg),
            "the fill is bound"
        );
    }

    /// A fill bound to a variable that passes in every mode stays bound: only
    /// the run whose own colour fails takes the fix.
    ///
    /// flip: `fix_transaction` judging the bound fill by its default mode's
    /// colour against every mode's backdrop → fails at "the fill stays bound".
    #[test]
    fn a_fix_leaves_a_passing_bound_fill_bound() {
        let (mut doc, mut ids, _, title) = titled();
        let (light, dark, fg) = light_and_dark(&mut doc, &mut ids);
        let grey = Color::from_rgba8(0x77, 0x77, 0x77, 255);
        let fix = Color::from_rgba8(0x60, 0x60, 0x60, 255);
        grey_run(&mut doc, title, grey);
        let bind = crate::bind_brush_at_all(&doc, &[title], crate::PaintTarget::Fill, 0, fg);
        doc.apply(&bind).unwrap();
        let mut finding = on_white(title, grey, 4.48, 4.5, false);
        finding.samples[0].mode = Some(light);
        finding.samples.push(Sample {
            mode: Some(dark),
            score: 4.69,
            under: Color::BLACK,
            backdrop: Color::BLACK,
            ..finding.samples[0].clone()
        });
        let key = ids.mint_item();
        let tx = fix_transaction(&doc, title, &finding, Level::Aa, Fix::Colour(fix), key)
            .expect("a fix");
        let doc = after(&doc, &tx);
        assert_eq!(
            doc.get(title).unwrap().paint().fills[0].brush.var(),
            Some(fg),
            "the fill stays bound"
        );
        let runs = run_colours(&doc, title);
        assert_eq!(runs[0], Some(fix), "the grey run takes the fix");
        assert_eq!(runs[1], None, "the rest still draws in fg");
    }

    /// A variable fix on text with no visible fill adds one on top: the hidden
    /// fill stays, and a passing run with no colour of its own keeps the black
    /// it drew in.
    ///
    /// flip: `fix_transaction` replacing the fill list rather than adding to it
    /// → fails at "the hidden fill stays".
    #[test]
    fn a_variable_fix_keeps_hidden_fills() {
        let (mut doc, mut ids, _, title) = titled();
        let (_, _, fg) = light_and_dark(&mut doc, &mut ids);
        let grey = Color::from_rgba8(170, 170, 170, 255);
        grey_run(&mut doc, title, grey);
        let hidden = crate::keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)).into(),
            visible: false,
        }]);
        doc.apply(&Transaction(vec![Operation::SetFills {
            id: title,
            fills: hidden.clone(),
        }]))
        .unwrap();
        let finding = on_white(title, grey, 2.3, 4.5, false);
        let key = ids.mint_item();
        let tx =
            fix_transaction(&doc, title, &finding, Level::Aa, Fix::Var(fg), key).expect("a fix");
        let doc = after(&doc, &tx);
        let fills = &doc.get(title).unwrap().paint().fills;
        assert_eq!(fills.first(), hidden.first(), "the hidden fill stays");
        assert_eq!(fills.len(), 2);
        assert!(fills[1].visible && fills[1].brush.var() == Some(fg));
        let runs = run_colours(&doc, title);
        assert_eq!(runs[0], None, "the failing run falls onto the fill");
        assert_eq!(runs[1], Some(Color::BLACK), "the passing run keeps black");
    }

    /// A mark's fix changes the paint its measured ink came from: a white box
    /// whose faint border fails takes the fix on its border, its fill left white.
    ///
    /// flip: `fix_transaction` rewriting every failing paint of a mark → fails at
    /// "the fill stays white".
    #[test]
    fn a_mark_s_fix_changes_the_paint_it_was_measured_by() {
        let (mut doc, mut ids, card, _) = titled();
        let check_box = ids.mint();
        let faint = Color::from_rgba8(0xdd, 0xdd, 0xdd, 255);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: check_box,
                parent: card,
                index: 0,
                kind: crate::NodeKind::Rect {
                    size: crate::kurbo::Size::new(16.0, 16.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(crate::kurbo::Affine::translate((200.0, 20.0))),
                name: Some("Box".into()),
            },
            Operation::SetFills {
                id: check_box,
                fills: solid(Color::WHITE),
            },
            Operation::SetStrokes {
                id: check_box,
                strokes: crate::keyed_by_position([crate::Stroke {
                    brush: Brush::Solid(faint).into(),
                    ..Default::default()
                }]),
            },
        ]))
        .unwrap();
        let finding = on_white(check_box, faint, 1.4, 3.0, true);
        let fix = Color::from_rgba8(0x80, 0x80, 0x80, 255);
        let key = ids.mint_item();
        let tx = fix_transaction(&doc, check_box, &finding, Level::Aa, Fix::Colour(fix), key)
            .expect("a fix");
        let doc = after(&doc, &tx);
        let paint = doc.get(check_box).unwrap().paint();
        assert_eq!(
            paint.fills[0].brush,
            Brush::Solid(Color::WHITE).into(),
            "the fill stays white"
        );
        assert_eq!(paint.strokes[0].brush, Brush::Solid(fix).into());
    }
}
