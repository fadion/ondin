//! Text shaping, measurement, and layout for Text nodes (§5.4, §5.4a).
//!
//! Real layout via `parley`. Core never scans system fonts (parley's `system`
//! feature is off): it shapes only against fonts it has been handed, so the engine
//! stays filesystem/network-free and measurement is deterministic.
//!
//! Inter (upright + italic variable) is bundled as the default and fallback; the
//! app supplies other families at runtime via [`register_fonts`].
//!
//! `layout()` returns, per run, the exact [`peniko::FontData`] parley resolved, so
//! `parley` stays inside this crate. Results are cached in [`crate::resolve::Resolved`]
//! (§5.9); `layout()` is the uncached path that fills it.
//!
//! ## What moves text after parley is done with it
//!
//! Paragraph spacing, box trim and vertical alignment shift the ink after the lines
//! are broken. One [`YMap`] (layout-y ↔ local-y) applies at both the scene walk and
//! the hit test, which keeps the caret under the pointer.
//!
//! ## Horizontal paragraph geometry is set before the break
//!
//! Indents change how wide a line may be, so [`break_lines`] sets parley's per-line
//! `line_x` and `line_max_advance` from the paragraph the line falls in
//! ([`Paragraphs`]); the caret reads them as the line's inline coords.
//! `Layout::set_text_indent` is deliberately unused (§15 D163).
//!
//! ## The brush is a run index
//!
//! The brush carries the index of *our* run, so the decoration walk can recover
//! attributes parley cannot hold (a line style, a decoration colour).
//!
//! [`TextEdit`] is the caret/selection model for in-canvas editing; it lives here
//! because cluster boundaries and bidi-aware movement are parley's.

use crate::node::{TextRef, TextSizing};
use crate::typography::{
    AXES_DRIVEN_ELSEWHERE, AxisSetting, BlockStyle, BoxTrim, CharSpans, ITAL, JustifyLast, Length,
    LineStyle, ListMarker, MAX_LIST_LEVEL, OPSZ, ParaAttr, ParaAttrKind, ParaSpans, ParagraphStyle,
    SLNT, Tag, TextAlign, TextCase, TextDirection, TextOverflow, TextStyle, VerticalAlign, WGHT,
};
use kurbo::{
    BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathEl, PathSeg,
    Point, Rect, Shape, Size, Vec2,
};
use peniko::{Color, FontData};
use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::style::{FontFeature, FontVariation, Language};
use parley::{
    Affinity, Alignment, AlignmentOptions, BoundingBox, BreakReason, Cursor, FontContext,
    FontFamily, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext, LineHeight,
    PositionedLayoutItem, Selection, StyleProperty,
};

/// The two bundled Inter variable fonts (OFL, see `assets/fonts/OFL.txt`). One
/// covers the whole upright weight axis, the other italic. Always registered as
/// the default family and the fallback for any unknown `font_family`.
static INTER_UPRIGHT: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
static INTER_ITALIC: &[u8] = include_bytes!("../assets/fonts/Inter-Italic-Variable.ttf");

/// The brush parley carries for us: the index of the run in [`Shaped::runs`].
type RunIndex = u32;

/// The brush the node's defaults are pushed with — a value no style-run index can
/// take.
///
/// `shape` skips pushing a style run equal to the defaults, so those bytes keep this
/// brush; brush `0` would be indistinguishable from style run 0 (§15 D153).
const DEFAULT_RUN: RunIndex = RunIndex::MAX;

/// Register font data (a `.ttf`/`.otf`) with the shaping engine on the current
/// thread, returning the family name(s) it contains.
///
/// Registration is per-thread (the engine is thread-local), so the app registers on
/// the thread it renders and measures on.
pub fn register_fonts(bytes: Vec<u8>) -> Vec<String> {
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let registered = engine
            .font_cx
            .collection
            .register_fonts(Blob::from(bytes), None);
        registered
            .iter()
            .filter_map(|(id, _)| {
                engine
                    .font_cx
                    .collection
                    .family_name(*id)
                    .map(str::to_string)
            })
            .collect()
    })
}

/// Un-register every face of `family` from this thread's shaping engine; returns
/// whether anything was removed.
///
/// The counterpart to [`register_fonts`]: the picker registers a family per row, so
/// without eviction scrolling leaks. The family *name* survives with an empty face
/// list, which is why [`is_family_available`] asks about faces.
pub fn unregister_family(family: &str) -> bool {
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let collection = &mut engine.font_cx.collection;
        let Some(id) = collection.family_id(family) else {
            return false;
        };
        let Some(info) = collection.family(id) else {
            return false;
        };
        // Collected first: `unregister_font` rebuilds the face list on each call.
        let faces: Vec<_> = info
            .fonts()
            .iter()
            .map(|f| (f.width(), f.style(), f.weight()))
            .collect();
        let mut any = false;
        for (width, style, weight) in faces {
            any |= collection.unregister_font(id, width, style, weight);
        }
        any
    })
}

/// Whether `family` is registered with the shaping engine on this thread.
///
/// A document may reference a family the machine lacks; it renders via the Inter
/// fallback and the UI warns (§5.4a). Checks faces, not the name, which
/// `unregister_family` leaves behind empty.
pub fn is_family_available(family: &str) -> bool {
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let collection = &mut engine.font_cx.collection;
        let Some(id) = collection.family_id(family) else {
            return false;
        };
        collection
            .family(id)
            .is_some_and(|info| !info.fonts().is_empty())
    })
}

// ---------------------------------------------------------------------------
// Font metadata — what the Font tab builds itself from
// ---------------------------------------------------------------------------

/// One variable-font axis, as the panel needs to draw a row for it. Read from the
/// font, never from a table of our own.
#[derive(Clone, Debug, PartialEq)]
pub struct FontAxis {
    pub tag: Tag,
    pub min: f64,
    pub default: f64,
    pub max: f64,
    /// The axis's own name from the font's `name` table, falling back to the tag.
    pub name: String,
    /// `fvar`'s hidden flag — an axis the typeface asks not to be shown.
    pub hidden: bool,
}

impl FontAxis {
    /// One axis as a font's `fvar` table states it, with the range made usable.
    ///
    /// `fvar` is validated by nothing, so a reversed or `NaN` range would otherwise
    /// panic `f64::clamp` (also in release) on the next layout.
    fn from_fvar(tag: Tag, min: f64, default: f64, max: f64, name: String, hidden: bool) -> Self {
        let (min, max) = ordered(min, max);
        Self {
            tag,
            min,
            default: default.clamp(min, max),
            max,
            name,
            hidden,
        }
    }

    /// `value` brought inside this axis's range.
    ///
    /// Not `f64::clamp`, which panics on `min > max`; the fields are `pub`.
    pub fn clamp(&self, value: f64) -> f64 {
        value
            .max(self.min.min(self.max))
            .min(self.min.max(self.max))
    }
}

/// `(low, high)` from a pair a font asserted was one, with a non-finite end
/// replaced by the other.
///
/// A reversed pair and a `NaN` end each panic `f64::clamp`, and `NaN` survives
/// `f64::min`/`max`, so ordering alone would not close it.
fn ordered(a: f64, b: f64) -> (f64, f64) {
    match (a.is_finite(), b.is_finite()) {
        (true, true) => (a.min(b), a.max(b)),
        (true, false) => (a, a),
        (false, true) => (b, b),
        (false, false) => (0.0, 0.0),
    }
}

/// One of a variable font's named instances — "Condensed Bold", "Display".
#[derive(Clone, Debug, PartialEq)]
pub struct NamedInstance {
    pub name: String,
    /// The axis values this instance stands for, in user units.
    pub coords: Vec<AxisSetting>,
    /// Whether the face this instance was read from is italic.
    ///
    /// Not derivable from `coords`: a family keeping italics in a second file (as
    /// bundled Inter does) has no `ital` axis, so only the face's style bit says so.
    pub italic: bool,
}

/// The variation axes of `family`'s default face, in `fvar` order.
///
/// Empty for a family with no variable face. Default face only, unlike
/// [`family_named_instances`]: a union across faces would draw rows for axes the
/// shaping face lacks.
pub fn family_axes(family: &str) -> Vec<FontAxis> {
    with_family_font(family, |font| {
        use skrifa::MetadataProvider as _;
        let axes = font.axes();
        axes.iter()
            .map(|axis| {
                let tag = Tag::new(axis.tag().to_be_bytes());
                FontAxis::from_fvar(
                    tag,
                    f64::from(axis.min_value()),
                    f64::from(axis.default_value()),
                    f64::from(axis.max_value()),
                    localized_name(font, axis.name_id()).unwrap_or_else(|| tag.to_string()),
                    axis.is_hidden(),
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The named instances of **every** face in `family`, the default face's first.
///
/// Every face, because a variable family often ships upright and italic as two
/// files and the italic file's `fvar` names the italic cuts. Deduplicated by name
/// and slope, first face winning.
pub fn family_named_instances(family: &str) -> Vec<NamedInstance> {
    let mut out: Vec<NamedInstance> = Vec::new();
    for face in with_family_fonts(family, |font, italic| {
        use skrifa::MetadataProvider as _;
        let axes: Vec<Tag> = font
            .axes()
            .iter()
            .map(|a| Tag::new(a.tag().to_be_bytes()))
            .collect();
        font.named_instances()
            .iter()
            .filter_map(|instance| {
                let name = localized_name(font, instance.subfamily_name_id())?;
                let coords = instance
                    .user_coords()
                    .zip(axes.iter())
                    .map(|(v, tag)| AxisSetting::new(*tag, f64::from(v)))
                    .collect();
                Some(NamedInstance {
                    name,
                    coords,
                    italic,
                })
            })
            .collect::<Vec<_>>()
    }) {
        for instance in face {
            if !out
                .iter()
                .any(|o| o.name == instance.name && o.italic == instance.italic)
            {
                out.push(instance);
            }
        }
    }
    out
}

/// One entry in the family's variant list — what the panel's *variant* dropdown
/// offers. A variable family answers with its `fvar` named instances, a static one
/// with its faces.
#[derive(Clone, Debug, PartialEq)]
pub struct FontVariant {
    pub name: String,
    pub weight: u16,
    pub italic: bool,
    /// The axis values this variant stands for — empty for a static face, where
    /// weight and slope are the whole story.
    pub coords: Vec<AxisSetting>,
}

/// The cuts of `family` a picker should offer.
pub fn family_variants(family: &str) -> Vec<FontVariant> {
    let instances = family_named_instances(family);
    if !instances.is_empty() {
        return instances
            .into_iter()
            .map(|i| {
                // Weight and slope are lifted out of `coords` into the fields that
                // drive their dedicated controls.
                let weight = axis_of(&i.coords, WGHT).unwrap_or(400.0).round() as u16;
                // Italic is an `ital` axis, a pushed `slnt`, or an italic second face.
                let italic = i.italic
                    || axis_of(&i.coords, ITAL).is_some_and(|v| v > 0.5)
                    || axis_of(&i.coords, SLNT).is_some_and(|v| v.abs() > 0.5);
                let coords = i
                    .coords
                    .into_iter()
                    .filter(|a| !AXES_DRIVEN_ELSEWHERE.contains(&a.tag))
                    .collect();
                FontVariant {
                    name: i.name,
                    weight,
                    italic,
                    coords,
                }
            })
            .collect();
    }
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let collection = &mut engine.font_cx.collection;
        let Some(id) = collection.family_id(family) else {
            return Vec::new();
        };
        let Some(info) = collection.family(id) else {
            return Vec::new();
        };
        let mut out: Vec<FontVariant> = info
            .fonts()
            .iter()
            .map(|f| {
                let weight = f.weight().value().round().clamp(1.0, 1000.0) as u16;
                let italic = !matches!(f.style(), parley::style::FontStyle::Normal);
                FontVariant {
                    name: face_name(weight, italic),
                    weight,
                    italic,
                    coords: Vec::new(),
                }
            })
            .collect();
        ordered_cuts(&mut out);
        out
    })
}

/// A static family's cuts in the order the picker draws them, one per cut.
///
/// Uprights before italics, ascending weight, a cut registered twice offered once.
/// Lifted out of [`family_variants`] so it can be tested: no fixture holds a static
/// face.
fn ordered_cuts(out: &mut Vec<FontVariant>) {
    out.sort_by_key(|v| (v.italic, v.weight));
    out.dedup_by(|a, b| a.weight == b.weight && a.italic == b.italic);
}

/// What `family` can do about weight and slope — what *Bold* and *Italic* choose
/// from ([`bold_step`], [`italic_step`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceOffer {
    /// [`family_variants`]: named instances, else the registered faces.
    pub variants: Vec<FontVariant>,
    /// The default face's `wght` axis, when it has one.
    pub wght: Option<FontAxis>,
    /// Whether an `ital` or `slnt` axis slopes the default face.
    pub slope_axis: bool,
}

impl FaceOffer {
    /// Read off the registered faces of `family`. Empty for a family core has
    /// not been handed, which the steps treat as "unknown", not "lacking".
    pub fn of(family: &str) -> Self {
        let axes = family_axes(family);
        FaceOffer {
            variants: family_variants(family),
            wght: axes.iter().find(|a| a.tag == WGHT).cloned(),
            slope_axis: axes.iter().any(|a| a.tag == ITAL || a.tag == SLNT),
        }
    }

    fn is_unknown(&self) -> bool {
        self.variants.is_empty() && self.wght.is_none() && !self.slope_axis
    }
}

/// The weight at which *Bold* reads as on: semibold and up.
pub const BOLD_FROM: u16 = 600;

/// What *Bold* (`Ctrl+B`, the panel's toggle) writes over text at `weight` and
/// `italic`: the face to apply, or `None` when the family has nothing in that
/// direction.
///
/// On at [`BOLD_FROM`] and up, unless `mixed` (a selection of several weights
/// is made bold, never un-bolded). Off steps toward 400 and on toward 700: to
/// the variant of the same slope nearest the target in that direction, else
/// along `wght`. A family core knows nothing of gets the bare weight.
pub fn bold_step(offer: &FaceOffer, weight: u16, italic: bool, mixed: bool) -> Option<FontVariant> {
    let on = weight >= BOLD_FROM && !mixed;
    let target: u16 = if on { 400 } else { 700 };
    let toward = |w: u16| if on { w < weight } else { w > weight };
    let bare = |w: u16| FontVariant {
        name: face_name(w, italic),
        weight: w,
        italic,
        coords: Vec::new(),
    };
    if offer.is_unknown() {
        return Some(bare(target));
    }
    let same_slope = offer.variants.iter().filter(|v| v.italic == italic);
    if let Some(v) = same_slope
        .filter(|v| toward(v.weight))
        .min_by_key(|v| v.weight.abs_diff(target))
    {
        return Some(v.clone());
    }
    let w = offer.wght.as_ref()?.clamp(f64::from(target)).round() as u16;
    toward(w).then(|| bare(w))
}

/// What *Italic* (`Ctrl+I`, the panel's toggle) writes over text at `weight` and
/// `italic`: the face to apply, or `None` when the family has no slope at all.
///
/// Prefers a variant of the other slope, nearest in weight; else the bare flag
/// when an `ital` or `slnt` axis can carry it ([`axis_settings`] leans `slnt`
/// itself). Upright is always reachable. A family core knows nothing of gets the
/// bare flag. Nothing is synthesized: a slanted copy of an upright face is not drawn.
pub fn italic_step(
    offer: &FaceOffer,
    weight: u16,
    italic: bool,
    mixed: bool,
) -> Option<FontVariant> {
    let target = !(italic && !mixed);
    if let Some(v) = offer
        .variants
        .iter()
        .filter(|v| v.italic == target)
        .min_by_key(|v| v.weight.abs_diff(weight))
    {
        return Some(v.clone());
    }
    (!target || offer.slope_axis || offer.is_unknown()).then(|| FontVariant {
        name: face_name(weight, target),
        weight,
        italic: target,
        coords: Vec::new(),
    })
}

fn axis_of(coords: &[AxisSetting], tag: Tag) -> Option<f64> {
    coords.iter().find(|a| a.tag == tag).map(|a| a.value)
}

/// The conventional name for a weight/slope pair — the CSS weight names, which
/// are what a font's own subfamily strings almost always say.
fn face_name(weight: u16, italic: bool) -> String {
    let base = match weight {
        0..=149 => "Thin",
        150..=249 => "Extra Light",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "Semi Bold",
        650..=749 => "Bold",
        750..=849 => "Extra Bold",
        _ => "Black",
    };
    match (base, italic) {
        ("Regular", true) => "Italic".to_string(),
        (b, true) => format!("{b} Italic"),
        (b, false) => b.to_string(),
    }
}

/// One OpenType feature the face offers, plus whatever the font itself says
/// about it.
///
/// Only `ss01`–`ss20` and `cv01`–`cv99` are named by the font (a `FeatureParams`
/// table); every other tag's name belongs to the registry beside the panel. So
/// `None` means "the registry knows", not "unnamed".
#[derive(Clone, Debug, PartialEq)]
pub struct FaceFeature {
    pub tag: Tag,
    /// The font's own UI label (`ssXX`'s `UINameID`, `cvXX`'s `featUiLabelNameID`);
    /// `None` for other tags and NULL tables.
    pub name: Option<String>,
    /// `cvXX`'s `featUiTooltipTextNameID`, for a row's hover.
    pub tooltip: Option<String>,
    /// One entry per named value the `cvXX` feature declares, in value order:
    /// index `k` is `value = k + 1`. Empty for a plain on/off feature.
    ///
    /// Past one entry the feature is a 1..n choice ([`crate::FeatureSetting::value`]).
    pub values: Vec<Option<String>>,
}

/// The OpenType features `family`'s default face actually offers (`GSUB` and
/// `GPOS`), sorted by tag and de-duplicated.
///
/// Reads the default face ([`family_axes`]), so a feature only the italic face
/// carries is not offered; the panel curates what to show.
pub fn family_features(family: &str) -> Vec<FaceFeature> {
    with_family_font(family, |font| {
        use read_fonts::{TableProvider as _, tables::layout::FeatureList};
        let mut out: Vec<FaceFeature> = Vec::new();
        let mut collect = |list: Result<FeatureList<'_>, _>| {
            let Ok(list) = list else { return };
            for record in list.feature_records() {
                let tag = Tag::new(record.feature_tag().to_be_bytes());
                // A feature is listed once per script and language system.
                if out.iter().any(|f| f.tag == tag) {
                    continue;
                }
                out.push(
                    record
                        .feature(list.offset_data())
                        .ok()
                        .and_then(|f| feature_params(font, tag, &f))
                        .unwrap_or_else(|| FaceFeature {
                            tag,
                            name: None,
                            tooltip: None,
                            values: Vec::new(),
                        }),
                );
            }
        };
        collect(font.gsub().and_then(|t| t.feature_list()));
        collect(font.gpos().and_then(|t| t.feature_list()));
        out.sort_unstable_by_key(|f| f.tag);
        out
    })
    .unwrap_or_default()
}

/// The `FeatureParams` strings for a `ssXX` or `cvXX` tag, or `None`.
///
/// The tag gate is load bearing: read-fonts picks the params table from the first
/// two bytes of the tag, so `ssty` would be read as a stylistic set.
fn feature_params(
    font: &skrifa::FontRef<'_>,
    tag: Tag,
    feature: &read_fonts::tables::layout::Feature<'_>,
) -> Option<FaceFeature> {
    use read_fonts::tables::layout::FeatureParams;
    if crate::typography::stylistic_set(tag).is_none()
        && crate::typography::character_variant(tag).is_none()
    {
        return None;
    }
    let params = feature.feature_params()?.ok()?;
    let (name, tooltip, values) = match params {
        FeatureParams::StylisticSet(p) => (feature_string(font, p.ui_name_id()), None, Vec::new()),
        FeatureParams::CharacterVariant(p) => (
            feature_string(font, p.feat_ui_label_name_id()),
            feature_string(font, p.feat_ui_tooltip_text_name_id()),
            named_values(
                font,
                p.first_param_ui_label_name_id(),
                p.num_named_parameters(),
            ),
        ),
        // Unreachable past the tag gate above.
        FeatureParams::Size(_) => return None,
    };
    Some(FaceFeature {
        tag,
        name,
        tooltip,
        values,
    })
}

/// Run `f` against the raw bytes of every face in `family`, the default face
/// first, skipping any that will not load. `f`'s second argument is whether the
/// face is italic, from fontique's `FontInfo::style`.
fn with_family_fonts<T>(
    family: &str,
    mut f: impl FnMut(&skrifa::FontRef<'_>, bool) -> T,
) -> Vec<T> {
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let Some(id) = engine.font_cx.collection.family_id(family) else {
            return Vec::new();
        };
        let Some(info) = engine.font_cx.collection.family(id) else {
            return Vec::new();
        };
        // Copied out first: `load` needs `source_cache` mutably, a sibling of the collection.
        let mut fonts: Vec<_> = info.fonts().to_vec();
        // Moved rather than swapped, so the other faces keep their order.
        if info.default_font_index() < fonts.len() {
            let default = fonts.remove(info.default_font_index());
            fonts.insert(0, default);
        }
        fonts
            .iter()
            .filter_map(|font| {
                let blob = font.load(Some(&mut engine.font_cx.source_cache))?;
                let font_ref = skrifa::FontRef::from_index(blob.as_ref(), font.index()).ok()?;
                let italic = !matches!(font.style(), parley::style::FontStyle::Normal);
                Some(f(&font_ref, italic))
            })
            .collect()
    })
}

/// Run `f` against the raw bytes of `family`'s default face.
///
/// The blob is held only for the call, so callers return owned data.
fn with_family_font<T>(family: &str, f: impl FnOnce(&skrifa::FontRef<'_>) -> T) -> Option<T> {
    ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let collection = &mut engine.font_cx.collection;
        let id = collection.family_id(family)?;
        let info = collection.family(id)?;
        let font = info.default_font()?.clone();
        let blob = font.load(Some(&mut engine.font_cx.source_cache))?;
        let font_ref = skrifa::FontRef::from_index(blob.as_ref(), font.index()).ok()?;
        Some(f(&font_ref))
    })
}

/// The `ParamUILabelNameID` strings of a `cvXX` feature, one per named value.
///
/// The ids run consecutively: `count` of them from `first`, in value order. An
/// entry is `None` independently of its neighbours; the length still says how many
/// values there are.
fn named_values(
    font: &skrifa::FontRef<'_>,
    first: skrifa::string::StringId,
    count: u16,
) -> Vec<Option<String>> {
    (0..count)
        .map(|k| {
            first
                .to_u16()
                .checked_add(k)
                .and_then(|id| feature_string(font, skrifa::string::StringId::new(id)))
        })
        .collect()
}

/// A `FeatureParams` string, or `None` when the font left the id NULL.
///
/// A NULL id is `0`, the copyright notice, so a plain [`localized_name`] would put
/// it in a tooltip. Feature name ids are 256..=32767.
fn feature_string(font: &skrifa::FontRef<'_>, id: skrifa::string::StringId) -> Option<String> {
    if !(256..=32767).contains(&id.to_u16()) {
        return None;
    }
    localized_name(font, id)
}

/// The English (or first) localized string for a `name` table id.
fn localized_name(font: &skrifa::FontRef<'_>, id: skrifa::string::StringId) -> Option<String> {
    use skrifa::MetadataProvider as _;
    let strings = font.localized_strings(id);
    let english = strings
        .clone()
        .find(|s| s.language().is_some_and(|l| l.starts_with("en")));
    let chosen = english.or_else(|| font.localized_strings(id).next())?;
    let name: String = chosen.chars().collect();
    (!name.is_empty()).then_some(name)
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// One positioned glyph in text-local space (origin at the node's local origin,
/// y down; `y` sits on the baseline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// Rotation about this glyph's own origin in radians, clockwise (y-down).
    /// `0.0` for ordinary type, which keeps the backends' one-call fast path; only
    /// type on a path pays for the split ([`bend`]).
    pub rot: f32,
}

/// A run of glyphs sharing one font, size, and variable-axis instance.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    /// The exact font blob (+ face index) parley resolved for this run. A cheap
    /// handle (shared `Blob`); the renderer draws glyphs from it directly.
    pub font: FontData,
    pub font_size: f32,
    /// Normalized variable-font axis coordinates (skrifa convention) for the
    /// instance parley selected.
    pub coords: Vec<i16>,
    pub glyphs: Vec<Glyph>,
    /// This run's own ink, or `None` for the node's fill stack
    /// ([`crate::TextStyle::color`]).
    pub color: Option<Color>,
}

/// One band of underline or strikethrough ink, in text-local space.
///
/// Geometry here, drawing at the render boundary: dashed and dotted styles reuse
/// the stroke dash machinery (§6.3). [`Self::gaps`] is an interval, not a path, so
/// it is computed once per relayout ([`skip_ink`]).
#[derive(Clone, Debug, PartialEq)]
pub struct DecorationInk {
    /// The band: `x0..x1` along the run, `y0..y1` its thickness.
    pub band: Rect,
    pub style: LineStyle,
    /// `None` means the text's own paint, so recolouring the text takes the
    /// decoration with it.
    pub color: Option<Color>,
    /// Where the band is interrupted by the glyph ink it would otherwise cross:
    /// `x0..x1` pairs in the space of [`Self::band`], clamped to it, sorted and
    /// non-overlapping (skip-ink, §5.4, [`skip_ink`]). Empty means continuous —
    /// always so for a strikethrough.
    pub gaps: Vec<(f64, f64)>,
}

/// A shaped text node: its box, the glyph runs to draw, and the decoration ink.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    /// The node's box.
    pub size: Size,
    /// Offset from the node's local origin to the top-left of [`Self::size`].
    ///
    /// Zero unless the box is trimmed (`BoxTrim`). Trimming shrinks the box around
    /// glyphs that keep their coordinates, so toggling it moves no saved text node.
    pub origin: Vec2,
    /// The box before trim — the full line box, leading included, measured from
    /// the local origin. Equal to [`Self::size`] when trim did nothing.
    pub untrimmed: Size,
    pub runs: Vec<GlyphRun>,
    pub decorations: Vec<DecorationInk>,
    /// Whether the content outran its line limit or its box, so the panel can
    /// say so and the ellipsis has somewhere to be.
    pub truncated: bool,
    /// The first line's box height as parley resolved it — what `LineHeight::Auto`
    /// is worth. From `LineMetrics::line_height`, not the block coordinates, which
    /// measure ink and come out smaller (§15 D147).
    pub line_height: f64,
    /// The face's own underline and strikethrough thicknesses on the first run, in
    /// px — `(underline, strikethrough)`, `None` for a node with nothing shaped.
    ///
    /// Seeds `Decoration::thickness`'s absent state ("the font decides"); a zero
    /// seed would erase the underline, since `decoration_path` and [`crossings`]
    /// refuse a zero-height band. Straight off `run.metrics()`, before clamp.
    pub decoration_sizes: Option<(f64, f64)>,
    /// Every line's baseline in text-local space, top to bottom.
    ///
    /// Stored because [`Self::runs`] cannot give it back: a shifted run's glyphs sit
    /// off the baseline, so collecting distinct [`Glyph::y`]s invents one per
    /// superscript. Empty for type on a rail ([`Self::warp`]), where a bent baseline
    /// is not a *y*.
    pub baselines: Vec<f64>,
    /// The rail this layout was bent onto, or `None` for ordinary horizontal type.
    ///
    /// The glyphs are already bent ([`Glyph::rot`]); this is for geometry that must
    /// follow the curve, i.e. [`DecorationInk`], built at the render boundary (§6.3).
    pub warp: Option<PathWarp>,
}

impl TextLayout {
    /// The node's box in its own local space — the rect every bounds, hit-test
    /// and export consumer reads.
    pub fn bounds(&self) -> Rect {
        Rect::from_origin_size(self.origin.to_point(), self.size)
    }

    /// The node's box before trim — the full line box, at the local origin. Only
    /// drawn; every measurement reads [`Self::bounds`]. On a rail it is always equal
    /// to it: a bent box has no looser second rectangle.
    pub fn untrimmed_bounds(&self) -> Rect {
        if self.warp.is_some() {
            return self.bounds();
        }
        Rect::from_origin_size((0.0, 0.0), self.untrimmed)
    }
}

// ---------------------------------------------------------------------------
// Type on a path
// ---------------------------------------------------------------------------

/// The accuracy asked of kurbo's arc length and its inverse, in document units.
///
/// Both are iterative; a hundredth of a unit is far below a pixel at any zoom.
const RAIL_ACCURACY: f64 = 0.01;

/// How finely a warped edge is resampled before it is mapped, in flat units.
///
/// A curvature-driven flattener would return a band's straight edge as one `LineTo`
/// and the underline would cut across a curved rail.
const RAIL_STEP: f64 = 2.0;

/// How far inside the true swept ribbon [`bend`]'s box may fall, in document units.
/// Small because the box feeds `resize_to_handle`'s solver, which turns box noise
/// into artwork movement (`tools::tests::dragging_railed_text_scales_its_rail`).
const BOX_SAG: f64 = 0.005;

/// Selects `bend`'s curvature-aware box sweep; `false` restores the uniform sweep.
const SKETCH: bool = true;

/// The curvature of a path segment at `t`; `PathSeg` does not expose it, and a
/// line's is zero.
fn curvature_at(seg: &PathSeg, t: f64) -> f64 {
    use kurbo::ParamCurveCurvature;
    match seg {
        PathSeg::Line(_) => 0.0,
        PathSeg::Quad(q) => q.curvature(t),
        PathSeg::Cubic(c) => c.curvature(t),
    }
}

/// The map from flat text-local space onto a rail: `x` becomes distance **along**
/// the curve, `y` distance **across** it.
///
/// A glyph is carried rigidly (placed and rotated, never distorted); continuous
/// geometry running along the line, today only [`DecorationInk`], is warped point
/// by point.
#[derive(Clone, Debug, PartialEq)]
pub struct PathWarp {
    /// Each segment with the arc length at its start and its own length;
    /// cumulative, so a lookup is a binary search.
    segs: Vec<(PathSeg, f64, f64)>,
    len: f64,
    /// The arc length flat `x = 0` maps to — alignment along the rail.
    start: f64,
    /// The text-local `y` that maps onto the rail itself: the first line's
    /// baseline. Subtracted inside [`Self::place`] so callers hand it ordinary
    /// text-local coordinates.
    baseline: f64,
    /// Whether the rail closes on itself. An open rail drops text past its end; a
    /// closed one wraps arc length.
    closed: bool,
    /// Traverse the rail backwards ([`TextRef::on_path_flip`]). Reversing negates
    /// the tangent and so the normal: text reverses and changes sides from this one
    /// bool.
    flip: bool,
}

impl PathWarp {
    /// The rail, measured. `None` for a path with no length, which is not a rail.
    ///
    /// The whole warp is built here, alignment included, so the live edit session
    /// and the layout agree where along the rail the text starts. `width` is the
    /// shaped content width — see [`bend`].
    fn of(
        path: &BezPath,
        flip: bool,
        offset: f64,
        baseline: f64,
        align: TextAlign,
        width: f64,
    ) -> Option<Self> {
        let mut segs: Vec<(PathSeg, f64, f64)> = Vec::new();
        let mut len = 0.0;
        for seg in path.segments() {
            let l = seg.arclen(RAIL_ACCURACY);
            // A zero-length segment has no direction and would make the binary search ambiguous.
            if l > 0.0 {
                segs.push((seg, len, l));
                len += l;
            }
        }
        // `shape` skips alignment on a rail, so it is applied here; `Justify` acts as `Start`.
        let start = offset * len
            + match align {
                TextAlign::Center => (len - width) * 0.5,
                TextAlign::End | TextAlign::Right => len - width,
                TextAlign::Start | TextAlign::Left | TextAlign::Justify => 0.0,
            };
        (!segs.is_empty()).then_some(PathWarp {
            segs,
            len,
            start,
            baseline,
            // The last element, not any `ClosePath`: a multi-subpath rail where only
            // the first closes is not a loop.
            closed: matches!(path.elements().last(), Some(PathEl::ClosePath)),
            flip,
        })
    }

    /// The rail measured and nothing on it — no text, alignment or baseline.
    ///
    /// For purely geometric questions, such as the Text tool's click asking how far
    /// along a point is before any text exists.
    pub fn bare(path: &BezPath) -> Option<Self> {
        Self::of(path, false, 0.0, 0.0, TextAlign::Start, 0.0)
    }

    /// The point on the rail where the type starts — where a canvas puts the slide
    /// handle: the curve at flat `x = 0`, not the first glyph's origin. `None` when
    /// the text begins off the end of an open rail.
    pub fn start_point(&self) -> Option<Point> {
        self.place(0.0, self.baseline).map(|(p, _)| p)
    }

    /// The rail's own bounding box — where the layer is when nothing lands on it.
    pub fn bounds(&self) -> Rect {
        self.segs
            .iter()
            .map(|(s, _, _)| s.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap_or_default()
    }

    /// Whether the rail closes on itself, so a stored offset can be normalised into
    /// one lap.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The rail's total length — the measure a line is broken against instead of
    /// the box's width.
    pub fn length(&self) -> f64 {
        self.len
    }

    /// Where flat `(x, y)` lands, and the rail's direction there in radians.
    ///
    /// `(x, y)` is ordinary text-local space, not baseline-relative. `None` when `x`
    /// runs off either end; the caller turns that into [`TextLayout::truncated`].
    pub fn place(&self, x: f64, y: f64) -> Option<(Point, f64)> {
        if !self.fits(x) {
            return None;
        }
        let (seg, t) = self.at(self.start + x);
        let d = self.direction(&seg, t);
        // y-down: the quarter turn taking `(1, 0)` to `(0, 1)`; the wrong sign mirrors the text.
        let n = Vec2::new(-d.y, d.x);
        Some((seg.eval(t) + n * (y - self.baseline), d.atan2()))
    }

    /// Whether flat `x` is somewhere the text may go.
    ///
    /// An open rail drops a glyph whose arc length `start + x` leaves it; a closed
    /// rail wraps, so only text longer than the whole loop outruns it. The closed
    /// bound is `|x|`, not `0..=len`: the box sweep hands `s − start`, negative
    /// before the text begins.
    fn fits(&self, x: f64) -> bool {
        match self.closed {
            true => x.abs() <= self.len,
            false => (0.0..=self.len).contains(&(self.start + x)),
        }
    }

    /// The segment holding arc length `along`, and the parameter within it.
    ///
    /// The flip is applied here so both directions of the map agree which end is which.
    fn at(&self, along: f64) -> (PathSeg, f64) {
        // A closed rail wraps.
        let along = match self.closed {
            true => along.rem_euclid(self.len),
            false => along,
        };
        let s = if self.flip { self.len - along } else { along };
        let i = self
            .segs
            .partition_point(|(_, at, _)| *at <= s)
            .saturating_sub(1);
        let (seg, at, l) = self.segs[i];
        (seg, seg.inv_arclen((s - at).clamp(0.0, l), RAIL_ACCURACY))
    }

    /// The unit direction the *text* runs in at `t` on `seg` — both halves of the
    /// flip, from one sign (see [`Self::flip`]).
    fn direction(&self, seg: &PathSeg, t: f64) -> Vec2 {
        let d = tangent(seg, t);
        if self.flip { -d } else { d }
    }

    /// The inverse of [`Self::place`]: the text-local `(x, y)` that lands nearest
    /// to `p`. The arc length is `subsegment(0..t).arclen()`, not `t × len`.
    /// Unclamped in `x` on purpose; `Cursor::from_point` clamps to the last cluster.
    pub fn nearest(&self, p: Point) -> (f64, f64) {
        let mut best = (f64::MAX, 0.0, self.segs[0].0, 0.0);
        for (seg, at, _) in &self.segs {
            let n = seg.nearest(p, RAIL_ACCURACY);
            if n.distance_sq < best.0 {
                best = (n.distance_sq, *at, *seg, n.t);
            }
        }
        let (_, at, seg, t) = best;
        let s = at + seg.subsegment(0.0..t).arclen(RAIL_ACCURACY);
        let along = if self.flip { self.len - s } else { s };
        // Signed across the rail in the direction `place` offsets by, so the two are inverses.
        let d = self.direction(&seg, t);
        let across = (p - seg.eval(t)).dot(Vec2::new(-d.y, d.x));
        (along - self.start, across + self.baseline)
    }

    /// `x` moved by whole laps of a closed rail into the half-turn nearest the
    /// text's own start.
    ///
    /// [`Self::nearest`] deliberately does not: a caret wants the answer near the
    /// text it is placed in, while an offset for a fresh node wants the plain
    /// distance round the rail, or a half-way click would write `−50%`.
    pub fn shortest_way_round(&self, x: f64) -> f64 {
        match self.closed {
            true => x - (x / self.len).round() * self.len,
            false => x,
        }
    }

    /// A text-local rectangle as **convex quads** following the rail.
    ///
    /// Several quads, because a band on a curve is concave and
    /// `egui::Shape::convex_polygon` misrenders a concave polygon. Empty where the
    /// rectangle is entirely off the end of the rail.
    pub fn warp_rect(&self, r: Rect) -> Vec<[Point; 4]> {
        let steps = (r.width() / RAIL_STEP).ceil().clamp(1.0, 512.0);
        let mut out = Vec::new();
        for i in 0..(steps as usize) {
            let (a, b) = (
                r.x0 + r.width() * (i as f64) / steps,
                r.x0 + r.width() * ((i + 1) as f64) / steps,
            );
            let corners = [(a, r.y0), (b, r.y0), (b, r.y1), (a, r.y1)];
            let mut quad = [Point::ZERO; 4];
            let mut ok = true;
            for (slot, (x, y)) in quad.iter_mut().zip(corners) {
                match self.place(x, y) {
                    Some((p, _)) => *slot = p,
                    None => ok = false,
                }
            }
            if ok {
                out.push(quad);
            }
        }
        out
    }

    /// `path` resampled and mapped onto the rail — for geometry that runs along the
    /// line. Subpaths break wherever the geometry leaves the rail.
    pub fn warp(&self, path: &BezPath) -> BezPath {
        let mut out = BezPath::new();
        let mut cur = Point::ZERO;
        let mut sub_start = Point::ZERO;
        // A mapped point may be `None`, so this cannot be inferred from a `MoveTo`.
        let mut open = false;
        let emit = |p: Point, out: &mut BezPath, open: &mut bool| match self.place(p.x, p.y) {
            Some((q, _)) if *open => out.line_to(q),
            Some((q, _)) => {
                out.move_to(q);
                *open = true;
            }
            None => *open = false,
        };
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    open = false;
                    cur = p;
                    sub_start = p;
                    emit(p, &mut out, &mut open);
                }
                PathEl::ClosePath => {
                    resample(cur, sub_start, &mut |p| emit(p, &mut out, &mut open));
                    if open {
                        out.close_path();
                        open = false;
                    }
                    cur = sub_start;
                }
                _ => {
                    let seg = match *el {
                        PathEl::LineTo(p) => PathSeg::Line(kurbo::Line::new(cur, p)),
                        PathEl::QuadTo(a, p) => PathSeg::Quad(kurbo::QuadBez::new(cur, a, p)),
                        PathEl::CurveTo(a, b, p) => {
                            PathSeg::Cubic(kurbo::CubicBez::new(cur, a, b, p))
                        }
                        _ => unreachable!("MoveTo and ClosePath are handled above"),
                    };
                    // By length rather than curvature (`RAIL_STEP`).
                    let steps = (seg.arclen(RAIL_ACCURACY) / RAIL_STEP).ceil().max(1.0);
                    for i in 1..=(steps as usize) {
                        emit(seg.eval(i as f64 / steps), &mut out, &mut open);
                    }
                    cur = seg.end();
                }
            }
        }
        out
    }
}

/// A straight run from `a` to `b`, resampled by length. The `ClosePath` half of
/// [`PathWarp::warp`], which has no explicit segment to subdivide.
fn resample(a: Point, b: Point, emit: &mut impl FnMut(Point)) {
    let steps = ((b - a).hypot() / RAIL_STEP).ceil().max(1.0);
    for i in 1..=(steps as usize) {
        emit(a.lerp(b, i as f64 / steps));
    }
}

/// The unit tangent of `seg` at `t`.
///
/// kurbo's derivative vanishes at a cusp or a repeated control point (a pen-tool
/// corner) and a zero vector normalizes to `NaN`, so the chord either side stands in.
fn tangent(seg: &PathSeg, t: f64) -> Vec2 {
    let d = match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
    };
    if d.hypot() > 1e-9 {
        return d.normalize();
    }
    let chord = seg.eval((t + 1e-3).min(1.0)) - seg.eval((t - 1e-3).max(0.0));
    if chord.hypot() > 1e-9 {
        chord.normalize()
    } else {
        Vec2::new(1.0, 0.0)
    }
}

/// The warp a node's parts and its shaping ask for, or `None` for flat type.
///
/// Shared by [`extract`] and [`TextEdit`], which answers pointers against the same
/// bend without building a `TextLayout`. The baseline is the first line's.
fn warp_for(parts: TextRef<'_>, shaped: &Shaped) -> Option<PathWarp> {
    let line = shaped.layout.lines().next()?;
    PathWarp::of(
        parts.on_path?,
        parts.on_path_flip,
        parts.on_path_offset,
        shaped.ymap.to_local(f64::from(line.metrics().baseline)),
        parts.paragraph.align,
        f64::from(shaped.layout.width()),
    )
}

/// Bend a finished flat layout onto its rail.
///
/// A pass over the output rather than a mode inside the shaper: everything computes
/// in flat space, then the whole plane is mapped. The `width` passed to
/// [`PathWarp::of`] is the shaped content width, not `layout.size`.
fn bend(layout: &mut TextLayout, warp: PathWarp) {
    // Each glyph is placed by its own midpoint (SVG's rule): rotating about the left
    // edge fans wide glyphs outward on a tight curve. `GlyphRun` carries no advances
    // (§15 D489), so one is read from the next glyph on the same line, across run
    // boundaries; the last glyph of a line borrows its predecessor's. The same-line
    // test matters: under right or centre alignment the next line's first `x` can
    // exceed this line's last.
    let flat: Vec<(usize, usize, Glyph)> = layout
        .runs
        .iter()
        .enumerate()
        .flat_map(|(r, run)| run.glyphs.iter().enumerate().map(move |(g, s)| (r, g, *s)))
        .collect();
    let mut advances: Vec<Vec<f64>> = layout
        .runs
        .iter()
        .map(|run| vec![0.0; run.glyphs.len()])
        .collect();
    let mut last = 0.0_f64;
    for (k, (r, g, this)) in flat.iter().enumerate() {
        let next = flat
            .get(k + 1)
            .filter(|(_, _, n)| n.x > this.x && (n.y - this.y).abs() < 1e-4);
        let a = match next {
            Some((_, _, n)) => f64::from(n.x - this.x),
            None => last,
        };
        last = a;
        advances[*r][*g] = a;
    }

    let before: usize = layout.runs.iter().map(|r| r.glyphs.len()).sum();
    for (run, advances) in layout.runs.iter_mut().zip(&advances) {
        run.glyphs = run
            .glyphs
            .iter()
            .zip(advances)
            .filter_map(|(g, &adv)| {
                let flat = f64::from(g.x);
                let (p, angle) = warp.place(flat + adv * 0.5, f64::from(g.y))?;
                let origin = p - Vec2::from_angle(angle) * (adv * 0.5);
                Some(Glyph {
                    id: g.id,
                    x: origin.x as f32,
                    y: origin.y as f32,
                    rot: angle as f32,
                })
            })
            .collect();
    }
    let after: usize = layout.runs.iter().map(|r| r.glyphs.len()).sum();
    layout.runs.retain(|r| !r.glyphs.is_empty());
    // Text that outran the rail is truncated, reported by the same field.
    layout.truncated |= after < before;

    // The box is the whole rail swept end to end with the flat box's two verticals
    // carried along it; a box over the glyphs would move as the string is typed.
    let top = layout.origin.y;
    let bottom = layout.origin.y + layout.size.height;
    // Swept in arc length so `warp.start` (alignment) stays out of the box.
    let mut bbox: Option<Rect> = None;
    let sample = |s: f64, bbox: &mut Option<Rect>| {
        for v in [top, bottom] {
            if let Some((p, _)) = warp.place(s - warp.start, v) {
                *bbox = Some(match *bbox {
                    Some(b) => b.union_pt(p),
                    None => Rect::from_points(p, p),
                });
            }
        }
    };
    // Behind [`SKETCH`]: every segment is sampled at both ends (cusps), nudged inward
    // because `place` resolves a corner to one segment; inside a segment the step
    // follows curvature, `h = sqrt(8·BOX_SAG/κ)`.
    let reach = (top - warp.baseline)
        .abs()
        .max((bottom - warp.baseline).abs());
    let want: Vec<f64> = warp
        .segs
        .iter()
        .map(|(seg, _, slen)| {
            let kappa = [0.0, 0.25, 0.5, 0.75, 1.0]
                .into_iter()
                .map(|t| curvature_at(seg, t).abs())
                .fold(0.0_f64, f64::max);
            // Offsetting toward the centre of curvature takes a radius `ρ` to `ρ − v`.
            let kappa = kappa / (1.0 - (kappa * reach).min(0.5));
            let h = match kappa > 0.0 {
                true => (8.0 * BOX_SAG / kappa).sqrt(),
                false => f64::INFINITY,
            };
            (slen / h).ceil().max(1.0)
        })
        .collect();
    let total: f64 = want.iter().sum();
    let budget = match total > 4096.0 {
        true => 4096.0 / total,
        false => 1.0,
    };
    if SKETCH {
        for ((_, s0, slen), want) in warp.segs.iter().zip(&want) {
            let n = (want * budget).ceil().max(1.0);
            // Mirrored under the flip (`PathWarp::at`), so `s0` is the segment's end
            // there; read the wrong way, the nudges land in the next segment
            // (`a_flipped_rails_box_clears_the_rail_on_every_side`).
            let a0 = match warp.flip {
                true => warp.len - s0 - slen,
                false => *s0,
            };
            for i in 0..(n as usize) {
                sample(a0 + slen * (i as f64) / n, &mut bbox);
            }
            // Both ends nudged inward, so the result is independent of traversal.
            sample(a0 + slen * 1e-9, &mut bbox);
            sample(a0 + slen * (1.0 - 1e-9), &mut bbox);
        }
        sample(warp.len, &mut bbox);
    } else {
        let steps = (warp.len / RAIL_STEP).ceil().clamp(1.0, 4096.0);
        for i in 0..=(steps as usize) {
            sample(warp.len * (i as f64) / steps, &mut bbox);
        }
    }
    // Floating point can leave a too-short rail unsampled; fall back to the rail's
    // own box, since a node with no box has no handles.
    let b = bbox.unwrap_or_else(|| warp.bounds());
    layout.origin = b.origin().to_vec2();
    layout.size = b.size();
    layout.untrimmed = b.size();
    layout.baselines.clear();
    layout.warp = Some(warp);
}

/// The filled outline of a laid-out text node's glyphs, in the node's own
/// text-local space — the path a text stroke is drawn along and clipped against.
///
/// Built from the cached [`TextLayout`] at the render boundary, only for a node with
/// a visible stroke. Every contour comes back closed, forced if the face did not say so:
/// `geometry::stroke_align_applies` needs a clip with an interior (§15 D145).
pub fn outline(layout: &TextLayout) -> BezPath {
    let mut out = BezPath::new();
    for run in &layout.runs {
        append_run_outline(run, &mut out);
    }
    out
}

/// One run's glyphs as an outline — [`outline`] for a single run.
///
/// For a bent run under a positional brush: a gradient or image is authored in
/// node-local space, and a bent glyph drawn under its own transform would restart
/// the gradient at every letter. A solid brush keeps the cheaper glyph path.
pub fn run_outline(run: &GlyphRun) -> BezPath {
    let mut out = BezPath::new();
    append_run_outline(run, &mut out);
    out
}

/// One run's glyphs, each drawn where it sits.
fn append_run_outline(run: &GlyphRun, out: &mut BezPath) {
    let Some(face) = RunFace::of(run) else {
        return;
    };
    for g in &run.glyphs {
        face.draw(g.id, f64::from(g.x), f64::from(g.y), f64::from(g.rot), out);
    }
}

/// Collects one glyph's outline into a shared [`BezPath`], baseline-mirrored and
/// translated to the caller's glyph origin.
///
/// `down` guards kurbo's assertion that a path starts with a `MoveTo`, and tells
/// [`RunFace::draw`] whether a contour is left to close.
struct Pen<'a> {
    out: &'a mut BezPath,
    dx: f64,
    dy: f64,
    /// `(cos, sin)` of the glyph's rotation, precomputed because [`Self::at`] runs
    /// per control point; `(1.0, 0.0)` keeps flat type on plain arithmetic.
    rot: (f64, f64),
    down: bool,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Point {
        // Mirror about the baseline first; rotating a y-up point then mirroring turns it the wrong way.
        let (x, y) = (f64::from(x), -f64::from(y));
        let (c, s) = self.rot;
        if s == 0.0 && c == 1.0 {
            return Point::new(self.dx + x, self.dy + y);
        }
        Point::new(self.dx + x * c - y * s, self.dy + x * s + y * c)
    }
}

impl skrifa::outline::OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.out.move_to(p);
        self.down = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        if self.down {
            self.out.line_to(p);
        } else {
            self.out.move_to(p);
            self.down = true;
        }
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx, cy), self.at(x, y));
        if self.down {
            self.out.quad_to(c, p);
        } else {
            self.out.move_to(p);
            self.down = true;
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        if self.down {
            self.out.curve_to(c0, c1, p);
        } else {
            self.out.move_to(p);
            self.down = true;
        }
    }

    fn close(&mut self) {
        if self.down {
            self.out.close_path();
            self.down = false;
        }
    }
}

/// How far paragraph spacing, box trim and vertical alignment move each line —
/// layout y **to** the node's local y. One direction only (§15 D166): a paragraph
/// gap has no layout y, so a subtraction cannot invert it; see `Shaped::layout_y`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct YMap {
    /// Constant shift — box trim and vertical alignment, which move everything.
    base: f64,
    /// `(layout y of a paragraph's first line, that line's index, extra space
    /// inserted before it)`, ascending and cumulative. [`Self::to_local`] decides
    /// from the y, [`Self::shift_for_line`] from the index.
    breaks: Vec<(f64, usize, f64)>,
}

impl YMap {
    /// The node's local y for a point in parley's layout space, deciding from the
    /// coordinate.
    ///
    /// Right for a baseline, wrong for an ink coordinate, which can fall on the wrong
    /// side of its break. Use [`Self::to_local_on_line`] when the line is known.
    pub fn to_local(&self, layout_y: f64) -> f64 {
        let mut shift = 0.0;
        for (at, _, s) in &self.breaks {
            if layout_y >= *at {
                shift = *s;
            } else {
                break;
            }
        }
        layout_y + shift + self.base
    }

    /// [`Self::to_local`] for a y known to belong to line `index`.
    pub fn to_local_on_line(&self, layout_y: f64, index: usize) -> f64 {
        layout_y + self.shift_for_line(index) + self.base
    }

    /// The extra space inserted above line `index`.
    ///
    /// A shift belongs to a line, not to a y: `breaks` is keyed on line-box tops, but
    /// `block_min_coord` is an ink top, above the box top under negative leading, so
    /// a caret rectangle would come out one gap too high.
    pub fn shift_for_line(&self, index: usize) -> f64 {
        self.breaks
            .iter()
            .rev()
            .find(|(_, line, _)| *line <= index)
            .map(|(_, _, shift)| *shift)
            .unwrap_or(0.0)
    }

    /// Total extra height the paragraph spacing adds.
    fn total_spacing(&self) -> f64 {
        self.breaks.last().map(|(_, _, s)| *s).unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Per-thread parley resources: the font collection (bundled faces plus whatever
/// the app registered) and reusable layout scratch space.
struct Engine {
    font_cx: FontContext,
    layout_cx: LayoutContext<RunIndex>,
    /// The bundled family name (Inter) — default and last-resort fallback.
    default_family: String,
}

impl Engine {
    fn new() -> Self {
        // `system_fonts: false` backs up the disabled `system` feature.
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let registered = collection.register_fonts(Blob::from(INTER_UPRIGHT.to_vec()), None);
        let default_family = registered
            .first()
            .and_then(|(id, _)| collection.family_name(*id))
            .unwrap_or("Inter")
            .to_string();
        collection.register_fonts(Blob::from(INTER_ITALIC.to_vec()), None);
        Self {
            font_cx: FontContext {
                collection,
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
            default_family,
        }
    }
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
}

// ---------------------------------------------------------------------------
// Model → parley
// ---------------------------------------------------------------------------

/// The concrete alignment a paragraph style asks for.
///
/// `Start`/`End` are resolved here under an explicit direction: parley infers the
/// base direction from content, so an empty RTL paragraph would align the wrong way.
fn parley_alignment(align: TextAlign, direction: TextDirection) -> Alignment {
    match (align, direction) {
        (TextAlign::Left, _) => Alignment::Left,
        (TextAlign::Right, _) => Alignment::Right,
        (TextAlign::Center, _) => Alignment::Center,
        (TextAlign::Justify, _) => Alignment::Justify,
        (TextAlign::Start, TextDirection::Auto) => Alignment::Start,
        (TextAlign::End, TextDirection::Auto) => Alignment::End,
        (TextAlign::Start, TextDirection::Ltr) => Alignment::Left,
        (TextAlign::Start, TextDirection::Rtl) => Alignment::Right,
        (TextAlign::End, TextDirection::Ltr) => Alignment::Right,
        (TextAlign::End, TextDirection::Rtl) => Alignment::Left,
    }
}

fn parley_line_height(style: &TextStyle) -> LineHeight {
    match style.line_height {
        // Auto: ascent + descent + leading, as CSS `normal`.
        None => LineHeight::MetricsRelative(1.0),
        Some(Length::Em(m)) => LineHeight::FontSizeRelative(m as f32),
        Some(Length::Px(v)) => LineHeight::Absolute(v as f32),
    }
}

fn parley_wrap(mode: crate::typography::WrapMode) -> parley::TextWrapMode {
    match mode {
        crate::typography::WrapMode::Wrap => parley::TextWrapMode::Wrap,
        crate::typography::WrapMode::NoWrap => parley::TextWrapMode::NoWrap,
    }
}

fn parley_word_break(b: crate::typography::WordBreak) -> parley::WordBreak {
    match b {
        crate::typography::WordBreak::Normal => parley::WordBreak::Normal,
        crate::typography::WordBreak::BreakAll => parley::WordBreak::BreakAll,
        crate::typography::WordBreak::KeepAll => parley::WordBreak::KeepAll,
    }
}

fn parley_overflow_wrap(o: crate::typography::OverflowWrap) -> parley::OverflowWrap {
    match o {
        crate::typography::OverflowWrap::Normal => parley::OverflowWrap::Normal,
        crate::typography::OverflowWrap::Anywhere => parley::OverflowWrap::Anywhere,
        crate::typography::OverflowWrap::BreakWord => parley::OverflowWrap::BreakWord,
    }
}

fn parley_tag(tag: Tag) -> parley::setting::Tag {
    parley::setting::Tag::from_bytes(tag.to_u32().to_be_bytes())
}

/// The axis settings to shape a run with — the stored list, plus `opsz` when
/// that axis is on Auto.
///
/// The `opsz` synthesis is ours (CSS `font-optical-sizing: auto`; §15 D571). A
/// stored coordinate reaches the shaper raw (§15 D425); the synthesized value is
/// clamped with `axis.clamp`, not `f64::clamp`, which panics on a bad range.
fn axis_settings(style: &TextStyle, axes: &[FontAxis]) -> Vec<FontVariation> {
    let mut out: Vec<FontVariation> = style
        .variations
        .iter()
        .map(|a| FontVariation::new(parley_tag(a.tag), a.value as f32))
        .collect();
    if style.optical_size_is_auto()
        && let Some(axis) = axes.iter().find(|a| a.tag == OPSZ)
    {
        let value = axis.clamp(style.font_size);
        out.push(FontVariation::new(parley_tag(OPSZ), value as f32));
    }
    // Italic on a face that slopes only through `slnt`: fontique's synthesis
    // writes `slnt = +14`, which is a backslant (the axis is counter-clockwise
    // positive) and clamps to 0 on the usual −15…0 range. Written here, after
    // synthesis in parley's order, so it wins; a hand-set `slnt` wins over this.
    if style.italic
        && !axes.iter().any(|a| a.tag == ITAL)
        && !style.variations.iter().any(|a| a.tag == SLNT)
        && let Some(axis) = axes.iter().find(|a| a.tag == SLNT)
    {
        let value = axis.clamp(-ITALIC_SLANT);
        out.push(FontVariation::new(parley_tag(SLNT), value as f32));
    }
    out
}

/// The lean, in degrees, a `slnt`-only face is given for italic — CSS's
/// synthesized oblique, and fontique's.
const ITALIC_SLANT: f64 = 14.0;

/// Push one style's properties onto the builder — as defaults when `range` is
/// `None`, otherwise over that byte range — so the two sets cannot drift.
fn push_style(
    builder: &mut parley::RangedBuilder<'_, RunIndex>,
    style: &TextStyle,
    paragraph: &ParagraphStyle,
    fallback_family: &str,
    axes: &[FontAxis],
    run_index: RunIndex,
    range: Option<Range<usize>>,
) {
    let mut props: Vec<StyleProperty<'_, RunIndex>> = Vec::with_capacity(20);
    // Requested family first, bundled Inter as the fallback.
    props.push(StyleProperty::FontFamily(FontFamily::List(Cow::Owned(
        vec![
            FontFamilyName::named(&style.font_family),
            FontFamilyName::named(fallback_family),
        ],
    ))));
    props.push(StyleProperty::FontSize(style.font_size as f32));
    props.push(StyleProperty::FontWeight(FontWeight::new(
        style.weight as f32,
    )));
    props.push(StyleProperty::FontStyle(if style.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    }));
    props.push(StyleProperty::FontVariations(
        parley::style::FontVariations::List(Cow::Owned(axis_settings(style, axes))),
    ));
    props.push(StyleProperty::FontFeatures(
        parley::style::FontFeatures::List(Cow::Owned(
            style
                .features
                .iter()
                .map(|f| FontFeature::new(parley_tag(f.tag), f.value))
                .collect(),
        )),
    ));
    props.push(StyleProperty::Locale(
        style
            .locale
            .as_deref()
            .and_then(|l| Language::parse(l).ok()),
    ));
    props.push(StyleProperty::LineHeight(parley_line_height(style)));
    props.push(StyleProperty::LetterSpacing(
        style.letter_spacing.resolve(style.font_size) as f32,
    ));
    props.push(StyleProperty::WordSpacing(
        style.word_spacing.resolve(style.font_size) as f32,
    ));
    // The brush and both decoration brushes are the run index (see the module docs).
    props.push(StyleProperty::Brush(run_index));
    props.push(StyleProperty::Underline(style.underline.is_some()));
    props.push(StyleProperty::UnderlineBrush(
        style.underline.map(|_| run_index),
    ));
    props.push(StyleProperty::UnderlineOffset(
        style
            .underline
            .and_then(|d| d.offset)
            .map(|l| l.resolve(style.font_size) as f32),
    ));
    props.push(StyleProperty::UnderlineSize(
        style
            .underline
            .and_then(|d| d.thickness)
            .map(|l| l.resolve(style.font_size) as f32),
    ));
    props.push(StyleProperty::Strikethrough(style.strikethrough.is_some()));
    props.push(StyleProperty::StrikethroughBrush(
        style.strikethrough.map(|_| run_index),
    ));
    props.push(StyleProperty::StrikethroughOffset(
        style
            .strikethrough
            .and_then(|d| d.offset)
            .map(|l| l.resolve(style.font_size) as f32),
    ));
    props.push(StyleProperty::StrikethroughSize(
        style
            .strikethrough
            .and_then(|d| d.thickness)
            .map(|l| l.resolve(style.font_size) as f32),
    ));
    // Wrapping is a paragraph attribute but a per-cluster parley property, so it rides along here.
    props.push(StyleProperty::TextWrapMode(parley_wrap(paragraph.wrap)));
    props.push(StyleProperty::WordBreak(parley_word_break(
        paragraph.word_break,
    )));
    props.push(StyleProperty::OverflowWrap(parley_overflow_wrap(
        paragraph.overflow_wrap,
    )));

    for prop in props {
        match &range {
            None => builder.push_default(prop),
            Some(r) => builder.push(prop, r.clone()),
        }
    }
}

/// The string to hand the shaper, when a case transform is in force.
///
/// Length-preserving only: byte indices are the currency of the span list, caret
/// and hit tests. A character whose cased form changes the byte count (`ß`→`SS`)
/// is left alone. Code export writes this string, so the page draws the letters
/// the canvas shaped.
pub fn cased_text(content: &str, runs: &[(Range<usize>, TextStyle)]) -> Option<String> {
    if runs.iter().all(|(_, s)| s.case == TextCase::Original) {
        return None;
    }
    let mut out = String::with_capacity(content.len());
    let mut prev_alnum = false;
    for (range, style) in runs {
        for c in content[range.clone()].chars() {
            let at_word_start = !prev_alnum;
            prev_alnum = c.is_alphanumeric();
            out.push(case_char(c, style.case, at_word_start));
        }
    }
    debug_assert_eq!(
        out.len(),
        content.len(),
        "the case transform must preserve byte length"
    );
    Some(out)
}

fn case_char(c: char, case: TextCase, at_word_start: bool) -> char {
    let up = match case {
        TextCase::Original => return c,
        TextCase::Upper => true,
        TextCase::Lower => false,
        TextCase::Title => at_word_start,
    };
    let mut cased: Box<dyn Iterator<Item = char>> = if up {
        Box::new(c.to_uppercase())
    } else {
        Box::new(c.to_lowercase())
    };
    match (cased.next(), cased.next()) {
        (Some(m), None) if m.len_utf8() == c.len_utf8() => m,
        // Anything that would change the byte count is left as typed.
        _ => c,
    }
}

// ---------------------------------------------------------------------------
// Shaping
// ---------------------------------------------------------------------------

/// A laid-out text node, before it is flattened to glyph runs.
///
/// Held whole by [`TextEdit`], which needs the parley `Layout` for caret motion
/// and the [`YMap`] to turn a canvas point into a layout one.
struct Shaped {
    layout: Layout<RunIndex>,
    /// Our own runs, indexed by the brush parley carries.
    runs: Vec<(Range<usize>, TextStyle)>,
    ymap: YMap,
    origin: Vec2,
    size: Size,
    /// See [`NodeBox::untrimmed`].
    untrimmed: Size,
    truncated: bool,
}

impl Shaped {
    /// The layout y to hit-test a text-local y with: the middle of the line whose
    /// local box is nearest to it.
    ///
    /// Not a subtracted shift (§15 D166): a point in a paragraph gap has no layout y
    /// and the subtraction walks into the next paragraph. A point in a gap takes the
    /// nearer bounding line, ties to the line above. The box centre is returned
    /// because `Layout::line_for_offset` searches ink ranges that overlap under
    /// negative leading.
    fn layout_y(&self, local_y: f64) -> f64 {
        let mut best: Option<(f64, f64)> = None;
        let mut top = 0.0;
        for (index, line) in self.layout.lines().enumerate() {
            let height = f64::from(line.metrics().line_height);
            let box_top = self.ymap.to_local_on_line(top, index);
            // Zero inside the box, otherwise how far outside it.
            let distance = (box_top - local_y)
                .max(local_y - (box_top + height))
                .max(0.0);
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, top + height / 2.0));
            }
            if distance == 0.0 {
                break;
            }
            top += height;
        }
        // Nothing laid out: the plain inverse of the constant shift.
        best.map_or(local_y - self.ymap.base, |(_, y)| y)
    }
}

fn shape(parts: TextRef<'_>) -> Shaped {
    // The one seam every shaping call goes through (see [`shapes`]).
    SHAPES.with(|n| n.set(n.get() + 1));
    let runs = parts.spans.runs(parts.style, parts.content.len());
    let cased = cased_text(parts.content, &runs);
    let text: &str = cased.as_deref().unwrap_or(parts.content);

    let mut layout = ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let default_family = engine.default_family.clone();
        // Axis metadata is gathered before the builder exists; it borrows the whole engine.
        let mut families: Vec<&str> = vec![parts.style.font_family.as_str()];
        for (_, style) in &runs {
            if !families.contains(&style.font_family.as_str()) {
                families.push(&style.font_family);
            }
        }
        // An unknown family shapes with the fallback face, so it needs that face's
        // axes (`opsz`) or the text measures differently once the family appears (§15 D711).
        if !families.contains(&default_family.as_str()) {
            families.push(default_family.as_str());
        }
        // `known` is not `!axes.is_empty()`: empty axes mean either an unknown family
        // (which inherits the fallback's) or a known static face (which must not be
        // asked for a variation it lacks).
        let axes: Vec<(String, Vec<FontAxis>, bool)> = families
            .iter()
            .map(|f| {
                let known = engine.font_cx.collection.family_id(f).is_some();
                ((*f).to_owned(), family_axes_locked(engine, f), known)
            })
            .collect();
        let fallback_axes: &[FontAxis] = axes
            .iter()
            .find(|(f, _, _)| *f == default_family)
            .map_or(&[], |(_, a, _)| a.as_slice());
        let axes_for = |family: &str| -> &[FontAxis] {
            match axes.iter().find(|(f, _, _)| f == family) {
                Some((_, a, true)) => a.as_slice(),
                _ => fallback_axes,
            }
        };
        let default_axes = axes_for(&parts.style.font_family);

        let mut builder = engine
            .layout_cx
            .ranged_builder(&mut engine.font_cx, text, 1.0, true);
        push_style(
            &mut builder,
            parts.style,
            parts.paragraph,
            &default_family,
            default_axes,
            DEFAULT_RUN,
            None,
        );
        for (i, (range, style)) in runs.iter().enumerate() {
            if style == parts.style {
                continue;
            }
            push_style(
                &mut builder,
                style,
                parts.paragraph,
                &default_family,
                axes_for(&style.font_family),
                i as RunIndex,
                Some(range.clone()),
            );
        }
        builder.build(text)
    });

    // Not `Layout::set_text_indent`: with a per-line `line_x` it shrinks the measure twice.
    let paras = Paragraphs::new(parts);

    // On a rail the box is inert: the path's length is the measure and [`bend`]
    // aligns along it, so parley must not centre inside the box too.
    let on_rail = parts.on_path.is_some();
    let wrap_width = if on_rail {
        None
    } else {
        parts.sizing.wrap_width()
    };
    let height_limit = match (parts.sizing, parts.block.overflow) {
        // A fixed box truncates only when told to clip or ellipsize; `Visible` lets text hang out.
        (TextSizing::Fixed(s), TextOverflow::Clip | TextOverflow::Ellipsis) => Some(s.height),
        _ => None,
    };
    let mut truncated = break_lines(
        &mut layout,
        &paras,
        parts.style.font_size,
        wrap_width,
        parts.block.max_lines,
        height_limit,
        None,
    );
    // Optical margin alignment (§15 D830, D855): a second break pass, only for a node
    // that asked. Every measure grows by its two bearings, so a pass can break
    // somewhere the first did not, and offsets are read per line index; it repeats
    // until every line is corrected by its own bearings, settled from the top (the
    // first mismatch only moves down, which bounds the loop). A line that revisits a
    // guess is pinned (exact left bearing, right zero) to end a cycle; past the cap
    // the node is laid out uncorrected. The auto-width box keeps the uncorrected width.
    let mut natural_width = None;
    if paras.any_optical_margins() {
        natural_width = Some(layout.width());
        let mut used = optical_offsets(&layout);
        let mut pinned = vec![false; used.len()];
        let mut tried: Vec<(usize, (f32, f32))> = Vec::new();
        let mut settled = false;
        for _ in 0..4 + 3 * used.len() {
            truncated = break_lines(
                &mut layout,
                &paras,
                parts.style.font_size,
                wrap_width,
                parts.block.max_lines,
                height_limit,
                Some(&used),
            );
            let found = optical_offsets(&layout);
            let agrees = |i: usize| match (found.get(i), used.get(i)) {
                (Some(f), Some(u)) if pinned.get(i) == Some(&true) => f.0 == u.0,
                (Some(f), Some(u)) => f == u,
                (None, None) => true,
                _ => false,
            };
            let Some(j) = (0..found.len().max(used.len())).find(|&i| !agrees(i)) else {
                settled = true;
                break;
            };
            // Above `j` keep what those lines were given; from `j` down, this pass's readings.
            let mut next = found.clone();
            for (i, slot) in next.iter_mut().enumerate().take(j) {
                if let Some(&u) = used.get(i) {
                    *slot = u;
                }
            }
            pinned.resize(next.len(), false);
            if let Some(&guess) = next.get(j) {
                if tried.contains(&(j, guess)) {
                    next[j] = (guess.0, 0.0);
                    pinned[j] = true;
                } else {
                    tried.push((j, guess));
                }
            }
            used = next;
        }
        if !settled {
            truncated = break_lines(
                &mut layout,
                &paras,
                parts.style.font_size,
                wrap_width,
                parts.block.max_lines,
                height_limit,
                None,
            );
        }
    }

    layout.align(
        // Redundant with the wrap gate above, but both are one decision, "the box is
        // inert on a rail"; removing one double-counts the alignment.
        if on_rail {
            Alignment::Start
        } else {
            parley_alignment(parts.paragraph.align, parts.paragraph.direction)
        },
        AlignmentOptions {
            // `false` start-aligns a line wider than its box: a centred label that outgrew its frame jumps left.
            align_when_overflowing: true,
        },
    );

    let ymap = y_map(&layout, parts, &paras);
    let node_box = box_of(&layout, parts, &ymap, natural_width);
    Shaped {
        layout,
        runs,
        ymap,
        origin: node_box.origin,
        size: node_box.size,
        untrimmed: node_box.untrimmed,
        truncated,
    }
}

/// [`family_axes`] with the engine already borrowed.
fn family_axes_locked(engine: &mut Engine, family: &str) -> Vec<FontAxis> {
    use skrifa::MetadataProvider as _;
    let Some(id) = engine.font_cx.collection.family_id(family) else {
        return Vec::new();
    };
    let Some(info) = engine.font_cx.collection.family(id) else {
        return Vec::new();
    };
    let Some(font) = info.default_font().cloned() else {
        return Vec::new();
    };
    let Some(blob) = font.load(Some(&mut engine.font_cx.source_cache)) else {
        return Vec::new();
    };
    let Ok(font_ref) = skrifa::FontRef::from_index(blob.as_ref(), font.index()) else {
        return Vec::new();
    };
    font_ref
        .axes()
        .iter()
        .map(|axis| {
            let tag = Tag::new(axis.tag().to_be_bytes());
            // The name is the tag here and localized in `family_axes`.
            FontAxis::from_fvar(
                tag,
                f64::from(axis.min_value()),
                f64::from(axis.default_value()),
                f64::from(axis.max_value()),
                tag.to_string(),
                axis.is_hidden(),
            )
        })
        .collect()
}

/// The paragraph style in force in each paragraph of one text node.
///
/// A paragraph is the text between hard breaks ([`BreakReason::Explicit`]). The
/// table is built by scanning the text, since `BreakLines` does not hand back a
/// line's `text_range`; `paragraph_starts_agree_with_parleys_hard_breaks` stops the
/// two drifting.
struct Paragraphs<'a> {
    /// One resolved style per paragraph, or empty when nothing is overridden
    /// (allocation-free for the per-keystroke path).
    styles: Vec<ParagraphStyle>,
    default: &'a ParagraphStyle,
}

impl<'a> Paragraphs<'a> {
    fn new(parts: TextRef<'a>) -> Self {
        let styles = if parts.para_spans.is_empty() {
            Vec::new()
        } else {
            let last_byte = parts.content.len().saturating_sub(1);
            paragraph_starts(parts.content)
                .into_iter()
                // Clamped so a trailing hard break's empty paragraph (no bytes of its
                // own) inherits from the left: Enter at a paragraph's end carries its indent.
                .map(|start| {
                    parts
                        .para_spans
                        .resolve(parts.paragraph, start.min(last_byte))
                })
                .collect()
        };
        Paragraphs {
            styles,
            default: parts.paragraph,
        }
    }

    /// The style of paragraph `index`; past the end is the node default.
    fn style(&self, index: usize) -> &ParagraphStyle {
        self.styles.get(index).unwrap_or(self.default)
    }

    /// Whether any paragraph asks for optical margin alignment — the gate on the
    /// second break pass. The second clause is inert today;
    /// `no_paragraph_attribute_reaches_optical_margins` makes the day it is not a
    /// compile error.
    fn any_optical_margins(&self) -> bool {
        self.default.optical_margins || self.styles.iter().any(|s| s.optical_margins)
    }
}

/// The byte each paragraph starts at, in content order. Never empty — an empty
/// node is one empty paragraph.
///
/// The break set is parley's: `\n`, `\r`, U+2028 and U+2029. `\r\n` is two breaks
/// with an empty paragraph between, because parley clusters the CR and LF separately.
fn paragraph_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0_usize];
    for (at, c) in text.char_indices() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            starts.push(at + c.len_utf8());
        }
    }
    starts
}

/// The byte range of every paragraph that `range` touches, snapped outward to
/// hard breaks — what a paragraph-scoped write acts on. `pub` so the panel and
/// [`TextEdit`] share the hard-break set.
///
/// - A selection ending exactly on a boundary does not reach the paragraph after
///   it; a bare caret in the same position does.
/// - A paragraph with no bytes of its own (after a trailing newline) reaches back
///   over its own break, or [`crate::typography::Spans::set`] would no-op and the
///   control would be dead.
pub fn paragraph_bounds(content: &str, range: Range<usize>) -> Range<usize> {
    let len = content.len();
    let starts = paragraph_starts(content);
    let begin = starts
        .iter()
        .rev()
        .find(|s| **s <= range.start.min(len))
        .copied()
        .unwrap_or(0);
    // The last byte covered: a caret covers none and belongs to its paragraph.
    let last = if range.is_empty() {
        range.start.min(len)
    } else {
        range.end.min(len).saturating_sub(1)
    };
    let end = starts.iter().find(|s| **s > last).copied().unwrap_or(len);
    if begin < end {
        return begin..end;
    }
    // An empty trailing paragraph. `next_back` rather than `begin - 1`: the break
    // may be the three-byte U+2028.
    let back = content[..begin]
        .chars()
        .next_back()
        .map(char::len_utf8)
        .unwrap_or(0);
    begin - back..end
}

/// Which paragraphs `range` touches, as indices into [`paragraph_starts`].
///
/// [`paragraph_bounds`]' rule in index space: an empty trailing paragraph shares its
/// break byte with the one above, so a loop intersecting byte ranges would touch
/// both. `the_paragraphs_touched_are_the_ones_paragraph_bounds_spans` pins the agreement.
fn paragraphs_touched(content: &str, range: Range<usize>) -> Range<usize> {
    let starts = paragraph_starts(content);
    let len = content.len();
    // There is always at least one start, so the subtraction cannot underflow.
    let index_of = |byte: usize| starts.partition_point(|s| *s <= byte).saturating_sub(1);
    // The last byte covered, as in `paragraph_bounds`.
    let last = if range.is_empty() {
        range.start.min(len)
    } else {
        range.end.min(len).saturating_sub(1)
    };
    index_of(range.start.min(len))..index_of(last) + 1
}

/// Where one line starts and how much measure it gets: `(line_x, line_max_advance)`.
/// Per *line* because the first-line indent lands on one line of the paragraph and
/// [`ParagraphStyle::hanging`] chooses which.
fn line_geometry(
    style: &ParagraphStyle,
    font_size: f64,
    max_advance: f32,
    first_line: bool,
) -> (f32, f32) {
    // `start_edge`, not `indent_start`, so a nested item's lines and marker share one function.
    let start = style.start_edge(font_size);
    let end = style.indent_end.resolve(font_size);
    // The indent lands on the paragraph's opening line; `hanging` moves it to the others.
    let extra = if first_line != style.hanging {
        style.indent.resolve(font_size)
    } else {
        0.0
    };
    let x = (start + extra) as f32;
    // The right edge stays at `max_advance - end`, so a negative first-line indent
    // hangs into the margin. Clamped at zero for indents wider than the box.
    let measure = (max_advance - end as f32 - x).max(0.0);
    (x, measure)
}

/// Each line's left and right side bearings, in layout px (§15 D830).
///
/// Read off an already-broken layout, hence the second break pass. A glyph with no
/// ink contributes nothing: a line ending in a space would hang a whole advance past
/// the measure. Blank lines give zero.
fn optical_offsets(layout: &Layout<RunIndex>) -> Vec<(f32, f32)> {
    use skrifa::MetadataProvider as _;
    /// A glyph's ink extent at its own position, or `None` for an empty one.
    fn inked(glyph_run: &parley::GlyphRun<'_, RunIndex>, g: parley::Glyph) -> Option<Rect> {
        let run = glyph_run.run();
        let font = run.font().clone();
        let ff = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
        let face = RunFace {
            glyphs: ff.outline_glyphs(),
            coords: run
                .normalized_coords()
                .iter()
                .map(|c| skrifa::instance::NormalizedCoord::from_bits(*c))
                .collect(),
            size: skrifa::instance::Size::new(run.font_size()),
        };
        let mut path = BezPath::new();
        face.draw(g.id, f64::from(g.x), 0.0, 0.0, &mut path);
        let b = path.bounding_box();
        (b.width() > 0.0 && b.height() > 0.0).then_some(b)
    }
    let mut out = Vec::new();
    for line in layout.lines() {
        let runs: Vec<_> = line
            .items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => Some(glyph_run),
                _ => None,
            })
            .collect();
        // The first inked glyph's left bearing, walking forward.
        let lsb = runs
            .iter()
            .find_map(|r| {
                r.positioned_glyphs()
                    .find_map(|g| inked(r, g).map(|b| (b.x0 - f64::from(g.x)) as f32))
            })
            .unwrap_or(0.0);
        // The last inked glyph's right bearing, walking back (positions only are collected).
        let rsb =
            runs.iter()
                .rev()
                .find_map(|r| {
                    let glyphs: Vec<_> = r.positioned_glyphs().collect();
                    glyphs.into_iter().rev().find_map(|g| {
                        inked(r, g).map(|b| (f64::from(g.x + g.advance) - b.x1) as f32)
                    })
                })
                .unwrap_or(0.0);
        out.push((lsb, rsb));
    }
    out
}

/// Break the lines, giving each the measure its paragraph asks for and stopping
/// early at a line limit or a box height.
///
/// Returns whether anything was left unplaced. Incremental rather than
/// `break_all_lines`: a limit is honoured by [`parley::BreakLines::revert`]ing the
/// line that went too far, and per-paragraph geometry must be set before each
/// `break_next`. Hyphenation, widow/orphan control, tab stops and columns would live
/// here (declined on cost, §15 D399, §15 D825).
fn break_lines(
    layout: &mut Layout<RunIndex>,
    paras: &Paragraphs<'_>,
    font_size: f64,
    wrap_width: Option<f64>,
    max_lines: u32,
    height_limit: Option<f64>,
    optical: Option<&[(f32, f32)]>,
) -> bool {
    let max_advance = wrap_width.map(|w| w as f32).unwrap_or(f32::MAX);
    let mut breaker = layout.break_lines();
    // `INFINITY`, not the wrap width: `break_next` asserts the measure is within 1.0
    // of it, so an outdented first line would panic.
    breaker.state_mut().set_layout_max_advance(f32::INFINITY);
    let mut lines = 0_u32;
    let mut truncated = false;
    let mut paragraph = 0_usize;
    let mut first_line = true;
    loop {
        let (mut x, mut measure) =
            line_geometry(paras.style(paragraph), font_size, max_advance, first_line);
        // Optical margins: start the line a left bearing earlier and widen its
        // measure by both bearings, so every alignment puts the ink on the nominal edges.
        if let Some(offs) = optical
            && paras.style(paragraph).optical_margins
            && let Some(&(lsb, rsb)) = offs.get(lines as usize)
        {
            x -= lsb;
            measure += lsb + rsb;
        }
        breaker.state_mut().set_line_x(x);
        breaker.state_mut().set_line_max_advance(measure);
        let Some(yielded) = breaker.break_next() else {
            break;
        };
        let reason = match yielded {
            parley::YieldData::LineBreak(data) => data.reason,
            // Cannot arise (no `line_max_height`, no inline boxes); resuming would loop forever.
            _ => break,
        };
        lines += 1;
        let too_tall = height_limit.is_some_and(|h| breaker.committed_y() > h);
        // Never revert the first line: an empty layout would lose the caret.
        if too_tall && lines > 1 {
            breaker.revert();
            truncated = true;
            break;
        }
        if max_lines > 0 && lines >= max_lines {
            truncated = !breaker.is_done();
            break;
        }
        if reason == BreakReason::Explicit {
            paragraph += 1;
            first_line = true;
        } else {
            first_line = false;
        }
    }
    breaker.finish();
    truncated
}

/// The whole layout-y ↔ local-y translation: the paragraph-spacing table plus the
/// constant shift box trim and vertical alignment ask for.
///
/// Stored on the shaped layout because the draw and hit-test boundaries both read it
/// (`a_click_in_a_bottom_aligned_box_lands_on_the_line_under_the_pointer`).
fn y_map(layout: &Layout<RunIndex>, parts: TextRef<'_>, paras: &Paragraphs<'_>) -> YMap {
    let mut breaks = Vec::new();
    let mut cumulative = 0.0;
    // A running sum of line heights, not `block_min_coord` (ink extent), which would key the shift early.
    let mut top = 0.0_f64;
    let mut previous_was_hard_break = false;
    let mut paragraph = 0_usize;
    for (index, line) in layout.lines().enumerate() {
        if previous_was_hard_break {
            paragraph += 1;
            // The paragraph below the break owns the gap: `spacing` is space-before.
            let spacing = paras
                .style(paragraph)
                .spacing
                .resolve(parts.style.font_size)
                .max(0.0);
            if spacing > 0.0 {
                cumulative += spacing;
                breaks.push((top, index, cumulative));
            }
        }
        previous_was_hard_break = line.break_reason() == BreakReason::Explicit;
        top += f64::from(line.metrics().line_height);
    }
    let total_spacing = breaks.last().map(|(_, _, s)| *s).unwrap_or(0.0);
    let (trim_top, trim_bottom) = trim_insets(layout, parts.block.trim);
    let content_height = f64::from(layout.height()) + total_spacing;
    let visible_height = (content_height - trim_top - trim_bottom).max(0.0);
    let base = match *parts.sizing {
        // `TextLayout::origin` carries the inset, so trim moves no ink.
        TextSizing::Auto | TextSizing::AutoHeight(_) => 0.0,
        TextSizing::Fixed(size) => vertical_shift(
            parts.block.vertical_align,
            size.height,
            visible_height,
            trim_top,
        ),
    };
    YMap { base, breaks }
}

/// What [`box_of`] answers.
struct NodeBox {
    origin: Vec2,
    size: Size,
    /// The box before trim, always at the local origin. Equal to `size` exactly when
    /// trim did nothing, including every `Fixed` box (§15 D78); the canvas tests
    /// that equality before drawing a second outline.
    untrimmed: Size,
}

/// The node's box and its offset from the local origin.
///
/// `natural_width` is the layout's width before optical margin alignment shifted
/// any line: an auto-width box is sized by it, so the box never moves.
fn box_of(
    layout: &Layout<RunIndex>,
    parts: TextRef<'_>,
    ymap: &YMap,
    natural_width: Option<f32>,
) -> NodeBox {
    let content_height = f64::from(layout.height()) + ymap.total_spacing();
    let content_width = f64::from(natural_width.unwrap_or_else(|| layout.width()));
    let (trim_top, trim_bottom) = trim_insets(layout, parts.block.trim);
    let visible_height = (content_height - trim_top - trim_bottom).max(0.0);

    match *parts.sizing {
        // Trimming tightens an emergent box around ink that does not move.
        TextSizing::Auto => NodeBox {
            origin: Vec2::new(0.0, trim_top),
            size: Size::new(content_width, visible_height),
            untrimmed: Size::new(content_width, content_height),
        },
        TextSizing::AutoHeight(w) => NodeBox {
            origin: Vec2::new(0.0, trim_top),
            size: Size::new(w, visible_height),
            untrimmed: Size::new(w, content_height),
        },
        // Trim only changes the datum vertical alignment measures.
        TextSizing::Fixed(size) => NodeBox {
            origin: Vec2::ZERO,
            size,
            untrimmed: size,
        },
    }
}

/// The shift that puts the (possibly trimmed) text where vertical alignment asks
/// for it inside a box of `height`.
fn vertical_shift(
    align: VerticalAlign,
    box_height: f64,
    visible_height: f64,
    trim_top: f64,
) -> f64 {
    let free = box_height - visible_height;
    match align {
        VerticalAlign::Top => -trim_top,
        VerticalAlign::Middle => free / 2.0 - trim_top,
        VerticalAlign::Bottom => free - trim_top,
    }
}

/// How far the trimmed box is inset from the top and bottom of the line box.
///
/// Reads the first line's rise and the last line's drop only.
fn trim_insets(layout: &Layout<RunIndex>, trim: BoxTrim) -> (f64, f64) {
    if trim == BoxTrim::None {
        return (0.0, 0.0);
    }
    let first = layout.lines().next();
    let last = layout.lines().last();
    let (Some(first), Some(last)) = (first, last) else {
        return (0.0, 0.0);
    };
    // Per-run metrics, because a line mixing two faces takes the tallest.
    let rise = first
        .runs()
        .map(|run| {
            let m = run.metrics();
            match trim {
                BoxTrim::CapToBaseline => f64::from(m.cap_height.unwrap_or(m.ascent)),
                _ => f64::from(m.ascent),
            }
        })
        .fold(0.0_f64, f64::max);
    let drop = match trim {
        BoxTrim::CapToBaseline => 0.0,
        _ => last
            .runs()
            .map(|run| f64::from(run.metrics().descent))
            .fold(0.0_f64, f64::max),
    };
    let top = (f64::from(first.metrics().baseline) - rise).max(0.0);
    let bottom = (f64::from(layout.height()) - f64::from(last.metrics().baseline) - drop).max(0.0);
    (top, bottom)
}

// ---------------------------------------------------------------------------
// parley → renderable
// ---------------------------------------------------------------------------

/// One run of a text node as an exporter needs it: the characters and their style.
///
/// It owns its substring because the characters are already case-transformed.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportRun {
    /// The characters, case-transformed as the canvas drew them.
    pub text: String,
    /// The style these characters carry.
    pub style: TextStyle,
}

/// One line of a text node, as an exporter needs to place it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportLine {
    /// The line's left edge in text-local space, with alignment, indent and
    /// justification already in it.
    pub x: f64,
    /// The line's baseline in text-local space.
    pub baseline: f64,
    /// Its runs, in logical order.
    pub runs: Vec<ExportRun>,
}

/// A text node's content as the shaper broke it: one [`ExportLine`] per line, each
/// carrying its runs.
///
/// For the exporters, not the render path: SVG needs one `<text>` per line with one
/// `<tspan>` per run (§15 D81), which the cached [`TextLayout`] cannot give (glyph
/// ids do not map back to characters). [`cased_text`] preserves byte length, so the
/// cased text and `content` share one index space.
pub fn export_lines(parts: TextRef<'_>) -> Vec<ExportLine> {
    let shaped = shape(parts);
    // Recomputed rather than carried in `Shaped`, keeping the allocation off the cached path.
    let spans = parts.spans.runs(parts.style, parts.content.len());
    let cased = cased_text(parts.content, &spans);
    let text: &str = cased.as_deref().unwrap_or(parts.content);

    let ymap = &shaped.ymap;
    let justify = (parts.paragraph.align == TextAlign::Justify)
        .then_some(parts.paragraph.justify_last)
        .filter(|j| *j != JustifyLast::Start);
    let line_count = shaped.layout.len();
    let markers = ListMarkers::of(parts);
    let mut paragraph = 0_usize;
    let mut first_line = true;
    let mut lines: Vec<ExportLine> = Vec::new();
    for (index, line) in shaped.layout.lines().enumerate() {
        // A marker is a line of its own: tspans flow, so one inside the line would
        // sit against the first word. Its x is the left edge (`ListMarkers::at` gives the right).
        if let Some(markers) = markers.as_ref().filter(|_| first_line)
            && let Some((text, style, right)) = markers.at(paragraph)
        {
            lines.push(ExportLine {
                x: right - marker_advance(&style, &text),
                baseline: ymap.to_local(f64::from(line.metrics().baseline)),
                runs: vec![ExportRun { text, style }],
            });
        }
        if line.break_reason() == BreakReason::Explicit {
            paragraph += 1;
            first_line = true;
        } else {
            first_line = false;
        }
        // The same justification shift `extract` applies to a justified paragraph's last line.
        let dx = justify
            .filter(|_| index + 1 == line_count || line.break_reason() == BreakReason::Explicit)
            .map(|j| justify_shift(&line, j))
            .unwrap_or(0.0);
        // The leftmost glyph run is the line's left edge; `offset` includes `Layout::align`.
        let x = line
            .items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(g) => Some(f64::from(g.offset())),
                _ => None,
            })
            .fold(f64::INFINITY, f64::min);
        if !x.is_finite() {
            continue;
        }
        let span = line.text_range();
        let mut runs: Vec<ExportRun> = Vec::new();
        for (range, style) in &spans {
            let piece = range.start.max(span.start)..range.end.min(span.end);
            if piece.start >= piece.end {
                continue;
            }
            // Guarded rather than indexed: a non-char-edge boundary would panic.
            let Some(slice) = text.get(piece) else {
                continue;
            };
            runs.push(ExportRun {
                text: slice.to_owned(),
                style: style.clone(),
            });
        }
        if runs.is_empty() {
            continue;
        }
        lines.push(ExportLine {
            x: x + dx,
            baseline: ymap.to_local(f64::from(line.metrics().baseline)),
            runs,
        });
    }
    // The truncation ellipsis is appended to the last line's runs, not placed: tspans
    // flow. Its style is the one at the last visible byte, as in `extract`.
    if shaped.truncated
        && parts.block.overflow == TextOverflow::Ellipsis
        && let Some(last) = lines.last_mut()
        && let Some(visible_end) = shaped.layout.lines().last().map(|l| l.text_range().end)
    {
        let style = spans
            .iter()
            .rev()
            .find(|(r, _)| r.start < visible_end)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| parts.style.clone());
        last.runs.push(ExportRun {
            text: "…".into(),
            style,
        });
    }
    lines
}

/// Flatten a shaped layout into the box, glyph runs and decoration ink the
/// renderer draws.
fn extract(shaped: &Shaped, parts: TextRef<'_>) -> TextLayout {
    // The one map, as the hit test reads it (see [`y_map`]).
    let ymap = &shaped.ymap;

    let justify = (parts.paragraph.align == TextAlign::Justify)
        .then_some(parts.paragraph.justify_last)
        .filter(|j| *j != JustifyLast::Start);

    let mut runs: Vec<GlyphRun> = Vec::new();
    let mut decorations: Vec<DecorationInk> = Vec::new();
    // Which bands are underlines, in push order — `skip_ink` cannot recover that from geometry.
    let mut underlines: Vec<usize> = Vec::new();
    let mut last_line_end: Option<(f64, f64, usize)> = None;
    // Where the last line's ink starts in `runs` and `decorations`, and how far left
    // it reaches: the ellipsis makes room by moving that line (§15 D610).
    let mut last_line_from = (usize::MAX, usize::MAX, f64::INFINITY);
    // Taken here because the `decoration_ink` calls below run only for decorations that exist.
    let mut decoration_sizes: Option<(f64, f64)> = None;

    // List markers ride this walk (a third `Explicit` counter would be a third thing
    // to keep in agreement, §15 D163); shaping happens afterwards.
    let markers = ListMarkers::of(parts);
    let mut paragraph_heads: Vec<(usize, f64)> = Vec::new();
    let mut paragraph = 0_usize;
    let mut first_line = true;

    let line_count = shaped.layout.len();
    let mut baselines: Vec<f64> = Vec::with_capacity(line_count);
    for (index, line) in shaped.layout.lines().enumerate() {
        // A baseline is the one y `to_local` may be handed (§15 D166).
        let baseline = ymap.to_local(f64::from(line.metrics().baseline));
        baselines.push(baseline);
        if markers.is_some() && first_line {
            paragraph_heads.push((paragraph, baseline));
        }
        if line.break_reason() == BreakReason::Explicit {
            paragraph += 1;
            first_line = true;
        } else {
            first_line = false;
        }
        let dx = justify
            .filter(|_| index + 1 == line_count || line.break_reason() == BreakReason::Explicit)
            .map(|j| justify_shift(&line, j))
            .unwrap_or(0.0);
        if index + 1 == line_count {
            last_line_from = (runs.len(), decorations.len(), f64::INFINITY);
        }
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            decoration_sizes.get_or_insert_with(|| {
                let m = run.metrics();
                (f64::from(m.underline_size), f64::from(m.strikethrough_size))
            });
            // `DEFAULT_RUN` is out of range on purpose: it resolves to the defaults.
            let style = shaped
                .runs
                .get(glyph_run.style().brush as usize)
                .map(|(_, s)| s)
                .unwrap_or(parts.style);
            // Baseline shift is a draw offset, not a metric, so the line box does not grow.
            let shift = style.baseline_shift.resolve(style.font_size);
            let baseline = ymap.to_local(f64::from(glyph_run.baseline())) - shift;
            let glyphs: Vec<Glyph> = glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x + dx as f32,
                    // Re-based on the mapped baseline, which moves the whole run.
                    y: (f64::from(g.y) - f64::from(glyph_run.baseline()) + baseline) as f32,
                    // Flat here; only `bend` writes this.
                    rot: 0.0,
                })
                .collect();
            let x0 = f64::from(glyph_run.offset()) + dx;
            let x1 = x0 + f64::from(glyph_run.advance());
            if let Some(ink) = decoration_ink(
                glyph_run.style().underline.as_ref(),
                style.underline.as_ref(),
                run.metrics().underline_offset,
                run.metrics().underline_size,
                baseline,
                x0,
                x1,
                style.color,
            ) {
                // Skip-ink off means not listing the band (§15 D357).
                if style.underline.as_ref().is_some_and(|d| d.skip_ink) {
                    underlines.push(decorations.len());
                }
                decorations.push(ink);
            }
            if let Some(ink) = decoration_ink(
                glyph_run.style().strikethrough.as_ref(),
                style.strikethrough.as_ref(),
                run.metrics().strikethrough_offset,
                run.metrics().strikethrough_size,
                baseline,
                x0,
                x1,
                style.color,
            ) {
                decorations.push(ink);
            }
            if index + 1 == line_count {
                // The third field is the last visible byte, which says whose type the
                // ellipsis is set in. `line.text_range()`, not `run.text_range()`: a
                // parley run spans every line it was broken over.
                last_line_end = Some((x1, baseline, line.text_range().end));
                last_line_from.2 = last_line_from.2.min(x0);
            }
            runs.push(GlyphRun {
                font: run.font().clone(),
                font_size: run.font_size(),
                coords: run.normalized_coords().to_vec(),
                glyphs,
                color: style.color,
            });
        }
    }

    // Taken before the markers are appended: the ellipsis shift must not move one.
    let last_line_to = runs.len();

    if let Some(markers) = &markers {
        for (paragraph, baseline) in paragraph_heads {
            let Some((text, style, right)) = markers.at(paragraph) else {
                continue;
            };
            if let Some(run) = marker_run(&style, &text, right, baseline) {
                runs.push(run);
            }
        }
    }

    if shaped.truncated
        && parts.block.overflow == TextOverflow::Ellipsis
        && let Some((x, baseline, visible_end)) = last_line_end
    {
        // The style at the last drawn byte (§15 D490): a span in the invisible tail
        // must not restyle the ellipsis. Not containment: that byte can fall in a gap
        // between spans.
        let style = shaped
            .runs
            .iter()
            .rev()
            .find(|(r, _)| r.start < visible_end)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| parts.style.clone());
        // The line makes room for the ellipsis (§15 D610): clamping the glyph's x alone
        // pushed the `…` over the last characters under `Center` and `End`. `Start`
        // ends short of the box, so nothing moves. The ellipsis goes at the right
        // edge even for an RTL line (not fixed).
        let mut at = x;
        if let Some(w) = parts.sizing.wrap_width() {
            let advance = marker_advance(&style, ELLIPSIS);
            // Never past the line's own left edge, or a narrow box drags the ink out of the node.
            let over = (x + advance - w).min(last_line_from.2).max(0.0);
            if over > 0.0 {
                let (from_run, from_decor, _) = last_line_from;
                for run in &mut runs[from_run..last_line_to] {
                    for g in &mut run.glyphs {
                        g.x -= over as f32;
                    }
                }
                // `skip_ink` runs below this block; above it the gaps would be stale by `over`.
                for ink in &mut decorations[from_decor..] {
                    let b = ink.band;
                    ink.band = Rect::new(b.x0 - over, b.y0, b.x1 - over, b.y1);
                }
                at = x - over;
            }
        }
        if let Some(run) = ellipsis_run(&style, at, baseline) {
            runs.push(run);
        }
    }

    // After the markers and the ellipsis, which are ink too.
    skip_ink(&runs, &mut decorations, &underlines);

    let mut out = TextLayout {
        size: shaped.size,
        origin: shaped.origin,
        untrimmed: shaped.untrimmed,
        runs,
        decorations,
        truncated: shaped.truncated,
        // Zero for an empty node, which the panel reads as "nothing resolved".
        line_height: shaped
            .layout
            .lines()
            .next()
            .map_or(0.0, |l| f64::from(l.metrics().line_height)),
        decoration_sizes,
        baselines,
        warp: None,
    };
    // Last, and after `skip_ink` on purpose: the pipeline stays flat until here (see [`bend`]).
    if let Some(warp) = warp_for(parts, shaped) {
        bend(&mut out, warp);
    }
    out
}

/// How far a justified paragraph's last line moves; it is translated, not re-spaced.
fn justify_shift(line: &parley::Line<'_, RunIndex>, j: JustifyLast) -> f64 {
    let m = line.metrics();
    let width = f64::from(m.inline_max_coord - m.inline_min_coord);
    let ink = f64::from(m.advance - m.trailing_whitespace);
    let free = width - ink;
    if free <= 0.0 {
        return 0.0;
    }
    match j {
        JustifyLast::Start => 0.0,
        JustifyLast::Center => free / 2.0,
        JustifyLast::End => free,
    }
}

/// One decoration band, or `None` where the run carries no decoration.
///
/// `parley` supplies the geometry; our own attribute supplies the line style and
/// colour: the decoration's own, else `run_color`, else `None` for the fill stack.
#[allow(clippy::too_many_arguments)]
fn decoration_ink(
    resolved: Option<&parley::Decoration<RunIndex>>,
    ours: Option<&crate::typography::Decoration>,
    metric_offset: f32,
    metric_size: f32,
    baseline: f64,
    x0: f64,
    x1: f64,
    run_color: Option<Color>,
) -> Option<DecorationInk> {
    let resolved = resolved?;
    let ours = ours?;
    // Offsets are measured from the baseline upwards (parley's convention).
    let offset = f64::from(resolved.offset.unwrap_or(metric_offset));
    let size = f64::from(resolved.size.unwrap_or(metric_size)).max(0.0);
    let top = baseline - offset;
    Some(DecorationInk {
        band: Rect::new(x0.min(x1), top, x0.max(x1), top + size),
        style: ours.style,
        color: ours.color.or(run_color),
        // Filled in by [`skip_ink`], and only for underlines.
        gaps: Vec::new(),
    })
}

/// The clearance a broken band leaves either side of the ink it steps around, as a
/// multiple of the band's thickness (see [`skip_ink`]).
const SKIP_CLEARANCE: f64 = 1.0;

/// How short a fragment of rule has to be to count as dirt rather than underline,
/// as a share of the band's thickness (see [`absorb_specks`]).
const SKIP_SPECK: f64 = 0.5;

/// Flattening tolerance for the glyph outlines [`skip_ink`] measures, in
/// text-local units. Coarser than the render boundary's `TOLERANCE`: the result is
/// an x-interval widened by a thickness each side.
const SKIP_TOLERANCE: f64 = 0.125;

/// Break each underline band around the glyph ink it would otherwise cross, like
/// CSS `text-decoration-skip-ink: auto`. Underlines only: [`extract`] passes the
/// indices of the bands that skip. The gaps are x-intervals cached in the layout, so
/// no boolean subtract runs uncached on the frame path.
fn skip_ink(runs: &[GlyphRun], decorations: &mut [DecorationInk], underlines: &[usize]) {
    if underlines.is_empty() {
        return;
    }
    // The bands' vertical reach, so a whole run can be rejected without outlining it.
    let mut reach = (f64::INFINITY, f64::NEG_INFINITY);
    for &i in underlines {
        let (top, bottom) = ink_extent(&decorations[i]);
        reach = (reach.0.min(top), reach.1.max(bottom));
    }

    // The distinct glyph inks, and one entry per drawn glyph pointing into them.
    // `sigs` is the (face, instance, size) per run; an index keeps the memo key exact.
    let mut inks: Vec<GlyphInk> = Vec::new();
    let mut sigs: Vec<(*const u8, u32, u32, &[i16])> = Vec::new();
    let mut memo: rustc_hash::FxHashMap<(usize, u32), usize> = Default::default();
    let mut placed: Vec<(usize, f64, f64)> = Vec::new();
    let mut path = BezPath::new();
    for run in runs {
        let Some(first) = run.glyphs.first() else {
            continue;
        };
        // Ink stays within two ems of the baseline in any sane face; past that a gap is lost.
        let span = f64::from(run.font_size) * 2.0;
        let y = f64::from(first.y);
        if y + span <= reach.0 || y - span >= reach.1 {
            continue;
        }
        let Some(face) = RunFace::of(run) else {
            continue;
        };
        // The blob's address stands for the face: `FontData` is a shared handle.
        let sig = (
            run.font.data.as_ref().as_ptr(),
            run.font.index,
            run.font_size.to_bits(),
            run.coords.as_slice(),
        );
        let at = sigs.iter().position(|s| *s == sig).unwrap_or_else(|| {
            sigs.push(sig);
            sigs.len() - 1
        });
        for g in &run.glyphs {
            let next = inks.len();
            let ink = *memo.entry((at, g.id)).or_insert(next);
            if ink == next {
                path.truncate(0);
                // Upright: `skip_ink` runs before `bend`, so every `rot` is still zero.
                face.draw(g.id, 0.0, 0.0, 0.0, &mut path);
                inks.push(GlyphInk::of(&path));
            }
            placed.push((ink, f64::from(g.x), f64::from(g.y)));
        }
    }
    if placed.is_empty() {
        return;
    }
    for &i in underlines {
        let gaps = crossings(&inks, &placed, &decorations[i]);
        decorations[i].gaps = gaps;
    }
}

/// One glyph's flattened ink in glyph-local space — the outline with the pen at the
/// origin, so placing it is an addition and one ink serves every occurrence.
struct GlyphInk {
    /// Flattened edges as `[x0, y0, x1, y1]`, y already mirrored.
    edges: Vec<[f64; 4]>,
    /// The box those edges cover, so a band rejects a glyph on four comparisons.
    /// Empty for a glyph with no ink, in the form that fails every overlap test.
    bounds: Rect,
}

impl GlyphInk {
    fn of(path: &BezPath) -> Self {
        let mut edges = Vec::new();
        flatten_edges(path, &mut edges);
        let bounds = edges.iter().fold(
            Rect::new(
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |b, &[ax, ay, bx, by]| {
                Rect::new(
                    b.x0.min(ax).min(bx),
                    b.y0.min(ay).min(by),
                    b.x1.max(ax).max(bx),
                    b.y1.max(ay).max(by),
                )
            },
        );
        Self { edges, bounds }
    }
}

/// The skrifa side of one [`GlyphRun`] — the face, instance and size, resolved once
/// per run. Shared by [`append_run_outline`] and [`skip_ink`].
struct RunFace<'a> {
    glyphs: skrifa::outline::OutlineGlyphCollection<'a>,
    coords: Vec<skrifa::instance::NormalizedCoord>,
    size: skrifa::instance::Size,
}

impl<'a> RunFace<'a> {
    fn of(run: &'a GlyphRun) -> Option<Self> {
        use skrifa::MetadataProvider as _;
        let font = skrifa::FontRef::from_index(run.font.data.as_ref(), run.font.index).ok()?;
        Some(Self {
            glyphs: font.outline_glyphs(),
            // Already normalized F2Dot14 bits: a re-wrap, not a conversion.
            coords: run
                .coords
                .iter()
                .map(|c| skrifa::instance::NormalizedCoord::from_bits(*c))
                .collect(),
            size: skrifa::instance::Size::new(run.font_size),
        })
    }

    /// Append glyph `id`'s outline to `out`, with its origin at `(dx, dy)` and
    /// turned `rot` radians about that origin — [`Glyph`]'s own three numbers.
    fn draw(&self, id: u32, dx: f64, dy: f64, rot: f64, out: &mut BezPath) {
        let Some(glyph) = self.glyphs.get(skrifa::GlyphId::new(id)) else {
            return;
        };
        // The pen borrows `out`, so whether a contour is still open is carried out of the block.
        let open = {
            let mut pen = Pen {
                out,
                dx,
                dy,
                rot: if rot == 0.0 {
                    (1.0, 0.0)
                } else {
                    (rot.cos(), rot.sin())
                },
                down: false,
            };
            // A fresh `DrawSettings` per glyph because `draw` consumes it.
            let settings =
                skrifa::outline::DrawSettings::unhinted(self.size, self.coords.as_slice());
            let _ = glyph.draw(settings, &mut pen);
            pen.down
        };
        if open && !matches!(out.elements().last(), Some(PathEl::ClosePath)) {
            out.close_path();
        }
    }
}

/// The band's true vertical extent — what the render boundary paints: the band
/// itself, except a wavy ribbon, whose ink reaches `1.5t` from the centre. The one
/// number core borrows from the render boundary;
/// `scene::tests::a_wavy_ribbon_stays_inside_the_reach_core_assumes` pins the pair.
fn ink_extent(ink: &DecorationInk) -> (f64, f64) {
    let t = ink.band.height();
    match ink.style {
        LineStyle::Wavy => {
            let mid = ink.band.center().y;
            (mid - t * 1.5, mid + t * 1.5)
        }
        _ => (ink.band.y0, ink.band.y1),
    }
}

/// Every flattened edge of `path` as `[x0, y0, x1, y1]`, appended to `out`.
///
/// The closing edge is emitted by hand: `kurbo::flatten` passes `ClosePath` through,
/// and in a glyph that edge is often a stem's flat side.
fn flatten_edges(path: &BezPath, out: &mut Vec<[f64; 4]>) {
    let mut start = Point::ZERO;
    let mut last = Point::ZERO;
    kurbo::flatten(path, SKIP_TOLERANCE, |el| match el {
        PathEl::MoveTo(p) => {
            start = p;
            last = p;
        }
        PathEl::LineTo(p) => {
            out.push([last.x, last.y, p.x, p.y]);
            last = p;
        }
        PathEl::ClosePath => {
            out.push([last.x, last.y, start.x, start.y]);
            last = start;
        }
        // `flatten`'s output is polylines by construction.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
}

/// Where the band is interrupted, clearance included, so `scene::kept_spans` is a
/// plain complement. Structure is decided first with no clearance, and only then
/// does [`give_air`] widen gaps, or the drawing pops at some size. A band with no
/// qualifying fragment keeps its widest. Clearance is horizontal only: vertical
/// padding would break the rule under every `o`.
fn crossings(
    inks: &[GlyphInk],
    placed: &[(usize, f64, f64)],
    ink: &DecorationInk,
) -> Vec<(f64, f64)> {
    let t = ink.band.height();
    if t <= 0.0 || ink.band.width() <= 0.0 {
        return Vec::new();
    }
    let (bare, letters) = raw_crossings(inks, placed, ink, 0.0);
    let structure = absorb_specks(&bare, ink.band, t, &letters);
    if fragments(&structure, ink.band).is_empty() {
        // Keep the widest raw fragment, or nothing if the ink covers the band end to end.
        return match fragments(&bare, ink.band)
            .into_iter()
            .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
        {
            Some(widest) => complement(&[widest], ink.band),
            None => bare,
        };
    }
    give_air(&structure, ink.band, t * SKIP_CLEARANCE, t)
}

/// Grow every gap by up to `clear` on each side, taking only what the bar it eats
/// into can spare above `floor`.
///
/// Air second makes the drawing continuous: a marginal bar stops getting air instead
/// of ceasing to exist. A bar with two gaps beside it gives each at most half of its
/// spare; a bar against the band's edge spends it all on its one gap.
fn give_air(gaps: &[(f64, f64)], band: Rect, clear: f64, floor: f64) -> Vec<(f64, f64)> {
    if clear <= 0.0 || gaps.is_empty() {
        return gaps.to_vec();
    }
    let mut segs: Vec<(bool, f64, f64)> = Vec::with_capacity(gaps.len() * 2 + 1);
    let mut x = band.x0;
    for &(a, b) in gaps {
        if a > x {
            segs.push((false, x, a));
        }
        segs.push((true, a, b));
        x = b;
    }
    if x < band.x1 {
        segs.push((false, x, band.x1));
    }
    let eat: Vec<f64> = segs
        .iter()
        .map(|&(is_gap, a, b)| {
            if is_gap {
                return 0.0;
            }
            let spare = (b - a - floor).max(0.0);
            let sides = usize::from(a > band.x0) + usize::from(b < band.x1);
            match sides {
                0 => 0.0,
                n => clear.min(spare / n as f64),
            }
        })
        .collect();
    segs.iter()
        .enumerate()
        .filter(|(_, s)| s.0)
        .map(|(i, &(_, a, b))| {
            let left = i.checked_sub(1).map_or(0.0, |j| eat[j]);
            let right = segs.get(i + 1).map_or(0.0, |_| eat[i + 1]);
            ((a - left).max(band.x0), (b + right).min(band.x1))
        })
        .collect()
}

/// What [`raw_crossings`] answers: the x-spans of the band that glyph ink crosses,
/// and the x-box of every letter that crossed it, as `(x0, x1)` intervals.
type Crossings = (Vec<(f64, f64)>, Vec<(f64, f64)>);

/// One crossing, widened by the clearance and clamped to the band — `None` if
/// nothing of it lands inside.
fn widened(x0: f64, x1: f64, band: Rect, clear: f64) -> Option<(f64, f64)> {
    let (a, b) = ((x0 - clear).max(band.x0), (x1 + clear).min(band.x1));
    (b > a).then_some((a, b))
}

/// The clearance-widened, merged x-spans of `ink`'s band that glyph ink crosses,
/// and the placed x-box of every letter that contributed one.
///
/// The boxes tell a fragment of rule showing through a hole inside a letter from one
/// in the bearing beside it ([`absorb_specks`]).
///
/// Three scanlines give the filled ink and every edge's x-extent catches thin
/// features between the rows. Edge extents alone are wrong: a vertical edge has zero
/// x-extent, so a stem wider than `2 · clear` would be struck through
/// (`tests::a_bold_stem_is_one_gap_and_not_two`).
fn raw_crossings(
    inks: &[GlyphInk],
    placed: &[(usize, f64, f64)],
    ink: &DecorationInk,
    clear: f64,
) -> Crossings {
    let band = ink.band;
    let (top, bottom) = ink_extent(ink);
    let mut letters: Vec<(f64, f64)> = Vec::new();
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let mut hits: Vec<f64> = Vec::new();
    for &(i, dx, dy) in placed {
        let glyph = &inks[i];
        let (gx0, gy0) = (glyph.bounds.x0 + dx, glyph.bounds.y0 + dy);
        let (gx1, gy1) = (glyph.bounds.x1 + dx, glyph.bounds.y1 + dy);
        if gy1 <= top || gy0 >= bottom || gx1 + clear <= band.x0 || gx0 - clear >= band.x1 {
            continue;
        }
        let before = spans.len();
        for row in [top, (top + bottom) * 0.5, bottom] {
            hits.clear();
            for &[ex, ey, fx, fy] in &glyph.edges {
                let (ax, ay, bx, by) = (ex + dx, ey + dy, fx + dx, fy + dy);
                // Half-open in y, so a shared vertex counts once and a closed contour crosses evenly.
                if (ay <= row) == (by <= row) {
                    continue;
                }
                hits.push(ax + (bx - ax) * (row - ay) / (by - ay));
            }
            hits.sort_by(f64::total_cmp);
            for pair in hits.as_chunks::<2>().0 {
                if let Some(s) = widened(pair[0], pair[1], band, clear) {
                    spans.push(s);
                }
            }
        }
        for &[ex, ey, fx, fy] in &glyph.edges {
            let (ax, ay, bx, by) = (ex + dx, ey + dy, fx + dx, fy + dy);
            let (ymin, ymax) = if ay <= by { (ay, by) } else { (by, ay) };
            // Strict, so ink that merely touches the band's edge does not break it.
            if ymax <= top || ymin >= bottom {
                continue;
            }
            // The x extent of the part of this edge inside the band's rows.
            let (x0, x1) = if (by - ay).abs() <= f64::EPSILON {
                (ax.min(bx), ax.max(bx))
            } else {
                let at = |y: f64| ax + (bx - ax) * ((y - ay) / (by - ay)).clamp(0.0, 1.0);
                let (p, q) = (at(top), at(bottom));
                (p.min(q), p.max(q))
            };
            if let Some(s) = widened(x0, x1, band, clear) {
                spans.push(s);
            }
        }
        // Only a letter that actually broke the band.
        if spans.len() > before {
            letters.push((gx0, gx1));
        }
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut gaps: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for (x0, x1) in spans {
        match gaps.last_mut() {
            Some(last) if x0 <= last.1 => last.1 = last.1.max(x1),
            _ => gaps.push((x0, x1)),
        }
    }
    (gaps, letters)
}

/// What is left of `band` once `gaps` are taken out of it — the complement
/// `scene::kept_spans` takes. `gaps` must be sorted and non-overlapping, as the
/// merge at the end of [`raw_crossings`] guarantees.
fn fragments(gaps: &[(f64, f64)], band: Rect) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(gaps.len() + 1);
    let mut x = band.x0;
    for &(a, b) in gaps {
        if a > x {
            out.push((x, a.min(band.x1)));
        }
        x = x.max(b);
    }
    if x < band.x1 {
        out.push((x, band.x1));
    }
    out
}

/// The gaps that leave exactly `keep` showing — [`fragments`] the other way round.
fn complement(keep: &[(f64, f64)], band: Rect) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(keep.len() + 1);
    let mut x = band.x0;
    for &(a, b) in keep {
        if a > x {
            out.push((x, a));
        }
        x = b;
    }
    if x < band.x1 {
        out.push((x, band.x1));
    }
    out
}

/// `gaps`, grown to swallow every fragment of rule that is not a rule:
///
/// 1. A fragment inside one letter's ink box is a hole in the letter (the band
///    showing through a `g`'s loop), whatever its width.
/// 2. A fragment shorter than [`SKIP_SPECK`] of the band's thickness is too short to
///    read, such as what a clamp leaves when a word opens with a descender.
///
/// The share is a half because real bearings run `0.75t` to `1.23t` and dirt sits at
/// `0.07t` to `0.19t`; a whole thickness flipped an underlined Inter `g` in and out.
/// Applying this before clearance is what stops the pop (see [`give_air`]).
fn absorb_specks(
    gaps: &[(f64, f64)],
    band: Rect,
    thickness: f64,
    letters: &[(f64, f64)],
) -> Vec<(f64, f64)> {
    let least = thickness * SKIP_SPECK;
    let mut keep = fragments(gaps, band);
    keep.retain(|&(a, b)| b - a >= least && !letters.iter().any(|&(l, r)| a >= l && b <= r));
    complement(&keep, band)
}

/// The list markers a text node draws, answered per paragraph.
///
/// Two passes walk these lines (`extract`, `export_lines`, §15 D81); one decision
/// here keeps them agreeing on which paragraph is an item, its number and gutter.
struct ListMarkers<'a> {
    parts: TextRef<'a>,
    paras: Paragraphs<'a>,
    /// The byte each paragraph starts at, to resolve an item's character style there.
    starts: Vec<usize>,
    /// The marker and its ordinal per paragraph — see [`Self::ordinals`].
    ordinals: Vec<Option<(ListMarker, usize)>>,
}

impl<'a> ListMarkers<'a> {
    /// `None`, with no allocation, when nothing in the node asks for a marker.
    fn of(parts: TextRef<'a>) -> Option<Self> {
        let wanted = parts.paragraph.marker.is_some()
            || parts
                .para_spans
                .as_slice()
                .iter()
                .any(|s| s.attr.kind() == ParaAttrKind::Marker);
        if !wanted {
            return None;
        }
        let paras = Paragraphs::new(parts);
        let starts = paragraph_starts(parts.content);
        let ordinals = Self::ordinals(&paras, starts.len());
        Some(ListMarkers {
            parts,
            paras,
            starts,
            ordinals,
        })
    }

    /// What each paragraph's marker draws, or `None` where it has no marker.
    ///
    /// Consecutive paragraphs belong to one list only if they ask for the same marker
    /// at the same level; anything else ends the run, and each run counts from one.
    /// One counter per open level ([`ParagraphStyle::level`]); going back out
    /// truncates the deeper ones, so `1, (a, b), 2, (a, b)` restarts each sublist.
    fn ordinals(paras: &Paragraphs<'_>, count: usize) -> Vec<Option<(ListMarker, usize)>> {
        let mut out = Vec::with_capacity(count);
        // `None` is a level skipped on the way in, which must not lend a count to a later item.
        let mut open: Vec<Option<(ListMarker, usize)>> = Vec::new();
        for index in 0..count {
            let style = paras.style(index);
            let Some(marker) = style.marker else {
                // A non-item ends every list at every level (see `ParagraphStyle::level`).
                open.clear();
                out.push(None);
                continue;
            };
            let level = usize::from(style.level);
            open.truncate(level + 1);
            if open.len() <= level {
                open.resize(level + 1, None);
            }
            let counter = &mut open[level];
            *counter = match *counter {
                Some((previous, n)) if previous == marker => Some((marker, n + 1)),
                // This level had not opened, or was a different list.
                _ => Some((marker, 1)),
            };
            out.push(*counter);
        }
        out
    }

    /// What paragraph `index` draws: the marker text, the character style to set it
    /// in, and the x its right edge lands on.
    fn at(&self, index: usize) -> Option<(String, TextStyle, f64)> {
        let (marker, n) = self.ordinals.get(index).copied().flatten()?;
        // Resolved at the item's first byte, so restyling an item takes its marker along.
        let last_byte = self.parts.content.len().saturating_sub(1);
        let start = self.starts.get(index).copied().unwrap_or(0).min(last_byte);
        let style = self.parts.spans.resolve(self.parts.style, start);
        // Against the node's font size, as `line_geometry` does; the item's own would split the gutters.
        let right = self
            .paras
            .style(index)
            .start_edge(self.parts.style.font_size);
        Some((marker.text(n), style, right))
    }
}

/// A list marker's own advance, for `export_lines`, whose `<text x>` names the
/// ink's left edge.
fn marker_advance(style: &TextStyle, text: &str) -> f64 {
    let spans = CharSpans::default();
    let para_spans = ParaSpans::default();
    let paragraph = ParagraphStyle::default();
    let block = BlockStyle::default();
    let sizing = TextSizing::Auto;
    measure(TextRef {
        content: text,
        style,
        spans: &spans,
        para_spans: &para_spans,
        paragraph: &paragraph,
        block: &block,
        sizing: &sizing,
        // Flat: `bend` maps the finished layout, so bending here would bend twice.
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    })
    .width()
}

/// A shaped list marker, right-aligned so its right edge lands on `right`.
///
/// Right-aligned so `1.` and `10.` start their text in the same column. No gap
/// constant: the user sizes the gutter with `indent_start`. The marker follows
/// neither the paragraph's alignment nor its first-line indent.
fn marker_run(style: &TextStyle, text: &str, right: f64, baseline: f64) -> Option<GlyphRun> {
    let spans = CharSpans::default();
    let para_spans = ParaSpans::default();
    // The default carries no marker, so this cannot recurse.
    let paragraph = ParagraphStyle::default();
    let block = BlockStyle::default();
    let sizing = TextSizing::Auto;
    let shaped = shape(TextRef {
        content: text,
        style,
        spans: &spans,
        para_spans: &para_spans,
        paragraph: &paragraph,
        block: &block,
        sizing: &sizing,
        // Flat — see [`marker_advance`].
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    });
    let mut run = extract_flat(&shaped)?;
    let at = right - shaped.size.width;
    for g in &mut run.glyphs {
        g.x += at as f32;
        g.y += baseline as f32;
    }
    // The item's own ink; `extract_flat` leaves this `None` for the font picker.
    run.color = style.color;
    Some(run)
}

/// The truncation mark, named once because `extract` measures it and
/// [`ellipsis_run`] shapes it; two spellings would open a gap silently.
const ELLIPSIS: &str = "…";

/// A shaped `…` placed at `at`, the end of a truncated line.
///
/// Shaped on its own so it never enters the byte space of the caret and spans.
fn ellipsis_run(style: &TextStyle, at: f64, baseline: f64) -> Option<GlyphRun> {
    let parts_style = style.clone();
    let spans = CharSpans::default();
    let para_spans = ParaSpans::default();
    let paragraph = ParagraphStyle::default();
    let block = BlockStyle::default();
    let sizing = TextSizing::Auto;
    let shaped = shape(TextRef {
        content: ELLIPSIS,
        style: &parts_style,
        spans: &spans,
        para_spans: &para_spans,
        paragraph: &paragraph,
        block: &block,
        sizing: &sizing,
        // Flat — see [`marker_advance`].
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    });
    let mut run = extract_flat(&shaped)?;
    for g in &mut run.glyphs {
        g.x += at as f32;
        g.y += baseline as f32;
    }
    // The ink of the type it follows, as in `marker_run` (§15 D490).
    run.color = style.color;
    Some(run)
}

/// The single glyph run of a trivially-shaped string, with its glyphs still at
/// the layout origin.
fn extract_flat(shaped: &Shaped) -> Option<GlyphRun> {
    let line = shaped.layout.lines().next()?;
    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
            continue;
        };
        let run = glyph_run.run();
        return Some(GlyphRun {
            font: run.font().clone(),
            font_size: run.font_size(),
            coords: run.normalized_coords().to_vec(),
            glyphs: glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y - glyph_run.baseline(),
                    rot: 0.0,
                })
                .collect(),
            // A family preview draws in the picker's own text colour.
            color: None,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Shape and lay out a text node, returning its box and the ink to draw.
///
/// Callers that render or measure repeatedly should read the cached layout from
/// [`crate::resolve::Resolved`] instead — this re-shapes from scratch.
pub fn layout(parts: TextRef<'_>) -> TextLayout {
    let shaped = shape(parts);
    extract(&shaped, parts)
}

thread_local! {
    /// How many times [`shape`] has run on this thread. See [`shapes`].
    static SHAPES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many text layouts this thread has shaped since it started. Never decreases.
///
/// Counted in `shape`, not [`layout`], which `TextEdit::reshape` bypasses. A test
/// reads it either side of a commit to assert no needless shaping (§15 D590).
pub fn shapes() -> u64 {
    SHAPES.with(std::cell::Cell::get)
}

/// Local (untransformed) box of a text node's content. Thin wrapper over
/// [`layout`] for geometry/hit-testing that doesn't need glyphs.
pub fn measure(parts: TextRef<'_>) -> Rect {
    layout(parts).bounds()
}

/// The narrowest and the widest a text node's lines can be laid out — CSS's
/// **min-content** width (every soft break taken: the widest word) and
/// **max-content** width (none taken) — for container layout's measure (§15 D872).
/// Shaped through the same seam as [`layout`], so [`shapes`] counts it.
///
/// Plus the paragraph's own edges (§15 D931): start edge, a positive first-line or
/// hanging indent and the end indent, the widest sum over the paragraphs. parley
/// knows none of them ([`line_geometry`] places every line); without them a
/// bulleted item at min-content drew over its neighbour.
pub fn content_widths(parts: TextRef<'_>) -> (f64, f64) {
    let shaped = shape(parts);
    // Whitespace-only: parley counts the spaces, the auto box (`layout.width()`) does not.
    if parts.content.trim().is_empty() {
        let w = f64::from(shaped.layout.width());
        return (w, w);
    }
    let w = shaped.layout.calculate_content_widths();
    let paras = Paragraphs::new(parts);
    let font_size = parts.style.font_size;
    let edges = (0..paragraph_starts(parts.content).len().max(1))
        .map(|i| {
            let style = paras.style(i);
            style.start_edge(font_size)
                + style.indent.resolve(font_size).max(0.0)
                + style.indent_end.resolve(font_size)
        })
        .fold(0.0_f64, f64::max);
    (f64::from(w.min) + edges, f64::from(w.max) + edges)
}

/// A family name shaped in the family it names — one row of the font picker.
///
/// Positions are strip-local: `x` from 0, `baseline` down from the line box top.
#[derive(Clone, Debug, PartialEq)]
pub struct FamilyPreview {
    pub run: GlyphRun,
    /// Advance width of the shaped name.
    pub width: f64,
    /// The line box height, and the baseline's distance from its top.
    pub height: f64,
    pub baseline: f64,
}

/// Shape `label` in `family` for a picker row, or `None` when the result would
/// not be a truthful preview of that family.
///
/// All-or-nothing, since a silent fallback to Inter misrepresents the font. It
/// declines when the family has no faces, the name needed another face or blob, or
/// a glyph came back `.notdef`; the caller then draws the name in the UI font.
pub fn family_preview(family: &str, label: &str, size: f64) -> Option<FamilyPreview> {
    let expected = ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let id = engine.font_cx.collection.family_id(family)?;
        let info = engine.font_cx.collection.family(id)?;
        let font = info.default_font()?.clone();
        let blob = font.load(Some(&mut engine.font_cx.source_cache))?;
        Some(blob.id())
    })?;

    let parts = TextParts {
        style: TextStyle {
            font_family: family.to_string(),
            font_size: size,
            // The font's own leading keeps ink inside the box.
            line_height: None,
            ..TextStyle::default()
        },
        ..TextParts::default()
    };
    let laid = layout(parts.as_ref(label));

    let [run] = &laid.runs[..] else {
        return None;
    };
    if run.font.data.id() != expected || run.glyphs.iter().any(|g| g.id == 0) {
        return None;
    }
    let baseline = f64::from(run.glyphs.first()?.y);
    Some(FamilyPreview {
        run: run.clone(),
        width: laid.size.width,
        height: laid.size.height,
        baseline,
    })
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

/// An in-progress text-editing session: content plus a caret/selection over the
/// shaped layout, driven entirely by byte indices and text-local points.
///
/// The model behind in-canvas text editing (§9.3). The app owns when a session
/// starts and ends; this type owns the text in between and never touches the
/// `Document`. Every point goes through the session's [`YMap`].
pub struct TextEdit {
    content: String,
    style: TextStyle,
    spans: CharSpans,
    para_spans: ParaSpans,
    paragraph: ParagraphStyle,
    block: BlockStyle,
    sizing: TextSizing,
    /// The rail, carried for the whole session: the caret and hit test are measured
    /// against the layout it produces, so a session on bent type must shape bent.
    on_path: Option<BezPath>,
    /// Which way round the rail runs.
    on_path_flip: bool,
    /// Where along the rail the type starts.
    on_path_offset: f64,
    shaped: Shaped,
    /// The bend, rebuilt beside [`Self::shaped`].
    ///
    /// Cached because `TextLayout::warp` would need `extract`, which allocates every
    /// glyph run, on every blink frame.
    warp: Option<PathWarp>,
    selection: Selection,
    /// Attributes set on an empty caret, waiting for the next character typed.
    /// Cleared by any caret movement.
    pending: Vec<crate::typography::CharAttr>,
    /// The x a vertical move aims at, in parley's layout space and corrected as
    /// [`Self::caret_rect`] corrects it, kept across consecutive Up/Downs so a short
    /// line does not pull the caret in (see [`Self::move_line`]).
    sticky_x: Option<f64>,
}

impl TextEdit {
    /// Start editing `content`, caret at the end (what clicking a layer and
    /// typing implies); use [`Self::click`] to place it somewhere else.
    pub fn new(content: impl Into<String>, parts: TextParts) -> Self {
        let content = content.into();
        let shaped = shape(parts.as_ref(&content));
        let selection =
            Selection::from_byte_index(&shaped.layout, content.len(), Affinity::Upstream);
        let warp = warp_for(parts.as_ref(&content), &shaped);
        Self {
            content,
            style: parts.style,
            spans: parts.spans,
            para_spans: parts.para_spans,
            paragraph: parts.paragraph,
            block: parts.block,
            sizing: parts.sizing,
            on_path: parts.on_path,
            on_path_flip: parts.on_path_flip,
            on_path_offset: parts.on_path_offset,
            shaped,
            warp,
            selection,
            pending: Vec::new(),
            sticky_x: None,
        }
    }

    /// Rebuild the bend beside the shaping. Every writer of [`Self::shaped`] calls it:
    /// a stale warp drifts the caret off the ink on a centred rail.
    fn rewarp(&mut self) {
        self.warp = warp_for(self.parts(), &self.shaped);
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// The character overrides as editing has left them — what the app commits
    /// alongside the content.
    pub fn spans(&self) -> &CharSpans {
        &self.spans
    }

    /// The paragraph overrides as editing has left them, committed with the content
    /// ([`crate::Operation::SetText`]).
    pub fn para_spans(&self) -> &ParaSpans {
        &self.para_spans
    }

    fn parts(&self) -> TextRef<'_> {
        TextRef {
            content: &self.content,
            style: &self.style,
            spans: &self.spans,
            para_spans: &self.para_spans,
            paragraph: &self.paragraph,
            block: &self.block,
            sizing: &self.sizing,
            on_path: self.on_path.as_ref(),
            on_path_flip: self.on_path_flip,
            on_path_offset: self.on_path_offset,
        }
    }

    /// The shaped result, for previewing the in-progress text.
    pub fn text_layout(&self) -> TextLayout {
        extract(&self.shaped, self.parts())
    }

    /// Adopt a style change made while the session is open (the inspector stays
    /// live during editing), keeping the caret where it is.
    pub fn restyle(&mut self, parts: TextParts) {
        self.style = parts.style;
        self.spans = parts.spans;
        self.para_spans = parts.para_spans;
        self.paragraph = parts.paragraph;
        self.block = parts.block;
        self.sizing = parts.sizing;
        self.on_path = parts.on_path;
        self.on_path_flip = parts.on_path_flip;
        self.on_path_offset = parts.on_path_offset;
        self.reshape_keeping_selection();
    }

    /// The bytes a paragraph-scoped write from this session acts on: the selection
    /// snapped outward to hard breaks ([`paragraph_bounds`]). Read by the panel
    /// because the session's content is a commit ahead of the document's.
    pub fn paragraph_range(&self) -> Range<usize> {
        paragraph_bounds(&self.content, self.selected_range())
    }

    /// Apply one paragraph attribute to the paragraph(s) the selection touches; the
    /// twin of [`Self::style_selection`] with no `pending`, since a caret is always
    /// in a paragraph.
    pub fn style_paragraph(&mut self, attr: crate::typography::ParaAttr) {
        let range = self.paragraph_range();
        self.para_spans.set(range, attr, &self.paragraph);
        self.reshape_keeping_selection();
    }

    /// Nest or un-nest the list items the selection touches: `+1` for `Tab`, `-1`
    /// for `Shift`+`Tab`. Returns whether anything moved.
    ///
    /// Relative per paragraph, where [`Self::style_paragraph`] writes one absolute
    /// value: an item and its sublist must become levels 1 and 2. Only paragraphs
    /// with a marker move.
    ///
    /// [`ParagraphStyle::level`]: crate::typography::ParagraphStyle::level
    pub fn step_list_level(&mut self, delta: i32) -> bool {
        let starts = paragraph_starts(&self.content);
        let last_byte = self.content.len().saturating_sub(1);
        let mut moved = false;
        for index in paragraphs_touched(&self.content, self.selected_range()) {
            let Some(start) = starts.get(index).copied() else {
                continue;
            };
            let style = self
                .para_spans
                .resolve(&self.paragraph, start.min(last_byte));
            if style.marker.is_none() {
                continue;
            }
            let next = (i32::from(style.level) + delta).clamp(0, i32::from(MAX_LIST_LEVEL)) as u8;
            if next == style.level {
                continue;
            }
            // `paragraph_bounds`, so an empty trailing paragraph reaches back over its break.
            let write = paragraph_bounds(&self.content, start..start);
            self.para_spans
                .set(write, ParaAttr::Level(next), &self.paragraph);
            moved = true;
        }
        if moved {
            self.reshape_keeping_selection();
        }
        moved
    }

    /// Apply one character attribute to the selection, or — with a bare caret —
    /// hold it for the next character typed.
    ///
    /// The bare-caret half has no caller in the app today (§15 D733); it stays as
    /// designed in `architecture.md` §5.4, pinned by the tests below, and `insert`
    /// consumes `pending` where a `cut` must not (§15 D217).
    pub fn style_selection(&mut self, attr: crate::typography::CharAttr) {
        let range = self.selected_range();
        if range.is_empty() {
            self.pending.retain(|a| a.kind() != attr.kind());
            self.pending.push(attr);
            return;
        }
        self.spans.set(range, attr, &self.style);
        self.reshape_keeping_selection();
    }

    /// Rewrite one attribute across the selection run by run, each run's new value
    /// computed from its own old one (§15 D752).
    ///
    /// [`Self::style_selection`] writes one value over everything, which flattens a
    /// list-valued attribute: toggling `liga` over differing runs would put the first
    /// run's other settings on all of them. A bare caret does nothing.
    pub fn style_selection_per_run(
        &mut self,
        kind: crate::typography::CharAttrKind,
        f: impl Fn(&crate::typography::CharAttr) -> crate::typography::CharAttr,
    ) {
        let range = self.selected_range();
        if range.is_empty() {
            return;
        }
        // Collected first: `runs_of` borrows the spans the loop writes.
        let runs = self.spans.runs_of(&self.style, kind, range);
        for (at, old) in runs {
            self.spans.set(at, f(&old), &self.style);
        }
        self.reshape_keeping_selection();
    }

    // --- editing ---------------------------------------------------------

    /// Replace the selection (or insert at the caret) with `s`.
    ///
    /// Newlines are normalized on the way in: a `\r\n` pair would raise two breaks
    /// and double-space pasted Windows text (`a_crlf_pair_is_one_line_break_not_two`).
    /// Not in [`Self::new`], which is handed the document's content and must not put
    /// editor and document out of step.
    pub fn insert(&mut self, s: &str) {
        let owned = normalized_newlines(s);
        let s = owned.as_deref().unwrap_or(s);
        let range = self.selected_range();
        let at = range.start;
        let removed = range.len();
        self.content.replace_range(range, s);
        self.spans = self.spans.edited(at, removed, s.len(), &self.style);
        self.para_spans = self
            .para_spans
            .edited(at, removed, s.len(), &self.paragraph);
        let pending = std::mem::take(&mut self.pending);
        if !s.is_empty() {
            for attr in pending {
                self.spans.set(at..at + s.len(), attr, &self.style);
            }
        }
        self.reshape(at + s.len());
    }

    /// Delete the selection, or the cluster before the caret.
    pub fn backspace(&mut self) {
        let range = self.selected_range();
        if !range.is_empty() {
            self.replace(range, "");
            return;
        }
        let focus = self.selection.focus();
        let prev = focus.previous_visual(&self.shaped.layout).index();
        if prev < focus.index() {
            self.replace(prev..focus.index(), "");
        }
    }

    /// Delete the selection, or the cluster after the caret.
    pub fn delete_forward(&mut self) {
        let range = self.selected_range();
        if !range.is_empty() {
            self.replace(range, "");
            return;
        }
        let focus = self.selection.focus();
        let next = focus.next_visual(&self.shaped.layout).index();
        if next > focus.index() {
            self.replace(focus.index()..next, "");
        }
    }

    /// Delete the selection, or back to the start of the previous word.
    pub fn delete_word_back(&mut self) {
        let range = self.selected_range();
        if !range.is_empty() {
            self.backspace();
            return;
        }
        let focus = self.selection.focus();
        let prev = focus.previous_visual_word(&self.shaped.layout).index();
        if prev < focus.index() {
            self.replace(prev..focus.index(), "");
        }
    }

    fn replace(&mut self, range: Range<usize>, with: &str) {
        let at = range.start;
        let removed = range.len();
        self.content.replace_range(range, with);
        self.spans = self.spans.edited(at, removed, with.len(), &self.style);
        self.para_spans = self
            .para_spans
            .edited(at, removed, with.len(), &self.paragraph);
        self.reshape(at + with.len());
    }

    // --- caret movement ---------------------------------------------------

    /// Everything a caret move invalidates: the pending style and the sticky
    /// vertical column. [`Self::move_line`] is the one mover that must not call this.
    ///
    /// [`Self::pending`]: TextEdit::pending
    fn caret_moved(&mut self) {
        self.pending.clear();
        self.sticky_x = None;
    }

    /// `extend` grows the selection instead of collapsing it (Shift held).
    pub fn move_left(&mut self, extend: bool, by_word: bool) {
        self.caret_moved();
        self.selection = if by_word {
            self.selection
                .previous_visual_word(&self.shaped.layout, extend)
        } else {
            self.selection.previous_visual(&self.shaped.layout, extend)
        };
    }

    pub fn move_right(&mut self, extend: bool, by_word: bool) {
        self.caret_moved();
        self.selection = if by_word {
            self.selection.next_visual_word(&self.shaped.layout, extend)
        } else {
            self.selection.next_visual(&self.shaped.layout, extend)
        };
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_line(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_line(1, extend);
    }

    /// One line up or down, aiming at the column the caret is visually in.
    ///
    /// Not `Selection::previous_line`/`next_line`: parley seeds its sticky column
    /// from `Cursor::geometry`, which omits the line's start edge, then lands with
    /// `Cursor::from_point`, which reads it, so the caret lands an indent too far
    /// left (see [`Self::caret_rect`]).
    fn move_line(&mut self, delta: isize, extend: bool) {
        // `pending` alone, not `caret_moved`: the sticky column must survive.
        self.pending.clear();
        let index = self.caret_line_index();
        let last = self.shaped.layout.len().saturating_sub(1);
        let x = self.sticky_x.unwrap_or_else(|| self.caret_x());
        self.sticky_x = Some(x);
        let target = index.checked_add_signed(delta).filter(|t| *t <= last);
        let Some(line) = target.and_then(|t| self.shaped.layout.get(t)) else {
            // Past either end, move to the far end as parley does; the column rides along.
            self.selection = if delta < 0 {
                self.selection.line_start(&self.shaped.layout, extend)
            } else {
                self.selection.line_end(&self.shaped.layout, extend)
            };
            return;
        };
        let m = line.metrics();
        let y = m.block_max_coord - m.ascent * 0.5;
        let focus = Cursor::from_point(&self.shaped.layout, x as f32, y);
        self.selection = if extend {
            self.selection.extend(focus)
        } else {
            Selection::new(focus, focus)
        };
    }

    pub fn move_line_start(&mut self, extend: bool) {
        self.caret_moved();
        self.selection = self.selection.line_start(&self.shaped.layout, extend);
    }

    pub fn move_line_end(&mut self, extend: bool) {
        self.caret_moved();
        self.selection = self.selection.line_end(&self.shaped.layout, extend);
    }

    pub fn select_all(&mut self) {
        self.caret_moved();
        let start = Selection::from_byte_index(&self.shaped.layout, 0usize, Affinity::Downstream);
        self.selection = start.extend(
            Selection::from_byte_index(&self.shaped.layout, self.content.len(), Affinity::Upstream)
                .focus(),
        );
    }

    // --- pointer ----------------------------------------------------------

    /// A text-local point in parley's own layout space.
    ///
    /// The inverse half of the one translation (see [`YMap`]): every pointer
    /// question comes through here, so ink and hit test agree. It resolves the line
    /// first ([`Shaped::layout_y`], §15 D166). The rail is un-bent first, since the
    /// bend was applied on top of the flat offsets.
    fn to_layout(&self, p: Point) -> (f32, f32) {
        let p = match &self.warp {
            Some(w) => {
                let (x, y) = w.nearest(p);
                Point::new(w.shortest_way_round(x), y)
            }
            None => p,
        };
        (p.x as f32, self.shaped.layout_y(p.y) as f32)
    }

    /// Place the caret at a text-local point (a click).
    pub fn click(&mut self, p: Point) {
        self.caret_moved();
        let (x, y) = self.to_layout(p);
        self.selection = Selection::from_point(&self.shaped.layout, x, y);
    }

    /// Select the word under a text-local point (a double-click).
    pub fn select_word_at(&mut self, p: Point) {
        self.caret_moved();
        let (x, y) = self.to_layout(p);
        self.selection = Selection::word_from_point(&self.shaped.layout, x, y);
    }

    /// Extend the selection to a text-local point (a drag, or shift-click).
    pub fn drag_to(&mut self, p: Point) {
        self.sticky_x = None;
        let (x, y) = self.to_layout(p);
        self.selection = self.selection.extend_to_point(&self.shaped.layout, x, y);
    }

    // --- geometry ---------------------------------------------------------

    /// The caret's rectangle in text-local space, `width` units wide.
    ///
    /// Shifted by the line's own start edge, which `Cursor::geometry` leaves out (it
    /// never reads `inline_min_coord`, where a paragraph indent lives, §15 D163).
    /// `Selection::geometry`, `Cursor::from_point` and the glyph runs include it.
    pub fn caret_rect(&self, width: f64) -> Rect {
        let b = self
            .selection
            .focus()
            .geometry(&self.shaped.layout, width as f32);
        let line = self.caret_line_index();
        let dx = self.line_start_x(line);
        let r = self.to_local_rect(b, line);
        Rect::new(r.x0 + dx, r.y0, r.x1 + dx, r.y1)
    }

    /// The index of the line the caret is on.
    ///
    /// The last line at or above the caret's `y0` (that line's `block_min_coord`),
    /// not the first containing it: the ranges overlap under tight line height.
    fn caret_line_index(&self) -> usize {
        let y = self.selection.focus().geometry(&self.shaped.layout, 0.0).y0;
        let last = self.shaped.layout.len().saturating_sub(1);
        self.shaped
            .layout
            .lines()
            .rposition(|line| f64::from(line.metrics().block_min_coord) <= y)
            .unwrap_or(last)
    }

    /// Line `index`'s start edge — the term [`Self::caret_rect`] explains.
    fn line_start_x(&self, index: usize) -> f64 {
        self.shaped
            .layout
            .get(index)
            .map(|line| f64::from(line.metrics().inline_min_coord))
            .unwrap_or(0.0)
    }

    /// The caret's x in parley's layout space, corrected the way
    /// [`Self::caret_rect`] corrects it — the column a vertical move aims at.
    fn caret_x(&self) -> f64 {
        self.selection.focus().geometry(&self.shaped.layout, 0.0).x0
            + self.line_start_x(self.caret_line_index())
    }

    /// The caret as convex quads in text-local space, `width` units wide — what a
    /// canvas draws. The bent counterpart of [`Self::caret_rect`]: on a rail a caret
    /// is a short segment across the curve. Ordinary type comes back as one quad.
    pub fn caret_quads(&self, width: f64) -> Vec<[Point; 4]> {
        self.warped(self.caret_rect(width))
    }

    /// The selection highlight as convex quads in text-local space; empty for a
    /// bare caret. [`Self::caret_quads`]'s twin.
    pub fn selection_quads(&self) -> Vec<[Point; 4]> {
        self.selection_rects()
            .into_iter()
            .flat_map(|r| self.warped(r))
            .collect()
    }

    /// One text-local rectangle as the quads that draw it — bent if this session is
    /// on a rail, and its own four corners if it is not.
    fn warped(&self, r: Rect) -> Vec<[Point; 4]> {
        match &self.warp {
            Some(w) => w.warp_rect(r),
            None => vec![[
                Point::new(r.x0, r.y0),
                Point::new(r.x1, r.y0),
                Point::new(r.x1, r.y1),
                Point::new(r.x0, r.y1),
            ]],
        }
    }

    /// Highlight rectangles for the selected range, in text-local space; empty for
    /// a bare caret. Always flat; [`Self::selection_quads`] follows a rail.
    pub fn selection_rects(&self) -> Vec<Rect> {
        self.selection
            .geometry(&self.shaped.layout)
            .into_iter()
            .map(|(b, line)| self.to_local_rect(b, line))
            .collect()
    }

    /// A parley rectangle in text-local space.
    ///
    /// The line is named, not inferred from the y (see [`YMap::shift_for_line`]).
    fn to_local_rect(&self, b: BoundingBox, line: usize) -> Rect {
        Rect::new(
            b.x0,
            self.shaped.ymap.to_local_on_line(b.y0, line),
            b.x1,
            self.shaped.ymap.to_local_on_line(b.y1, line),
        )
    }

    pub fn has_selection(&self) -> bool {
        !self.selection.is_collapsed()
    }

    /// The selected byte range, empty when the selection is a bare caret.
    pub fn selected_range(&self) -> Range<usize> {
        self.selection.text_range()
    }

    /// The selected text, empty for a bare caret — what a copy takes. Indexing
    /// cannot panic: every mover goes through parley's cluster boundaries.
    pub fn selected_text(&self) -> &str {
        &self.content[self.selected_range()]
    }

    /// Remove the selection and hand it back — the model half of a cut.
    ///
    /// `None` for a bare caret. [`Self::replace`], not `insert("")`: `insert`
    /// consumes [`Self::pending`], and a cut must leave a pending bold in place.
    pub fn cut(&mut self) -> Option<String> {
        let range = self.selected_range();
        if range.is_empty() {
            return None;
        }
        let text = self.content[range.clone()].to_string();
        self.replace(range, "");
        Some(text)
    }

    /// Re-shape after a content or style change and put the caret at `at`.
    fn reshape(&mut self, at: usize) {
        self.shaped = shape(self.parts());
        self.rewarp();
        let at = at.min(self.content.len());
        self.selection = Selection::from_byte_index(&self.shaped.layout, at, Affinity::Downstream);
        self.sticky_x = None;
    }

    /// Re-shape after a change that moved no text, keeping the selection.
    ///
    /// [`Self::reshape`]'s counterpart: `from_byte_index` builds a collapsed caret,
    /// wrong after a restyle, where every byte is still there.
    fn reshape_keeping_selection(&mut self) {
        let (anchor, focus) = (
            self.selection.anchor().index(),
            self.selection.focus().index(),
        );
        self.shaped = shape(self.parts());
        self.rewarp();
        let at = |i: usize| {
            Cursor::from_byte_index(
                &self.shaped.layout,
                i.min(self.content.len()),
                Affinity::Downstream,
            )
        };
        self.selection = Selection::new(at(anchor), at(focus));
        self.sticky_x = None;
    }
}

/// `s` with every `\r\n` pair and lone `\r` collapsed to a single `\n`, or `None`
/// when it holds no `\r`, which keeps the per-keystroke case allocation-free.
fn normalized_newlines(s: &str) -> Option<String> {
    if !s.contains('\r') {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// An owned copy of everything a text node's layout depends on.
///
/// The owned twin of [`TextRef`], for callers that cannot borrow from a `NodeKind`:
/// [`TextEdit`] and the app's default-style factory.
#[derive(Clone, Debug, PartialEq)]
pub struct TextParts {
    pub style: TextStyle,
    pub spans: CharSpans,
    pub para_spans: ParaSpans,
    pub paragraph: ParagraphStyle,
    pub block: BlockStyle,
    pub sizing: TextSizing,
    /// The rail, owned (see [`TextRef::on_path`]).
    pub on_path: Option<BezPath>,
    /// Which way round it runs. See [`TextRef::on_path_flip`].
    pub on_path_flip: bool,
    /// Where along it the type starts. See [`TextRef::on_path_offset`].
    pub on_path_offset: f64,
}

impl TextParts {
    /// Borrow these parts against `content`.
    pub fn as_ref<'a>(&'a self, content: &'a str) -> TextRef<'a> {
        TextRef {
            content,
            style: &self.style,
            spans: &self.spans,
            para_spans: &self.para_spans,
            paragraph: &self.paragraph,
            block: &self.block,
            sizing: &self.sizing,
            on_path: self.on_path.as_ref(),
            on_path_flip: self.on_path_flip,
            on_path_offset: self.on_path_offset,
        }
    }

    /// The parts of a text node, cloned out of its kind.
    pub fn of(kind: &crate::node::NodeKind) -> Option<Self> {
        let r = TextRef::of(kind)?;
        Some(TextParts {
            style: r.style.clone(),
            spans: r.spans.clone(),
            para_spans: r.para_spans.clone(),
            paragraph: r.paragraph.clone(),
            block: *r.block,
            sizing: *r.sizing,
            on_path: r.on_path.cloned(),
            on_path_flip: r.on_path_flip,
            on_path_offset: r.on_path_offset,
        })
    }
}

impl Default for TextParts {
    fn default() -> Self {
        TextParts {
            style: TextStyle::default(),
            spans: CharSpans::default(),
            para_spans: ParaSpans::default(),
            paragraph: ParagraphStyle::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Names in this module's prose use plain backticks, never intra-doc links:
    //! `cargo doc` builds without the `test` cfg, so no gate validates a link here.

    use super::*;
    use crate::typography::{CharAttr, Decoration, ParaAttr, VerticalAlign};

    pub(super) fn parts(style: &TextStyle, sizing: &TextSizing) -> TextParts {
        TextParts {
            style: style.clone(),
            spans: CharSpans::default(),
            para_spans: ParaSpans::default(),
            paragraph: ParagraphStyle::default(),
            block: BlockStyle::default(),
            sizing: *sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// Styling a selection keeps it selected, so the next attribute lands on the
    /// same range rather than the whole node.
    #[test]
    fn styling_a_selection_keeps_it_selected() {
        let d = style(20.0);
        let mut edit = TextEdit::new(
            "hello world",
            TextParts {
                style: d.clone(),
                spans: CharSpans::default(),
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
        );
        edit.select_all();
        let before = edit.selected_range();
        assert_eq!(before, 0..11, "select-all is the precondition");

        edit.style_selection(CharAttr::Weight(700));
        assert_eq!(
            edit.selected_range(),
            before,
            "the selection must survive a restyle — losing it is what lets the next \n             property hit the whole node"
        );

        edit.style_selection(CharAttr::Size(30.0));
        assert_eq!(edit.selected_range(), before);
    }

    /// An auto-height box is as wide as authored, not as wide as the ink; the
    /// `Auto` case is the control that does track the ink.
    #[test]
    fn an_auto_height_box_is_as_wide_as_it_was_authored() {
        let d = style(20.0);
        for w in [400.0, 30.0] {
            let b = measure(parts(&d, &TextSizing::AutoHeight(w)).as_ref("hello there world"));
            assert!(
                (b.width() - w).abs() < 1e-9,
                "auto-height authored at {w} measured {}",
                b.width()
            );
        }
        let auto = measure(parts(&d, &TextSizing::Auto).as_ref("hello there world"));
        assert!(
            auto.width() > 30.0 && (auto.width() - 400.0).abs() > 1.0,
            "the control is that an auto box follows its ink; it measured {}",
            auto.width()
        );
    }

    /// A span at byte 0 must not leak its style onto runs that merely inherit:
    /// a shift on "a" alone moves "a" alone.
    #[test]
    fn a_baseline_shift_on_the_first_run_does_not_move_the_rest() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::Auto);
        p.spans
            .set(0..1, CharAttr::BaselineShift(Length::Px(8.0)), &d);
        let tl = layout(p.as_ref("abc"));
        let ys: Vec<f32> = tl
            .runs
            .iter()
            .filter_map(|r| r.glyphs.first().map(|g| g.y))
            .collect();
        assert!(
            ys.len() >= 2,
            "the span has to split the text into runs: {ys:?}"
        );
        let lifted = ys.iter().filter(|y| **y < ys[ys.len() - 1] - 1.0).count();
        assert_eq!(
            lifted, 1,
            "only the shifted run may move — every run took the shift: {ys:?}"
        );
    }

    // --- export_lines -----------------------------------------------------

    /// A wrapped node exports as lines whose text together is the content, each
    /// lower than the last.
    #[test]
    fn export_lines_breaks_a_wrapped_node_into_its_lines() {
        let p = parts(&style(20.0), &TextSizing::AutoHeight(60.0));
        let lines = export_lines(p.as_ref("hello there world"));
        assert!(lines.len() > 1, "60pt at 20pt type has to wrap: {lines:?}");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.runs.iter().map(|r| r.text.as_str()))
            .collect();
        assert_eq!(
            joined.replace(' ', ""),
            "hellothereworld",
            "every character reaches exactly one run: {lines:?}"
        );
        let first = lines[0].baseline;
        assert!(
            lines[1].baseline > first,
            "line 2's baseline must be below line 1's: {} vs {first}",
            lines[1].baseline
        );
    }

    /// An ellipsized node exports the same ellipsis the canvas draws; asserted
    /// against the canvas's glyph count so the two walks must agree.
    #[test]
    fn an_ellipsized_node_exports_the_ellipsis_the_canvas_draws() {
        let text = "one two three four five six seven eight nine ten";
        let mut p = parts(&style(10.0), &TextSizing::AutoHeight(120.0));
        p.block.max_lines = 1;
        p.block.overflow = TextOverflow::Ellipsis;

        let laid = layout(p.as_ref(text));
        assert!(laid.truncated, "fixture: one line cannot hold the content");
        let canvas_glyphs: usize = laid.runs.iter().map(|r| r.glyphs.len()).sum();

        let joined = |p: &TextParts| -> String {
            export_lines(p.as_ref(text))
                .iter()
                .flat_map(|l| l.runs.iter().map(|r| r.text.as_str()))
                .collect()
        };
        let exported = joined(&p);
        assert!(
            exported.ends_with('…'),
            "the export ends in the ellipsis the canvas drew: {exported:?}"
        );
        assert_eq!(
            exported.chars().count(),
            canvas_glyphs,
            "and the two walks put out the same number of characters: {exported:?}"
        );

        // The control: truncation alone is not what puts an ellipsis on the page.
        let mut clipped = p.clone();
        clipped.block.overflow = TextOverflow::Clip;
        assert!(
            layout(clipped.as_ref(text)).truncated,
            "control: the clipped node is truncated too"
        );
        assert!(
            !joined(&clipped).contains('…'),
            "and `Clip` still exports no ellipsis"
        );
    }

    /// A styled span exports as its own run carrying its own style.
    #[test]
    fn export_lines_splits_at_a_span_and_carries_its_style() {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.spans.set(3..6, CharAttr::Size(40.0), &p.style.clone());
        let lines = export_lines(p.as_ref("abcdef"));
        assert_eq!(lines.len(), 1, "no wrap here: {lines:?}");
        let sizes: Vec<f64> = lines[0].runs.iter().map(|r| r.style.font_size).collect();
        assert!(
            sizes.contains(&20.0) && sizes.contains(&40.0),
            "both sizes have to survive as separate runs: {sizes:?}"
        );
        let joined: String = lines[0].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(joined, "abcdef");
    }

    /// Parley's ranges index the cased string, so run text exports transformed
    /// exactly as the canvas drew it.
    #[test]
    fn export_lines_returns_the_cased_text_the_canvas_drew() {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.spans.set(
            0..3,
            CharAttr::Case(crate::typography::TextCase::Upper),
            &p.style.clone(),
        );
        let lines = export_lines(p.as_ref("abc"));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.runs.iter().map(|r| r.text.as_str()))
            .collect();
        assert_eq!(joined, "ABC", "upper-cased runs export upper-cased");
    }

    /// A registered family previews with real glyphs, a width, and a baseline
    /// inside the line box.
    #[test]
    fn a_registered_family_previews_in_its_own_face() {
        let preview = family_preview("Inter", "Inter", 15.0).expect("Inter is bundled");
        assert_eq!(preview.run.glyphs.len(), 5, "one glyph per letter of Inter");
        assert!(
            preview.run.glyphs.iter().all(|g| g.id != 0),
            "no .notdef in a name the face can draw"
        );
        assert!(preview.width > 0.0 && preview.height > 0.0);
        assert!(
            preview.baseline > 0.0 && preview.baseline < preview.height,
            "the baseline sits inside the line box: {} of {}",
            preview.baseline,
            preview.height
        );
    }

    /// An unregistered family has no preview rather than one set in the fallback face.
    #[test]
    fn an_unregistered_family_has_no_preview_rather_than_a_fallback_one() {
        assert!(
            family_preview("No Such Family At All", "No Such Family At All", 15.0).is_none(),
            "a family with no faces must not preview in the fallback face"
        );
    }

    /// A registered family that shapes only `.notdef` for the name has no preview.
    #[test]
    fn a_name_the_face_cannot_draw_has_no_preview_either() {
        let parts = TextParts {
            style: TextStyle {
                font_family: "Inter".into(),
                font_size: 15.0,
                ..TextStyle::default()
            },
            ..TextParts::default()
        };
        let laid = layout(parts.as_ref("漢字"));
        assert!(
            laid.runs.iter().any(|r| r.glyphs.iter().any(|g| g.id == 0)),
            "Inter has no CJK, so shaping this yields .notdef — if it stops doing \
             so this test is no longer about anything"
        );
        assert!(family_preview("Inter", "漢字", 15.0).is_none());
    }

    /// A preview-only family can be un-registered and re-registered; fontique keeps
    /// the name after the last face goes, so availability asks about faces. The test
    /// restores Inter before it ends.
    #[test]
    fn an_evicted_family_goes_away_and_can_come_back() {
        assert!(is_family_available("Inter"));
        assert!(unregister_family("Inter"), "faces were removed");
        assert!(
            !is_family_available("Inter"),
            "a name with no faces behind it is not an available family"
        );
        assert!(
            family_preview("Inter", "Inter", 15.0).is_none(),
            "and it cannot be previewed while it is gone"
        );

        register_fonts(INTER_UPRIGHT.to_vec());
        register_fonts(INTER_ITALIC.to_vec());
        assert!(
            is_family_available("Inter"),
            "re-registering restores it — the picker re-fetches an evicted family \
             from the disk cache and expects it to work"
        );
        assert!(family_preview("Inter", "Inter", 15.0).is_some());
    }

    fn style(font_size: f64) -> TextStyle {
        TextStyle {
            font_family: "Inter".into(),
            font_size,
            // 1.0 line height keeps the height arithmetic about the font size.
            line_height: Some(Length::Em(1.0)),
            ..TextStyle::default()
        }
    }

    fn measure_with(content: &str, style: &TextStyle, sizing: &TextSizing) -> Size {
        let p = parts(style, sizing);
        measure(p.as_ref(content)).size()
    }

    fn layout_with(content: &str, style: &TextStyle, sizing: &TextSizing) -> TextLayout {
        let p = parts(style, sizing);
        layout(p.as_ref(content))
    }

    /// An unknown family measures like the Inter fallback it is shaped in, including
    /// the optical-size axis; 96pt because `opsz` only diverges at large sizes.
    ///
    /// flip: `axes_for` returning no axes for an unknown family → fails at the second assertion
    #[test]
    fn an_unknown_family_measures_the_same_as_the_fallback_it_is_shaped_in() {
        let big = |family: &str| TextStyle {
            font_family: family.into(),
            font_size: 96.0,
            line_height: Some(Length::Em(1.0)),
            ..TextStyle::default()
        };
        let width =
            |family: &str| measure_with("Hamburgefonstiv", &big(family), &TextSizing::Auto).width;

        let inter = width("Inter");
        assert!(
            inter > 0.0,
            "the fixture must reach the state: the bundled face shapes at all"
        );

        let unknown = width("NoSuchFamilyAnywhere");
        assert!(
            (unknown - inter).abs() < 0.5,
            "an unknown family is shaped in the fallback face and must measure \
             like it: {unknown} against {inter}"
        );
    }

    /// `decoration_sizes` is non-zero and scales with the type. Inter's underline and
    /// strikethrough sizes are equal, so the test cannot tell them apart.
    ///
    /// flip: `extract` setting `decoration_sizes` to `None` → fails at the first `expect`
    #[test]
    fn a_shaped_node_reports_the_faces_own_decoration_thicknesses() {
        let (u, s) = layout_with("hello", &style(20.0), &TextSizing::Auto)
            .decoration_sizes
            .expect("a shaped node reports the face's own thicknesses");
        assert!(
            u > 0.0 && s > 0.0,
            "a zero seed is the bug this field closes, got ({u}, {s})"
        );

        let (u40, _) = layout_with("hello", &style(40.0), &TextSizing::Auto)
            .decoration_sizes
            .expect("the same at twice the size");
        assert!(
            (u40 - u * 2.0).abs() < 1e-6,
            "the metric scales with the type: {u40} against {u} doubled"
        );

        assert_eq!(
            layout_with("", &style(20.0), &TextSizing::Auto).decoration_sizes,
            None,
            "and a node with nothing shaped has no run to ask, which is the `None` \
             the panel falls back on"
        );
    }

    /// The seeded thickness draws a band where a zero thickness draws none.
    ///
    /// flip: `Px(seed)` replaced by `Px(0.0)` → fails at "the seeded thickness draws a band"
    #[test]
    fn the_seeded_thickness_draws_a_band_where_zero_draws_none() {
        let band_height = |thickness: Option<Length>| {
            let mut s = style(20.0);
            s.underline = Some(Decoration {
                thickness,
                ..Decoration::default()
            });
            let l = layout_with("hello", &s, &TextSizing::Auto);
            l.decorations.first().map_or(0.0, |d| d.band.height())
        };

        let seed = layout_with("hello", &style(20.0), &TextSizing::Auto)
            .decoration_sizes
            .expect("a shaped node")
            .0;
        assert!(
            band_height(Some(Length::Px(seed))) > 0.0,
            "the seeded thickness draws a band"
        );
        assert_eq!(
            band_height(Some(Length::Px(0.0))),
            0.0,
            "and the old seed draws nothing at all — the reported symptom"
        );
        assert!(
            (band_height(Some(Length::Px(seed))) - band_height(None)).abs() < 1e-6,
            "the seed is the value auto was already giving, not a near miss"
        );
    }

    #[test]
    fn fixed_sizing_returns_box_verbatim() {
        let s = measure_with(
            "anything",
            &style(10.0),
            &TextSizing::Fixed(Size::new(120.0, 40.0)),
        );
        assert_eq!(s, Size::new(120.0, 40.0));
    }

    #[test]
    fn auto_height_grows_with_line_count() {
        let one = measure_with("one line", &style(10.0), &TextSizing::Auto);
        let three = measure_with("a\nb\nc", &style(10.0), &TextSizing::Auto);
        assert!(three.height > one.height);
        assert!(
            (three.height - 30.0).abs() < 0.5,
            "expected ~30, got {}",
            three.height
        );
    }

    #[test]
    fn auto_width_grows_with_longest_line() {
        let short = measure_with("hi", &style(10.0), &TextSizing::Auto);
        let long = measure_with("hello there", &style(10.0), &TextSizing::Auto);
        assert!(long.width > short.width);
        assert!(short.width > 0.0);
    }

    #[test]
    fn empty_content_has_one_line_height() {
        let s = measure_with("", &style(10.0), &TextSizing::Auto);
        assert_eq!(s.width, 0.0);
        assert!(
            (s.height - 10.0).abs() < 0.5,
            "expected ~10, got {}",
            s.height
        );
    }

    #[test]
    fn layout_produces_glyphs_for_visible_text() {
        let tl = layout_with("Ag", &style(48.0), &TextSizing::Auto);
        let glyph_count: usize = tl.runs.iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(glyph_count, 2, "two characters should shape to two glyphs");
        assert!(tl.size.width > 0.0 && tl.size.height > 0.0);
    }

    #[test]
    fn bold_is_wider_than_thin_via_weight_axis() {
        let mut thin = style(48.0);
        thin.weight = 100;
        let mut black = style(48.0);
        black.weight = 900;
        let thin_w = measure_with("Weight", &thin, &TextSizing::Auto).width;
        let black_w = measure_with("Weight", &black, &TextSizing::Auto).width;
        assert!(
            black_w > thin_w,
            "heavier weight should advance wider: {black_w} vs {thin_w}"
        );
    }

    #[test]
    fn italic_selects_a_different_face_than_upright() {
        let upright = layout_with("italic", &style(24.0), &TextSizing::Auto);
        let mut it = style(24.0);
        it.italic = true;
        let italic = layout_with("italic", &it, &TextSizing::Auto);
        assert_ne!(
            upright.runs[0].font, italic.runs[0].font,
            "italic should resolve to a different face"
        );
    }

    #[test]
    fn unknown_family_falls_back_to_inter() {
        let mut s = style(20.0);
        s.font_family = "No Such Font 12345".into();
        let tl = layout_with("hello", &s, &TextSizing::Auto);
        let glyphs: usize = tl.runs.iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(glyphs, 5);
        assert!(tl.size.width > 0.0);
    }

    // --- the new attributes ----------------------------------------------

    #[test]
    fn letter_spacing_widens_the_line() {
        let plain = measure_with("tracking", &style(20.0), &TextSizing::Auto).width;
        let mut wide = style(20.0);
        wide.letter_spacing = Length::Px(2.0);
        let spaced = measure_with("tracking", &wide, &TextSizing::Auto).width;
        assert!(spaced > plain + 10.0, "{spaced} vs {plain}");
    }

    #[test]
    fn an_em_letter_spacing_scales_with_the_font_size() {
        let mut small = style(10.0);
        small.letter_spacing = Length::Em(0.1);
        let mut big = style(20.0);
        big.letter_spacing = Length::Em(0.1);
        let a = measure_with("AAAA", &small, &TextSizing::Auto).width;
        let b = measure_with("AAAA", &big, &TextSizing::Auto).width;
        assert!(
            (b / a - 2.0).abs() < 0.05,
            "doubling the size should double the line: {a} then {b}"
        );
    }

    #[test]
    fn word_spacing_only_moves_lines_with_spaces_in_them() {
        let mut s = style(20.0);
        s.word_spacing = Length::Px(10.0);
        let with_space = measure_with("a b", &s, &TextSizing::Auto).width;
        let plain = measure_with("a b", &style(20.0), &TextSizing::Auto).width;
        assert!(
            (with_space - plain - 10.0).abs() < 1.0,
            "{with_space} vs {plain}"
        );
    }

    #[test]
    fn the_three_line_height_modes_are_all_distinct() {
        let auto = {
            let mut s = style(20.0);
            s.line_height = None;
            measure_with("x", &s, &TextSizing::Auto).height
        };
        let relative = {
            let mut s = style(20.0);
            s.line_height = Some(Length::Em(2.0));
            measure_with("x", &s, &TextSizing::Auto).height
        };
        let absolute = {
            let mut s = style(20.0);
            s.line_height = Some(Length::Px(11.0));
            measure_with("x", &s, &TextSizing::Auto).height
        };
        assert!(
            (relative - 40.0).abs() < 0.5,
            "Em(2) of 20px, got {relative}"
        );
        assert!((absolute - 11.0).abs() < 0.5, "Px(11), got {absolute}");
        assert!(auto > 20.0 && (auto - relative).abs() > 1.0, "got {auto}");
    }

    #[test]
    fn an_underline_produces_a_band_below_the_baseline() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let tl = layout_with("under", &s, &TextSizing::Auto);
        assert_eq!(tl.decorations.len(), 1, "one run, one band");
        let band = tl.decorations[0].band;
        let baseline = tl.runs[0].glyphs[0].y as f64;
        assert!(
            band.y0 > baseline,
            "the band must sit below the baseline: {band:?} vs baseline {baseline}"
        );
        assert!(band.height() > 0.0 && band.width() > 0.0);
    }

    #[test]
    fn a_strikethrough_crosses_the_glyphs() {
        let mut s = style(40.0);
        s.strikethrough = Some(Decoration::default());
        let tl = layout_with("struck", &s, &TextSizing::Auto);
        let band = tl.decorations[0].band;
        let baseline = tl.runs[0].glyphs[0].y as f64;
        assert!(band.y1 < baseline, "must be above the baseline: {band:?}");
    }

    #[test]
    fn an_overridden_decoration_thickness_is_what_gets_drawn() {
        let mut s = style(40.0);
        s.underline = Some(Decoration {
            thickness: Some(Length::Px(6.0)),
            ..Decoration::default()
        });
        let tl = layout_with("thick", &s, &TextSizing::Auto);
        assert!((tl.decorations[0].band.height() - 6.0).abs() < 0.01);
    }

    #[test]
    fn a_decoration_span_decorates_only_its_own_range() {
        let d = style(24.0);
        let mut spans = CharSpans::default();
        spans.set(0..3, CharAttr::Underline(Some(Decoration::default())), &d);
        let p = TextParts {
            style: d.clone(),
            spans,
            ..TextParts::default()
        };
        let tl = layout(p.as_ref("abcdef"));
        assert_eq!(tl.decorations.len(), 1);
        let full = layout(
            TextParts {
                style: d,
                ..TextParts::default()
            }
            .as_ref("abcdef"),
        );
        assert!(
            tl.decorations[0].band.width() < full.size.width * 0.75,
            "the band should cover only 'abc'"
        );
    }

    /// The fraction of each glyph's own advance that keeps its underline in a
    /// single-run node; per glyph because skip-ink answers which letters break.
    fn kept_per_glyph(tl: &TextLayout, band_index: usize) -> Vec<f64> {
        let ink = &tl.decorations[band_index];
        let glyphs = &tl.runs[0].glyphs;
        let mut out = Vec::new();
        for (i, g) in glyphs.iter().enumerate() {
            let x0 = f64::from(g.x);
            let x1 = glyphs
                .get(i + 1)
                .map(|n| f64::from(n.x))
                .unwrap_or(ink.band.x1);
            let covered: f64 = ink
                .gaps
                .iter()
                .map(|(a, b)| (b.min(x1) - a.max(x0)).max(0.0))
                .sum();
            out.push(1.0 - covered / (x1 - x0));
        }
        out
    }

    /// The band breaks under the descenders of "gypsy" and nowhere else; the `p`
    /// keeps most of its rule, which rules out a per-glyph gap. Thresholds are loose
    /// because `SKIP_CLEARANCE` is tuned by eye.
    ///
    /// flip: `skip_ink` returning early → fails at the `g` assertion
    #[test]
    fn an_underlined_descender_breaks_the_band() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let tl = layout_with("gypsy", &s, &TextSizing::Auto);
        assert_eq!(tl.decorations.len(), 1, "one run, one band");
        assert_eq!(tl.runs[0].glyphs.len(), 5, "the fixture is five letters");
        let kept = kept_per_glyph(&tl, 0);
        for (i, letter) in "gypsy".chars().enumerate() {
            if letter == 's' {
                assert!(
                    kept[i] > 0.95,
                    "an s has nothing below the baseline, so its rule survives: \
                     kept {kept:?}"
                );
            } else {
                assert!(
                    kept[i] < 0.999,
                    "the {letter} descends through the band and must break it: \
                     kept {kept:?}"
                );
            }
        }
        assert!(
            kept[2] > 0.4,
            "only the p's stem crosses the band, not its bowl: kept {kept:?}"
        );
        let total: f64 = tl.decorations[0].gaps.iter().map(|(a, b)| b - a).sum();
        assert!(
            total < tl.decorations[0].band.width() * 0.75,
            "gaps {:?} of a band {:?}",
            tl.decorations[0].gaps,
            tl.decorations[0].band
        );
    }

    /// A strikethrough is never skipped: a line-through is meant to cross the letters.
    ///
    /// flip: `extract` pushing the strikethrough index into `underlines` → fails at the empty-gaps assertion
    #[test]
    fn a_strikethrough_is_never_skipped() {
        let mut s = style(40.0);
        s.strikethrough = Some(Decoration::default());
        let tl = layout_with("gypsy", &s, &TextSizing::Auto);
        let ink = &tl.decorations[0];
        let baseline = f64::from(tl.runs[0].glyphs[0].y);
        assert!(
            ink.band.y1 < baseline,
            "the fixture must be a band across the letters, not below them: {:?}",
            ink.band
        );
        assert!(
            ink.gaps.is_empty(),
            "a line-through crosses the ink on purpose: {:?}",
            ink.gaps
        );
    }

    /// An underline with `skip_ink` off keeps one unbroken bar, and a span can switch
    /// it off for its own run alone. The control checks the word does break by default.
    ///
    /// flip: `extract` pushing the index unconditionally → fails at the first arm's gaps assertion
    /// flip: `extract` reading `parts.style.underline` for the flag → fails only at the span arm's first gaps assertion
    #[test]
    fn an_underline_can_be_told_not_to_skip_and_a_span_can_say_it_alone() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let control = layout_with("gypsy", &s, &TextSizing::Auto);
        assert!(
            !control.decorations[0].gaps.is_empty(),
            "the fixture has to be a word that breaks its rule, or the arms below \
             prove nothing"
        );

        s.underline = Some(Decoration {
            skip_ink: false,
            ..Decoration::default()
        });
        let tl = layout_with("gypsy", &s, &TextSizing::Auto);
        assert_eq!(tl.decorations.len(), 1, "one run, one band");
        assert!(
            tl.decorations[0].gaps.is_empty(),
            "skipping is switched off, so the band is the whole bar: {:?}",
            tl.decorations[0].gaps
        );
        assert_eq!(
            tl.decorations[0].band, control.decorations[0].band,
            "and the band itself is untouched — the toggle breaks the ink, not the \
             geometry"
        );

        let mut d = style(40.0);
        d.underline = Some(Decoration::default());
        let mut spans = CharSpans::default();
        spans.set(
            0..2,
            CharAttr::Underline(Some(Decoration {
                skip_ink: false,
                ..Decoration::default()
            })),
            &d,
        );
        let split = layout(
            TextParts {
                style: d,
                spans,
                ..TextParts::default()
            }
            .as_ref("gypsy"),
        );
        assert_eq!(
            split.decorations.len(),
            2,
            "the span splits the run, so there are two bands to disagree: {:?}",
            split.decorations
        );
        assert!(
            split.decorations[0].gaps.is_empty(),
            "`gy` opted out even though both letters descend: {:?}",
            split.decorations[0].gaps
        );
        assert!(
            !split.decorations[1].gaps.is_empty(),
            "`psy` did not opt out and its p and y still break the rule: {:?}",
            split.decorations[1].gaps
        );
    }

    /// A word with nothing below the baseline keeps one unbroken rule: round overshoot
    /// must not break it. Pins the margin, not that clearance is horizontal-only:
    /// padding the band by a whole thickness in `crossings` stays green.
    #[test]
    fn an_underline_with_nothing_in_its_way_is_one_unbroken_bar() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let tl = layout_with("onmax", &s, &TextSizing::Auto);
        let ink = &tl.decorations[0];
        assert!(
            ink.gaps.is_empty(),
            "no letter of `onmax` descends, so nothing may break its rule: {:?}",
            ink.gaps
        );
        let mut edges = Vec::new();
        let mut path = BezPath::new();
        for run in &tl.runs {
            path.truncate(0);
            append_run_outline(run, &mut path);
            flatten_edges(&path, &mut edges);
        }
        let deepest = edges
            .iter()
            .map(|e| e[1].max(e[3]))
            .fold(f64::NEG_INFINITY, f64::max);
        let margin = (ink.band.y0 - deepest) / ink.band.height();
        assert!(
            (margin - 1.31).abs() < 0.05,
            "the margin this test is really about moved: {margin} thicknesses \
             (deepest ink {deepest}, band {:?})",
            ink.band
        );
    }

    /// A wavy underline skips what its crests reach: its gaps total more than the
    /// solid band's and each solid gap is mostly covered (not contained: `give_air`
    /// spares less on a wavy band's narrower bars).
    ///
    /// flip: the `Wavy` arm of `ink_extent` using the band's own reach → fails at the total assertion
    #[test]
    fn a_wavy_underline_skips_what_its_crests_reach() {
        let band = |line: LineStyle| {
            let mut s = style(40.0);
            s.underline = Some(Decoration {
                style: line,
                ..Decoration::default()
            });
            layout_with("gypsy", &s, &TextSizing::Auto).decorations[0].clone()
        };
        let solid = band(LineStyle::Solid);
        let wavy = band(LineStyle::Wavy);
        assert_eq!(solid.band, wavy.band, "same geometry, different drawing");
        let total = |ink: &DecorationInk| -> f64 { ink.gaps.iter().map(|(a, b)| b - a).sum() };
        assert!(
            total(&wavy) > total(&solid),
            "a ribbon reaches further than its band and must break more: \
             wavy {:?} vs solid {:?}",
            wavy.gaps,
            solid.gaps
        );
        for (a, b) in &solid.gaps {
            let shared = wavy
                .gaps
                .iter()
                .map(|(c, d)| (b.min(*d) - a.max(*c)).max(0.0))
                .fold(0.0_f64, f64::max);
            assert!(
                shared > (b - a) * 0.8,
                "every solid gap must be most of a wavy one: {:?} shares only \
                 {shared} of {a}..{b}",
                wavy.gaps
            );
        }
    }

    /// The widths of the bars an underlined `word` keeps at `size`, with the band height.
    fn bars_of(word: &str, size: f64) -> (Vec<f64>, f64) {
        let mut s = style(size);
        s.underline = Some(Decoration::default());
        let tl = layout_with(word, &s, &TextSizing::Auto);
        let ink = &tl.decorations[0];
        let bars = fragments(&ink.gaps, ink.band)
            .into_iter()
            .map(|(a, b)| b - a)
            .collect();
        (bars, ink.band.height())
    }

    /// Air narrows a bar and never removes one (`give_air`): long bars keep full
    /// clearance, a bar the clearance would erase is held at one thickness, a
    /// narrower one gets no air.
    ///
    /// flip: `give_air` called with a floor of `0.0` → fails at the `page` assertion
    #[test]
    fn air_narrows_a_bar_and_never_removes_one() {
        let close = |a: &[f64], b: &[f64], t: f64| -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x / t - y).abs() < 0.02)
        };
        let (page, t) = bars_of("page", 40.0);
        assert!(
            close(&page, &[0.90, 12.86, 8.07], t),
            "a long bar keeps its air and a short one keeps its ink: {page:?} / {t}"
        );
        let (gypsy, t) = bars_of("gypsy", 40.0);
        assert!(
            close(&gypsy, &[1.00, 1.00, 2.65, 11.85, 2.74], t),
            "{gypsy:?} / {t}"
        );
        let (g, t) = bars_of("g", 40.0);
        assert!(
            close(&g, &[1.00, 1.00], t),
            "a g keeps a square of ink either side of its tail: {g:?} / {t}"
        );
        let (j, t) = bars_of("j", 40.0);
        assert!(close(&j, &[0.95], t), "{j:?} / {t}");
    }

    /// A stem's crossing is the ink between its two vertical sides, which edge-by-edge
    /// measurement misses; the scanlines in `raw_crossings` supply it.
    ///
    /// flip: deleting the scanline loop → fails at the first assertion (no gap for the `p`)
    #[test]
    fn a_stems_crossing_is_the_ink_between_its_sides() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let tl = layout_with("p", &s, &TextSizing::Auto);
        let ink = &tl.decorations[0];
        assert_eq!(ink.gaps.len(), 1, "the p's stem breaks the band: {ink:?}");
        let (a, b) = ink.gaps[0];
        let t = ink.band.height();
        assert!(
            b - a > 2.0 * t,
            "the gap must cover the stem and not just the air: {} against {t}",
            b - a
        );
        assert!(
            fragments(&ink.gaps, ink.band).len() == 2,
            "one gap, two bars: {:?}",
            fragments(&ink.gaps, ink.band)
        );
    }

    /// The rule never vanishes at any size, and a lone `j`'s mark stays about one
    /// thickness wide: the floor holds the width, not scale invariance.
    #[test]
    fn a_narrow_bands_rule_never_vanishes_and_keeps_its_width_at_every_size() {
        for size in [
            6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 20.0, 24.0, 40.0, 100.0, 400.0,
        ] {
            for word in ["j", "g", "gg", "ggg", "jjj"] {
                let (bars, _) = bars_of(word, size);
                assert!(
                    !bars.is_empty(),
                    "`{word}` at {size}pt lost its rule entirely"
                );
            }
            let (j, t) = bars_of("j", size);
            assert_eq!(j.len(), 1, "one mark at {size}pt: {j:?}");
            let ratio = j[0] / t;
            assert!(
                (0.93..=1.01).contains(&ratio),
                "the j's mark is about a thickness wide at {size}pt: {ratio}"
            );
            let (g, _) = bars_of("g", size);
            assert_eq!(g.len(), 2, "two marks on a g at {size}pt: {g:?}");
        }
    }

    /// Each band of a wrapped, underlined paragraph breaks only on its own line's
    /// descenders. Only removing both the box reject's `gy` test and `crossings`'
    /// per-edge y test breaks it; either alone stays green.
    #[test]
    fn each_line_of_a_wrapped_paragraph_breaks_on_its_own_descenders() {
        let mut s = style(16.0);
        s.underline = Some(Decoration::default());
        let content = "the quick brown gypsy jumped happily over a lazy dog and \
                       kept going past every judge in the paragraph "
            .repeat(4);
        let tl = layout_with(&content, &s, &TextSizing::Fixed(Size::new(300.0, 800.0)));
        assert!(
            tl.decorations.len() > 8,
            "the fixture must wrap into many bands: {}",
            tl.decorations.len()
        );
        for ink in &tl.decorations {
            assert!(
                !ink.gaps.is_empty(),
                "every line of this text has a descender in it: {:?}",
                ink.band
            );
            let total: f64 = ink.gaps.iter().map(|(a, b)| b - a).sum();
            assert!(
                total < ink.band.width() * 0.6,
                "band {:?} lost {total} of {} to gaps — that is another line's ink",
                ink.band,
                ink.band.width()
            );
        }
    }

    // --- paragraph ---------------------------------------------------------

    #[test]
    fn a_first_line_indent_moves_the_first_line_only() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(400.0));
        p.paragraph.indent = Length::Px(50.0);
        let tl = layout(p.as_ref("one two three four five six seven eight nine ten more words"));
        let first = tl.runs[0].glyphs[0].x;
        let second_line_x = tl
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .find(|g| g.y > tl.runs[0].glyphs[0].y + 1.0)
            .map(|g| g.x)
            .expect("the text should have wrapped");
        assert!(
            first > second_line_x + 40.0,
            "indent should push the first line right: {first} vs {second_line_x}"
        );
    }

    #[test]
    fn hanging_inverts_which_lines_are_indented() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(400.0));
        p.paragraph.indent = Length::Px(50.0);
        p.paragraph.hanging = true;
        let text = "one two three four five six seven eight nine ten more words";
        let tl = layout(p.as_ref(text));
        let first = tl.runs[0].glyphs[0].x;
        let second_line_x = tl
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .find(|g| g.y > tl.runs[0].glyphs[0].y + 1.0)
            .map(|g| g.x)
            .expect("the text should have wrapped");
        assert!(
            second_line_x > first + 40.0,
            "hanging indents the continuation: {first} then {second_line_x}"
        );
    }

    #[test]
    fn no_wrap_keeps_one_line_however_narrow_the_box() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(30.0));
        p.paragraph.wrap = crate::typography::WrapMode::NoWrap;
        let one = layout(p.as_ref("several words that would otherwise wrap"));
        let mut wrapping = p.clone();
        wrapping.paragraph.wrap = crate::typography::WrapMode::Wrap;
        let many = layout(wrapping.as_ref("several words that would otherwise wrap"));
        assert!(
            many.size.height > one.size.height * 2.0,
            "no-wrap should be much shorter: {} vs {}",
            one.size.height,
            many.size.height
        );
    }

    #[test]
    fn centre_alignment_survives_a_line_wider_than_its_box() {
        // parley's `align_when_overflowing` defaults to false, which start-aligns.
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::Fixed(Size::new(40.0, 60.0)));
        p.paragraph.align = TextAlign::Center;
        p.paragraph.wrap = crate::typography::WrapMode::NoWrap;
        let tl = layout(p.as_ref("far too wide for this box"));
        let first_x = tl.runs[0].glyphs[0].x;
        assert!(
            first_x < 0.0,
            "a centred overflowing line starts left of the box: {first_x}"
        );
    }

    #[test]
    fn justifying_the_last_line_to_the_end_moves_it_right() {
        let d = style(16.0);
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let mut start = parts(&d, &TextSizing::AutoHeight(300.0));
        start.paragraph.align = TextAlign::Justify;
        let a = layout(start.as_ref(text));
        let mut end = start.clone();
        end.paragraph.justify_last = JustifyLast::End;
        let b = layout(end.as_ref(text));
        let last_x = |tl: &TextLayout| {
            let bottom = tl
                .runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| g.y)
                .fold(f32::MIN, f32::max);
            tl.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .filter(|g| (g.y - bottom).abs() < 0.5)
                .map(|g| g.x)
                .fold(f32::MAX, f32::min)
        };
        assert!(
            last_x(&b) > last_x(&a) + 5.0,
            "the last line should move right: {} then {}",
            last_x(&a),
            last_x(&b)
        );
    }

    /// `JustifyLast::Center` puts the last line at the midpoint of start and end, in
    /// both callers of `justify_shift` (`extract` and `export_lines`).
    ///
    /// flip: `Center => free` in `justify_shift` → fails at "centre is short of end"
    #[test]
    fn centring_the_last_line_puts_it_halfway_between_the_other_two() {
        let d = style(16.0);
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let mut p = parts(&d, &TextSizing::AutoHeight(300.0));
        p.paragraph.align = TextAlign::Justify;

        let last_x = |tl: &TextLayout| {
            let bottom = tl
                .runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| g.y)
                .fold(f32::MIN, f32::max);
            f64::from(
                tl.runs
                    .iter()
                    .flat_map(|r| r.glyphs.iter())
                    .filter(|g| (g.y - bottom).abs() < 0.5)
                    .map(|g| g.x)
                    .fold(f32::MAX, f32::min),
            )
        };
        let canvas = |j: JustifyLast| {
            let mut q = p.clone();
            q.paragraph.justify_last = j;
            last_x(&layout(q.as_ref(text)))
        };
        let (start, centre, end) = (
            canvas(JustifyLast::Start),
            canvas(JustifyLast::Center),
            canvas(JustifyLast::End),
        );
        assert!(
            end > start + 5.0,
            "fixture: the last line has room to move at all ({start} .. {end})"
        );
        assert!(
            centre > start + 1.0,
            "centre is past start: {start} {centre}"
        );
        assert!(centre < end - 1.0, "centre is short of end: {centre} {end}");
        assert!(
            (centre - (start + end) / 2.0).abs() < 1.0,
            "and it is the midpoint, not merely between: {start} {centre} {end}"
        );

        let exported = |j: JustifyLast| {
            let mut q = p.clone();
            q.paragraph.justify_last = j;
            export_lines(q.as_ref(text))
                .last()
                .expect("the fixture lays out")
                .x
        };
        let (start, centre, end) = (
            exported(JustifyLast::Start),
            exported(JustifyLast::Center),
            exported(JustifyLast::End),
        );
        assert!(
            (centre - (start + end) / 2.0).abs() < 1.0,
            "the export walk agrees with the canvas: {start} {centre} {end}"
        );
    }

    // --- block -------------------------------------------------------------

    #[test]
    fn max_lines_stops_the_layout_and_says_so() {
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(60.0));
        p.block.max_lines = 2;
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let capped = layout(p.as_ref(text));
        let mut free = p.clone();
        free.block.max_lines = 0;
        let uncapped = layout(free.as_ref(text));
        assert!(capped.truncated, "the content outran two lines");
        assert!(!uncapped.truncated);
        assert!(
            (capped.size.height - 20.0).abs() < 0.5,
            "two 10px lines, got {}",
            capped.size.height
        );
        assert!(uncapped.size.height > capped.size.height);
    }

    /// A fixed box told to clip stops at the line that would overflow it; `Visible`
    /// is the control for the same box and content.
    ///
    /// flip: `too_tall` forced to `false` in `break_lines` → fails at "and `Clip` says it stopped"
    #[test]
    fn a_fixed_box_stops_at_the_line_that_would_overflow_it() {
        let d = style(10.0);
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let laid = |sizing, overflow| {
            let mut p = parts(&d, &sizing);
            p.block.overflow = overflow;
            layout(p.as_ref(text))
        };
        let baselines = |tl: &TextLayout| tl.baselines.len();

        let box_size = Size::new(120.0, 25.0);
        let visible = laid(TextSizing::Fixed(box_size), TextOverflow::Visible);
        let clipped = laid(TextSizing::Fixed(box_size), TextOverflow::Clip);

        assert!(
            baselines(&visible) > 2,
            "fixture: the content overflows a 25-unit box, {} lines",
            baselines(&visible)
        );
        assert!(!visible.truncated, "control: `Visible` lets it hang out");
        assert!(clipped.truncated, "and `Clip` says it stopped");
        assert!(
            baselines(&clipped) < baselines(&visible),
            "the clipped box stops short: {} lines against {}",
            baselines(&clipped),
            baselines(&visible)
        );
    }

    /// A box shorter than one line still shows that line, so the node keeps its caret
    /// and identity (the `&& lines > 1` clause beside the revert).
    ///
    /// flip: removing `&& lines > 1` → fails at the baseline-count assertion (0 against 1)
    #[test]
    fn a_box_shorter_than_one_line_still_shows_that_line() {
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::Fixed(Size::new(120.0, 4.0)));
        p.block.overflow = TextOverflow::Clip;
        // The content must overflow the width too, or the breaker never reverts.
        let tl = layout(p.as_ref("one two three four five six seven eight nine ten eleven twelve"));
        assert_eq!(
            tl.baselines.len(),
            1,
            "one line survives a box too short to hold it"
        );
        assert!(tl.truncated, "and it is still reported as truncated");
    }

    #[test]
    fn ellipsis_adds_a_glyph_where_clip_does_not() {
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(60.0));
        p.block.max_lines = 1;
        let text = "one two three four five six seven eight";
        p.block.overflow = TextOverflow::Clip;
        let clipped: usize = layout(p.as_ref(text))
            .runs
            .iter()
            .map(|r| r.glyphs.len())
            .sum();
        p.block.overflow = TextOverflow::Ellipsis;
        let ellipsized: usize = layout(p.as_ref(text))
            .runs
            .iter()
            .map(|r| r.glyphs.len())
            .sum();
        assert_eq!(ellipsized, clipped + 1, "one ellipsis glyph");
    }

    /// An ellipsis is set in the type it follows, not the style at the end of the
    /// hidden tail; the span-free node is the control.
    ///
    /// flip: the style lookup back to `shaped.runs.last()` → fails at the font size (40 against 10)
    #[test]
    fn an_ellipsis_is_set_in_the_type_it_follows() {
        let measured = |span: Option<CharAttr>| -> (f32, f64) {
            let d = style(10.0);
            let mut p = parts(&d, &TextSizing::AutoHeight(120.0));
            p.block.max_lines = 1;
            p.block.overflow = TextOverflow::Ellipsis;
            let text = "one two three four five six seven eight nine ten eleven";
            if let Some(attr) = span {
                p.spans.set(40..text.len(), attr, &p.style);
            }
            let l = layout(p.as_ref(text));
            let size = l
                .runs
                .last()
                .map(|r| r.font_size)
                .expect("the fixture shapes and truncates");
            (size, l.size.height)
        };

        let (control, box_h) = measured(None);
        assert!(
            (control - 10.0).abs() < 0.01,
            "the fixture's ellipsis is the node's own 10pt: {control}"
        );
        let (styled, styled_box) = measured(Some(CharAttr::Size(40.0)));
        assert!(
            (styled - control).abs() < 0.01,
            "a span on text that is not drawn set the ellipsis at {styled}pt"
        );
        // The box is measured from the drawn line, so it must not move between runs.
        assert!(
            (styled_box - box_h).abs() < 0.01,
            "the box moved from {box_h} to {styled_box}, so the truncated tail is \
             reaching the layout as well as the ellipsis"
        );
    }

    /// An ellipsis never sits over the text it ends and never hangs past the wrap
    /// width; `Start` is the control that was already correct.
    ///
    /// flip: `ellipsis_run` placing at `x.min(w - advance)` without the line shift → fails at the overlap assertion for `Center`
    #[test]
    fn an_ellipsis_never_sits_over_the_text_it_ends() {
        for align in [TextAlign::Start, TextAlign::Center, TextAlign::End] {
            let d = style(10.0);
            let mut p = parts(&d, &TextSizing::AutoHeight(120.0));
            p.block.max_lines = 1;
            p.block.overflow = TextOverflow::Ellipsis;
            p.paragraph.align = align;
            let tl =
                layout(p.as_ref("one two three four five six seven eight nine ten eleven twelve"));

            let (ell, body) = tl.runs.split_last().expect("the fixture truncates");
            let at = f64::from(ell.glyphs.first().expect("one ellipsis glyph").x);
            let over = body
                .iter()
                .flat_map(|r| &r.glyphs)
                .filter(|g| f64::from(g.x) >= at - 0.01)
                .count();
            assert_eq!(
                over, 0,
                "{align:?}: {over} of the line's glyphs are at or past the ellipsis at {at}"
            );

            let advance = marker_advance(&d, ELLIPSIS);
            assert!(
                at + advance <= 120.0 + 0.01,
                "{align:?}: the ellipsis ends at {} and the box is 120",
                at + advance
            );
        }
    }

    #[test]
    fn trimming_tightens_the_box_without_moving_the_ink() {
        // Toggling trim must not shift a text node (`TextLayout::origin`).
        let mut d = style(40.0);
        d.line_height = None; // the font's own, so there is leading to trim
        let untrimmed = layout_with("Hxy", &d, &TextSizing::Auto);
        let mut p = parts(&d, &TextSizing::Auto);
        p.block.trim = BoxTrim::CapToBaseline;
        let trimmed = layout(p.as_ref("Hxy"));
        assert!(
            trimmed.size.height < untrimmed.size.height,
            "the box should tighten: {} vs {}",
            trimmed.size.height,
            untrimmed.size.height
        );
        assert!(trimmed.origin.y > 0.0, "the box starts below the origin");
        assert_eq!(
            trimmed.runs[0].glyphs[0], untrimmed.runs[0].glyphs[0],
            "the ink must not move"
        );
    }

    /// The line box comes back beside the trimmed one: it is the untrimmed height
    /// (trimmed plus both insets) and contains the trimmed box.
    #[test]
    fn the_line_box_contains_the_trimmed_box_and_is_taller_by_the_trim() {
        let mut d = style(40.0);
        d.line_height = None; // the font's own, so there is leading to trim
        let plain = layout_with("Hxy", &d, &TextSizing::Auto);
        let mut p = parts(&d, &TextSizing::Auto);
        p.block.trim = BoxTrim::CapToBaseline;
        let tl = layout(p.as_ref("Hxy"));

        let line = tl.untrimmed_bounds();
        assert!(
            line != tl.bounds(),
            "a trimmed node reported no line box, so nothing would be drawn: {line:?}"
        );
        assert!(
            (line.height() - plain.size.height).abs() < 1e-9,
            "line box {} should be the untrimmed height {}",
            line.height(),
            plain.size.height
        );
        assert_eq!(line.y0, 0.0, "the line box starts at the local origin");
        assert!(
            line.contains_rect(tl.bounds()),
            "{:?} does not contain {:?}",
            line,
            tl.bounds()
        );
        assert!(tl.bounds().y0 > line.y0, "trimmed from the top");
        assert!(tl.bounds().y1 < line.y1, "and from the bottom");
    }

    /// A node whose trim changed nothing reports no line box: an untrimmed one, and a
    /// `Fixed` one whose box is authored and trim only moves the alignment datum.
    #[test]
    fn a_node_whose_box_trim_did_not_change_reports_no_line_box() {
        let mut d = style(40.0);
        d.line_height = None;

        let none = layout_with("Hxy", &d, &TextSizing::Auto);
        assert_eq!(none.untrimmed_bounds(), none.bounds());

        // `Fixed` is smaller than the line box so a leak would show as a larger rectangle.
        let sizing = TextSizing::Fixed(Size::new(200.0, 12.0));
        let mut p = parts(&d, &sizing);
        p.block.trim = BoxTrim::CapToBaseline;
        let fixed = layout(p.as_ref("Hxy"));
        assert_eq!(
            fixed.untrimmed_bounds(),
            fixed.bounds(),
            "a Fixed node reported a line box, so the canvas would outline its content"
        );
        assert_eq!(fixed.bounds(), Rect::new(0.0, 0.0, 200.0, 12.0));
    }

    #[test]
    fn a_cap_trimmed_box_ends_on_the_baseline() {
        let mut d = style(40.0);
        d.line_height = None;
        let mut p = parts(&d, &TextSizing::Auto);
        p.block.trim = BoxTrim::CapToBaseline;
        let tl = layout(p.as_ref("H"));
        let baseline = f64::from(tl.runs[0].glyphs[0].y);
        assert!(
            (tl.bounds().y1 - baseline).abs() < 0.5,
            "box bottom {} should be the baseline {baseline}",
            tl.bounds().y1
        );
    }

    #[test]
    fn vertical_alignment_moves_the_text_inside_a_fixed_box() {
        let d = style(10.0);
        let sizing = TextSizing::Fixed(Size::new(200.0, 100.0));
        let top = {
            let mut p = parts(&d, &sizing);
            p.block.vertical_align = VerticalAlign::Top;
            layout(p.as_ref("x")).runs[0].glyphs[0].y
        };
        let middle = {
            let mut p = parts(&d, &sizing);
            p.block.vertical_align = VerticalAlign::Middle;
            layout(p.as_ref("x")).runs[0].glyphs[0].y
        };
        let bottom = {
            let mut p = parts(&d, &sizing);
            p.block.vertical_align = VerticalAlign::Bottom;
            layout(p.as_ref("x")).runs[0].glyphs[0].y
        };
        assert!(top < middle && middle < bottom, "{top} {middle} {bottom}");
        assert!((middle - top - 45.0).abs() < 1.0, "half of 100 - 10");
        assert!((bottom - top - 90.0).abs() < 1.0);
    }

    #[test]
    fn an_auto_box_ignores_vertical_alignment() {
        // No free space to distribute, so all three must agree.
        let d = style(10.0);
        let ys: Vec<f32> = VerticalAlign::ALL
            .iter()
            .map(|a| {
                let mut p = parts(&d, &TextSizing::Auto);
                p.block.vertical_align = *a;
                layout(p.as_ref("x")).runs[0].glyphs[0].y
            })
            .collect();
        assert!(ys.windows(2).all(|w| w[0] == w[1]), "{ys:?}");
    }

    // --- paragraph spacing, at both boundaries ----------------------------

    #[test]
    fn paragraph_spacing_pushes_later_paragraphs_down_and_grows_the_box() {
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::Auto);
        p.paragraph.spacing = Length::Px(20.0);
        let spaced = layout(p.as_ref("a\nb\nc"));
        let plain = layout_with("a\nb\nc", &d, &TextSizing::Auto);
        assert!(
            (spaced.size.height - plain.size.height - 40.0).abs() < 0.5,
            "two gaps of 20: {} vs {}",
            spaced.size.height,
            plain.size.height
        );
    }

    // --- list markers ------------------------------------------------------

    fn list_parts(style: &TextStyle, marker: ListMarker) -> TextParts {
        let mut p = parts(style, &TextSizing::Auto);
        p.paragraph.marker = Some(marker);
        p.paragraph.indent_start = Length::Px(30.0);
        p
    }

    /// The runs a layout holds, as `(first glyph x, glyph ids)`.
    fn run_starts(tl: &TextLayout) -> Vec<(f32, Vec<u32>)> {
        tl.runs
            .iter()
            .map(|r| {
                (
                    r.glyphs.first().map(|g| g.x).unwrap_or(0.0),
                    r.glyphs.iter().map(|g| g.id).collect(),
                )
            })
            .collect()
    }

    /// A marker is ink, not content: the content is untouched, the marker draws left
    /// of the text column, and the caret's byte space has not moved.
    #[test]
    fn a_list_marker_is_drawn_without_entering_the_content() {
        let d = style(20.0);
        let p = list_parts(&d, ListMarker::Disc);
        let e = TextEdit::new("ab", p);
        assert_eq!(e.content(), "ab", "the marker is not in the string");
        assert_eq!(
            e.selected_range(),
            2..2,
            "and the caret at the end is at byte 2, not past a marker"
        );

        let tl = e.text_layout();
        let runs = run_starts(&tl);
        assert_eq!(runs.len(), 2, "one text run and one marker run: {runs:?}");
        let (text_x, _) = runs[0];
        let (marker_x, ref ids) = runs[1];
        assert_eq!(text_x, 30.0, "the text is at `indent_start`");
        assert!(
            marker_x < text_x && marker_x > 0.0,
            "the marker sits in the gutter, not on the text and not off the box: {runs:?}"
        );
        assert_eq!(ids.as_slice(), &[1462], "Inter's U+2022 (see ListMarker)");
    }

    /// `1.` and `10.` put their text in the same column because markers are
    /// right-aligned rather than laid out in flow.
    #[test]
    fn ordered_markers_are_right_aligned_so_the_text_column_holds() {
        let d = style(20.0);
        let content = (1..=10).map(|_| "x").collect::<Vec<_>>().join("\n");
        let e = TextEdit::new(&content, list_parts(&d, ListMarker::Decimal));
        let tl = e.text_layout();
        // Ten text runs then ten markers: `extract` appends markers after the line walk.
        let runs = run_starts(&tl);
        assert_eq!(runs.len(), 20, "{runs:?}");
        let (text, markers) = runs.split_at(10);
        assert!(
            text.iter().all(|(x, _)| *x == 30.0),
            "every item's text is in one column: {text:?}"
        );
        let first = markers[0].0;
        let tenth = markers[9].0;
        assert!(
            tenth < first,
            "the two-digit marker starts further left, its right edge being the fixed \
             one: `1.` at {first} vs `10.` at {tenth}"
        );
        assert_eq!(
            markers[9].1.len(),
            3,
            "`10.` is three glyphs: {:?}",
            markers[9].1
        );
    }

    /// Numbering restarts at the first neighbour that has no marker or a different
    /// one; the two boundaries are separate code paths in `list_ordinals`.
    #[test]
    fn numbering_restarts_where_the_marker_changes_or_stops() {
        let d = style(20.0);
        let content = "a\nb\nc\nd\ne";
        let third = 4..5;

        let markers = |interrupt: ParaAttr| {
            let mut p = list_parts(&d, ListMarker::Decimal);
            let flat = p.paragraph.clone();
            p.para_spans.set(third.clone(), interrupt, &flat);
            let tl = TextEdit::new(content, p).text_layout();
            run_starts(&tl)
                .into_iter()
                .skip(5)
                .map(|(_, ids)| ids)
                .collect::<Vec<_>>()
        };

        let m = markers(ParaAttr::Marker(None));
        assert_eq!(m.len(), 4, "four items, the third opted out: {m:?}");
        assert_ne!(m[0], m[1], "1. is not 2.");
        assert_eq!(m[0], m[2], "the run restarts after the gap");
        assert_eq!(m[1], m[3]);

        let m = markers(ParaAttr::Marker(Some(ListMarker::Disc)));
        assert_eq!(m.len(), 5, "all five are items now: {m:?}");
        assert_eq!(m[2].len(), 1, "the interrupting bullet is one glyph: {m:?}");
        assert_eq!(
            m[0], m[3],
            "and the numbered run restarts after it rather than counting through — \
             a bullet between two numbers is a different list"
        );
        assert_eq!(m[1], m[4]);
    }

    /// A marker sits on its paragraph's own baseline, so paragraph spacing and box trim
    /// apply to it for free; it adds no metric to `trim_insets` because it shares the
    /// text's face and size.
    #[test]
    fn a_marker_shares_its_paragraphs_baseline_through_the_y_map() {
        let d = style(20.0);
        let mut p = list_parts(&d, ListMarker::Decimal);
        p.paragraph.spacing = Length::Px(40.0);
        p.block.trim = BoxTrim::CapToBaseline;
        let tl = TextEdit::new("a\nb\nc", p).text_layout();
        let ys: Vec<f32> = tl
            .runs
            .iter()
            .map(|r| r.glyphs.first().map(|g| g.y).unwrap_or(f32::NAN))
            .collect();
        assert_eq!(ys.len(), 6, "three items and three markers: {ys:?}");
        let (text, markers) = ys.split_at(3);
        assert_eq!(
            text, markers,
            "each marker is on the baseline of the item it belongs to, gap and trim \
             included: text {text:?} vs markers {markers:?}"
        );
        assert!(
            text[1] - text[0] > 50.0,
            "20pt lines 40pt apart step by 60: {text:?}"
        );
    }

    /// The counter systems at their edges, where reading the code is least reliable.
    #[test]
    fn the_counter_systems_match_css() {
        use crate::typography::ListMarker as M;
        assert_eq!(M::Decimal.text(1), "1.");
        assert_eq!(M::LowerAlpha.text(26), "z.");
        assert_eq!(M::LowerAlpha.text(27), "aa.");
        assert_eq!(M::UpperAlpha.text(28), "AB.");
        assert_eq!(M::LowerRoman.text(4), "iv.");
        assert_eq!(M::UpperRoman.text(1990), "MCMXC.");
        // Out of system, decimal rather than nothing.
        assert_eq!(M::UpperRoman.text(4000), "4000.");
        assert_eq!(M::LowerAlpha.text(0), "0.");
        // Unordered markers ignore the number entirely.
        assert_eq!(M::Disc.text(7), M::Disc.text(1));
    }

    // --- nested lists ------------------------------------------------------

    /// One paragraph of one letter per entry in `levels`, each at the level given;
    /// the span sits on the first byte, where `Paragraphs::new` resolves the style.
    fn levelled(style: &TextStyle, marker: ListMarker, levels: &[u8]) -> (String, TextParts) {
        let content = levels
            .iter()
            .enumerate()
            .fold(String::new(), |mut s, (i, _)| {
                if i > 0 {
                    s.push('\n');
                }
                s.push((b'a' + i as u8) as char);
                s
            });
        let mut p = list_parts(style, marker);
        let flat = p.paragraph.clone();
        for (index, level) in levels.iter().enumerate() {
            let start = index * 2;
            p.para_spans
                .set(start..start + 1, ParaAttr::Level(*level), &flat);
        }
        (content, p)
    }

    /// Every string the exporter emits, markers and text interleaved; a text run
    /// carries its own hard break, so expected vectors read `"a\n"`.
    fn export_texts(parts: TextRef<'_>) -> Vec<String> {
        export_lines(parts)
            .into_iter()
            .flat_map(|l| l.runs.into_iter().map(|r| r.text))
            .collect()
    }

    /// A level steps the start edge by one more gutter and takes the marker with it;
    /// the text and the marker are two readers of `ParagraphStyle::start_edge`.
    #[test]
    fn a_nested_item_puts_its_text_and_its_marker_one_gutter_further_in() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 1]);
        let tl = TextEdit::new(&content, p).text_layout();
        let runs = run_starts(&tl);
        assert_eq!(runs.len(), 4, "two items and two markers: {runs:?}");
        let (text, markers) = runs.split_at(2);
        assert_eq!(text[0].0, 30.0, "the outer item is at one gutter");
        assert_eq!(text[1].0, 60.0, "the nested one at two");
        assert!(
            markers[0].0 < 30.0,
            "the outer marker hangs in the first gutter: {markers:?}"
        );
        assert!(
            markers[1].0 > 30.0 && markers[1].0 < 60.0,
            "and the nested marker in the second — not in the outer item's gutter, \
             which is where a multiply applied to the lines alone would leave it: \
             {markers:?}"
        );
    }

    /// A nested run counts from one while its parent counts through it: item `d`
    /// follows two nested items and is still `2.`.
    #[test]
    fn a_nested_run_numbers_from_one_while_its_parent_counts_through() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 1, 1, 0, 0]);
        let texts = export_texts(p.as_ref(&content));
        assert_eq!(
            texts,
            vec![
                "1.", "a\n", "1.", "b\n", "2.", "c\n", "2.", "d\n", "3.", "e"
            ],
            "the outer list is 1, 2, 3 with a sublist of 1, 2 inside its first item"
        );
    }

    /// A skipped level (level 2 under level 0) gets no count of its own, so the first
    /// level-1 item after it is `1.`; filling skipped levels would number it `2.`.
    #[test]
    fn a_level_skipped_on_the_way_in_lends_no_count_to_one_arriving_later() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 2, 1]);
        let texts = export_texts(p.as_ref(&content));
        assert_eq!(texts, vec!["1.", "a\n", "1.", "b\n", "1.", "c"]);
    }

    /// A paragraph that is not an item ends every list at every level; item `d`
    /// would be `2.` if the break cleared only its own level and deeper.
    #[test]
    fn a_paragraph_with_no_marker_ends_every_open_level() {
        let d = style(20.0);
        let (content, mut p) = levelled(&d, ListMarker::Decimal, &[0, 1, 0, 0]);
        let flat = p.paragraph.clone();
        p.para_spans.set(4..5, ParaAttr::Marker(None), &flat);
        let texts = export_texts(p.as_ref(&content));
        assert_eq!(
            texts,
            vec!["1.", "a\n", "1.", "b\n", "c\n", "1.", "d"],
            "`c` is not an item, and `d` starts a new list rather than continuing `a`'s"
        );
    }

    /// The same marker at two levels is two lists.
    #[test]
    fn one_marker_at_two_levels_is_two_lists() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::LowerAlpha, &[0, 1, 1, 1]);
        let texts = export_texts(p.as_ref(&content));
        assert_eq!(
            texts,
            vec!["a.", "a\n", "a.", "b\n", "b.", "c\n", "c.", "d"]
        );
    }

    // --- Tab and Shift+Tab in a live session -------------------------------

    /// The level of every paragraph of a live editor, in order.
    fn levels(e: &TextEdit) -> Vec<u8> {
        let content = e.content().to_string();
        paragraph_starts(&content)
            .into_iter()
            .map(|s| {
                e.para_spans
                    .resolve(&e.paragraph, s.min(content.len().saturating_sub(1)))
                    .level
            })
            .collect()
    }

    /// `Tab` shifts each selected item by one rather than setting one absolute level,
    /// which would flatten a parent and its sublist.
    #[test]
    fn tab_shifts_each_selected_item_rather_than_levelling_them() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 1, 1]);
        let mut e = TextEdit::new(&content, p);
        assert_eq!(levels(&e), vec![0, 1, 1], "the fixture is nested to start");
        e.select_all();
        assert!(e.step_list_level(1));
        assert_eq!(levels(&e), vec![1, 2, 2], "each item moved by one");
        assert!(e.step_list_level(-1));
        assert_eq!(levels(&e), vec![0, 1, 1], "and back");
    }

    /// A bare caret nests its own paragraph and nothing else.
    #[test]
    fn tab_with_a_bare_caret_nests_the_paragraph_it_sits_in() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 0, 0]);
        let mut e = TextEdit::new(&content, p);
        e.move_up(false);
        assert_eq!(
            paragraphs_touched(e.content(), e.selected_range()),
            1..2,
            "the caret is in the second paragraph"
        );
        assert!(e.step_list_level(1));
        assert_eq!(levels(&e), vec![0, 1, 0]);
    }

    /// The level bounds are per paragraph, and a step that moves nothing reports
    /// `false` so the caller leaves the document alone.
    #[test]
    fn the_level_bounds_are_per_paragraph_and_a_no_op_reports_itself() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 1, 1]);
        let mut e = TextEdit::new(&content, p);
        e.select_all();
        assert!(e.step_list_level(-1), "two of the three can still go out");
        assert_eq!(levels(&e), vec![0, 0, 0]);
        assert!(
            !e.step_list_level(-1),
            "and now none of them can, which is not an edit"
        );
        assert_eq!(levels(&e), vec![0, 0, 0]);

        for _ in 0..MAX_LIST_LEVEL + 4 {
            e.step_list_level(1);
        }
        assert_eq!(levels(&e), vec![MAX_LIST_LEVEL; 3], "clamped, not wrapped");
        assert!(!e.step_list_level(1));
    }

    /// Only list items move; the panel's Level field is reachable only through an item.
    #[test]
    fn tab_leaves_a_paragraph_that_is_not_a_list_item_alone() {
        let d = style(20.0);
        let (content, mut p) = levelled(&d, ListMarker::Decimal, &[0, 0, 0]);
        let flat = p.paragraph.clone();
        p.para_spans.set(2..3, ParaAttr::Marker(None), &flat);
        let mut e = TextEdit::new(&content, p);
        e.select_all();
        assert!(e.step_list_level(1));
        assert_eq!(
            levels(&e),
            vec![1, 0, 1],
            "the items either side moved and the plain paragraph did not"
        );
    }

    /// A text node with no list is untouched and the caller is told so, so no undo
    /// step is left behind.
    #[test]
    fn tab_in_ordinary_text_changes_nothing_and_says_so() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::Auto);
        p.paragraph.indent_start = Length::Px(30.0);
        let mut e = TextEdit::new("a\nb", p);
        e.select_all();
        assert!(!e.step_list_level(1));
        assert_eq!(levels(&e), vec![0, 0]);
    }

    /// The empty paragraph after a trailing break can be nested alone: it owns no
    /// bytes, so a write by byte overlap would also move the paragraph above.
    #[test]
    fn tab_on_the_empty_paragraph_after_a_break_nests_only_that_one() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0]);
        let mut e = TextEdit::new(&content, p);
        e.select_all();
        e.move_line_end(false);
        e.insert("\n");
        assert_eq!(
            e.content(),
            "a\n",
            "one item and an empty paragraph after it"
        );
        assert_eq!(levels(&e), vec![0, 0]);
        assert!(e.step_list_level(1));
        assert_eq!(
            levels(&e),
            vec![0, 1],
            "the new paragraph nested and the item above it stayed put"
        );
    }

    /// `paragraphs_touched` (indices) and `paragraph_bounds` (bytes) agree for every
    /// selection except a caret in the empty tail after a trailing break, where the
    /// byte span reaches back over the break; that one divergence is the point.
    #[test]
    fn the_paragraphs_touched_are_the_ones_paragraph_bounds_spans() {
        let content = "one\ntwo\nthree\n";
        let starts = paragraph_starts(content);
        assert_eq!(starts, vec![0, 4, 8, 14], "three lines and the empty tail");
        let mut divergences = 0;
        for start in 0..=content.len() {
            for end in start..=content.len() {
                let range = start..end;
                let touched = paragraphs_touched(content, range.clone());
                let bytes = paragraph_bounds(content, range.clone());
                let first = starts.partition_point(|s| *s <= bytes.start) - 1;
                let last = starts.partition_point(|s| *s < bytes.end) - 1;
                if starts[touched.start] == content.len() {
                    divergences += 1;
                    assert_eq!(range, 14..14, "only a caret in the empty tail");
                    assert_eq!(touched, 3..4, "which is the paragraph it is in");
                    assert_eq!(bytes, 13..14, "reaching back over its own break");
                    assert_eq!(
                        first..last + 1,
                        2..3,
                        "so read as bytes it is the paragraph above — the ambiguity a \
                         per-paragraph write cannot live with"
                    );
                    continue;
                }
                assert_eq!(
                    touched,
                    first..last + 1,
                    "{range:?} → indices {touched:?} but bytes {bytes:?}"
                );
            }
        }
        assert_eq!(divergences, 1, "and it is the only one");
    }

    /// The distinct first-glyph baselines of the runs, ascending, in local space.
    fn baselines(tl: &TextLayout) -> Vec<f32> {
        let mut ys: Vec<f32> = tl
            .runs
            .iter()
            .filter_map(|r| r.glyphs.first().map(|g| g.y))
            .collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        ys
    }

    #[test]
    fn a_click_between_spaced_paragraphs_lands_on_the_middle_one() {
        // The middle paragraph, not the last: a click on the last line clamps there
        // even without the inverse y map.
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::Auto);
        p.paragraph.spacing = Length::Px(40.0);
        let mut e = TextEdit::new("a\nb\nc", p.clone());
        let ys = baselines(&e.text_layout());
        assert_eq!(ys.len(), 3, "three paragraphs, three baselines: {ys:?}");
        e.click(Point::new(0.0, f64::from(ys[1]) - 2.0));
        e.insert(">");
        assert_eq!(
            e.content(),
            "a\n>b\nc",
            "the caret should land at the start of the middle paragraph"
        );
    }

    /// A paragraph gap belongs to the two lines that bound it and no other
    /// (`Shaped::layout_y` resolves the line first).
    #[test]
    fn a_click_in_a_paragraph_gap_lands_on_a_line_that_bounds_it() {
        let mut p = parts(
            &TextStyle {
                font_size: 20.0,
                line_height: Some(Length::Em(1.0)),
                ..style(20.0)
            },
            // Narrow, so the second paragraph wraps and a third line exists for a
            // gap click to wrongly reach.
            &TextSizing::Fixed(Size::new(60.0, 400.0)),
        );
        p.paragraph.spacing = Length::Px(40.0);
        assert_eq!(
            TextEdit::new("a\nbb bb bb", p.clone()).shaped.layout.len(),
            3,
            "one line, then two wrapped"
        );
        // Local space: line 0 is 0..20, the gap 20..60, line 1 60..80, line 2 80..100.
        let line_at = |y: f64| {
            let mut c = TextEdit::new("a\nbb bb bb", p.clone());
            c.click(Point::new(1.0, y));
            c.caret_line_index()
        };
        for (y, want) in [
            (10.0, 0),
            // The gap splits at its middle between the two lines that bound it.
            (25.0, 0),
            (40.0, 0),
            (42.5, 1),
            (57.5, 1),
            // Line 1's own ink starts at local 58, two above its box.
            (59.0, 1),
            (70.0, 1),
            (90.0, 2),
        ] {
            assert_eq!(line_at(y), want, "local y {y} should land on line {want}");
        }
    }

    #[test]
    fn a_click_in_a_bottom_aligned_box_lands_on_the_line_under_the_pointer() {
        // The click aims at the first line: the last would clamp without the map.
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::Fixed(Size::new(200.0, 100.0)));
        p.block.vertical_align = VerticalAlign::Bottom;
        let mut e = TextEdit::new("a\nb", p);
        let ys = baselines(&e.text_layout());
        assert_eq!(ys.len(), 2, "{ys:?}");
        assert!(ys[0] > 80.0, "bottom-aligned text sits low: {ys:?}");
        e.click(Point::new(0.0, f64::from(ys[0]) - 2.0));
        e.insert("[");
        assert_eq!(e.content(), "[a\nb");
    }

    // --- case --------------------------------------------------------------

    #[test]
    fn upper_case_shapes_capitals_without_touching_the_content() {
        let mut d = style(20.0);
        d.case = TextCase::Upper;
        let upper = measure_with("abc", &d, &TextSizing::Auto).width;
        let plain = measure_with("abc", &style(20.0), &TextSizing::Auto).width;
        let real_caps = measure_with("ABC", &style(20.0), &TextSizing::Auto).width;
        assert!((upper - real_caps).abs() < 0.01, "{upper} vs {real_caps}");
        assert!(upper > plain);
    }

    #[test]
    fn a_case_transform_that_would_change_the_byte_count_is_declined() {
        // ß→SS would break every downstream byte index, so the character is left as typed.
        let mut d = style(20.0);
        d.case = TextCase::Upper;
        let cased = measure_with("straße", &d, &TextSizing::Auto).width;
        let ss = measure_with("STRASSE", &style(20.0), &TextSizing::Auto).width;
        assert!(cased < ss, "{cased} should be narrower than {ss}");
    }

    #[test]
    fn title_case_capitalises_each_word() {
        let mut d = style(20.0);
        d.case = TextCase::Title;
        let titled = measure_with("hello there", &d, &TextSizing::Auto).width;
        let typed = measure_with("Hello There", &style(20.0), &TextSizing::Auto).width;
        assert!((titled - typed).abs() < 0.01, "{titled} vs {typed}");
    }

    // --- baseline shift ---------------------------------------------------

    #[test]
    fn baseline_shift_lifts_a_run_without_moving_the_line() {
        let d = style(20.0);
        let mut spans = CharSpans::default();
        spans.set(1..2, CharAttr::BaselineShift(Length::Px(8.0)), &d);
        let p = TextParts {
            style: d.clone(),
            spans,
            ..TextParts::default()
        };
        let tl = layout(p.as_ref("x2y"));
        let plain = layout(
            TextParts {
                style: d,
                ..TextParts::default()
            }
            .as_ref("x2y"),
        );
        assert_eq!(tl.size, plain.size, "a draw offset must not change the box");
        let ys: Vec<f32> = tl
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.y)
            .collect();
        assert!(
            ys.iter().any(|y| (*y - ys[0]).abs() > 7.0),
            "one run should sit 8px higher: {ys:?}"
        );
    }

    // --- baselines ---------------------------------------------------------

    /// One baseline per line, ascending, and a shifted run adds no phantom baseline
    /// (`baseline_shift` offsets glyphs, not the line); the shift is on line two.
    ///
    /// flip: filling `baselines` from the glyphs' `y` → fails at the count (3 against 2)
    #[test]
    fn every_line_has_one_baseline_and_a_shifted_run_does_not_add_another() {
        let d = style(20.0);
        let mut spans = CharSpans::default();
        let content = "first\nx2y";
        let two = content.find('2').expect("the fixture has a 2");
        spans.set(two..two + 1, CharAttr::BaselineShift(Length::Px(8.0)), &d);
        let tl = layout(
            TextParts {
                style: d,
                spans,
                ..TextParts::default()
            }
            .as_ref(content),
        );

        assert_eq!(
            tl.baselines.len(),
            2,
            "two lines, two baselines — a third is the shifted run being counted \
             as a line of its own: {:?}",
            tl.baselines
        );
        assert!(
            tl.baselines[0] < tl.baselines[1],
            "baselines run top to bottom: {:?}",
            tl.baselines
        );

        let ys: Vec<f32> = tl
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.y)
            .collect();
        let second = tl.baselines[1] as f32;
        assert!(
            ys.iter().any(|y| (*y - second).abs() > 7.0),
            "the fixture: no glyph sits clear of line two's baseline {second}, so \
             the shift did not take and nothing here is being tested: {ys:?}"
        );
    }

    /// An empty node has at most one baseline, the arm a `[0]` read would panic on.
    #[test]
    fn an_empty_node_reports_no_baselines() {
        let tl = layout(
            TextParts {
                style: style(20.0),
                ..TextParts::default()
            }
            .as_ref(""),
        );
        assert!(
            tl.baselines.len() <= 1,
            "an empty node has at most the one line parley keeps for the caret: {:?}",
            tl.baselines
        );
    }

    // --- font metadata ----------------------------------------------------

    /// The variant list reaches the family's second file: Inter's italic cuts are
    /// named in the italic face's `fvar` and neither face has an `ital` axis.
    #[test]
    fn inters_variant_list_offers_both_slopes() {
        let variants = family_variants("Inter");
        let names: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();
        for want in ["Regular", "Bold", "Italic", "Bold Italic"] {
            assert!(names.contains(&want), "no {want:?} in {names:?}");
        }
        for italic in [false, true] {
            assert!(
                variants
                    .iter()
                    .any(|v| v.weight == 700 && v.italic == italic),
                "no bold at italic={italic} in {variants:?}"
            );
        }
    }

    /// The static-family arm of `family_variants`, which core's variable-only fonts
    /// never reach: `ordered_cuts` sorts uprights before italics and dedups. Tests the
    /// decisions, not the parley query above them.
    ///
    /// flip: sort tuple reversed to `(v.weight, v.italic)` → fails at the order assertion
    /// flip: the `dedup_by` disabled → fails at the same assertion
    #[test]
    fn a_static_family_offers_one_variant_per_cut_in_weight_order() {
        let cut = |weight: u16, italic: bool| FontVariant {
            name: face_name(weight, italic),
            weight,
            italic,
            coords: Vec::new(),
        };
        // Out of order with one cut twice; the 400 italic sorts before the 700
        // upright on weight and after it on slope, so the two key orders disagree.
        let mut got = vec![
            cut(700, false),
            cut(400, true),
            cut(400, false),
            cut(700, false),
        ];
        ordered_cuts(&mut got);
        assert_eq!(
            got.iter().map(|v| (v.weight, v.italic)).collect::<Vec<_>>(),
            vec![(400, false), (700, false), (400, true)],
            "uprights before italics, ascending weight within each slope, \
             each cut once"
        );
    }

    /// `face_name`'s weight buckets at their boundaries, where an off-by-one shows,
    /// and the special case `("Regular", true)` named "Italic".
    #[test]
    fn face_name_names_every_weight_bucket_at_its_own_boundary() {
        for (weight, want) in [
            (1u16, "Thin"),
            (149, "Thin"),
            (150, "Extra Light"),
            (249, "Extra Light"),
            (250, "Light"),
            (349, "Light"),
            (350, "Regular"),
            (449, "Regular"),
            (450, "Medium"),
            (549, "Medium"),
            (550, "Semi Bold"),
            (649, "Semi Bold"),
            (650, "Bold"),
            (749, "Bold"),
            (750, "Extra Bold"),
            (849, "Extra Bold"),
            (850, "Black"),
            (1000, "Black"),
        ] {
            assert_eq!(face_name(weight, false), want, "upright at {weight}");
        }
        assert_eq!(
            face_name(400, true),
            "Italic",
            "the one case that is not `{{base}} Italic`"
        );
        assert_eq!(face_name(700, true), "Bold Italic");
        assert_eq!(face_name(150, true), "Extra Light Italic");
    }

    #[test]
    fn inter_reports_its_own_axes() {
        let axes = family_axes("Inter");
        assert!(
            axes.iter().any(|a| a.tag.to_string() == "wght"),
            "expected a weight axis, got {:?}",
            axes.iter().map(|a| a.tag.to_string()).collect::<Vec<_>>()
        );
        let wght = axes
            .iter()
            .find(|a| a.tag == crate::typography::WGHT)
            .unwrap();
        assert!(wght.min < wght.max, "{wght:?}");
        assert!(wght.min <= 400.0 && 400.0 <= wght.max);
    }

    /// A reversed or `NaN` axis range must not panic the opsz clamp; `f64::min`/`max`
    /// swallow `NaN`, hence two assertions. Built through `from_fvar`.
    ///
    /// flip: returning `(a, b)` unordered → fails inside `from_fvar` with the clamp panic
    #[test]
    fn a_font_axis_whose_range_is_reversed_or_not_a_number_is_still_usable() {
        let axis = |min, max| {
            FontAxis::from_fvar(
                crate::typography::OPSZ,
                min,
                14.0,
                max,
                "opsz".into(),
                false,
            )
        };

        let backwards = axis(200.0, 32.0);
        assert!(
            backwards.min <= backwards.max,
            "the ends come back in order"
        );
        assert_eq!((backwards.min, backwards.max), (32.0, 200.0));
        assert_eq!(backwards.clamp(16.0), 32.0, "and clamping does not panic");
        assert_eq!(backwards.clamp(400.0), 200.0);

        for (min, max) in [(f64::NAN, 32.0), (8.0, f64::NAN), (f64::NAN, f64::NAN)] {
            let a = axis(min, max);
            assert!(a.min.is_finite() && a.max.is_finite(), "{a:?}");
            assert!(a.min <= a.max, "{a:?}");
            assert!(a.clamp(16.0).is_finite(), "{a:?}");
            assert!(a.default.is_finite(), "and so is the default: {a:?}");
        }

        assert_eq!(axis(20.0, 30.0).default, 20.0);
        let ok = axis(8.0, 100.0);
        assert_eq!((ok.min, ok.default, ok.max), (8.0, 14.0, 100.0));
        assert_eq!(ok.clamp(16.0), 16.0);
    }

    #[test]
    fn inter_offers_features_including_tabular_figures() {
        // Read off the face: Inter ships no `liga` (its ligatures ride `calt`).
        let features = family_features("Inter");
        let tags: Vec<String> = features.iter().map(|f| f.tag.to_string()).collect();
        for want in ["tnum", "calt", "dlig", "frac", "zero", "ss01"] {
            assert!(tags.contains(&want.to_string()), "no {want} in {tags:?}");
        }
        assert!(
            !tags.contains(&"liga".to_string()),
            "Inter has no `liga`; if that changes the comment above is stale"
        );
    }

    /// `ssXX`/`cvXX` rows are named by the font: the strings come through, no other
    /// tag gets one (read-fonts would parse `ssty` as a stylistic set), and the NULL
    /// tooltips stay `None` rather than reading name id 0, the copyright notice.
    #[test]
    fn inter_labels_its_own_stylistic_sets_and_character_variants() {
        let features = family_features("Inter");
        let named = |tag: &str| {
            features
                .iter()
                .find(|f| f.tag.as_str() == tag)
                .unwrap_or_else(|| panic!("Inter has no {tag}"))
        };
        assert_eq!(named("ss01").name.as_deref(), Some("Open digits"));
        assert_eq!(named("cv01").name.as_deref(), Some("Alternate one"));
        for f in &features {
            let font_defined = crate::typography::stylistic_set(f.tag).is_some()
                || crate::typography::character_variant(f.tag).is_some();
            assert_eq!(
                f.name.is_some(),
                font_defined,
                "{}: only the two font-defined ranges carry a name — got {:?}",
                f.tag,
                f.name
            );
            assert!(
                f.tooltip.is_none(),
                "{} has a tooltip; Inter names none, so this is a NULL id read as name 0 \
                 (the copyright notice): {:?}",
                f.tag,
                f.tooltip
            );
            assert!(
                f.values.is_empty(),
                "{} claims named values: {:?}",
                f.tag,
                f.values
            );
        }
    }

    /// A layout reports the line height its first line got: the face's own for
    /// `None`, exactly the authored value otherwise, never the ink height.
    #[test]
    fn a_layout_reports_the_line_height_its_first_line_actually_got() {
        // `None`, which the fixture's `style` does not give.
        let mut s = style(20.0);
        s.line_height = None;
        let auto = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!(
            auto > 20.0 && auto < 30.0,
            "Inter's own leading on 20px, not the em: {auto}"
        );
        let mut s = style(20.0);
        s.line_height = Some(crate::typography::Length::Em(2.0));
        let authored = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!((authored - 40.0).abs() < 1e-9, "200% of 20px: {authored}");
        s.line_height = Some(crate::typography::Length::Em(4.0));
        let tall = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!((tall - 80.0).abs() < 1e-9, "400% of 20px: {tall}");
    }

    #[test]
    fn tabular_figures_make_every_digit_the_same_width() {
        let mut tabular = style(40.0);
        tabular.features = vec![crate::typography::FeatureSetting {
            tag: Tag::parse("tnum").unwrap(),
            value: 1,
        }];
        let ones = measure_with("111", &tabular, &TextSizing::Auto).width;
        let zeros = measure_with("000", &tabular, &TextSizing::Auto).width;
        assert!((ones - zeros).abs() < 0.01, "{ones} vs {zeros}");
        let prop_ones = measure_with("111", &style(40.0), &TextSizing::Auto).width;
        assert!(
            ones > prop_ones,
            "tabular 1 is wider: {ones} vs {prop_ones}"
        );
    }

    #[test]
    fn an_unknown_family_reports_no_axes_rather_than_guessing() {
        assert!(family_axes("No Such Font 12345").is_empty());
        assert!(family_features("No Such Font 12345").is_empty());
    }

    /// The `opsz` rule in `axis_settings`: Auto synthesizes the font size clamped to
    /// the axis, Manual passes the stored coordinate through raw (the panel shows the
    /// stored value; the shaper clamps), and no such axis synthesizes nothing.
    ///
    /// flip: `axis.clamp(style.font_size)` replaced by `style.font_size` → fails at "Auto shapes at the axis it has" (96 against 32)
    #[test]
    fn a_stored_opsz_reaches_the_shaper_raw_and_an_absent_one_tracks_the_size() {
        let axis = FontAxis {
            tag: OPSZ,
            min: 8.0,
            default: 14.0,
            max: 32.0,
            name: "Optical size".into(),
            hidden: false,
        };
        let opsz = |v: &[FontVariation]| {
            v.iter()
                .find(|x| x.tag == parley_tag(OPSZ))
                .map(|x| f64::from(x.value))
        };

        let mut s = style(96.0);
        assert!(
            s.optical_size_is_auto(),
            "the fixture: no stored coordinate"
        );
        assert_eq!(
            opsz(&axis_settings(&s, std::slice::from_ref(&axis))),
            Some(32.0),
            "Auto shapes at the axis it has, not at the size it was asked for"
        );
        assert_eq!(
            opsz(&axis_settings(&s, &[])),
            None,
            "and a face with no such axis is given nothing to set"
        );

        s.set(CharAttr::Variations(vec![AxisSetting::new(OPSZ, 200.0)]));
        assert!(!s.optical_size_is_auto(), "the fixture: Manual now");
        assert_eq!(
            opsz(&axis_settings(&s, std::slice::from_ref(&axis))),
            Some(200.0),
            "a stored coordinate reaches the shaper raw — the panel shows what the \
             file holds (§15 D425) and the shaper clamps it for itself"
        );
    }

    /// Italic on a `slnt`-only face leans right (negative), clamped to the axis,
    /// and yields to an `ital` axis and to a hand-set `slnt`.
    ///
    /// flip: `axis.clamp(-ITALIC_SLANT)` → `axis.clamp(ITALIC_SLANT)` → fails at
    /// "leans right".
    #[test]
    fn italic_on_a_slnt_face_leans_right_unless_told_otherwise() {
        let axis = |tag, min| FontAxis {
            tag,
            min,
            default: 0.0,
            max: if tag == ITAL { 1.0 } else { 0.0 },
            name: tag.to_string(),
            hidden: false,
        };
        let slnt = |v: &[FontVariation]| {
            v.iter()
                .find(|x| x.tag == parley_tag(SLNT))
                .map(|x| f64::from(x.value))
        };
        let mut s = style(16.0);
        s.italic = true;
        assert_eq!(
            slnt(&axis_settings(&s, &[axis(SLNT, -10.0)])),
            Some(-10.0),
            "leans right, as far as the axis goes"
        );
        assert_eq!(slnt(&axis_settings(&s, &[axis(SLNT, -20.0)])), Some(-14.0));
        assert_eq!(
            slnt(&axis_settings(&s, &[axis(SLNT, -10.0), axis(ITAL, 0.0)])),
            None,
            "an ital axis is fontique's to set"
        );
        s.italic = false;
        assert_eq!(slnt(&axis_settings(&s, &[axis(SLNT, -10.0)])), None);
        s.italic = true;
        s.set(CharAttr::Variations(vec![AxisSetting::new(SLNT, -3.0)]));
        assert_eq!(
            slnt(&axis_settings(&s, &[axis(SLNT, -10.0)])),
            Some(-3.0),
            "a hand-set slnt is the only one written"
        );
    }

    fn variant(weight: u16, italic: bool) -> FontVariant {
        FontVariant {
            name: face_name(weight, italic),
            weight,
            italic,
            coords: Vec::new(),
        }
    }

    /// *Bold* steps to the nearest face of the same slope toward 700 or 400, else
    /// along `wght`, and refuses where the family has nothing that way.
    ///
    /// flip: dropping the `toward` filter in `bold_step` → fails at "mixed is made
    /// bold, nothing above 900" (it steps down to 600).
    #[test]
    fn bold_steps_to_the_nearest_face_that_way() {
        let statics = FaceOffer {
            variants: vec![
                variant(300, false),
                variant(400, false),
                variant(600, false),
                variant(900, false),
                variant(800, true),
            ],
            ..FaceOffer::default()
        };
        let w = |o: &FaceOffer, weight, italic, mixed| {
            bold_step(o, weight, italic, mixed).map(|v| (v.weight, v.italic))
        };
        assert_eq!(
            w(&statics, 400, false, false),
            Some((600, false)),
            "nearest to 700 above"
        );
        assert_eq!(
            w(&statics, 600, false, false),
            Some((400, false)),
            "on at 600: off to 400"
        );
        assert_eq!(
            w(&statics, 900, false, true),
            None,
            "mixed is made bold, nothing above 900"
        );
        assert_eq!(
            w(&statics, 400, true, false),
            Some((800, true)),
            "the slope is kept"
        );

        let one = FaceOffer {
            variants: vec![variant(400, false)],
            ..FaceOffer::default()
        };
        assert_eq!(
            w(&one, 400, false, false),
            None,
            "a family with one weight has no bold"
        );

        let axis = FaceOffer {
            variants: vec![variant(400, false)],
            wght: Some(FontAxis {
                tag: WGHT,
                min: 100.0,
                default: 400.0,
                max: 650.0,
                name: "Weight".into(),
                hidden: false,
            }),
            slope_axis: false,
        };
        assert_eq!(
            w(&axis, 400, false, false),
            Some((650, false)),
            "along wght, clamped"
        );
        assert_eq!(
            w(&FaceOffer::default(), 400, false, false),
            Some((700, false)),
            "an unknown family gets the bare weight"
        );
    }

    /// *Italic* takes the other slope's face nearest in weight, else the bare flag
    /// on a sloping axis; a family with no slope refuses, and upright is always
    /// reachable.
    ///
    /// flip: `italic_step` returning the bare flag without the `slope_axis`
    /// guard → fails at "no italic to be had".
    #[test]
    fn italic_takes_the_other_slope_or_refuses() {
        let i = |o: &FaceOffer, weight, italic| {
            italic_step(o, weight, italic, false).map(|v| (v.weight, v.italic))
        };
        let pair = FaceOffer {
            variants: vec![variant(400, false), variant(700, false), variant(600, true)],
            ..FaceOffer::default()
        };
        assert_eq!(
            i(&pair, 700, false),
            Some((600, true)),
            "nearest italic weight"
        );
        assert_eq!(
            i(&pair, 600, true),
            Some((700, false)),
            "and back, nearest upright"
        );

        let upright = FaceOffer {
            variants: vec![variant(400, false)],
            ..FaceOffer::default()
        };
        assert_eq!(i(&upright, 400, false), None, "no italic to be had");
        assert_eq!(
            i(&upright, 400, true),
            Some((400, false)),
            "upright is reachable"
        );

        let slanted = FaceOffer {
            slope_axis: true,
            ..upright
        };
        assert_eq!(
            i(&slanted, 400, false),
            Some((400, true)),
            "a slnt axis carries it"
        );
    }

    #[test]
    fn optical_size_auto_reaches_the_shaper() {
        // Drives `WGHT` rather than `opsz`, so it asserts the plumbing, not the picture.
        let mut s = style(40.0);
        let axes = family_axes("Inter");
        let Some(axis) = axes.iter().find(|a| a.tag == crate::typography::WGHT) else {
            return;
        };
        let plain = layout_with("W", &s, &TextSizing::Auto);
        s.variations = vec![AxisSetting::new(axis.tag, axis.max)];
        let heavy = layout_with("W", &s, &TextSizing::Auto);
        assert_ne!(
            plain.runs[0].coords, heavy.runs[0].coords,
            "an axis setting must reach the shaper"
        );
    }

    // --- paragraph layout -------------------------------------------------
    // These assert on parley's line metrics, which the caret reads too, not glyph ink.

    /// Every line's `(left, right)` edge, grouped by paragraph.
    fn paragraph_line_edges(content: &str, p: &TextParts) -> Vec<Vec<(f64, f64)>> {
        let shaped = shape(p.as_ref(content));
        let mut out: Vec<Vec<(f64, f64)>> = vec![Vec::new()];
        for line in shaped.layout.lines() {
            let m = line.metrics();
            out.last_mut()
                .expect("there is always one group")
                .push((f64::from(m.inline_min_coord), f64::from(m.inline_max_coord)));
            if line.break_reason() == BreakReason::Explicit {
                out.push(Vec::new());
            }
        }
        out.retain(|group| !group.is_empty());
        out
    }

    /// `paragraph_starts` (a text scan) and parley's `Explicit` breaks (counted by
    /// `break_lines` and `y_map`) must agree on what a hard break is, or paragraphs
    /// past the disagreement take their neighbour's indent; CRLF is the trap.
    #[test]
    fn paragraph_starts_agree_with_parleys_hard_breaks() {
        let d = style(20.0);
        for text in [
            "",
            "a",
            "a\nb",
            "a\r\nb",
            "a\rb",
            "a\u{2028}b",
            "a\u{2029}b",
            "a\n\nb",
            "a\n",
            "\n",
            "a\r\n\r\nb",
        ] {
            let p = parts(&d, &TextSizing::Auto);
            let shaped = shape(p.as_ref(text));
            let explicit = shaped
                .layout
                .lines()
                .filter(|l| l.break_reason() == BreakReason::Explicit)
                .count();
            let starts = paragraph_starts(text);
            assert_eq!(
                starts.len() - 1,
                explicit,
                "{text:?}: scanned {starts:?} but parley broke {explicit} time(s)"
            );
        }
    }

    #[test]
    fn block_indents_hold_every_line_in_from_both_edges() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(300.0));
        p.paragraph.indent_start = Length::Px(40.0);
        p.paragraph.indent_end = Length::Px(30.0);
        let edges =
            paragraph_line_edges("one two three four five six seven eight nine", &p).concat();
        assert!(edges.len() > 1, "the text should have wrapped: {edges:?}");
        for (i, (left, right)) in edges.iter().enumerate() {
            assert!(
                (left - 40.0).abs() < 0.01,
                "line {i} starts at {left}, want 40"
            );
            assert!(
                (right - 270.0).abs() < 0.01,
                "line {i} ends at {right}, want 270 — a block indent holds the \
                 continuation lines too, which is what makes it a block indent"
            );
        }
    }

    #[test]
    fn a_paragraph_span_indents_only_its_own_paragraph() {
        let d = style(20.0);
        let content = "first\nsecond";
        let mut p = parts(&d, &TextSizing::AutoHeight(300.0));
        let second = content.find('\n').expect("two paragraphs") + 1;
        p.para_spans.set(
            second..content.len(),
            ParaAttr::IndentStart(Length::Px(50.0)),
            &p.paragraph,
        );
        let groups = paragraph_line_edges(content, &p);
        assert_eq!(groups.len(), 2, "one line each: {groups:?}");
        assert!((groups[0][0].0 - 0.0).abs() < 0.01, "{:?}", groups[0]);
        assert!((groups[1][0].0 - 50.0).abs() < 0.01, "{:?}", groups[1]);
        assert!((groups[0][0].1 - 300.0).abs() < 0.01, "{:?}", groups[0]);
        assert!((groups[1][0].1 - 300.0).abs() < 0.01, "{:?}", groups[1]);
    }

    #[test]
    fn a_paragraph_span_narrows_only_its_own_measure() {
        let d = style(20.0);
        let content = "aaa bbb ccc ddd eee fff\naaa bbb ccc ddd eee fff";
        let mut p = parts(&d, &TextSizing::AutoHeight(300.0));
        let second = content.find('\n').expect("two paragraphs") + 1;
        p.para_spans.set(
            second..content.len(),
            ParaAttr::IndentEnd(Length::Px(200.0)),
            &p.paragraph,
        );
        let rights: Vec<f64> = paragraph_line_edges(content, &p)
            .concat()
            .iter()
            .map(|(_, right)| *right)
            .collect();
        let narrow = rights.iter().filter(|r| (**r - 100.0).abs() < 0.01).count();
        let wide = rights.iter().filter(|r| (**r - 300.0).abs() < 0.01).count();
        assert_eq!(
            narrow + wide,
            rights.len(),
            "every line should sit at one measure or the other: {rights:?}"
        );
        assert!(
            narrow > wide,
            "the indented paragraph should have wrapped more often: {rights:?}"
        );
    }

    #[test]
    fn a_hanging_span_flips_which_line_of_its_own_paragraph_is_indented() {
        let d = style(20.0);
        let content = "aaa bbb ccc ddd eee fff\naaa bbb ccc ddd eee fff";
        let mut p = parts(&d, &TextSizing::AutoHeight(200.0));
        p.paragraph.indent = Length::Px(40.0);
        let second = content.find('\n').expect("two paragraphs") + 1;
        p.para_spans
            .set(second..content.len(), ParaAttr::Hanging(true), &p.paragraph);
        let groups = paragraph_line_edges(content, &p);
        assert_eq!(groups.len(), 2, "{groups:?}");
        for (i, group) in groups.iter().enumerate() {
            assert!(group.len() > 1, "paragraph {i} should wrap: {group:?}");
        }
        assert!(
            (groups[0][0].0 - 40.0).abs() < 0.01 && (groups[0][1].0 - 0.0).abs() < 0.01,
            "the node's own first-line indent: {:?}",
            groups[0]
        );
        assert!(
            (groups[1][0].0 - 0.0).abs() < 0.01 && (groups[1][1].0 - 40.0).abs() < 0.01,
            "hanging is spannable, so the second paragraph indents its \
             continuation instead: {:?}",
            groups[1]
        );
    }

    /// An outdenting first line keeps its right edge; it needs more measure than the
    /// box, and with a finite layout maximum parley's `break_next` would panic, so the
    /// layout maximum must be `f32::INFINITY`.
    #[test]
    fn an_outdenting_first_line_keeps_the_right_edge() {
        let d = style(20.0);
        let mut p = parts(&d, &TextSizing::AutoHeight(200.0));
        p.paragraph.indent = Length::Px(-40.0);
        let edges = paragraph_line_edges("aaa bbb ccc ddd eee fff ggg", &p).concat();
        assert!(edges.len() > 1, "the text should have wrapped: {edges:?}");
        assert!(
            (edges[0].0 + 40.0).abs() < 0.01,
            "the first line hangs into the margin: {:?}",
            edges[0]
        );
        assert!((edges[1].0 - 0.0).abs() < 0.01, "{:?}", edges[1]);
        for (i, (_, right)) in edges.iter().enumerate() {
            assert!(
                (right - 200.0).abs() < 0.01,
                "line {i} ends at {right}, want 200 — the outdent must not drag \
                 the wrap point left with it"
            );
        }
    }

    #[test]
    fn an_auto_width_node_grows_by_a_start_indent_and_ignores_an_end_one() {
        let d = style(20.0);
        let plain = measure_with("one two", &d, &TextSizing::Auto).width;

        let mut started = parts(&d, &TextSizing::Auto);
        started.paragraph.indent_start = Length::Px(30.0);
        let with_start = measure(started.as_ref("one two")).size().width;
        assert!(
            (with_start - (plain + 30.0)).abs() < 0.01,
            "an emergent box grows by the offset its ink was given: \
             {with_start} vs {plain} + 30"
        );

        let mut ended = parts(&d, &TextSizing::Auto);
        ended.paragraph.indent_end = Length::Px(30.0);
        let with_end = measure(ended.as_ref("one two")).size().width;
        assert!(
            (with_end - plain).abs() < 0.01,
            "an auto-width node has no wrap width for an end indent to narrow, \
             so it is inert rather than quietly something else: {with_end} vs {plain}"
        );
    }

    /// `paragraph_bounds` snaps outward to whole paragraphs, case by case.
    #[test]
    fn a_paragraph_write_snaps_outward_to_whole_paragraphs() {
        // Paragraph starts at 0, 4, 8; length 11.
        let text = "one\ntwo\nsix";
        for (range, want, why) in [
            (0..0, 0..4, "a caret in the first paragraph"),
            (1..1, 0..4, "a caret inside it"),
            (4..4, 4..8, "a caret on the second's first byte"),
            (3..3, 0..4, "a caret before the newline is still the first"),
            (5..6, 4..8, "a selection inside one paragraph grows to it"),
            (1..6, 0..8, "a selection across two takes both"),
            (
                0..4,
                0..4,
                "a selection of the whole first paragraph, newline included, must \
                 not reach the second — its end is the second's first byte",
            ),
            (0..11, 0..11, "everything is everything"),
            (9..11, 8..11, "the last paragraph has no newline to end it"),
        ] {
            assert_eq!(
                paragraph_bounds(text, range.clone()),
                want,
                "{why}: {range:?}"
            );
        }
    }

    /// A paragraph made by a trailing newline owns no bytes, so the range reaches
    /// back over the whole break (U+2028 is three bytes) or `Spans::set` no-ops.
    #[test]
    fn an_empty_trailing_paragraph_reaches_back_over_its_own_break() {
        assert_eq!(paragraph_bounds("a\n", 2..2), 1..2);
        assert_eq!(paragraph_bounds("a\u{2028}", 4..4), 1..4);
        assert_eq!(paragraph_bounds("a\n", 0..0), 0..2);
        // An empty range is how the panel learns to write the defaults instead.
        assert_eq!(paragraph_bounds("", 0..0), 0..0);
    }

    /// A snapped range can be set as a span and is read back by `Paragraphs` as that
    /// paragraph's alone.
    #[test]
    fn a_span_over_a_snapped_range_reaches_that_paragraph_and_no_other() {
        let d = style(20.0);
        let content = "one\ntwo\n";
        let mut p = parts(&d, &TextSizing::AutoHeight(300.0));
        let range = paragraph_bounds(content, content.len()..content.len());
        p.para_spans
            .set(range, ParaAttr::IndentStart(Length::Px(50.0)), &p.paragraph);
        assert!(
            !p.para_spans.is_empty(),
            "the write must land somewhere, which an empty range would not"
        );
        let groups = paragraph_line_edges(content, &p);
        assert_eq!(groups.len(), 3, "{groups:?}");
        assert!((groups[0][0].0 - 0.0).abs() < 0.01, "{:?}", groups[0]);
        assert!((groups[1][0].0 - 0.0).abs() < 0.01, "{:?}", groups[1]);
        assert!(
            (groups[2][0].0 - 50.0).abs() < 0.01,
            "only the empty last paragraph should have moved: {groups:?}"
        );
    }

    #[test]
    fn per_paragraph_spacing_opens_only_the_gap_above_its_own_paragraph() {
        let d = style(10.0);
        let content = "one\ntwo\nthree";
        let third = content.rfind('\n').expect("three paragraphs") + 1;
        let mut p = parts(&d, &TextSizing::Auto);
        p.para_spans.set(
            third..content.len(),
            ParaAttr::Spacing(Length::Px(25.0)),
            &p.paragraph,
        );

        let plain = parts(&d, &TextSizing::Auto);
        let plain_height = measure(plain.as_ref(content)).size().height;
        let spaced_height = measure(p.as_ref(content)).size().height;
        assert!(
            (spaced_height - (plain_height + 25.0)).abs() < 0.5,
            "one gap of 25, not two and not none: {spaced_height} vs {plain_height}"
        );

        let tops = |p: &TextParts| -> Vec<f64> {
            let tl = layout(p.as_ref(content));
            let mut ys: Vec<f64> = tl
                .runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| f64::from(g.y))
                .collect();
            ys.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
            ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            ys
        };
        let flush = tops(&plain);
        let shifted = tops(&p);
        assert_eq!(flush.len(), 3, "one baseline per paragraph: {flush:?}");
        assert_eq!(shifted.len(), 3, "{shifted:?}");
        assert!(
            (shifted[1] - shifted[0] - (flush[1] - flush[0])).abs() < 0.01,
            "the second paragraph must not have moved: {shifted:?} vs {flush:?}"
        );
        assert!(
            (shifted[2] - shifted[1] - (flush[2] - flush[1]) - 25.0).abs() < 0.01,
            "the third one carries the whole gap: {shifted:?} vs {flush:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Type on a path
    // -----------------------------------------------------------------------

    /// A rail running left to right along `y = 0`, `len` long.
    fn straight_rail(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((len, 0.0));
        p
    }

    /// A rail turning a quarter circle from `(0, 0)` heading right to `(r, r)`
    /// heading down, so the tangent sweeps 0 to π/2.
    fn quarter_rail(r: f64) -> BezPath {
        let k = 0.552_284_749_83 * r;
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((k, 0.0), (r, r - k), (r, r));
        p
    }

    /// `on_rail` at a chosen point size with the cap-to-baseline trim a text-on-path
    /// node carries, which decides how far the ribbon reaches from the rail.
    fn on_rail_at(size: f64, content: &str, rail: BezPath) -> TextLayout {
        let mut p = parts(&style(size), &TextSizing::Auto);
        p.block.trim = BoxTrim::CapToBaseline;
        p.on_path = Some(rail);
        // Not starting at a corner: where the run begins decides which corner's
        // normal a sampling bug is asked for.
        p.on_path_offset = 0.057_339_449_541_284_41;
        layout(p.as_ref(content))
    }

    fn on_rail(content: &str, rail: BezPath) -> TextLayout {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(rail);
        layout(p.as_ref(content))
    }

    /// A one-character span recolours a glyph on a rail without moving it; the rail
    /// is curved because a straight one hides the rotation about the left edge.
    ///
    /// flip: the advance scan confined to its own run → fails at the rotation assertion
    #[test]
    fn a_one_character_span_on_a_rail_moves_nothing() {
        let content = "Wave along the rail";
        let plain = on_rail(content, quarter_rail(120.0));

        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(quarter_rail(120.0));
        p.spans.set(
            0..1,
            CharAttr::Color(Some(Color::from_rgb8(0xEB, 0x6E, 0x5A))),
            &p.style,
        );
        let styled = layout(p.as_ref(content));

        let first = |l: &TextLayout| -> Glyph {
            *l.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .next()
                .expect("the fixture shapes")
        };
        let (a, b) = (first(&plain), first(&styled));
        assert!(
            styled.runs.len() > plain.runs.len(),
            "the fixture has to split a run of one, or this proves nothing: \
             {} vs {}",
            plain.runs.len(),
            styled.runs.len()
        );
        assert!(
            (f64::from(a.rot) - f64::from(b.rot)).abs() < 1e-4,
            "recolouring one character rotated it by {} rad",
            f64::from(b.rot) - f64::from(a.rot)
        );
        assert!(
            (f64::from(a.x) - f64::from(b.x)).hypot(f64::from(a.y) - f64::from(b.y)) < 1e-3,
            "…and moved it: {a:?} became {b:?}"
        );
    }

    fn on_rail_flipped(content: &str, rail: BezPath) -> TextLayout {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(rail);
        p.on_path_flip = true;
        layout(p.as_ref(content))
    }

    /// A straight rail reproduces flat type lifted by one first baseline, since the
    /// rail is the baseline; the identity case that checks the whole map is
    /// consistent. The rail is longer than the text to stay off the clamped end.
    #[test]
    fn a_straight_rail_reproduces_flat_type() {
        let flat = layout_with("Hamburgefonstiv", &style(20.0), &TextSizing::Auto);
        let bent = on_rail("Hamburgefonstiv", straight_rail(flat.size.width * 2.0));

        let pos = |l: &TextLayout| -> Vec<(f32, f32, f32)> {
            l.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| (g.x, g.y, g.rot))
                .collect()
        };
        let (a, b) = (pos(&flat), pos(&bent));
        assert_eq!(
            a.len(),
            b.len(),
            "no glyph may be dropped on a rail it fits"
        );
        assert!(!a.is_empty(), "the fixture must actually shape");
        let lift = f64::from(a[0].1) - f64::from(b[0].1);
        assert!(
            (lift - flat.baselines[0]).abs() < 0.01,
            "the lift must be exactly the first baseline: {lift} vs {:?}",
            flat.baselines[0]
        );
        for (i, (f, r)) in a.iter().zip(&b).enumerate() {
            assert!(
                (f.0 - r.0).abs() < 0.01 && (f64::from(f.1) - f64::from(r.1) - lift).abs() < 0.01,
                "glyph {i} moved by something other than the lift: flat {f:?} vs bent {r:?}"
            );
            assert_eq!(r.2, 0.0, "a straight rail rotates nothing: glyph {i}");
        }
    }

    /// The tangent turns the glyphs, and the last one is turned furthest. A negated
    /// normal leaves these rotations unchanged; the second-line test catches that.
    #[test]
    fn type_turns_with_the_rail() {
        let bent = on_rail("iiiiiiiiii", quarter_rail(200.0));
        let rots: Vec<f32> = bent
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.rot)
            .collect();
        assert!(rots.len() >= 2, "need two glyphs to see a turn: {rots:?}");
        assert!(
            rots[0].abs() < 0.05,
            "the rail leaves the origin heading right: {:?}",
            rots[0]
        );
        assert!(
            rots.windows(2).all(|w| w[1] >= w[0] - 1e-4),
            "the tangent turns one way along a quarter circle: {rots:?}"
        );
        let last = *rots.last().expect("non-empty");
        assert!(
            last > 0.05,
            "ten glyphs on a 200-unit quarter circle must visibly turn: {rots:?}"
        );
    }

    /// A second line runs parallel to the first, one line height to the side a
    /// descender goes: larger `y` on a straight rail, toward the centre on
    /// `quarter_rail`.
    ///
    /// flip: negating the normal in `PathWarp::place` → fails at the straight-rail line-height assertion
    #[test]
    fn a_second_line_runs_parallel_one_line_height_along_the_normal() {
        let ys = |l: &TextLayout| -> Vec<f64> {
            l.runs
                .iter()
                .flat_map(|run| run.glyphs.iter())
                .map(|g| f64::from(g.y))
                .collect()
        };
        let flat_rail = on_rail("nn\nnn", straight_rail(400.0));
        let d = ys(&flat_rail);
        assert!(d.len() >= 4, "both lines must shape: {d:?}");
        let (lo, hi) = d
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), y| (l.min(*y), h.max(*y)));
        assert!(
            (lo - 0.0).abs() < 0.01,
            "the first line sits on the rail: {lo}"
        );
        assert!(
            (hi - flat_rail.line_height).abs() < 0.5,
            "the second sits one line height *below* it, not above: {hi} against a \
             line height of {}",
            flat_rail.line_height
        );

        let r = 200.0;
        let curved = on_rail("nn\nnn", quarter_rail(r));
        let centre = Point::new(0.0, r);
        let radii: Vec<f64> = curved
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter())
            .map(|g| (Point::new(f64::from(g.x), f64::from(g.y)) - centre).hypot())
            .collect();
        let (near, far) = radii
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
        assert!(
            (far - r).abs() < 1.0,
            "the first line runs along the rail itself: {far} against {r}"
        );
        assert!(
            (near - (r - curved.line_height)).abs() < 1.0,
            "the second line is one line height toward the centre — the same side \
             the straight rail put it: {near} against {}",
            r - curved.line_height
        );
    }

    /// Text longer than its rail is dropped, not stacked on the last point, and
    /// `truncated` says so.
    #[test]
    fn text_past_the_end_of_the_rail_is_dropped() {
        let long = "Hamburgefonstiv Hamburgefonstiv";
        let flat = layout_with(long, &style(20.0), &TextSizing::Auto);
        let bent = on_rail(long, straight_rail(flat.size.width * 0.5));

        let drawn: Vec<f32> = bent
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.x)
            .collect();
        let all: usize = flat.runs.iter().map(|r| r.glyphs.len()).sum();
        assert!(!drawn.is_empty(), "the half that fits must still draw");
        assert!(
            drawn.len() < all,
            "{} of {all} glyphs fit a half-length rail",
            drawn.len()
        );
        assert!(bent.truncated, "and the panel has to be able to say so");
        let last = drawn
            .iter()
            .copied()
            .fold(f64::MIN, |m, x| m.max(f64::from(x)));
        assert!(
            last <= flat.size.width * 0.5 + 1.0,
            "nothing may be stacked past the rail's end: {last}"
        );
    }

    /// Alignment is applied along the rail, once: without the `Start` gate in `shape`
    /// parley would centre inside the box first. `AutoHeight(300)` gives an authored
    /// width (under `Auto` the gate is unreachable); the assertion reads the
    /// leftmost glyph, not `origin.x`.
    #[test]
    fn alignment_offsets_along_the_rail() {
        let len = 600.0;
        let sizing = TextSizing::AutoHeight(300.0);
        let mut p = parts(&style(20.0), &sizing);
        p.on_path = Some(straight_rail(len));
        let flat_width = layout_with("Ondin", &style(20.0), &TextSizing::Auto)
            .size
            .width;
        // The rail runs along `y = 0`, so a glyph's `x` is its distance along it.
        let first_glyph_x = |l: &TextLayout| -> f64 {
            l.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| f64::from(g.x))
                .fold(f64::MAX, f64::min)
        };

        let start_x = first_glyph_x(&layout(p.as_ref("Ondin")));
        p.paragraph.align = TextAlign::Center;
        let centre_x = first_glyph_x(&layout(p.as_ref("Ondin")));
        p.paragraph.align = TextAlign::End;
        let end_x = first_glyph_x(&layout(p.as_ref("Ondin")));

        assert!(
            start_x.abs() < 1.0,
            "start sits at the rail's own origin: {start_x}"
        );
        assert!(
            (centre_x - (len - flat_width) * 0.5).abs() < 1.0,
            "centred means centred on the rail, not in the 300-wide box: {centre_x} \
             vs {}",
            (len - flat_width) * 0.5
        );
        assert!(
            (end_x - (len - flat_width)).abs() < 1.0,
            "end means the rail's end: {end_x} vs {}",
            len - flat_width
        );
    }

    /// The bent box holds every glyph the layout draws. The large-radius case is
    /// weak (the box is the rail's ribbon, which covers the whole curve);
    /// `the_bent_box_holds_the_rail_itself` is the one with teeth.
    #[test]
    fn the_bent_box_contains_the_bent_ink() {
        for (what, rail) in [
            ("a tight rail", quarter_rail(150.0)),
            // The warp is two orders longer than the ink; containment holds under
            // either sweep density, so this case is weak, not a guard.
            ("a rail past the step cap", quarter_rail(12_000.0)),
        ] {
            let bent = on_rail("Ondin on a curve", rail);
            let b = bent.bounds();
            assert!(
                b.width() > 1.0 && b.height() > 1.0,
                "{what}: a real box: {b:?}"
            );
            for g in bent.runs.iter().flat_map(|r| r.glyphs.iter()) {
                let p = Point::new(f64::from(g.x), f64::from(g.y));
                assert!(
                    b.x0 - 1.0 <= p.x
                        && p.x <= b.x1 + 1.0
                        && b.y0 - 1.0 <= p.y
                        && p.y <= b.y1 + 1.0,
                    "{what}: glyph at {p:?} is outside the reported box {b:?}"
                );
            }
            assert_eq!(bent.untrimmed_bounds(), bent.bounds(), "{what}");
        }
    }

    /// The reported box holds the rail, re-walked at 200,001 points, so a cheaper
    /// sweep that steps over an extremum fails. Sharp corners are the fixtures with
    /// teeth (first order in the step, against second for a smooth arch); the
    /// spike's legs are unequal so the apex is never sampled exactly.
    ///
    /// flip: `bend`'s box sweep coarsened (or `BOX_SAG` raised) → fails at "a sharp corner"
    #[test]
    fn the_bent_box_holds_the_rail_itself() {
        for (what, rail, wraps) in [
            ("an arch past the step cap", arch_rail(12_000.0), false),
            ("a sharp corner", spike_rail(200.0), false),
            ("a sharper, taller corner", spike_rail(1000.0), false),
            // Small on purpose: the type must wrap past a corner onto the far side,
            // or there is no ink for a missing offset to strand.
            ("a closed rectangle", rect_rail(341.0, 172.0), true),
        ] {
            let bent = match wraps {
                true => on_rail_at(
                    24.0,
                    "Hello world this is Ondin text on rail tool and it's looking good",
                    rail,
                ),
                false => on_rail("Ondin on a curve", rail),
            };
            let b = bent.bounds();

            // The ink, not only the centreline, which cannot see a lost offset side;
            // green is not "ribbon whole". One point of slack for trimmed-box overshoot.
            let ink = outline(&bent).bounding_box();
            // The type must have gone round the rail, or there is no ink to strand.
            if let Some(w) = bent.warp.as_ref().filter(|_| wraps) {
                let rail = w.bounds();
                assert!(
                    ink.width() >= rail.width() * 0.8,
                    "{what}: the fixture's type does not reach the far side — \
                     ink {:.1} wide against a rail {:.1} wide, so a lost offset \
                     would strand nothing",
                    ink.width(),
                    rail.width()
                );
            }
            if ink.width() > 0.0 {
                let esc = (b.x0 - ink.x0)
                    .max(ink.x1 - b.x1)
                    .max(b.y0 - ink.y0)
                    .max(ink.y1 - b.y1);
                assert!(
                    esc <= 1.0,
                    "{what}: the type escapes its own box by {esc:.2} — box {b:?}, ink {ink:?}"
                );
            }
            let w = bent
                .warp
                .as_ref()
                .expect("a railed layout carries its warp");

            assert!(w.len > 1.0, "{what}: a rail with length: {}", w.len);
            assert!(
                b.width() > 1.0 && b.height() > 1.0,
                "{what}: a real box: {b:?}"
            );

            let n = 200_000;
            let (mut ox, mut oy) = (0.0_f64, 0.0_f64);
            for i in 0..=n {
                let s = w.len * (i as f64) / f64::from(n);
                if let Some((p, _)) = w.place(s - w.start, w.baseline) {
                    ox = ox.max((b.x0 - p.x).max(p.x - b.x1).max(0.0));
                    oy = oy.max((b.y0 - p.y).max(p.y - b.y1).max(0.0));
                }
            }
            assert!(
                ox <= 0.001 && oy <= 0.001,
                "{what}: the rail escapes its own reported box by ({ox}, {oy}) — \
                 the box is {b:?} and every handle, snap target and W/H field reads it"
            );
        }
    }

    /// A railed box clears its rail on all four sides, equally, whichever way the rail
    /// is traversed; a lost edge is a margin of exactly zero. The fixtures do not
    /// separate the flip mirroring from the end nudges, so green is not a licence to
    /// delete the mirroring.
    ///
    /// flip: `bend` sampling `let a0 = *s0;` and without the `a0 + slen * 1e-9` sample → fails at the right clearance for `flip=true`
    #[test]
    fn a_flipped_rails_box_clears_the_rail_on_every_side() {
        for (what, rail) in [
            ("a square-ish rectangle", rect_rail(200.0, 100.0)),
            ("a long thin rectangle", rect_rail(400.0, 20.0)),
        ] {
            for flip in [false, true] {
                let mut p = parts(&style(20.0), &TextSizing::Auto);
                p.on_path = Some(rail.clone());
                p.on_path_flip = flip;
                let bent = layout(p.as_ref("Ondin on a curve"));
                let b = bent.bounds();
                let rail = bent
                    .warp
                    .as_ref()
                    .expect("a railed layout carries its warp")
                    .bounds();

                assert!(
                    rail.width() > 1.0 && rail.height() > 1.0,
                    "{what} flip={flip}: a real rail box: {rail:?}"
                );

                let sides = [
                    ("left", rail.x0 - b.x0),
                    ("top", rail.y0 - b.y0),
                    ("right", b.x1 - rail.x1),
                    ("bottom", b.y1 - rail.y1),
                ];
                for (side, margin) in sides {
                    assert!(
                        margin > 0.5,
                        "{what} flip={flip}: the box's {side} edge sits on the rail \
                         — clearance {margin:.4}, box {b:?} against rail {rail:?}. A \
                         whole edge's outward ribbon is missing and the type hangs \
                         outside its own box."
                    );
                }
                let lo = sides.iter().map(|(_, m)| *m).fold(f64::MAX, f64::min);
                let hi = sides.iter().map(|(_, m)| *m).fold(f64::MIN, f64::max);
                assert!(
                    hi - lo <= 0.01,
                    "{what} flip={flip}: the four clearances disagree by {:.4} — \
                     {sides:?}. On a rectangle rail the normal is axis-aligned and \
                     the offset constant, so an edge short of the others has lost \
                     part of its sweep.",
                    hi - lo
                );
            }
        }
    }

    /// A half-circle-ish arch whose extremum is between the endpoints, unlike
    /// `quarter_rail`, so a sweep that steps over it is detectable.
    fn arch_rail(r: f64) -> BezPath {
        let k = 0.552_284_749_83 * r;
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((0.0, -k), (r - k, -r), (r, -r));
        p.curve_to((r + k, -r), (2.0 * r, -k), (2.0 * r, 0.0));
        p
    }

    /// A closed rectangle with sharp corners, the rail the text-on-path tool makes
    /// from a rectangle.
    fn rect_rail(w: f64, h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((w, 0.0));
        p.line_to((w, h));
        p.line_to((0.0, h));
        p.close_path();
        p
    }

    /// A rail with a cusp at `(100, -h)` and unequal legs (see
    /// `the_bent_box_holds_the_rail_itself`), where a coarser box sweep loses real units.
    fn spike_rail(h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, -h));
        p.line_to((237.0, 13.0));
        p
    }

    /// A warped band follows the rail rather than cutting across it: mapping only
    /// each edge's endpoints leaves a straight ribbon far off the curve at its middle.
    #[test]
    fn a_warped_band_bends_with_the_rail() {
        let r = 200.0;
        let warp = PathWarp::bare(&quarter_rail(r)).expect("a rail with length");
        let mut band = BezPath::new();
        band.move_to((0.0, 0.0));
        band.line_to((warp.length(), 0.0));
        let bent = warp.warp(&band);

        let pts: Vec<Point> = bent
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert!(
            pts.len() > 8,
            "the edge must be resampled: {} points",
            pts.len()
        );
        let centre = Point::new(0.0, r);
        for p in &pts {
            assert!(
                ((*p - centre).hypot() - r).abs() < 2.0,
                "every point of a band on the rail sits on the rail: {p:?}"
            );
        }
    }

    /// The flip reverses both the direction and the side; doing only one gives
    /// upside-down or backwards letters.
    ///
    /// flip: negating the tangent without the normal → direction assertions stay green, the side assertion fails
    #[test]
    fn flipping_reverses_the_direction_and_the_side_together() {
        let len = 400.0;
        let plain = on_rail("nn\nnn", straight_rail(len));
        let flipped = on_rail_flipped("nn\nnn", straight_rail(len));

        let xs = |l: &TextLayout| -> (f64, f64) {
            l.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .fold((f64::MAX, f64::MIN), |(a, b), g| {
                    (a.min(f64::from(g.x)), b.max(f64::from(g.x)))
                })
        };
        let ys = |l: &TextLayout| -> (f64, f64) {
            l.runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .fold((f64::MAX, f64::MIN), |(a, b), g| {
                    (a.min(f64::from(g.y)), b.max(f64::from(g.y)))
                })
        };

        let (plain_lo, plain_hi) = xs(&plain);
        let (flip_lo, flip_hi) = xs(&flipped);
        assert!(
            plain_lo.abs() < 1.0,
            "the unflipped run starts at the rail's origin: {plain_lo}"
        );
        assert!(
            (flip_hi - len).abs() < 1.0,
            "the flipped run starts at the rail's far end: {flip_hi} against {len}"
        );
        assert!(
            flip_lo > plain_hi - 1.0,
            "and it occupies the other end of the rail entirely: flipped \
             {flip_lo}..{flip_hi} against plain {plain_lo}..{plain_hi}"
        );

        let (_, plain_below) = ys(&plain);
        let (flip_above, _) = ys(&flipped);
        assert!(
            plain_below > plain.line_height * 0.5,
            "unflipped, the second line is below the rail: {plain_below}"
        );
        assert!(
            flip_above < -flipped.line_height * 0.5,
            "flipped, it is above it — this is the assertion a direction-only flip \
             leaves failing: {flip_above}"
        );

        for g in flipped.runs.iter().flat_map(|r| r.glyphs.iter()) {
            assert!(
                (f64::from(g.rot).abs() - std::f64::consts::PI).abs() < 0.01,
                "a flipped glyph on a straight rail is turned by π: {}",
                g.rot
            );
        }
    }

    /// The flip survives the rail being taken off and put back, because the flag
    /// lives beside the rail rather than inside the `Option`.
    #[test]
    fn the_flip_outlives_the_rail() {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path_flip = true;

        let flat = layout(p.as_ref("nn"));
        let plain = layout_with("nn", &style(20.0), &TextSizing::Auto);
        assert_eq!(
            flat.size, plain.size,
            "a flip with no rail must be inert, not a second kind of layout"
        );

        p.on_path = Some(straight_rail(400.0));
        let bent = layout(p.as_ref("nn"));
        for g in bent.runs.iter().flat_map(|r| r.glyphs.iter()) {
            assert!(
                (f64::from(g.rot).abs() - std::f64::consts::PI).abs() < 0.01,
                "the flip has to have survived: {}",
                g.rot
            );
        }
    }

    /// A decoration band warps onto its rail, not a baseline away: `place` takes a
    /// baseline-relative `y` while a `DecorationInk::band` is text-local. The band
    /// comes off a real layout because `y = 0` means the same in both spaces.
    #[test]
    fn a_decoration_warps_onto_the_rail_and_not_a_baseline_below_it() {
        let mut style = style(20.0);
        style.set(CharAttr::Underline(Some(Decoration::default())));
        let mut p = parts(&style, &TextSizing::Auto);

        let flat = layout(p.as_ref("Hxy"));
        let band = flat.decorations.first().expect("an underline").band;
        let baseline = flat.baselines[0];
        assert!(
            band.y0 > baseline && baseline > 5.0,
            "the fixture must separate the two spaces: band {band:?}, baseline {baseline}"
        );

        p.on_path = Some(straight_rail(400.0));
        let bent = layout(p.as_ref("Hxy"));
        let warp = bent.warp.as_ref().expect("a rail");
        let (p0, _) = warp.place(band.x0, band.y0).expect("on the rail");
        assert!(
            (p0.y - (band.y0 - baseline)).abs() < 0.01,
            "the band sits its own distance below the rail, not a baseline further: \
             {} against {}",
            p0.y,
            band.y0 - baseline
        );
        let g = bent.runs[0].glyphs[0];
        assert!(
            f64::from(g.y).abs() < 0.01,
            "the baseline is the rail: {}",
            g.y
        );
    }

    /// `PathWarp::nearest` inverts `PathWarp::place`, so a click on bent type finds
    /// its character; round-tripped on a curved rail, where `t` and arc length differ.
    #[test]
    fn nearest_inverts_place() {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(quarter_rail(200.0));
        p.paragraph.align = TextAlign::Center;
        let bent = layout(p.as_ref("Hamburgefonstiv"));
        let warp = bent.warp.as_ref().expect("a rail");

        for (x, y) in [(0.0, 19.0), (40.0, 19.0), (120.0, 4.0), (75.0, 31.0)] {
            let (at, _) = warp.place(x, y).expect("inside the rail");
            let (bx, by) = warp.nearest(at);
            assert!(
                (bx - x).abs() < 0.5 && (by - y).abs() < 0.5,
                "({x}, {y}) placed at {at:?} came back as ({bx}, {by})"
            );
        }
    }

    /// The offset moves the type along the rail and a closed rail wraps, so an
    /// offset near 1.0 does not truncate the text; both halves share one ellipse.
    #[test]
    fn an_offset_moves_the_type_along_a_closed_rail_and_wraps_past_the_end() {
        let ring = crate::geometry::local_path(&crate::node::NodeKind::Ellipse {
            size: Size::new(200.0, 200.0),
        })
        .expect("an ellipse has an outline");

        let glyphs = |offset: f64| -> Vec<Point> {
            let mut p = parts(&style(16.0), &TextSizing::Auto);
            p.on_path = Some(ring.clone());
            p.on_path_offset = offset;
            layout(p.as_ref("AROUND"))
                .runs
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| Point::new(f64::from(g.x), f64::from(g.y)))
                .collect()
        };

        let at_start = glyphs(0.0);
        let quarter = glyphs(0.25);
        assert!(!at_start.is_empty(), "the fixture must shape");
        assert_eq!(
            quarter.len(),
            at_start.len(),
            "moving the type along a closed rail must not drop any of it"
        );
        assert!(
            (quarter[0] - at_start[0]).hypot() > 50.0,
            "a quarter of the way round a 200-unit ring is a long way: {:?} against \
             {:?}",
            quarter[0],
            at_start[0]
        );

        // 0.97 starts the text near the rail's end, so the rest must come round.
        let nearly_round = glyphs(0.97);
        assert_eq!(
            nearly_round.len(),
            at_start.len(),
            "a closed rail has no end to fall off — this is what truncates without \
             the wrap"
        );
        let (first, last) = (nearly_round[0], nearly_round[nearly_round.len() - 1]);
        assert!(
            (first - last).hypot() > 20.0,
            "the run must be spread round the seam, not stacked on it: {first:?} to \
             {last:?}"
        );
    }

    /// An open rail still has an end and an offset pushes text off it: the control
    /// for the wrap above. The offset is needed because at zero offset the two
    /// wrap conditions are the same inequality.
    #[test]
    fn an_open_rail_still_runs_out_and_an_offset_pushes_text_off_it() {
        let word = "Hamburge";
        let flat = layout_with(word, &style(20.0), &TextSizing::Auto);
        let rail = straight_rail(flat.size.width * 1.5);

        let whole = on_rail(word, rail.clone());
        let all: usize = flat.runs.iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(
            whole.runs.iter().map(|r| r.glyphs.len()).sum::<usize>(),
            all,
            "the fixture must start with the whole word on the rail"
        );
        assert!(!whole.truncated);

        // Two thirds along a rail one and a half words long leaves room for a third.
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(rail);
        p.on_path_offset = 2.0 / 3.0;
        let pushed = layout(p.as_ref(word));
        let drawn: usize = pushed.runs.iter().map(|r| r.glyphs.len()).sum();
        assert!(
            drawn < all,
            "an open rail runs out: {drawn} of {all} still drawn — a rail that \
             wrapped would keep them all"
        );
        assert!(pushed.truncated, "and it says so");
    }

    /// A rail with no length is not a rail: the node falls back to flat type, so a
    /// corrupt file cannot produce an invisible layer.
    #[test]
    fn a_degenerate_rail_leaves_the_type_flat() {
        let flat = layout_with("Ondin", &style(20.0), &TextSizing::Auto);
        for rail in [BezPath::new(), straight_rail(0.0)] {
            let bent = on_rail("Ondin", rail);
            assert!(bent.warp.is_none(), "nothing to bend onto");
            assert_eq!(bent.size, flat.size);
            assert_eq!(bent.runs.len(), flat.runs.len());
        }
    }
}

#[cfg(test)]
mod edit_tests {
    //! The caret model behind in-canvas text editing: cluster boundaries,
    //! selection replacement, and vertical movement through a shaped layout.

    use super::*;
    use crate::typography::CharAttr;

    fn editor(content: &str) -> TextEdit {
        TextEdit::new(
            content,
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.2)),
                    ..TextStyle::default()
                },
                ..TextParts::default()
            },
        )
    }

    #[test]
    fn a_new_session_puts_the_caret_at_the_end() {
        let mut e = editor("abc");
        e.insert("d");
        assert_eq!(e.content(), "abcd");
    }

    #[test]
    fn backspace_deletes_whole_characters_not_bytes() {
        let mut e = editor("aé漢");
        e.backspace();
        assert_eq!(e.content(), "aé");
        e.backspace();
        assert_eq!(e.content(), "a");
        e.backspace();
        assert_eq!(e.content(), "");
        e.backspace();
        assert_eq!(e.content(), "", "backspace at the start is a no-op");
    }

    #[test]
    fn caret_moves_by_character_and_inserts_in_the_middle() {
        let mut e = editor("hello");
        e.move_left(false, false);
        e.move_left(false, false);
        e.insert("X");
        assert_eq!(e.content(), "helXlo");
    }

    #[test]
    fn shift_arrow_extends_a_selection_that_typing_replaces() {
        let mut e = editor("hello");
        e.move_left(true, false);
        e.move_left(true, false);
        assert!(e.has_selection());
        assert_eq!(e.selected_range(), 3..5);
        e.insert("p!");
        assert_eq!(e.content(), "help!");
        assert!(!e.has_selection());
    }

    #[test]
    fn select_all_then_typing_replaces_everything() {
        let mut e = editor("throw this away");
        e.select_all();
        assert_eq!(e.selected_range(), 0..15);
        e.insert("new");
        assert_eq!(e.content(), "new");
    }

    #[test]
    fn word_deletion_removes_a_whole_word() {
        let mut e = editor("one two three");
        e.delete_word_back();
        assert!(
            !e.content().contains("three"),
            "expected the last word gone, got {:?}",
            e.content()
        );
        assert!(e.content().starts_with("one two"));
    }

    #[test]
    fn delete_forward_at_the_end_is_a_no_op() {
        let mut e = editor("ab");
        e.delete_forward();
        assert_eq!(e.content(), "ab");
        e.move_left(false, false);
        e.delete_forward();
        assert_eq!(e.content(), "a");
    }

    #[test]
    fn vertical_movement_crosses_lines() {
        let mut e = editor("first\nsecond");
        e.move_up(false);
        e.move_line_start(false);
        e.insert(">");
        assert_eq!(e.content(), ">first\nsecond");
    }

    #[test]
    fn clicking_positions_the_caret_at_that_point() {
        let mut e = editor("wide enough to click into");
        e.click(Point::new(-100.0, 5.0));
        e.insert("[");
        assert!(e.content().starts_with('['), "got {:?}", e.content());

        e.click(Point::new(10_000.0, 5.0));
        e.insert("]");
        assert!(e.content().ends_with(']'), "got {:?}", e.content());
    }

    #[test]
    fn double_click_selects_a_word() {
        let mut e = editor("alpha beta gamma");
        e.click(Point::new(0.0, 5.0));
        e.select_word_at(Point::new(2.0, 5.0));
        assert!(e.has_selection());
        e.insert("X");
        assert!(
            e.content().starts_with('X') && e.content().contains("beta"),
            "expected only the first word replaced, got {:?}",
            e.content()
        );
    }

    #[test]
    fn caret_rect_rises_a_line_when_the_caret_does() {
        let mut e = editor("line one\nline two");
        e.move_line_start(false);
        let on_second = e.caret_rect(1.0);
        e.move_up(false);
        let on_first = e.caret_rect(1.0);
        assert!(
            on_first.y0 < on_second.y0,
            "caret should rise a line: {on_first:?} vs {on_second:?}"
        );
    }

    #[test]
    fn selection_rects_appear_only_when_something_is_selected() {
        let mut e = editor("highlight me");
        assert!(e.selection_rects().is_empty());
        e.select_all();
        assert!(!e.selection_rects().is_empty());
    }

    #[test]
    fn editing_reshapes_so_the_box_follows_the_content() {
        let mut e = editor("i");
        let narrow = e.text_layout().size.width;
        e.insert("mmmmmmmmmm");
        assert!(
            e.text_layout().size.width > narrow,
            "layout must be rebuilt after an edit"
        );
    }

    #[test]
    fn restyle_keeps_the_caret_and_reshapes() {
        let mut e = editor("abc");
        e.move_left(false, false);
        let before = e.text_layout().size.width;
        e.restyle(TextParts {
            style: TextStyle {
                font_size: 80.0,
                ..TextStyle::default()
            },
            ..TextParts::default()
        });
        assert!(e.text_layout().size.width > before);
        e.insert("X");
        assert_eq!(e.content(), "abXc", "caret survived the restyle");
    }

    // --- styling a range while editing ------------------------------------

    #[test]
    fn styling_a_selection_records_a_span_and_reshapes() {
        let mut e = editor("hello world");
        e.click(Point::new(0.0, 5.0));
        e.select_word_at(Point::new(2.0, 5.0));
        let before = e.text_layout().size.width;
        e.style_selection(CharAttr::Weight(900));
        assert!(!e.spans().is_empty(), "the span should have been recorded");
        assert!(
            e.text_layout().size.width > before,
            "the bolder word should be wider"
        );
    }

    #[test]
    fn styling_an_empty_caret_applies_to_the_next_thing_typed() {
        // "Bold, then type" is the only exception to inherit-from-the-left.
        let mut e = editor("ab");
        e.style_selection(CharAttr::Weight(900));
        assert!(e.spans().is_empty(), "nothing to style yet");
        e.insert("X");
        let spans = e.spans().as_slice();
        assert_eq!(spans.len(), 1, "got {spans:?}");
        assert_eq!(spans[0].range(), 2..3);
    }

    #[test]
    fn a_pending_style_does_not_survive_moving_the_caret() {
        let mut e = editor("ab");
        e.style_selection(CharAttr::Weight(900));
        e.move_left(false, false);
        e.insert("X");
        assert!(
            e.spans().is_empty(),
            "the pending style belonged to where the caret was"
        );
    }

    #[test]
    fn typing_inside_a_styled_run_inherits_it() {
        let mut e = editor("bold");
        e.select_all();
        e.style_selection(CharAttr::Weight(900));
        e.move_line_end(false);
        e.insert("er");
        assert_eq!(e.content(), "bolder");
        assert_eq!(
            e.spans().as_slice()[0].range(),
            0..6,
            "the run grows over what was typed at its edge"
        );
    }

    /// A `\r\n` pair is one line break, not two.
    ///
    /// flip: `insert` as a plain copy → only the CRLF case fails; the lone-CR case shows the `\r` is not swallowed
    #[test]
    fn a_crlf_pair_is_one_line_break_not_two() {
        let height = |c: &str| {
            let mut e = editor("");
            e.insert(c);
            e.text_layout().size.height
        };
        let one = height("ab");
        let two = height("a\nb");
        assert!(
            two > one,
            "the fixture: a bare newline already breaks the line"
        );

        assert_eq!(height("a\r\nb"), two, "a CRLF pair is one break");
        assert_eq!(
            height("a\rb"),
            two,
            "and a lone CR is still a break, not nothing"
        );
        assert_eq!(
            height("a\nb\nc"),
            two + (two - one),
            "three lines, for scale"
        );
    }

    // --- what a cut and a copy take ---------------------------------------

    /// A cut hands back exactly what it removed and a bare caret cuts `None`, so an
    /// empty cut never overwrites the system clipboard.
    ///
    /// flip: `cut` returning `Some(String::new())` for an empty range → only the `None` assertion fails
    #[test]
    fn cutting_a_selection_returns_it_and_a_bare_caret_returns_nothing() {
        let mut e = editor("hello world");
        e.select_all();
        assert_eq!(e.selected_text(), "hello world", "the fixture is selected");

        assert_eq!(e.cut().as_deref(), Some("hello world"));
        assert_eq!(e.content(), "");
        assert!(!e.has_selection(), "a cut leaves a caret, not a selection");

        assert_eq!(e.cut(), None, "there is nothing left to cut");
        assert_eq!(e.selected_text(), "", "and nothing to copy either");
    }

    /// A cut must not eat a pending style. `drag_to` is the only mover that does not
    /// call `caret_moved`, so it is the only route to a selection with a pending style.
    ///
    /// flip: `cut` as `insert("")` → fails at the last assertion (0 spans)
    #[test]
    fn a_cut_keeps_a_pending_style_where_an_insert_would_eat_it() {
        let mut e = editor("ab");
        e.click(Point::new(0.0, 5.0));
        e.style_selection(CharAttr::Weight(900));
        e.drag_to(Point::new(500.0, 5.0));
        assert_eq!(
            e.selected_text(),
            "ab",
            "the fixture holds a selection *and* a pending style"
        );
        assert!(e.spans().is_empty(), "the style is pending, not applied");

        assert_eq!(e.cut().as_deref(), Some("ab"));
        e.insert("X");
        assert_eq!(
            e.spans().as_slice().len(),
            1,
            "the pending bold outlived the cut and landed on what was typed next"
        );
    }

    /// The caret sits under the ink inside a paragraph indent (block and first-line):
    /// `caret_rect` corrects for the start edge `Cursor::geometry` omits.
    ///
    /// flip: removing the correction in `caret_rect` → the caret moves 0 instead of +60
    #[test]
    fn the_caret_sits_inside_a_paragraph_indent() {
        let content = "one two\nthree";
        for (attr, why) in [
            (
                crate::typography::ParaAttr::IndentStart(Length::Px(60.0)),
                "a block indent",
            ),
            (
                crate::typography::ParaAttr::Indent(Length::Px(60.0)),
                "a first-line indent, which used to follow for free",
            ),
        ] {
            let mut edit = TextEdit::new(
                content,
                TextParts {
                    style: TextStyle {
                        font_size: 20.0,
                        line_height: Some(Length::Em(1.0)),
                        ..TextStyle::default()
                    },
                    sizing: TextSizing::AutoHeight(400.0),
                    ..TextParts::default()
                },
            );
            edit.move_up(false);
            edit.move_line_start(false);
            assert_eq!(
                edit.selected_range(),
                0..0,
                "{why}: at the start of line one"
            );
            let flush = edit.caret_rect(1.0).x0;
            edit.style_paragraph(attr);
            let indented = edit.caret_rect(1.0).x0;
            assert!(
                (indented - flush - 60.0).abs() < 0.01,
                "{why}: the caret went from {flush} to {indented}, wanted +60"
            );
            edit.move_down(false);
            edit.move_line_start(false);
            assert!(
                (edit.caret_rect(1.0).x0 - flush).abs() < 0.01,
                "{why}: paragraph two should be where it was"
            );
        }
    }

    /// The selection highlight needs no indent correction: `Selection::geometry`
    /// already adds `inline_min_coord`, so correcting it too would push it one indent
    /// right. It must agree with the corrected caret.
    #[test]
    fn the_selection_highlight_already_follows_an_indent() {
        let mut edit = TextEdit::new(
            "one two\nthree",
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.0)),
                    ..TextStyle::default()
                },
                sizing: TextSizing::AutoHeight(400.0),
                ..TextParts::default()
            },
        );
        edit.move_up(false);
        edit.move_line_start(false);
        edit.move_line_end(true);
        let flush = edit.selection_rects();
        assert_eq!(flush.len(), 1, "one line selected: {flush:?}");
        edit.style_paragraph(crate::typography::ParaAttr::IndentStart(Length::Px(60.0)));
        let indented = edit.selection_rects();
        assert_eq!(indented.len(), 1, "{indented:?}");
        assert!(
            (indented[0].x0 - flush[0].x0 - 60.0).abs() < 0.01,
            "the highlight moves with the ink on its own: {} to {}",
            flush[0].x0,
            indented[0].x0
        );
        edit.move_line_start(false);
        assert!(
            (edit.caret_rect(1.0).x0 - indented[0].x0).abs() < 0.01,
            "the caret and the highlight have to agree about the line's start edge"
        );
    }

    /// Moving down inside an indented paragraph keeps the column: parley seeds the
    /// sticky column from `Cursor::geometry` but lands with `Cursor::from_point`,
    /// which reads the start edge, so each Down would drift one indent left.
    #[test]
    fn moving_down_inside_an_indented_paragraph_keeps_the_column() {
        let mut edit = TextEdit::new(
            "alpha bravo charlie delta echo foxtrot golf hotel",
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.0)),
                    ..TextStyle::default()
                },
                sizing: TextSizing::AutoHeight(260.0),
                ..TextParts::default()
            },
        );
        edit.style_paragraph(crate::typography::ParaAttr::IndentStart(Length::Px(60.0)));
        edit.move_up(false);
        edit.move_up(false);
        edit.move_up(false);
        edit.move_line_start(false);
        for _ in 0..6 {
            edit.move_right(false, false);
        }
        let from = edit.caret_rect(1.0);
        edit.move_down(false);
        let to = edit.caret_rect(1.0);
        assert!(to.y0 > from.y0, "it should be a line down: {from:?} {to:?}");
        assert!(
            (to.x0 - from.x0).abs() < 12.0,
            "the column should be held across the move, not pulled left by the \
             indent: {} to {}",
            from.x0,
            to.x0
        );
        assert!(
            to.x0 > 60.0,
            "and certainly not left of the indent itself: {}",
            to.x0
        );
    }

    /// Vertical motion survives paragraph spacing, the only thing that makes layout
    /// space (`caret_line_index`) differ from local space (`caret_rect` through the
    /// `YMap`, which is keyed on line-box tops, not ink).
    #[test]
    fn vertical_motion_survives_paragraph_spacing() {
        let mut edit = TextEdit::new(
            "one\ntwo\nsix",
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.0)),
                    ..TextStyle::default()
                },
                paragraph: ParagraphStyle {
                    spacing: Length::Px(40.0),
                    ..ParagraphStyle::default()
                },
                sizing: TextSizing::AutoHeight(400.0),
                ..TextParts::default()
            },
        );
        edit.move_line_start(false);
        assert_eq!(edit.selected_range(), 8..8, "the third paragraph");
        edit.move_up(false);
        assert_eq!(edit.selected_range(), 4..4, "up one line, not two");
        edit.move_up(false);
        assert_eq!(edit.selected_range(), 0..0);
        let mut tops = Vec::new();
        for _ in 0..3 {
            tops.push(edit.caret_rect(1.0).y0);
            edit.move_down(false);
        }
        for pair in tops.windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                (step - 60.0).abs() < 1.0,
                "a 20pt line and a 40pt gap: got {step} from {tops:?}"
            );
        }

        // The highlight takes two shifts (top and bottom) and must not reach up
        // through the gap.
        edit.move_up(false);
        edit.move_line_start(false);
        edit.move_line_end(true);
        let rects = edit.selection_rects();
        assert_eq!(rects.len(), 1, "one line selected: {rects:?}");
        assert!(
            rects[0].height() < 30.0,
            "a 20pt line's highlight must not reach up through the 40pt gap above \
             it: {:?}",
            rects[0]
        );
    }

    /// The sticky column survives a short line: walking down through it must not pull
    /// the caret in (the job `Selection::h_pos` did before `move_line`).
    #[test]
    fn a_short_line_does_not_pull_the_column_in() {
        let mut edit = TextEdit::new(
            "aaaaaaaaaaaa\nbb\ncccccccccccc",
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.0)),
                    ..TextStyle::default()
                },
                sizing: TextSizing::AutoHeight(400.0),
                ..TextParts::default()
            },
        );
        edit.move_up(false);
        edit.move_up(false);
        edit.move_line_end(false);
        assert_eq!(edit.selected_range(), 12..12, "the end of the first line");
        edit.move_down(false);
        assert_eq!(
            edit.selected_range(),
            15..15,
            "the short middle line can only offer its end"
        );
        edit.move_down(false);
        // Asserted on the byte offset: twelve `c`s and twelve `a`s measure
        // differently, so an x tolerance would hide the bug.
        assert_eq!(
            edit.selected_range(),
            28..28,
            "the column the first line was in has to survive the short one, or the \
             caret comes back two characters in rather than twelve"
        );
    }

    /// The panel's write path below the app crate: `paragraph_range` and
    /// `style_paragraph` agree about the range, and the re-shape puts the indent
    /// where the caret was.
    #[test]
    fn styling_the_paragraph_under_the_caret_indents_that_paragraph_only() {
        let content = "one\ntwo\nsix";
        let mut edit = TextEdit::new(
            content,
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.0)),
                    ..TextStyle::default()
                },
                sizing: TextSizing::AutoHeight(300.0),
                ..TextParts::default()
            },
        );
        edit.move_up(false);
        edit.move_line_start(false);
        assert_eq!(edit.paragraph_range(), 4..8, "the caret is in the second");

        edit.style_paragraph(crate::typography::ParaAttr::IndentStart(Length::Px(50.0)));
        let lines: Vec<f64> = edit
            .text_layout()
            .runs
            .iter()
            .map(|r| f64::from(r.glyphs[0].x))
            .collect();
        assert_eq!(lines.len(), 3, "one run per line: {lines:?}");
        assert!((lines[0] - 0.0).abs() < 0.01, "{lines:?}");
        assert!(
            (lines[1] - 50.0).abs() < 0.01,
            "only the paragraph under the caret should have moved: {lines:?}"
        );
        assert!((lines[2] - 0.0).abs() < 0.01, "{lines:?}");
        assert_eq!(edit.selected_range(), 4..4);
    }

    /// A later paragraph's indent stays on it through both edit paths (`replace` via
    /// the deletes, and `insert`), each of which re-bases on its own. The exact range
    /// is asserted because a containment check passes without the re-basing.
    #[test]
    fn editing_one_paragraph_keeps_a_later_paragraphs_indent_on_it() {
        let content = "first\nsecond";
        let second = content.find('\n').expect("two paragraphs") + 1;
        let mut p = TextParts {
            style: TextStyle {
                font_size: 20.0,
                ..TextStyle::default()
            },
            sizing: TextSizing::AutoHeight(300.0),
            ..TextParts::default()
        };
        p.para_spans.set(
            second..content.len(),
            crate::typography::ParaAttr::IndentStart(Length::Px(50.0)),
            &p.paragraph,
        );
        let mut edit = TextEdit::new(content, p);
        let span_over_paragraph_two = |e: &TextEdit| {
            let range = e.para_spans().as_slice()[0].range();
            let starts = paragraph_starts(e.content());
            assert_eq!(starts.len(), 2, "still two paragraphs: {starts:?}");
            assert!(
                range.contains(&starts[1]),
                "paragraph two starts at byte {} and its override covers {range:?} \
                 — a paragraph reads its style at its first byte, so this is the \
                 whole of whether the indent survived",
                starts[1]
            );
            range
        };

        edit.move_up(false);
        edit.move_line_start(false);
        edit.move_line_end(true);
        edit.backspace();
        assert_eq!(
            edit.content(),
            "\nsecond",
            "the first paragraph is empty now"
        );
        assert_eq!(span_over_paragraph_two(&edit), 1..7);

        edit.insert("ab");
        assert_eq!(edit.content(), "ab\nsecond");
        assert_eq!(span_over_paragraph_two(&edit), 3..9);
    }
}

#[cfg(test)]
mod rail_session_tests {
    //! A live edit session on type that is set along a path; its own module because
    //! every fixture needs a rail on the parts.

    use super::*;
    use crate::typography::TextAlign;

    /// A rail from `(0, 0)` running right, `len` long, so a point's `x` is its
    /// distance along the curve and its `y` its distance across.
    fn straight(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((len, 0.0));
        p
    }

    /// The centre of the first quad a caret draws as.
    fn caret_at(e: &TextEdit) -> Point {
        let q = e.caret_quads(1.0);
        let c = q.first().expect("a caret is drawn");
        (c.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / 4.0).to_point()
    }

    fn railed(content: &str, rail: BezPath, align: TextAlign) -> TextEdit {
        railed_at(content, rail, align, 0.0)
    }

    /// `railed` with an `on_path_offset`, the fraction of the rail the type starts at.
    fn railed_at(content: &str, rail: BezPath, align: TextAlign, offset: f64) -> TextEdit {
        let paragraph = ParagraphStyle {
            align,
            ..ParagraphStyle::default()
        };
        TextEdit::new(
            content,
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    line_height: Some(Length::Em(1.2)),
                    ..TextStyle::default()
                },
                paragraph,
                on_path: Some(rail),
                on_path_offset: offset,
                ..TextParts::default()
            },
        )
    }

    /// The caret is drawn where the bent ink is, not at parley's flat position;
    /// asserted against the glyphs rather than a hand-derived coordinate.
    #[test]
    fn the_caret_sits_on_the_bent_ink() {
        // `TextEdit::new` leaves the caret at the end of the content.
        let e = railed("Hamburgefonstiv", straight(600.0), TextAlign::Center);
        let layout = e.text_layout();
        let last = layout
            .runs
            .last()
            .and_then(|r| r.glyphs.last())
            .expect("the fixture shapes");
        let mid = caret_at(&e);

        assert!(
            (mid.x - f64::from(last.x)).abs() < 25.0,
            "the caret is beside the last glyph, not at the flat position: caret \
             {mid:?}, last glyph ({}, {})",
            last.x,
            last.y
        );
        assert!(
            (mid.y - f64::from(last.y)).abs() < layout.line_height,
            "and on the same line of it: caret {mid:?}, last glyph ({}, {})",
            last.x,
            last.y
        );
    }

    /// Clicking the bent ink puts the caret in the character under the pointer,
    /// round-tripped through the caret's own drawn position so both directions are
    /// tested together.
    #[test]
    fn clicking_bent_ink_lands_on_the_character_under_it() {
        for at in [1_usize, 5, 9, 14] {
            let mut e = railed("Hamburgefonstiv", straight(600.0), TextAlign::Center);
            while e.selected_range().start > at {
                e.move_left(false, false);
            }
            let want = e.selected_range().start;
            assert_eq!(want, at, "the fixture must reach the byte it names");
            let mid = caret_at(&e);
            e.click(mid);
            assert_eq!(
                e.selected_range().start,
                want,
                "clicking the caret's own drawn position must not move it (byte {want})"
            );
        }
    }

    /// A flat session is the control: `caret_quads` on ordinary type is the four
    /// corners of `caret_rect`, so a warp is never built for a node with no rail.
    #[test]
    fn a_flat_session_gets_exactly_its_own_rectangle() {
        let e = TextEdit::new(
            "hello",
            TextParts {
                style: TextStyle {
                    font_size: 20.0,
                    ..TextStyle::default()
                },
                ..TextParts::default()
            },
        );
        let r = e.caret_rect(2.0);
        let quads = e.caret_quads(2.0);
        assert_eq!(quads.len(), 1, "one quad for flat type");
        assert_eq!(
            quads[0],
            [
                Point::new(r.x0, r.y0),
                Point::new(r.x1, r.y0),
                Point::new(r.x1, r.y1),
                Point::new(r.x0, r.y1),
            ]
        );
    }

    /// A click past the wrap point of a closed rail lands under the pointer, not at
    /// character 0 (`shortest_way_round` normalises the negative arc length). The
    /// non-zero `on_path_offset` moves the wrap point into the middle of the text.
    ///
    /// flip: `shortest_way_round` reduced to `x` → fails at the `"D"` click (caret 0 against 3)
    #[test]
    fn clicking_bent_ink_on_a_closed_rail_lands_on_the_character_under_it() {
        let rail = kurbo::Circle::new((0.0, 0.0), 120.0).to_path(0.01);
        assert!(
            matches!(rail.elements().last(), Some(PathEl::ClosePath)),
            "the fixture must be a closed rail, which is what `PathWarp::of` reads"
        );

        let content = "ABCDEFGH";
        let mut e = railed_at(content, rail, TextAlign::Start, 0.95);
        let origins: Vec<Point> = e
            .text_layout()
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| Point::new(f64::from(g.x), f64::from(g.y)))
            .collect();
        assert_eq!(
            origins.len(),
            content.len(),
            "fixture: one glyph per letter, so a glyph index is a byte index"
        );

        for (byte, origin) in origins.iter().enumerate() {
            e.click(*origin);
            assert_eq!(
                e.selected_range().start,
                byte,
                "clicking the ink of {:?} puts the caret in it, not at 0 \
                 (glyph origin {origin:?})",
                &content[byte..=byte]
            );
        }
    }
}

#[cfg(test)]
mod optical_margin_tests {
    //! Optical margin alignment places a line by its ink. The three strings differ in
    //! left side bearing (`W` 2.34%, `H` 8.59%, `'` 10.06% of the font size); three
    //! `H` words would already agree.
    use super::*;
    use crate::typography::TextAlign;

    /// The union of every run's ink, not the layout's advance extents.
    fn ink(tl: &TextLayout) -> Rect {
        let mut b: Option<Rect> = None;
        for r in &tl.runs {
            let rb = run_outline(r).bounding_box();
            if rb.width() > 0.0 {
                b = Some(b.map_or(rb, |a: Rect| a.union(rb)));
            }
        }
        b.expect("the fixture must draw something")
    }

    fn label(s: &str, align: TextAlign, optical: bool, sizing: TextSizing) -> TextLayout {
        let d = TextStyle::default();
        let mut p = super::tests::parts(&d, &sizing);
        p.paragraph.align = align;
        p.paragraph.optical_margins = optical;
        layout(p.as_ref(s))
    }

    /// Three auto-width labels starting with different letters line their ink up on
    /// the start edge.
    ///
    /// flip: dropping `x -= lsb` from the optical arm in `break_lines` → fails at the all-equal assertion (and overshoots the end test)
    #[test]
    fn optical_margins_put_every_labels_ink_on_the_start_edge() {
        let off: Vec<f64> = ["Wamburg", "Hamburg", "'amburg"]
            .iter()
            .map(|s| ink(&label(s, TextAlign::Start, false, TextSizing::Auto)).x0)
            .collect();
        assert!(
            off.windows(2).any(|w| (w[0] - w[1]).abs() > 0.5),
            "the fixture is not in the state this test is about — these three \
             strings must disagree without the setting, or the assertion below is \
             true of nothing: {off:?}"
        );

        let on: Vec<f64> = ["Wamburg", "Hamburg", "'amburg"]
            .iter()
            .map(|s| ink(&label(s, TextAlign::Start, true, TextSizing::Auto)).x0)
            .collect();
        for x in &on {
            assert!(
                x.abs() < 0.01,
                "every label's ink starts on the edge, not its advance origin: {on:?}"
            );
        }
    }

    /// The node's box does not move when optical margins are switched on, neither
    /// its start edge nor its width (a line started earlier pulls parley's width in).
    ///
    /// flip: `box_of` reading `layout.width()` instead of the natural width → fails at "the box's width changed"
    #[test]
    fn optical_margins_leave_the_box_where_it_was() {
        for s in ["Wamburg", "Hamburg", "'amburg"] {
            let off = label(s, TextAlign::Start, false, TextSizing::Auto).bounds();
            let on = label(s, TextAlign::Start, true, TextSizing::Auto).bounds();
            assert!(
                (off.x0 - on.x0).abs() < 0.01,
                "{s}: the box's start edge moved, {} to {}",
                off.x0,
                on.x0
            );
            assert!(
                (off.width() - on.width()).abs() < 0.01,
                "{s}: the box's width changed, {} to {}",
                off.width(),
                on.width()
            );
        }
    }

    /// An end-aligned line hangs its last glyph's ink out to the measure.
    ///
    /// flip: widening the measure by `lsb` alone instead of `lsb + rsb` → fails at the ink-reaches-the-edge assertion (198.789 against 200)
    #[test]
    fn optical_margins_hang_an_end_aligned_line_out_to_the_measure() {
        let box_ = TextSizing::Fixed(kurbo::Size::new(200.0, 80.0));
        for s in ["Wamburg", "Hamburg", "'amburg"] {
            let off = ink(&label(s, TextAlign::End, false, box_)).x1;
            assert!(
                off < 199.5,
                "the fixture: without the setting the ink stops short of the \
                 measure, which is the gap this closes ({s} at {off})"
            );
            let on = ink(&label(s, TextAlign::End, true, box_)).x1;
            assert!(
                (on - 200.0).abs() < 0.01,
                "{s}: the last glyph's ink should reach the measure, not its \
                 advance — got {on}"
            );
        }
    }

    /// A justified block spans its measure by ink at both ends: the correction goes
    /// into the measure before parley justifies, since shifting a justified line
    /// cannot work. Every width from 120 to 300 is swept to reach the re-broken lines.
    ///
    /// flip: shifting the line (`x -= lsb`) without `measure += lsb + rsb` → fails at the right-edge assertion (199.023 against 200)
    /// flip: one correcting pass with offsets read by line index → fails at width 123
    #[test]
    fn a_justified_block_spans_its_measure_by_ink() {
        let s = "Hamburg quick brown fox jumps over lazy dogs and runs away fast";
        let tl = label(
            s,
            TextAlign::Justify,
            false,
            TextSizing::Fixed(kurbo::Size::new(200.0, 80.0)),
        );
        assert!(
            tl.runs.iter().map(|r| r.glyphs.len()).sum::<usize>() > 20,
            "the fixture must actually wrap, or there is no justification to test"
        );

        for w in 120..=300 {
            let w = f64::from(w);
            let box_ = TextSizing::Fixed(kurbo::Size::new(w, 400.0));
            let on = ink(&label(s, TextAlign::Justify, true, box_));
            assert!(
                on.x0.abs() < 0.01,
                "at {w} the justified block starts on the edge: {}",
                on.x0
            );
            assert!(
                (on.x1 - w).abs() < 0.01,
                "and at {w} reaches it at the far end, which a shift alone cannot do: {}",
                on.x1
            );
            // No line leaves the box; a pinned line (see `shape`) may end up to one
            // bearing short of the far edge, which is the allowed direction.
            let lines = line_inks(&label(s, TextAlign::Justify, true, box_));
            for (i, (x0, x1)) in lines.iter().enumerate() {
                assert!(
                    *x0 > -0.01 && *x1 < w + 0.01,
                    "at {w}, line {i}'s ink {x0}..{x1} leaves its box: {lines:?}"
                );
            }
        }
    }

    /// Each line's ink as `(x0, x1)`, keyed by the baseline of a run's first glyph.
    fn line_inks(tl: &TextLayout) -> Vec<(f64, f64)> {
        let mut lines: Vec<(f64, Rect)> = Vec::new();
        for r in &tl.runs {
            let Some(y) = r.glyphs.first().map(|g| f64::from(g.y)) else {
                continue;
            };
            let rb = run_outline(r).bounding_box();
            if rb.width() <= 0.0 {
                continue;
            }
            match lines.iter_mut().find(|(ly, _)| (ly - y).abs() < 0.5) {
                Some((_, b)) => *b = b.union(rb),
                None => lines.push((y, rb)),
            }
        }
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        lines.into_iter().map(|(_, b)| (b.x0, b.x1)).collect()
    }

    /// Every line of a wrapped, start-aligned block starts its ink on the edge at
    /// every width, though the second break pass moves words across lines.
    ///
    /// flip: one correcting pass → fails at width 123 on line 2
    /// flip: settling without pinning → fails at width 160 (the cap runs out and the fallback is uncorrected)
    #[test]
    fn every_line_of_a_wrapped_block_starts_on_the_edge() {
        let s = "Hamburg quick brown fox jumps over lazy dogs and runs away fast";
        for w in 120..=300 {
            let w = f64::from(w);
            let box_ = TextSizing::Fixed(kurbo::Size::new(w, 400.0));
            let lines = line_inks(&label(s, TextAlign::Start, true, box_));
            assert!(lines.len() > 1, "at {w} the fixture must wrap");
            for (i, (x0, _)) in lines.iter().enumerate() {
                assert!(
                    x0.abs() < 0.01,
                    "at {w}, line {i}'s ink starts at {x0}, not on the edge: {lines:?}"
                );
            }
        }
    }

    /// The SVG writer's `ExportLine::x` carries the correction too, since the
    /// correction is applied inside `shape`; asserted on `x`, not emitted markup.
    ///
    /// flip: gating the second pass off in `shape` → fails here and nowhere above (they all read `layout`)
    #[test]
    fn the_svg_writers_line_x_carries_the_correction_too() {
        let d = TextStyle::default();
        let mut p = super::tests::parts(&d, &TextSizing::Auto);
        let off = export_lines(p.as_ref("Hamburg"))[0].x;
        p.paragraph.optical_margins = true;
        let on = export_lines(p.as_ref("Hamburg"))[0].x;
        assert!(
            (off - on - 1.375).abs() < 0.01,
            "the exported line must start a left bearing earlier — `H` measures \
             1.375px at 16px, and the writer got {off} then {on}"
        );
    }

    /// `optical_offsets`' documented edges: a trailing space hangs nothing, a blank
    /// paragraph costs neither neighbour its correction, right-to-left text is
    /// corrected at its right edge, and empty content lays out.
    ///
    /// flip: `optical_offsets` taking the last glyph whatever its ink → fails at "a trailing space hangs nothing"
    #[test]
    fn optical_offsets_documented_edges_hold() {
        use crate::typography::TextDirection;
        let box_ = TextSizing::Fixed(kurbo::Size::new(200.0, 80.0));
        let x1 = |s: &str| ink(&label(s, TextAlign::End, true, box_)).x1;

        assert!(
            (x1("Hamburg ") - x1("Hamburg")).abs() < 0.01,
            "a trailing space hangs nothing: {} against {}",
            x1("Hamburg "),
            x1("Hamburg")
        );

        let blank = line_inks(&label("Hamburg\n\nWamburg", TextAlign::End, true, box_));
        assert_eq!(
            blank.len(),
            2,
            "the fixture: two inked lines around a blank one"
        );
        for (x0, x1) in &blank {
            assert!(
                (x1 - 200.0).abs() < 0.01,
                "a blank paragraph between them costs neither its hang: {x0}..{x1}"
            );
        }

        let rtl = |optical: bool| {
            let d = TextStyle::default();
            let mut p = super::tests::parts(&d, &box_);
            p.paragraph.align = TextAlign::Start;
            p.paragraph.direction = TextDirection::Rtl;
            p.paragraph.optical_margins = optical;
            ink(&layout(p.as_ref("שלום עולם")))
        };
        assert!(
            rtl(false).x1 < 199.5,
            "the fixture: a right-to-left start edge is the right one, and it is short \
             of the measure without the setting ({})",
            rtl(false).x1
        );
        assert!(
            (rtl(true).x1 - 200.0).abs() < 0.01,
            "and with it the ink reaches the right edge: {}",
            rtl(true).x1
        );

        let empty = label("", TextAlign::Start, true, TextSizing::Auto);
        assert!(
            empty.runs.is_empty(),
            "empty content lays out, with nothing in it"
        );
    }

    /// No paragraph attribute reaches optical margins, so the per-paragraph clauses in
    /// `Paragraphs::any_optical_margins` and `break_lines` are inert; the exhaustive
    /// `match` fails to compile when a new `ParaAttrKind` might change that.
    #[test]
    fn no_paragraph_attribute_reaches_optical_margins() {
        use crate::typography::{Length, ParaAttr, ParaAttrKind, ParaSpans};
        for default_on in [false, true] {
            let default = ParagraphStyle {
                optical_margins: default_on,
                ..ParagraphStyle::default()
            };
            let mut spans = ParaSpans::default();
            for kind in ParaAttrKind::ALL {
                let attr = match kind {
                    ParaAttrKind::Spacing => ParaAttr::Spacing(Length::Px(9.0)),
                    ParaAttrKind::Indent => ParaAttr::Indent(Length::Px(9.0)),
                    ParaAttrKind::Hanging => ParaAttr::Hanging(true),
                    ParaAttrKind::IndentStart => ParaAttr::IndentStart(Length::Px(9.0)),
                    ParaAttrKind::IndentEnd => ParaAttr::IndentEnd(Length::Px(9.0)),
                    ParaAttrKind::Marker => ParaAttr::Marker(None),
                    ParaAttrKind::Level => ParaAttr::Level(2),
                };
                spans.set(0..5, attr, &default);
            }
            let resolved = spans.resolve(&default, 0);
            assert_eq!(
                resolved.optical_margins, default_on,
                "every paragraph attribute set, and the node's setting still decides"
            );
            assert_ne!(
                resolved, default,
                "the fixture: the spans did override something"
            );
        }
    }
}
