//! Text shaping, measurement, and layout for Text nodes (§5.4, §5.4a).
//!
//! Real layout via `parley`. Core never scans system fonts (parley's `system`
//! feature is disabled in Cargo.toml) — it shapes only against fonts it's been
//! handed, so the engine stays filesystem/network-free (the headless invariant,
//! §3). With a fixed set of registered fonts, measurement is deterministic.
//!
//! Fonts: Inter (upright + italic variable) is bundled and always available as
//! the default/fallback. Additional families are supplied at runtime by the app
//! via [`register_fonts`] (system fonts, downloaded Google Fonts) — core never
//! reads files or the network itself, so the headless/FS-free invariant holds;
//! it only shapes with whatever bytes it's handed. Unknown families fall back to
//! Inter.
//!
//! `layout()` returns, per run, the exact [`peniko::FontData`] parley resolved
//! (a cheaply-cloned blob handle — no byte copying), so `parley` lives entirely
//! inside this crate and the renderer draws glyphs straight from that blob.
//! `Resolved` needs no font context threaded through it.
//!
//! Shaping is expensive, so results are cached in [`crate::resolve::Resolved`]
//! (§5.9): bounds, hit-testing and the scene walk all read that cache, and
//! `layout()` here is the uncached path used to fill it. Registering a new
//! family invalidates it — see [`crate::resolve::Resolved::invalidate_text`].
//!
//! ## The three things that move text after parley is done with it
//!
//! Paragraph spacing, box trim and vertical alignment are all ours, and all
//! three shift the ink *after* the lines are broken. They are folded into one
//! [`YMap`] — a single layout-y ↔ local-y translation applied at **both**
//! boundaries: the scene walk that draws, and the hit test that asks parley
//! where a point landed. One function used twice, in opposite directions, is
//! what keeps the caret under the pointer; two hand-rolled offsets is how a
//! click below a spaced paragraph lands a line out.
//!
//! ## Horizontal paragraph geometry is ours too, and set *before* the break
//!
//! The indents are the mirror image: they change how wide a line may be, so they
//! have to be in place before that line is broken rather than folded in
//! afterwards. [`break_lines`] sets parley's per-line `line_x` and
//! `line_max_advance` from the paragraph the next line falls in
//! ([`Paragraphs`]), which is what makes an indent vary by paragraph on a single
//! `Layout` — and the caret needs nothing for it, because those two values are
//! the line's `inline_min_coord`/`inline_max_coord`, which is what
//! `Selection::from_point` and the selection geometry already read.
//! `Layout::set_text_indent` is deliberately unused: it is a second mechanism on
//! the same edge (§15 D163).
//!
//! ## The brush is a run index
//!
//! parley is generic over its brush type and we have no use for its colouring
//! (glyph fills come from the node's `Paint`), so the brush carries the index of
//! *our* run instead. That is what lets the decoration walk recover the
//! attributes parley cannot hold — a line style, a decoration colour — from a
//! `GlyphRun` it is handed, rather than guessing from the geometry.
//!
//! [`TextEdit`] is the caret/selection model for in-canvas editing. It lives
//! here because cluster boundaries and bidi-aware movement are parley's, and
//! parley is core's dependency alone.

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
///
/// See the module docs — this is how a decoration's colour and line style get
/// back out of a laid-out `GlyphRun`.
type RunIndex = u32;

/// The brush the node's **defaults** are pushed with — deliberately a value no
/// style-run index can take.
///
/// **Because `0` was, and that was a live bug (§15 D153).** `shape` skips pushing
/// any style run whose style equals the defaults, so those bytes keep the
/// defaults' brush; with the defaults on `0`, an inheriting run was
/// indistinguishable from *style run 0*. Whenever style run 0 was itself a span —
/// i.e. whenever the user styled the first character — every inheriting run read
/// that span's baseline shift and decoration. A sentinel makes the lookup total the
/// other way: out of range means "the defaults", which is exactly what an
/// unpushed run inherits.
const DEFAULT_RUN: RunIndex = RunIndex::MAX;

/// Register additional font data (a `.ttf`/`.otf`) with the shaping engine on
/// the current thread, returning the family name(s) it contains so the app can
/// offer them in a picker. The app is responsible for obtaining the bytes
/// (reading a system font, downloading a web font); core stays FS/network-free.
///
/// Registration is per-thread (the engine is thread-local); the app registers
/// on the same thread it renders/measures on (the UI thread).
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

/// Un-register every face of `family` from this thread's shaping engine.
///
/// **The counterpart to [`register_fonts`], and the reason the app can afford to
/// preview a thousand families.** Browsing the picker registers each family it
/// draws a row for, and font bytes are megabytes, not kilobytes — without a way
/// back out, scrolling the list is a leak. Returns whether anything was removed.
///
/// The family *name* survives (fontique keeps the name once it has seen it, with
/// an empty face list), which is why [`is_family_available`] asks about faces
/// rather than about the name.
///
/// Bytes are only actually freed once nothing else holds them: a cached
/// [`TextLayout`] carries the blob its runs were shaped from, so a family the
/// document is using stays in memory until it is re-shaped without it. That is
/// the behaviour we want — eviction cannot pull a face out from under a layout.
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
        // Collected first: `unregister_font` borrows the collection mutably, and
        // it rebuilds the family's face list on each call, so iterating it live
        // would be iterating what is being replaced.
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
/// A document may reference a family the machine does not have; it still lays
/// out and renders (via the bundled Inter fallback), but consumers want to say
/// so — the MCP snapshot reports it as a warning, and the UI can surface it
/// (§5.4a: "a warning surfaced in the UI and in the MCP snapshot").
///
/// **A registered *name* is not a registered *face*.** `unregister_family`
/// leaves the name behind with nothing in it, so asking fontique whether it
/// knows the name would answer yes for a family that can no longer draw
/// anything — and every caller here means "will text in this family shape as
/// itself".
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

/// One variable-font axis, as the panel needs to draw a row for it.
///
/// **Read from the font, never from a table of our own.** Which axes exist,
/// their ranges and their names are the typeface's decision; a hardcoded list is
/// wrong the first time a family ships a `GRAD` or a `ROND`.
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
    /// One axis as a font's `fvar` table states it, with the range made usable
    /// (§15 D435).
    ///
    /// ⚠️ **The one place a `FontAxis` is built from a file, because `fvar` is
    /// validated by nothing on the way in.** `skrifa` hands back the raw
    /// `AxisRecord` Fixed values and fontique's own parse is not a filter either,
    /// so a `.ttf` whose `opsz` record has `minValue = 200` against
    /// `maxValue = 32` registers cleanly — and then the next layout of *any*
    /// text node panics, because `f64::clamp` is an `assert!` and fires in
    /// release too. One broken file in the user's own font folder is enough, and
    /// if the document's family is that font it fires at every launch, including
    /// through the library's cover rasterizer.
    ///
    /// **Two constructions used to spell this**, `family_axes` and
    /// `family_axes_locked`, differing only in where the name comes from — which
    /// is the divergence this project's own `is_paintable` comment predicts. One
    /// now, so a guard added here cannot be missing from the other.
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

    /// `value` brought inside this axis's range — the one spelling of that, so
    /// no reader has to know the range came out of a file.
    ///
    /// ⚠️ **Not `value.clamp(self.min, self.max)`, which is what the two readers
    /// used to say and is a panic.** `f64::clamp` is an `assert!` on
    /// `min <= max`, so it fires in release as well as in debug, and the fields
    /// are `pub` — [`crate::text::family_axes`] orders them on the way out of the
    /// font, but nothing stops a caller building a `FontAxis` by hand or a later
    /// reader of `fvar` skipping that path.
    pub fn clamp(&self, value: f64) -> f64 {
        value
            .max(self.min.min(self.max))
            .min(self.min.max(self.max))
    }
}

/// `(low, high)` from a pair a font asserted was one, with a non-finite end
/// replaced by the other.
///
/// **Both halves matter and they are different failures.** A reversed pair
/// panics `f64::clamp`; a `NaN` end panics it too, and `NaN` survives `f64::min`
/// and `f64::max` silently, so ordering alone would not close it.
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
    /// Whether the **face** this instance was read out of is an italic one.
    ///
    /// Not derivable from `coords`, and that is the whole reason it is here: a
    /// family that keeps its italics in a second file needs no `ital` axis to
    /// express them, so its italic instances carry coordinates indistinguishable
    /// from the upright ones and only the face's own style bit tells them apart.
    /// Bundled Inter is exactly that shape.
    pub italic: bool,
}

/// The variation axes of `family`'s default face, in `fvar` order.
///
/// Empty for a family with no variable face — which is also the honest answer
/// for the panel: no axes, no axis rows.
///
/// **The default face, deliberately, unlike [`family_named_instances`].** The
/// right answer is the axes of the face that will actually shape the run, which
/// depends on the weight and slope the panel is already showing; the default
/// face's is the closest available stand-in, and it is the same answer for every
/// two-file family seen so far (bundled Inter's two faces carry the same
/// `opsz`/`wght`). Unioning across faces was the other candidate and is worse: it
/// would draw a row for an axis the shaping face does not have, and dragging that
/// row would do nothing. `family_features` reads the default face for the same
/// reason. If a family ever turns up whose faces disagree, the fix is to thread
/// the style through, not to widen this.
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
/// **Every face, not just the default one.** A variable family very often ships
/// as two files — upright and italic — and the italic file's `fvar` is where its
/// italic cuts are named; reading only the default face therefore offered nine
/// upright cuts of bundled Inter and no italic at all, and since the panel hides
/// its bold/italic pair as soon as a variant list exists, italic Inter had no
/// control anywhere in the app. Each face reports its own `italic`, because
/// nothing in the coordinates does (see [`NamedInstance::italic`]).
///
/// Deduplicated by name and slope, first face winning, so a family that
/// registers the same cut twice does not offer it twice. The names come out of
/// the fonts' own `name` tables and already say "Bold Italic" where they mean it,
/// so nothing here has to invent or decorate one.
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

/// One entry in the family's variant list — what the main panel's *variant*
/// dropdown offers.
///
/// **One list from two sources**, because "which cut of this family" is one
/// question and the answer's shape depends on the typeface, not on the user: a
/// variable family answers with its `fvar` named instances, a static one with its
/// faces. Two controls would put the difference in the user's lap.
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
                // Weight and slope have dedicated controls, so an instance's own
                // values for those axes are lifted out of `coords` and into the
                // fields that already drive them — otherwise picking "Bold" would
                // leave the weight field reading 400 beside bold text.
                let weight = axis_of(&i.coords, WGHT).unwrap_or(400.0).round() as u16;
                // Three ways a family can say "italic", and it only has to say it
                // once: an `ital` axis, a pushed `slnt`, or — the common case for a
                // family that ships two variable files — a second *face* that is
                // italic with no axis mentioning it at all.
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
/// Uprights before italics, ascending weight — the dropdown's own order — and a
/// family that registers the same cut twice must not offer it twice.
///
/// 🚨 **Lifted out of [`family_variants`]' engine closure so that it can be
/// tested at all** (§15 D608, `[S5.1-L6-05]`). That arm serves nearly every
/// *static* system font — Arial, Georgia, Helvetica, the majority of any Windows
/// or macOS font folder — and had **zero test callers anywhere in the workspace**.
///
/// **Nothing in the tree can reach it, and that is a fact about the fixtures
/// rather than about the code**: `Engine::new` builds its collection with
/// `system_fonts: false`, deliberately, so core holds only what something
/// registers — and both bundled files are **variable**, so `family_variants`
/// returns early on the named-instance arm for every fixture there is. The one
/// other `register_fonts` test is `#[ignore]`d, needs the network, and downloads a
/// variable font too. This is §15 D269's move — a decision that cannot be reached
/// becomes a free function that can.
///
/// ⚠️ **The repository does hold one static face and it is not the answer.**
/// `ondin-app`'s `Phosphor.ttf` has no `fvar` — the first draft of this comment
/// called all three bundled faces variable and was wrong, which `arch-scribe`
/// caught by grepping the binary. It is an icon font installed into *egui*, never
/// registered with parley, in the wrong crate for a core test, and a single-face
/// family exercises neither the sort nor the dedup: it would reach the query below
/// and nothing else.
///
/// ⚠️ **What this does *not* cover, and nothing yet does**: the parley query above
/// it — `collection.family_id`, `family(id)`, and the `f.weight()` / `f.style()`
/// reads that build the list. Those need a registered static face, which the tree
/// has not got. The *decisions* are here; the plumbing is still on trust.
fn ordered_cuts(out: &mut Vec<FontVariant>) {
    out.sort_by_key(|v| (v.italic, v.weight));
    out.dedup_by(|a, b| a.weight == b.weight && a.italic == b.italic);
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
/// **A font can name two ranges of tags and no others.** Every registered tag's
/// meaning is fixed by the OpenType feature registry — a font cannot make `ccmp`
/// mean something else — so its name and its description belong to a static
/// table beside the panel that shows them. `ss01`–`ss20` and `cv01`–`cv99` are
/// the exceptions: they are *defined* by the font, which says so in a
/// `FeatureParams` table pointing at its own `name` entries.
///
/// That split is why these are `Option`s rather than a `String`: `None` means
/// "the registry knows what this is", not "unnamed".
#[derive(Clone, Debug, PartialEq)]
pub struct FaceFeature {
    pub tag: Tag,
    /// The font's own UI label — `ssXX`'s `UINameID`, `cvXX`'s
    /// `featUiLabelNameID`.
    ///
    /// `None` for every other tag, **and for the many faces that document their
    /// `cvXX` set on a web page and leave the table NULL in the binary**, which
    /// is why the generic fallback in the panel is mandatory rather than
    /// decoration.
    pub name: Option<String>,
    /// `cvXX`'s `featUiTooltipTextNameID`, for a row's hover.
    pub tooltip: Option<String>,
    /// One entry per named value the `cvXX` feature declares
    /// (`numNamedParameters`), **in value order: index `k` is `value = k + 1`**.
    /// Empty for a plain on/off feature, which is nearly all of them.
    ///
    /// **Past one entry, the feature is not a switch** but a 1..n choice, which is
    /// what [`crate::FeatureSetting::value`]'s "higher values select an alternate"
    /// was written for.
    ///
    /// The labels come from the `ParamUILabelNameID` range — `numNamedParameters`
    /// consecutive `name` ids starting at `firstParamUILabelNameID` — so a `Vec`
    /// rather than a count *and* a list: the two cannot then disagree about how
    /// many values there are, and the count this used to be is `values.len()`.
    /// An entry is `None` where the font left that id NULL, which includes the
    /// whole range when `firstParamUILabelNameID` itself is NULL; the generic
    /// "Alternate 2" for that case is the panel's to write, like every other
    /// fallback here.
    pub values: Vec<Option<String>>,
}

/// The OpenType features `family`'s default face actually offers (`GSUB` and
/// `GPOS`), sorted by tag and de-duplicated.
///
/// The Character tab's switch list is built from this, so a family that has no
/// small caps offers no small-caps switch — a switch that shapes to nothing is
/// worse than no switch.
///
/// **The default face, deliberately** — [`family_axes`] carries the reasoning,
/// and it is stronger here: a feature only the *italic* face carries would draw
/// a switch that shapes to nothing while upright text is selected, which is the
/// one thing this list exists to avoid.
///
/// **Both tables, and the curation is the panel's.** GPOS is where the shaping
/// mechanics live (`mark`, `mkmk`, `curs`, `dist`), so dropping it wholesale is a
/// tempting cheap cut — but it also holds `kern` and the CJK width features a
/// user genuinely chooses, and a tag's own registry entry already says which
/// kind it is. Two answers to one question is how they come to disagree, so this
/// reports what the face has and the panel decides what to show.
pub fn family_features(family: &str) -> Vec<FaceFeature> {
    with_family_font(family, |font| {
        use read_fonts::{TableProvider as _, tables::layout::FeatureList};
        let mut out: Vec<FaceFeature> = Vec::new();
        let mut collect = |list: Result<FeatureList<'_>, _>| {
            let Ok(list) = list else { return };
            for record in list.feature_records() {
                let tag = Tag::new(record.feature_tag().to_be_bytes());
                // One entry per tag: a feature is registered once per script and
                // language system, so a font supporting Latin and Cyrillic lists
                // `kern` twice with the same meaning both times.
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
/// **Gated on the tag being in one of the two ranges, and that guard is load
/// bearing** rather than an optimisation: read-fonts picks which params table to
/// parse from the first two bytes of the tag alone (`&tag[..2] == b"ss"`, with an
/// apology in the comment above it), so `ssty` — a mechanic with no params table
/// at all — would be read as a stylistic set and hand back whatever two bytes
/// happen to sit at that offset. `typography::stylistic_set` rejects it, so
/// nothing here has to know that.
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
        // `size` is the third variant and cannot be reached: the gate above lets
        // only the two font-defined ranges through.
        FeatureParams::Size(_) => return None,
    };
    Some(FaceFeature {
        tag,
        name,
        tooltip,
        values,
    })
}

/// Run `f` against the raw bytes of **every** face in `family`, the default face
/// first, skipping any that will not load.
///
/// `f`'s second argument is whether that face is an italic one — fontique's
/// `FontInfo::style`, which is read from `OS/2` and is the only place a
/// two-file variable family says which of its files is the italic (see
/// [`NamedInstance::italic`]).
///
/// The default face leads for the same reason it does everywhere else: it is what
/// a preview draws and what the document shapes with, so a list built from these
/// reads in the order the user already sees the family in.
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
        // Copied out before any blob is loaded: `source_cache` is a sibling field
        // of the collection, and `load` wants it mutably.
        let mut fonts: Vec<_> = info.fonts().to_vec();
        // Moved rather than swapped, so the faces behind it keep their own order.
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
/// The blob is held only for the duration of the call, so nothing borrowed from
/// the font escapes — which is why these three functions return owned data even
/// though the underlying tables are views.
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
/// The spec spends the ids **consecutively**: `count` of them starting at
/// `first`, one for each value the feature offers, in value order. So there is no
/// per-value offset to read — the arithmetic here *is* the format — and the only
/// two ways it can go wrong are both handled by [`feature_string`]'s range check
/// rather than by a second rule here: a NULL `first` (the copyright trap, which
/// would otherwise put the foundry's notice on every value of the control) and a
/// range that walks off the end of the legal id space.
///
/// A font may declare a count and name none of them, so an entry is `None`
/// independently of its neighbours and the length still says how many values
/// there are.
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
/// **A NULL id is `0`, and `0` is the copyright notice** — a name every font has.
/// So a plain [`localized_name`] on an absent tooltip does not come back empty,
/// it comes back with the foundry's copyright in it, and a `cvXX` row grows a
/// hover reading "© 2020 The Font Project Authors". The spec puts feature
/// `name` ids in 256..=32767, so anything outside that is the font declining to
/// name it.
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
    /// Rotation about this glyph's **own origin** — `(x, y)` — in radians,
    /// clockwise, because text-local space is y-down.
    ///
    /// **`0.0` for every glyph of ordinary horizontal type**, which is what keeps
    /// the two backends' fast path intact: a run whose glyphs all sit at zero is
    /// drawn in one `draw_glyphs` call exactly as it was before this field
    /// existed, and only type on a path pays for the split ([`bend`]).
    ///
    /// ⚠️ **The doc above still holds and is no longer the whole truth**: `y` is
    /// on the *baseline* the glyph sits on, and on a path that baseline is the
    /// curve's tangent line at this glyph rather than a line shared with its
    /// neighbours. A consumer that collects distinct `y`s to recover the line's
    /// baseline was already wrong for a shifted run ([`TextLayout::baselines`]
    /// says why) and is now wrong for every bent glyph.
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
    /// This run's own ink, or `None` for **the node's fill stack**
    /// ([`crate::TextStyle::color`]).
    ///
    /// The whole of what per-run colour costs the render boundary: `scene::TextRun`
    /// was already per-run and both backends already honoured its brush, so what was
    /// node-level was the walk's *loop*, not the interface. `Option<Color>` is
    /// `Copy` and allocates nothing, which is what makes it affordable on a field
    /// rebuilt by every relayout.
    pub color: Option<Color>,
}

/// One band of underline or strikethrough ink, in text-local space.
///
/// **Geometry here, drawing at the render boundary.** parley reports where a
/// decoration goes and how thick it is; the four line styles are ours, and the
/// dashed and dotted ones reuse the stroke dash machinery the same way a
/// stroke's pattern does (§6.3) — so the pattern is resolved where the width and
/// the path are both to hand rather than baked into stale numbers here.
///
/// **[`Self::gaps`] is the exception that proves the rule**: it is an *interval*
/// and not a path, so it survives the pattern being chosen afterwards. See
/// [`skip_ink`] for why that is the whole reason skip-ink can be computed once
/// per relayout instead of once per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct DecorationInk {
    /// The band: `x0..x1` along the run, `y0..y1` its thickness.
    pub band: Rect,
    pub style: LineStyle,
    /// `None` means the text's own paint, so recolouring the text takes the
    /// decoration with it.
    pub color: Option<Color>,
    /// Where the band is **interrupted** by the glyph ink it would otherwise
    /// cross: `x0..x1` pairs in the same space as [`Self::band`] and clamped to
    /// it, sorted and non-overlapping. Skip-ink (§5.4, [`skip_ink`]).
    ///
    /// **Empty is the ordinary case and costs nothing** — it is what every
    /// strikethrough carries (css-text-decor-4 skips underlines and overlines,
    /// never a `line-through`), and what an underline with nothing descending
    /// through it carries. The renderer reads the band as continuous when this is
    /// empty, which is what it did for every band before this field existed.
    ///
    /// A `Vec` here is what costs `DecorationInk` its `Copy`; nothing outside this
    /// module ever held one by value, so the whole price was two derives.
    pub gaps: Vec<(f64, f64)>,
}

/// A shaped text node: its box, the glyph runs to draw, and the decoration ink.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    /// The node's box.
    pub size: Size,
    /// Offset from the node's **local origin** to the top-left of [`Self::size`].
    ///
    /// Zero unless the box is trimmed (`BoxTrim`). **This is what lets trimming
    /// tighten the box without moving the ink** — the glyphs keep their
    /// coordinates and the reported box shrinks around them, so toggling trim
    /// does not shift a text node in any saved file, and `TextSizing::Fixed`
    /// goes on meaning exactly the box the user typed. See §15.
    pub origin: Vec2,
    /// The box **before trim** — the full line box, leading included, measured
    /// from the local origin.
    ///
    /// Equal to [`Self::size`] whenever trim did nothing to the box, so a consumer
    /// can test the two for equality instead of asking what the trim setting is.
    /// See [`Self::untrimmed_bounds`] for what it is drawn for.
    pub untrimmed: Size,
    pub runs: Vec<GlyphRun>,
    pub decorations: Vec<DecorationInk>,
    /// Whether the content outran its line limit or its box, so the panel can
    /// say so and the ellipsis has somewhere to be.
    pub truncated: bool,
    /// The **first line's** box height, as parley resolved it.
    ///
    /// **What `LineHeight::Auto` is actually worth**, which is not knowable from
    /// the style: it is the face's own metrics, so the panel could not report it
    /// or seed from it and had to offer a round 120% instead (§15 D162). Read from
    /// `LineMetrics::line_height`, *not* from the block coordinates — those
    /// measure the line's ink and come out smaller than the box (§15 D147).
    ///
    /// The first line because that is the one number a single field can honestly
    /// show: a node whose runs differ in size has a different line height per
    /// line, and the field already reads "–" for a mixed selection.
    pub line_height: f64,
    /// What the **face's own** underline and strikethrough thicknesses are worth on
    /// the first run, in px — `(underline, strikethrough)`, `None` for a node with
    /// nothing shaped.
    ///
    /// [`Self::line_height`]'s argument, one field along, and it exists for exactly
    /// the same reason (§15 D570). `Decoration::thickness` is an `Option<Length>`
    /// whose absent state means *"the font decides"*, and what the font decided is
    /// not knowable from the style — so the panel's thickness field could neither
    /// report it nor seed from it, and the unit chip seeded **zero** instead. A
    /// zero-height band is not a thin line: `decoration_path` and [`crossings`]
    /// each open by refusing one, so one click on the chip erased the underline.
    ///
    /// **Pre-clamp and pre-override**, straight off `run.metrics()`, which is the
    /// half [`DecorationInk::band`] cannot answer: a band's height is what the
    /// *style* resolved to, so on the node this is about it is already the zero.
    ///
    /// **Two numbers, because a face gives two** — `underline_size` and
    /// `strikethrough_size` are separate metrics and are routinely different. The
    /// **first run**, for [`Self::line_height`]'s reason: a node whose runs differ
    /// in size has a different answer per run, and the field already reads *"–"* for
    /// a mixed selection.
    pub decoration_sizes: Option<(f64, f64)>,
    /// Every line's baseline in text-local space, top to bottom (§15 D355).
    ///
    /// **Stored, because it cannot be recovered from [`Self::runs`].** The
    /// obvious read is [`Glyph::y`], whose own doc says it sits on the baseline —
    /// and it does, for a run with no baseline shift. A shifted run's glyphs are
    /// deliberately *off* the line's baseline (`baseline_shift` is a draw offset,
    /// not a metric, so the line box does not grow and neighbouring baselines stay
    /// aligned), so collecting distinct glyph `y`s invents a baseline per
    /// superscript. The true value is computed in `extract`'s line walk and used
    /// to be thrown away.
    ///
    /// **One line, one entry, whatever the runs do.** A line split across three
    /// fonts has one baseline, and a line whose only run is shifted still has the
    /// baseline it was shifted from.
    ///
    /// ⚠️ **Empty for type on a rail** ([`Self::warp`]), deliberately: a bent
    /// baseline is not a *y*, and every consumer of this wants a horizontal line
    /// to snap to or to draw. Reporting the flat value the glyphs were bent from
    /// would put a snap target where there is no ink.
    ///
    /// ⚠️ **Not [`crate::text::export_lines`]**, which also reports a per-line
    /// baseline and is the wrong door: it takes a [`TextRef`] rather than a
    /// layout, re-shapes through parley and allocates a `Vec<ExportLine>` with
    /// every run and its `String` on each call. Its own doc says it is for the
    /// exporters and is deliberately not on the render path. This is on a walk
    /// that already runs.
    pub baselines: Vec<f64>,
    /// The rail this layout was bent onto, or `None` for ordinary horizontal
    /// type (§15 D405).
    ///
    /// **The glyphs are already bent** — [`Glyph::rot`] carries each one's
    /// rotation and its `x`/`y` are where it ends up — so nothing has to consult
    /// this to *draw the type*. It is here for the geometry that is bent rather
    /// than rigidly placed, which is [`DecorationInk`] and only that: a band is a
    /// continuous ribbon along the line, so it has to follow the curve instead of
    /// being carried along it, and it is built at the render boundary from
    /// [`DecorationInk::band`] rather than here (§6.3). One warp, exposed once,
    /// beats the alternative of every consumer re-deriving the map from the
    /// node's `on_path` and the layout's first baseline.
    pub warp: Option<PathWarp>,
}

impl TextLayout {
    /// The node's box in its own local space — the rect every bounds, hit-test
    /// and export consumer reads.
    pub fn bounds(&self) -> Rect {
        Rect::from_origin_size(self.origin.to_point(), self.size)
    }

    /// The node's box **before trim** — the full line box, at the local origin.
    ///
    /// **Nothing measures with this.** It exists to be *drawn*: the canvas outlines
    /// it faintly beside the trimmed box while a text node is selected, so the
    /// leading the trim took off is visible rather than merely absent. Everything
    /// that measures — the handles, align, distribute, snapping, the size badge, the
    /// W/H fields, the export viewBox, the Alt-hover measure — goes on reading
    /// [`Self::bounds`], which is the whole point of trimming by default.
    ///
    /// Equal to [`Self::bounds`] when trim changed nothing, which is how the canvas
    /// knows there is no second box to draw.
    ///
    /// ⚠️ **On a rail it is always equal, and that is decided here rather than at
    /// the one call site.** A bent node's box is the *bent* line box, so there is
    /// no second, looser rectangle to show — and the equality the canvas tests
    /// could not arise on its own, because this rect is anchored at the local
    /// origin while [`Self::bounds`] is anchored at [`Self::origin`], which a bend
    /// moves off zero. Keeping the contract true beats teaching the caller about
    /// [`Self::warp`].
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
/// Both are iterative, and a hundredth of a unit is far below a pixel at any zoom
/// the canvas offers. It is paid per glyph on a **relayout**, not per frame — the
/// warp is built once and cached with the layout it belongs to (§5.9).
const RAIL_ACCURACY: f64 = 0.01;

/// How finely a warped edge is resampled before it is mapped, in flat units.
///
/// ⚠️ **Flattening the edge is not enough, and that is the whole reason this
/// constant exists.** A decoration band is a *rectangle*: its long edges are
/// straight, so a flattener — which subdivides by curvature — hands each of them
/// back as a single `LineTo` however long it is. Mapping two endpoints gives a
/// straight ribbon across a curved rail, i.e. an underline that walks away from
/// its own text. So [`PathWarp::warp`] resamples by *length*, whatever the
/// curvature says.
const RAIL_STEP: f64 = 2.0;

/// ⚠️ **SKETCH ON TRIAL — `[S5.1-L4-02]`, committed behind [`SKETCH`] and not yet
/// accepted.** How far inside the true swept ribbon [`bend`]'s box may fall, in
/// document units.
///
/// **Separating this from [`RAIL_STEP`] is the whole fix.** That constant is
/// justified for [`PathWarp::warp`], where every sample becomes a *drawn vertex*
/// of a decoration ribbon and the resolution is the point. The box sweep reused it
/// for a computation whose entire output is four numbers — so its sampling density
/// was denominated in **rail units**, and its cost grew with the length of the rail
/// instead of with what the answer needed.
///
/// 🚨 **`0.005` and not the `0.05` this started at.** The looser value broke
/// `tools::tests::dragging_railed_text_scales_its_rail`: the box feeds
/// `resize_to_handle`'s solver, which turns box noise into **artwork movement**,
/// and the rail crept 0.0087 on the axis a drag was holding against that test's
/// 1e-3 tolerance. A twentieth of a unit is invisible on screen and is not
/// invisible to a solver.
const BOX_SAG: f64 = 0.005;

/// SKETCH TOGGLE — `false` restores the previous uniform sweep, so the two can be
/// A/B'd against the same repro (§15 D791, which carries the trial's terms).
///
/// ⚠️ **Left in place while the sketch is under review**, because it is what
/// answered two of the maintainer's questions: the box-jumping report (identical
/// boxes either way) and the held-corner slide of §15 D789 (also identical, which
/// is what said that one was pre-existing). It is also what found the flip defect
/// [`bend`] records — the A/B was the whole measurement, and a defect whose only
/// symptom is *a box five units too small* is not one anybody spots by reading.
///
/// 🚨 **One question is open and this constant is what holds the door**, so do not
/// delete it without answering: the **cost** of a many-segment rail, measured at
/// 9.4× the uniform sweep and higher than the per-segment floor explains. A cost
/// that is not understood is not yet bounded. That is not a reason to revert and
/// it is a reason not to promote.
///
/// ⚠️ The other one — the **frame** `reach` was measured in — is **answered**: it
/// took its distance from the text-local origin rather than from the baseline, and
/// [`bend`] now carries the correction and its before/after.
const SKETCH: bool = true;

/// The curvature of a path segment at `t`. `PathSeg` does not expose it — only its
/// three concrete variants implement `ParamCurveCurvature`, and a line's is zero.
fn curvature_at(seg: &PathSeg, t: f64) -> f64 {
    use kurbo::ParamCurveCurvature;
    match seg {
        PathSeg::Line(_) => 0.0,
        PathSeg::Quad(q) => q.curvature(t),
        PathSeg::Cubic(c) => c.curvature(t),
    }
}

/// The map from flat text-local space onto a rail: `x` becomes distance **along**
/// the curve, `y` distance **across** it (§15 D405).
///
/// **Two kinds of geometry ride this differently, and the split is the design.**
/// A glyph is carried *rigidly* — placed and rotated, never distorted, because a
/// bent `e` is a font bug rather than a feature — which is why [`Glyph::rot`]
/// exists and why nothing needs this to draw the type. Continuous geometry that
/// runs *along* the line is warped instead, point by point, which today is
/// exactly [`DecorationInk`] and nothing else.
#[derive(Clone, Debug, PartialEq)]
pub struct PathWarp {
    /// Each segment with the arc length at its start and its own length.
    ///
    /// **Cumulative, so a lookup is a binary search rather than a walk** — it is
    /// asked once per glyph and once per resampled point of every decoration
    /// band, which is the difference between linear and quadratic in the length
    /// of the rail.
    segs: Vec<(PathSeg, f64, f64)>,
    len: f64,
    /// The arc length flat `x = 0` maps to — alignment along the rail.
    start: f64,
    /// The text-local `y` that maps onto the rail itself: the first line's
    /// baseline.
    ///
    /// **Held here so that [`Self::place`] speaks the same space as its callers**
    /// (§15 D407). Every other consumer of this type — a decoration band, a caret
    /// rectangle, a selection quad — has ordinary text-local coordinates and no
    /// reason to know what a baseline is; subtracting it inside the map is what
    /// makes "hand it the coordinates you already have" the correct call.
    baseline: f64,
    /// Whether the rail closes on itself — a circle, a rounded rect, a triangle
    /// the pen closed (§15 D409).
    ///
    /// **What it changes is whether the rail has an *end*.** An open curve runs
    /// out and text past it is dropped; a closed one does not, so arc length wraps
    /// and type started anywhere on it carries on round. That is not a nicety:
    /// once the click decides where the text begins, starting it just *before* the
    /// path's own first point leaves almost no room on a shape that plainly has a
    /// whole circumference of it. Without wrapping, the offset feature is broken on
    /// exactly the shapes it is most for.
    closed: bool,
    /// Traverse the rail backwards ([`TextRef::on_path_flip`], §15 D406).
    ///
    /// **The flip is this one bool and nothing else**, which is the whole reason it
    /// was worth doing here rather than in a second code path: reversing the
    /// direction negates the tangent, and the normal is the tangent turned a
    /// quarter, so it negates too. Text reverses *and* changes sides from one sign,
    /// and every consumer — the glyph rotations, the second line, the decoration
    /// ribbons, the box sweep — follows without knowing the flag exists.
    flip: bool,
}

impl PathWarp {
    /// The rail, measured. `None` for a path with no length at all, which is not
    /// a rail and would divide by zero if it were treated as one.
    /// **The whole warp is built here, alignment included**, so that the live edit
    /// session and the layout cannot come to disagree about where along the rail
    /// the text starts (§15 D407). `bend` computed `start` itself for a day, which
    /// was fine while it was the only caller and became a second copy of the rule
    /// the moment `TextEdit` needed one.
    ///
    /// `width` is the shaped **content** width — see [`bend`] for why it must not
    /// be read off the box.
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
            // A zero-length segment has no direction to offer and would make the
            // binary search below ambiguous at its own arc length.
            if l > 0.0 {
                segs.push((seg, len, l));
                len += l;
            }
        }
        // Alignment is applied here, along the rail, because `shape` deliberately
        // did not apply it in the box (see the `on_rail` gate there). `Justify`
        // falls in with `Start`: justification needs a measure to stretch to, and
        // the rail's length is not one the line was broken against.
        //
        // **The offset composes with it rather than replacing it** (§15 D409), so
        // a centred node nudged along the rail stays centred on where it was put.
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
            // **The last element, not "a `ClosePath` anywhere in it".** A rail of
            // several subpaths where only the first closes is not a loop, and
            // treating it as one would wrap the text from the end of the last
            // subpath onto the first. Everything reaching here through
            // `geometry::local_path` is a single subpath.
            closed: matches!(path.elements().last(), Some(PathEl::ClosePath)),
            flip,
        })
    }

    /// The rail measured and nothing on it — no text, no alignment, no baseline.
    ///
    /// **The door for asking a purely geometric question of a curve** (§15 D409),
    /// which is what the Text tool's click needs: *how far along is this point*,
    /// asked before any text exists to be laid on it. Building the real warp would
    /// mean inventing a baseline and a width for a node that has neither yet.
    pub fn bare(path: &BezPath) -> Option<Self> {
        Self::of(path, false, 0.0, 0.0, TextAlign::Start, 0.0)
    }

    /// The point on the rail where the type starts (§15 D410).
    ///
    /// **What a canvas puts the slide handle on**, and the one place the rail and
    /// the text agree on a single point: the first glyph's own origin is the
    /// *glyph's*, half an advance further along and lifted off the curve by any
    /// baseline shift the run carries. This is the curve itself at flat `x = 0`.
    ///
    /// `None` when the text begins off the end of an open rail, which is the same
    /// answer its first glyph gets — a handle for a start that is not on the curve
    /// would be a handle in mid-air.
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

    /// Whether the rail closes on itself — see [`Self::closed`].
    ///
    /// Asked by anything that stores an *offset* along the rail: on a loop it can
    /// be normalised into a single lap, and on an open curve it cannot, since past
    /// the end is a real place there and the type runs out.
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
    /// **`(x, y)` is ordinary text-local space — the same coordinates a glyph, a
    /// decoration band and a caret rectangle are already in** — and that is the
    /// whole of §15 D407. It took *baseline-relative* y for a day, because the
    /// glyph loop in [`bend`] happened to have the baseline to hand and subtracted
    /// it there; the two other callers did not, and handed it the coordinates they
    /// had. An underline came out **19 units** below its own rail on 20pt text,
    /// which is exactly one baseline.
    ///
    /// **`None` when `x` runs off either end**, which is what makes text longer
    /// than its path simply *absent* rather than piled up on the last point. The
    /// caller turns that into [`TextLayout::truncated`].
    pub fn place(&self, x: f64, y: f64) -> Option<(Point, f64)> {
        if !self.fits(x) {
            return None;
        }
        let (seg, t) = self.at(self.start + x);
        let d = self.direction(&seg, t);
        // **The normal is the tangent turned so that flat `+y` stays *below* the
        // line.** Text-local space is y-down, so the quarter turn that takes
        // `(1, 0)` to `(0, 1)` is `(-dy, dx)` — get the sign backwards and every
        // descender, every underline and every second line appears on the wrong
        // side of the curve, which reads as the path having been drawn the other
        // way round.
        let n = Vec2::new(-d.y, d.x);
        Some((seg.eval(t) + n * (y - self.baseline), d.atan2()))
    }

    /// Whether flat `x` is somewhere the text may go (§15 D409).
    ///
    /// **Two different questions, and which one is asked is [`Self::closed`].** An
    /// open rail has ends, so what must be inside them is the *arc length* the text
    /// has reached — `start + x` — and running off either drops the glyph. A closed
    /// rail has no ends and `start + x` wraps, so the only way to outrun it is for
    /// the text to be longer than the whole loop, which is a question about `x`
    /// alone.
    ///
    /// ⚠️ **The closed bound is `|x|`, not `0..=len`, because the box sweep asks
    /// backwards.** It walks the rail from arc length 0 and hands `s − start`,
    /// which is negative for every point before the text begins — and the box has
    /// to cover the whole rail, not the stretch after the offset.
    fn fits(&self, x: f64) -> bool {
        match self.closed {
            true => x.abs() <= self.len,
            false => (0.0..=self.len).contains(&(self.start + x)),
        }
    }

    /// The segment holding arc length `along`, and the parameter within it.
    ///
    /// The flip is applied here rather than by the callers, so that the two
    /// directions of the map cannot come to disagree about which end is which.
    fn at(&self, along: f64) -> (PathSeg, f64) {
        // A closed rail has no end to fall off: the length wraps, which is what
        // lets type started anywhere on a circle carry on round it.
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
    /// to `p` (§15 D407).
    ///
    /// **Every pointer question on a railed node comes through here**, which is why
    /// it exists at all: a click, a double-click and a drag are all asked in the
    /// node's local space, and on a rail that space is *bent*. Without the inverse
    /// the caret is placed by parley against the flat layout, so clicking the ink
    /// puts the caret somewhere else entirely — the "editing the ghost of the old
    /// position" this was reported as.
    ///
    /// **Nearest point on the rail, not a projection onto one segment.** kurbo
    /// solves that per `PathSeg`; the arc length is then the segment's own start
    /// plus the length of the piece before `t`. `subsegment(0..t).arclen()` rather
    /// than `t × len`, because a Bézier's parameter is not its arc length and using
    /// it as one puts the caret a whole letter out on a curved segment.
    ///
    /// **Unclamped in `x` on purpose.** A point past the end of the rail answers
    /// with an `x` past the end of the text, which `Cursor::from_point` then clamps
    /// to the last cluster — the same thing clicking past the end of a flat line
    /// does. Clamping here instead would make a click *beyond* the last letter land
    /// on it rather than after it.
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
        // Signed across the rail, in the same direction `place` offsets by — so the
        // two are inverses including on the side a descender falls.
        let d = self.direction(&seg, t);
        let across = (p - seg.eval(t)).dot(Vec2::new(-d.y, d.x));
        (along - self.start, across + self.baseline)
    }

    /// `x` moved by whole laps of a closed rail into the half-turn nearest the
    /// text's own start (§15 D409).
    ///
    /// ⚠️ **[`Self::nearest`] deliberately does *not* do this, and the two uses are
    /// why.** A caret wants the answer near the text it is being placed in: on a
    /// loop, `−0.9 × len` and `+0.1 × len` are the same point, and handing the
    /// first to `Cursor::from_point` clamps the caret to the *start* of the string
    /// instead of a tenth of the way along. But an *offset* being measured for a
    /// fresh node wants the plain distance round the rail — normalising there gave
    /// a click at the half-way point an offset of **−0.5**, the same place written
    /// as a negative, which is what the SVG writer would then emit as `−50%`.
    pub fn shortest_way_round(&self, x: f64) -> f64 {
        match self.closed {
            true => x - (x / self.len).round() * self.len,
            false => x,
        }
    }

    /// A text-local rectangle as **convex quads** following the rail (§15 D407).
    ///
    /// ⚠️ **Several quads and not one, because a band on a curve is concave** — and
    /// `egui::Shape::convex_polygon`, which is what draws a selection highlight and
    /// a caret, renders a concave polygon wrongly rather than refusing it. Cutting
    /// the rectangle into `RAIL_STEP` slices makes every piece convex by
    /// construction; a caret, being about a pixel wide, comes out as exactly one
    /// and pays nothing.
    ///
    /// Empty where the rectangle is entirely off the end of the rail, which is the
    /// same answer the glyphs there get.
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

    /// `path` resampled and mapped onto the rail — the door for geometry that
    /// runs *along* the line rather than being carried by it.
    ///
    /// Subpaths break wherever the geometry leaves the rail, so a band that
    /// outruns its path stops at the end instead of collapsing onto it.
    pub fn warp(&self, path: &BezPath) -> BezPath {
        let mut out = BezPath::new();
        let mut cur = Point::ZERO;
        let mut sub_start = Point::ZERO;
        // Whether `out` has a subpath open — a mapped point may be `None`, so this
        // cannot be inferred from having seen a `MoveTo`.
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
                    // By length rather than by curvature — see [`RAIL_STEP`].
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
/// **The fallback is not defensiveness.** kurbo's derivative genuinely vanishes
/// at a cusp and wherever a cubic repeats a control point — which is what an
/// authored path does at a corner the pen tool made — and a zero vector
/// normalizes to `NaN`, which would propagate into every glyph position after it.
/// The chord either side still says which way the curve is going.
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
/// **One door, because two things need the identical warp and they are reached
/// from opposite ends** (§15 D407): [`extract`], which bends a finished layout,
/// and [`TextEdit`], which has to answer a *pointer* against the same bend and
/// never builds a `TextLayout` at all on the hot path. A second construction would
/// be a second copy of the alignment rule — and a caret one alignment-offset away
/// from its own ink is precisely the bug this exists to have fixed once.
///
/// The baseline is the first line's, in text-local space, which is the same value
/// `extract` writes into `TextLayout::baselines[0]` and by the same route.
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

/// Bend a finished flat layout onto its rail (§15 D405).
///
/// **A pass over the output rather than a mode inside the shaper**, which is what
/// keeps the feature affordable: parley shapes and breaks exactly as it always
/// did, `decoration_ink` and [`skip_ink`] compute in flat space where their bands
/// and gaps are plain intervals, and only then is the whole plane mapped. Doing it
/// the other way would mean a curved measure inside the line breaker and skip-ink
/// crossings against rotated glyph ink — both of them the same answer, computed in
/// a harder space.
///
/// **The map is of the plane, not of the line**, which is why a second line needs
/// no special case: flat `y` is distance across the rail, so a line one
/// `line_height` below the first comes out running parallel one `line_height`
/// outside the curve, and a marker, a superscript and a descender all land where
/// the same arithmetic puts them.
/// ⚠️ **`width` is the shaped *content* width and must not be read off
/// `layout.size`.** An auto-height or fixed node's box is the width the user
/// authored — that is what the W field shows and what [`box_of`] deliberately
/// reports — so aligning against it slides a centred node along the rail by half
/// the difference between its box and its text. Measured, not reasoned: a
/// 300-wide box on a 600-long rail centred at 150 rather than 272.6. Nor can it be
/// summed from the glyph advances below, which are an *estimate* for the last
/// glyph of each run: that came out 47.8 against a true 54.8 for `Ondin`, because
/// the final `n` borrows the narrow `i`'s advance. Good enough to rotate a glyph
/// by, three points out for placing a whole line.
fn bend(layout: &mut TextLayout, warp: PathWarp) {
    // **Each glyph is placed by its own midpoint**, which is SVG's rule and is
    // worth the arithmetic: rotating about the left edge instead fans wide glyphs
    // outward on a tight curve, and the error is largest exactly where the feature
    // is most visible.
    //
    // ⚠️ **An advance is read from the next glyph on the same line, across run
    // boundaries**, because `GlyphRun` carries positions and no advances — parley's
    // own run advance covers every line the run was broken over, which is the same
    // trap `export_lines` documents. Only where there is no such glyph does a
    // letter borrow its predecessor's advance: a good estimate, wrong by half the
    // difference between two adjacent advances.
    //
    // ⚠️ **This used to scan each run separately, and a run of one glyph then
    // borrowed from a `scan` seed of `0.0`** (§15 D489, `[S5.1-L1-03]`). With
    // `adv = 0` both terms of the placement below vanish and the glyph takes the
    // tangent at its own **left edge** — exactly the placement the paragraph above
    // rejects, *"fans wide glyphs outward on a tight curve, and the error is
    // largest exactly where the feature is most visible"*. Measured on "Wave along
    // the rail" at 40pt on a 120-radius circle, against a metric-neutral
    // one-character `CharAttr::Color` span so the glyph itself is identical either
    // way: **0.149 rad — 8.56° — and 1.35 units of position**, on a character the
    // span was supposed to recolour and nothing else.
    //
    // Reached by an ordinary act. One styled character is a run of one, and so is
    // any run parley splits for font fallback that happens to be one glyph long —
    // an emoji, a CJK character in Latin text.
    //
    // **Runs are laid out in one flat coordinate space**, so the glyph after a
    // run's last is the next run's first, and its advance is a real measurement
    // rather than an estimate. The same-line test is what makes that safe: across a
    // line break the next glyph's `x` returns to the line's start, and under a right
    // or centre alignment it can still be *greater* than the previous line's last
    // `x` — so comparing `x` alone would silently accept an absurd advance at every
    // line end.
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
    // Text that outran the rail is truncated in the sense the panel already
    // reports, and by the same field.
    layout.truncated |= after < before;

    // **The box is the *rail's* bent line box, not the text's** — the whole rail,
    // swept from end to end, with the flat box's own two verticals carried along
    // it.
    //
    // ⚠️ **The obvious alternative is the stretch the glyphs occupy, and it is
    // wrong twice over.** It makes the layer's box — and so its handles, its snap
    // targets and its W/H fields — *move as the string is typed*, which is exactly
    // the instability `docs/roadmap.md`'s "don't use ink bounds for alignment"
    // rule is about. And it leaves a resize handle with almost nothing to say: the
    // rail a designer drew is usually longer than the words on it, so scaling the
    // rail would barely move a box measured over the words. Measured, on a
    // 150-long rail under a 78-wide word: doubling the rail took the box from 78.0
    // to 73.4 — the wrong direction, and small enough to read as a dead control.
    //
    // The rail is the node's authored geometry, the way a `Path`'s outline is.
    // This is that outline's box.
    let top = layout.origin.y;
    let bottom = layout.origin.y + layout.size.height;
    // Swept in *arc length*, so `warp.start` — which is alignment, not extent —
    // cannot get into the box. `place` takes flat `x`, hence the subtraction.
    // ⚠️ **SKETCH ON TRIAL — `[S5.1-L4-02]`, behind [`SKETCH`].** Per-segment,
    // curvature-aware sampling in place of a uniform step denominated in
    // `RAIL_STEP`. Two changes, and the first is a correctness one rather than a
    // saving:
    //
    // 1. **Every segment boundary is sampled**, which is where a rail's cusps are —
    //    a pen corner, a star point, a tight rounded-rect corner. A uniform sweep
    //    only hits a cusp by luck, and the error it pays there is *first order* in
    //    the step.
    // 2. **Inside a segment the step comes from that segment's own curvature**, not
    //    from the rail's length. The sagitta between two samples an arc `h` apart on
    //    curvature `κ` is about `κh²/8`, so holding it under `BOX_SAG` gives
    //    `h = sqrt(8·BOX_SAG/κ)` — and a straight segment needs its two ends and
    //    nothing else.
    //
    // ~~Measured, release, 100 reps: r=300 **0.690 → 0.131 ms**, r=3000
    // **3.239 → 0.437**, r=30000 **3.399 → 1.443**, a spike **0.022 → 0.0002**.~~
    // 🚨 **Struck: taken at `BOX_SAG = 0.05` and never re-taken after the tightening
    // to 0.005** that constant's own doc records, and the r=30000 figure contradicts
    // the block below it — *"3.399 → 1.443"* against *"3.44 → 4.54"*, the same case
    // in opposite directions, with nothing saying which was current. **Two timings
    // of one case is one timing too many**; the deterministic half is counted below
    // instead, because a sample count cannot go stale against a machine.
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
    // ⚠️ **SKETCH ON TRIAL — `[S5.1-L4-02]`, behind [`SKETCH`].** Per-segment,
    // curvature-aware sampling in place of a uniform step denominated in
    // `RAIL_STEP`.
    //
    // 1. **Every segment boundary is sampled, at both ends**, which is where a
    //    rail's cusps are. A uniform sweep only hits a cusp by luck and the error
    //    it pays there is *first order* in the step.
    // 2. **Inside a segment the step comes from that segment's own curvature.**
    //    The sagitta between two samples an arc `h` apart on curvature `κ` is
    //    about `κh²/8`, so holding it under `BOX_SAG` gives `h = sqrt(8·BOX_SAG/κ)`
    //    — and a straight segment needs its two ends and nothing else.
    //
    // 🚨 **Both ends, and the nudge, are the bug the maintainer caught.** `place`
    // resolves an arc length to **one** segment (`PathWarp::at`), so at a corner it
    // answers with one of the two normals — and a segment sampled only at its
    // *start* contributes one side's outward offset and never the other's. On a
    // sharp-cornered rectangle that lost a whole edge: on their document the box
    // came back **17.46 units narrow on the right**, sitting exactly on the rail,
    // with the type hanging outside it. Backing off by a whisker asks for *this*
    // segment's normal at its own end.
    //
    // 🚨 **The `clamp(…, 4096.0)` this replaces is an accuracy *surrender*, not a
    // safety bound** — past about r = 20,000 the uniform sweep stops honouring
    // `RAIL_STEP` at all — so a curvature rule that keeps its promise asks for
    // *more* there rather than less. `budget` shares the allowance across segments
    // in proportion to what each asked for.
    //
    // 🚨 **`budget` is not a cap and must not be read as one.** Every segment keeps
    // at least its two ends whatever `budget` says — `.ceil().max(1.0)` plus the two
    // end nudges cannot go below three samples — so a rail with more segments than
    // the allowance simply spends more. ~~"The same total budget is kept."~~
    // **Struck: measurably false.** Counted with the sampler instrumented, against
    // the uniform sweep on the same rails: circle r=300 **944 → 583**, r=3000
    // **4,097 → 1,747**, r=30000 **4,097 → 4,135**, and a 5,999-segment polyline
    // **1,922 → 17,998**. So the win is real at small and medium radius, it is
    // **parity** at r=30000, and a many-segment rail — a traced outline, an imported
    // path, a pen drawing — costs **9.4× more** with nothing to stop it (§15 D791).
    //
    // ⚠️ **17,998 is exactly `3n + 1` and this was briefly written up as
    // unexplained.** It is not: a straight segment has `κ = 0`, so `want` is 1;
    // `budget` is `4096/5999 ≈ 0.683`, and `(1 × 0.683).ceil().max(1.0)` is **1**
    // again; one loop sample plus two nudges is three, and `3 × 5999 + 1 = 17,998`
    // with the closing `sample(warp.len)`. 🚨 **So the sharp form of the open
    // question is that the allowance is *inert on precisely the rail that costs
    // most***: scaling a `want` already sitting on the floor buys nothing back, and
    // `budget` only ever bites where the cost was bounded anyway. **A bound that
    // relaxes as the input gets worse is not a bound.**
    // **How far the ribbon reaches from the rail, measured from the *baseline*.**
    // `place` offsets by `y - baseline`, so that — and not the distance from the
    // text-local origin — is the `v` the offset-curvature term below wants
    // (§15 D793).
    //
    // ⚠️ **This read `top.abs().max(bottom.abs())` until it was measured**, which
    // is the distance from the text-local *origin*. The two agree only when the
    // baseline sits at `y = 0`, and parley does not put it there: measured on this
    // module's own rail fixtures, `top = 0, bottom = 20, baseline = 17` at 20pt, so
    // the old form said **20.000** against a true **17.000** — and at 24pt under
    // `CapToBaseline`, `top = 2.539, bottom = baseline = 20`, so **20.000** against
    // **17.461**.
    //
    // 🚨 **It over-estimated on every fixture tried, so this buys accuracy rather
    // than speed — and almost no speed at all.** `reach` enters only through the
    // offset-curvature term, where `κ·reach` is small unless the rail is tight, so
    // the correction is a per-cent or two of the step and the `ceil` eats it.
    // Sample counts before → after, 20pt: **r=25 233 → 233** (the `.min(0.5)` clamp
    // binds either way), **r=40 293 → 273**, **r=60 311 → 301**, **r=120 391 →
    // 386**, and **r≥300 unchanged**, as are the arch, the spike and the rectangle.
    // Boxes move by at most **0.005**, which is `BOX_SAG` — i.e. inside the
    // tolerance this constant exists to declare.
    //
    // **The reason to do it anyway is the sign.** Nothing made "over-estimates" a
    // rule; it is what this frame happens to produce for these trims, and a reach
    // that came out *short* would thin the sampling on exactly the tight curves
    // where the term is the only thing keeping the box honest.
    //
    // 🚨 **Reverting this line leaves the whole suite green, and that is a fact
    // about the suite rather than about the change.** There is nothing for an
    // outcome assertion to grip: by construction the two frames differ only inside
    // `BOX_SAG`, which is the tolerance every box test here is written against, and
    // the rest of the difference is a sample count nothing asserts. **So this line
    // is guarded by its own derivation and by no test** — `place` offsets by
    // `y - baseline`, and that is the only argument there is. Change it only with
    // that identity in hand.
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
            // 🚨 **The ribbon curves harder than the rail does, and this is the
            // term a first draft leaves out.** `place` maps across by up to `v`,
            // and offsetting toward the centre of curvature takes a radius `ρ` to
            // `ρ − v`, so a step adequate for the rail is *not* adequate for the
            // curve actually being bounded. ⚠️ **`reach` is measured from the
            // *baseline*** — see its own paragraph above for why, and for the
            // before/after.
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
            // 🚨 **A segment's `along` interval is *mirrored* under the flip, so
            // `s0` is its **end** there and not its start** (§15 D792).
            // `PathWarp::at` maps
            // `along` to `len - along` before it searches, and this loop walks
            // `segs` — which is in rail order and knows nothing about that. Read
            // the interval the wrong way round and every claim above is void on a
            // flipped rail: the boundaries sampled are not boundaries at all, and
            // the nudge that exists to ask a segment for *its own* end normal
            // nudges into the **next** segment instead.
            //
            // Measured on a closed 200×100 rectangle, 20pt, `on_path_flip`: the
            // corners at arc 200 and 500 were never sampled, the right edge never
            // contributed its own normal, and the box came back **x1 = 200.0
            // against the uniform sweep's 205.0** — five units narrow, sitting
            // exactly on the rail with the type outside it. **The same defect this
            // sketch was written to remove, surviving on the other traversal.**
            // `a_flipped_rails_box_clears_the_rail_on_every_side` is the test.
            let a0 = match warp.flip {
                true => warp.len - s0 - slen,
                false => *s0,
            };
            for i in 0..(n as usize) {
                sample(a0 + slen * (i as f64) / n, &mut bbox);
            }
            // **Both ends, nudged inward**, which is what makes this independent of
            // the traversal: whichever end `a0` turns out to be, the segment is
            // asked for a normal a whisker inside each of its own two ends rather
            // than at a boundary `at` may resolve to the neighbour.
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
    // A rail too short to sample, which `PathWarp::of` has already refused for
    // zero length but which floating point can still leave empty. The rail's own
    // box is where the layer is, and a node with no box at all has no handles and
    // cannot be selected back out of.
    let b = bbox.unwrap_or_else(|| warp.bounds());
    layout.origin = b.origin().to_vec2();
    layout.size = b.size();
    layout.untrimmed = b.size();
    layout.baselines.clear();
    layout.warp = Some(warp);
}

/// The filled outline of a laid-out text node's glyphs, in the node's own
/// text-local space — the path a text **stroke** is drawn along and clipped
/// against.
///
/// **Outlines, not shaping.** The glyph ids and their positions come straight out
/// of the cached [`TextLayout`], so nothing here re-runs parley (§5.9); what it
/// asks the face for is the drawing of each glyph. It is built at the render
/// boundary rather than stored beside the layout, and gated on the node actually
/// carrying a visible stroke — which almost none do, so almost nothing pays. A
/// cache would pay for every text node on every relayout to serve an empty list.
///
/// **[`skip_ink`] is the second caller of the same machinery and it *is* on the
/// relayout**, for underlined nodes only (§15 D356). It does not go through this
/// function: it wants one glyph at a time, at the origin rather than at the pen, so
/// that the ink of a repeated letter is outlined once — hence [`RunFace`], which is
/// the per-run half of this lifted out for both to share. The paragraph above still
/// holds for the *stroke* path, and the "almost nothing pays" is now "almost nothing
/// pays for the outline of a whole node".
///
/// **Font outlines are y-up and text-local space is y-down**, so each point is
/// mirrored about the glyph's own baseline: `(g.x + x, g.y − y)`. Worth stating
/// rather than leaving as a sign to be rediscovered — get it backwards and the
/// type draws upside down beneath its own line, mirrored *per glyph*, so the
/// letters stay in reading order and it looks like a font bug.
///
/// **Every contour comes back closed**, forced below if the face did not say so.
/// That is the invariant `geometry::stroke_align_applies` leans on when it answers
/// yes for `Text` without measuring: inside and outside alignment are built out of
/// a doubled stroke and a clip, and a clip needs an interior.
///
/// §15 D145 has the measurements — about a microsecond a glyph, which is what makes
/// the gate load-bearing rather than tidy.
pub fn outline(layout: &TextLayout) -> BezPath {
    let mut out = BezPath::new();
    for run in &layout.runs {
        append_run_outline(run, &mut out);
    }
    out
}

/// One run's glyphs as an outline — [`outline`] for a single run.
///
/// **The render boundary's door for a bent run under a brush that has a
/// position**, which is the one case per-glyph drawing gets wrong (§15 D405). A
/// gradient or an image is authored in the node's local space, and a bent glyph is
/// drawn under a transform of its own, so drawing it as a *glyph* would restart
/// the gradient at every letter. Filled as one path in the node's own space it
/// cannot: the brush stays where it was authored. A solid brush has no position to
/// lose and keeps the cheaper path.
///
/// Everything [`outline`]'s doc says about cost and about the y-mirror applies
/// here unchanged; this is the same walk over one run.
pub fn run_outline(run: &GlyphRun) -> BezPath {
    let mut out = BezPath::new();
    append_run_outline(run, &mut out);
    out
}

/// One run's glyphs, each drawn where it sits. The interesting half is
/// [`RunFace`], which [`skip_ink`] asks for the same glyphs at the origin instead.
fn append_run_outline(run: &GlyphRun, out: &mut BezPath) {
    let Some(face) = RunFace::of(run) else {
        return;
    };
    for g in &run.glyphs {
        face.draw(g.id, f64::from(g.x), f64::from(g.y), f64::from(g.rot), out);
    }
}

/// Collects one glyph's outline into a shared [`BezPath`], baseline-mirrored and
/// translated to wherever its caller wants the glyph's origin — where it sits in
/// the run for [`append_run_outline`], and `(0, 0)` for [`skip_ink`], which reuses
/// one glyph's ink at every position it appears.
///
/// `down` is not defensiveness for its own sake: kurbo asserts that a path starts
/// with a `MoveTo`, so a face that emitted a contour without one would panic the
/// canvas in a debug build and corrupt the path in a release one. It is also what
/// [`RunFace::draw`] asks to decide whether there is a contour left to close.
struct Pen<'a> {
    out: &'a mut BezPath,
    dx: f64,
    dy: f64,
    /// `(cos, sin)` of the glyph's own rotation ([`Glyph::rot`]), pre-computed
    /// because [`Self::at`] is called once per *control point* and a trig pair per
    /// point would be paid by flat type as well as bent.
    ///
    /// `(1.0, 0.0)` is upright, and the branch on it below keeps horizontal type
    /// on exactly the arithmetic it had before rotation existed.
    rot: (f64, f64),
    down: bool,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Point {
        // Font outlines are y-up and text-local space is y-down, so the glyph is
        // mirrored about its own baseline *first* — rotating a y-up point and then
        // mirroring would turn it the wrong way.
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
/// layout y **to** the node's local y.
///
/// See the module docs: all three move text after the lines are broken, and all
/// three have to be applied at the draw boundary *and* the hit-test boundary.
/// Bundling them here means the two cannot disagree about the amount.
///
/// **One direction only, and that is the correction §15 D166 finished.** This was
/// a two-way map, and the way back was a subtraction — which cannot be right,
/// because a paragraph gap is space that exists in local y and not in layout y at
/// all, so the points inside one have no layout y to be given. The way back is
/// `Shaped::layout_y` (private, since it needs the parley `Layout` beside this):
/// resolve which line's box the point is in or nearest to, then ask this for that
/// line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct YMap {
    /// Constant shift — box trim and vertical alignment, which move everything.
    base: f64,
    /// `(layout y of a paragraph's first line, that line's index, extra space
    /// inserted before it)`, ascending. Cumulative: the second entry already
    /// includes the first.
    ///
    /// The y is what [`Self::to_local`] decides from and the index what
    /// [`Self::shift_for_line`] does — two keys on one fact, because a baseline
    /// arrives as a coordinate and a rectangle's edges arrive with a line.
    breaks: Vec<(f64, usize, f64)>,
}

impl YMap {
    /// The node's local y for a point in parley's layout space, deciding from the
    /// coordinate.
    ///
    /// **Right for a baseline and wrong for an ink coordinate** — use
    /// [`Self::to_local_on_line`] when the line is known, which is the only honest
    /// answer for the top or bottom of a box. A baseline always sits below its own
    /// line-box top, so it cannot fall on the wrong side of its break; the *ink*
    /// extents can and do.
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
    /// **A shift belongs to a line, not to a y**, and that is a bug rather than a
    /// nicety. `breaks` is keyed on line-*box* tops — the running sum of line
    /// heights — while `block_min_coord` is a line's **ink** top, which sits *above*
    /// the box top whenever the leading is negative: Inter at 20pt is 24.4 of type
    /// in a 20pt box, so this is the ordinary case. A caret rectangle and a
    /// selection rectangle are both ink coordinates, so both fell short of their own
    /// break and came out **one whole gap too high** on every paragraph after the
    /// first — 40pt at a 40pt spacing, which puts the caret in the gap above the
    /// paragraph it is in.
    ///
    /// Nor can a scalar boundary fix it: consecutive lines' ink ranges *overlap*, so
    /// there is no y that separates one line's descender from the next line's
    /// ascender (the same overlap that made a containment search pick the wrong line
    /// in [`TextEdit::caret_line_index`]). The index can, and both callers have it —
    /// `parley::Selection::geometry` hands back the line beside each rectangle.
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

/// Per-thread parley resources (a font DB holding the bundled faces plus
/// whatever the app has registered, and reusable layout scratch space). Kept in
/// a `thread_local` so shaping is a simple free function while still reusing
/// allocations.
struct Engine {
    font_cx: FontContext,
    layout_cx: LayoutContext<RunIndex>,
    /// The bundled family name (Inter) — default and last-resort fallback.
    default_family: String,
}

impl Engine {
    fn new() -> Self {
        // system_fonts: false — never touch the OS font set (belt-and-braces with
        // the disabled `system` feature). Register both bundled faces; they share
        // the "Inter" family and differ by style/weight. The app adds more
        // families later via `register_fonts`.
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
/// **`Start`/`End` are resolved here rather than left to parley**, because
/// parley resolves them against the base direction it inferred from the
/// *content* — so an empty RTL paragraph, or one holding only digits, would
/// align the wrong way while the direction control said otherwise. With an
/// explicit direction the mapping is a fact about the setting; on `Auto` the
/// logical variants are passed through and parley's inference is exactly what is
/// wanted.
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
        // Auto: the font's own line height — the sum of ascent, descent and
        // leading, which is what CSS `normal` means.
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
/// **The `opsz` synthesis is ours.** Neither parley nor fontique tracks the
/// optical-size axis from the font size, and a variable family that ships one
/// expects it to be tracked (CSS `font-optical-sizing: auto`); leaving it at the
/// axis default is what makes a 96pt heading look like blown-up body text.
///
/// ⚠️ **This is the only implementation of that rule, and it used not to be**
/// (§15 D571). `TextStyle::optical_size` computed the same value in
/// `typography.rs`, carried the only test of the Manual arm, and was called by
/// nothing — so the tested copy was not the one that ran, and the two disagreed
/// about exactly the case neither of them was driven on.
///
/// **A *stored* coordinate reaches the shaper raw, for every axis including
/// `opsz`** — that is the disagreement, settled. Three reasons, in order of weight:
/// the panel already refuses to *rewrite* a stored value outside its field's range
/// (§15 D425, `clamp_existing_to_range(false)`), so clamping it here would make the
/// readout and the file disagree with the ink for no gain; the shaper clamps it
/// anyway, so nothing renders differently; and clamping `opsz` while passing
/// `wght`, `wdth` and every other axis through would be the divergence in miniature,
/// one function along. **The synthesized value is the exception and is clamped**,
/// because it is not a coordinate anybody stored — it is the font size, which has
/// nothing to do with this axis's range and is routinely outside it.
///
/// `axis.clamp`, not `f64::clamp`: the range comes out of a file and `f64::clamp`
/// is an `assert!` that fires in release (§15 D435).
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
    out
}

/// Push one style's properties onto the builder — as defaults when `range` is
/// `None`, otherwise over that byte range.
///
/// One function for both so the default set and a span's set cannot drift: a
/// property pushed as a default but forgotten here is a property a styled run
/// silently inherits from its neighbour.
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
    // Requested family first, bundled Inter as the fallback if it's absent.
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
    // The brush is the run index (see the module docs), and the decoration
    // brushes are the same index — the decoration walk reads either.
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
    // Wrapping is a paragraph attribute but a *per-cluster* parley property, so
    // it rides along with the character set rather than being pushed once.
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
/// **Length-preserving only, and that is a deliberate limit.** Byte indices are
/// the currency of the span list, the caret and every hit test, so a transform
/// that changed them would need a full offset map threaded through all three —
/// its own project. A character whose cased form is a different number of UTF-8
/// bytes (`ß`→`SS`, `İ`→`i̇`) is therefore left alone, which covers the whole of
/// Latin, Greek and Cyrillic bar a handful of characters. Recorded in §15.
fn cased_text(content: &str, runs: &[(Range<usize>, TextStyle)]) -> Option<String> {
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
    /// The layout y to hit-test a text-local y with: the **middle of the line whose
    /// own local box is nearest to it**.
    ///
    /// **Not the coordinate with a shift subtracted from it**, which is what
    /// `YMap::to_layout` used to be and what §15 D166 left open. A paragraph gap
    /// exists only in *local* space — in parley's it is zero wide — so a point
    /// inside one has no layout y at all, and taking the shift of the paragraph
    /// *above* off it walks forward into the paragraph *below* by as much as the gap
    /// is tall. D166 recorded the symptom as 2.2pt of negative leading above the
    /// first line; the 2.2pt is only the tail of it. Measured on three 20pt lines
    /// 40pt apart, a click anywhere in the lower half of the gap — 20pt of it —
    /// landed on the paragraph's **second** line, and the upper half on its first
    /// while the pointer was 40pt clear of any ink.
    ///
    /// **Line boxes tile local space**, being the running sum of `line_height` —
    /// the very key `YMap::breaks` is built on — so which box a point is in, or is
    /// nearest to, is exact, and this direction becomes the tiling's own inverse.
    /// A point in a gap takes the nearer of the two lines bounding it, ties going to
    /// the line above.
    ///
    /// What stays approximate is the y handed back, and it is parley's rather than
    /// ours: `Layout::line_for_offset` binary-searches
    /// `block_min_coord..block_max_coord`, which **overlap** whenever the leading is
    /// negative, so under a tight enough line height no y identifies a middle line
    /// uniquely. The box centre is the furthest a y can sit from both neighbours'
    /// ink, which is the most this can promise.
    fn layout_y(&self, local_y: f64) -> f64 {
        let mut best: Option<(f64, f64)> = None;
        let mut top = 0.0;
        for (index, line) in self.layout.lines().enumerate() {
            let height = f64::from(line.metrics().line_height);
            let box_top = self.ymap.to_local_on_line(top, index);
            // Zero inside the box, otherwise how far outside it — so the comparison
            // below is "contains" and "nearest" in one number.
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
        // Nothing laid out: no line to resolve to, so the plain inverse of the
        // constant shift, which is all the map has left to say.
        best.map_or(local_y - self.ymap.base, |(_, y)| y)
    }
}

fn shape(parts: TextRef<'_>) -> Shaped {
    // The one seam every shaping call goes through — [`layout`] is this plus
    // `extract`, and `TextEdit::reshape` calls it directly. See [`shapes`].
    SHAPES.with(|n| n.set(n.get() + 1));
    let runs = parts.spans.runs(parts.style, parts.content.len());
    let cased = cased_text(parts.content, &runs);
    let text: &str = cased.as_deref().unwrap_or(parts.content);

    let mut layout = ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let default_family = engine.default_family.clone();
        // Axis metadata is per family, and a run may name a different one — so
        // gather it for every distinct family *before* the builder exists,
        // because the builder borrows the whole engine for its lifetime.
        let mut families: Vec<&str> = vec![parts.style.font_family.as_str()];
        for (_, style) in &runs {
            if !families.contains(&style.font_family.as_str()) {
                families.push(&style.font_family);
            }
        }
        // 🚨 **The fallback family is gathered too, because an unknown family
        // inherits its axes** (§15 D711, `[S5.1-L1-06]`). The module doc says
        // *"unknown families fall back to Inter"* and the **face** did:
        // `push_style` names `default_family` as the second entry of the family
        // list, so parley resolves the bundled Inter for a family the machine
        // does not have. The **axes** did not — `axes_for` answered `&[]`, so
        // `axis_settings` found no `OPSZ`, pushed no `opsz` variation, and the
        // face shaped at its axis default of 14 instead of the clamped 32 the
        // same text gets when the family is spelled `"Inter"`. Measured in
        // release, `"Hamburgefonstiv"` at 96pt: **670.500** against **726.609**,
        // 8.4% wider — and it snaps back the moment that family becomes
        // available, which is a silent reflow of a document nobody edited. It is
        // the exact symptom `axis_settings`' own doc names as its reason to
        // exist: *"leaving it at the axis default is what makes a 96pt heading
        // look like blown-up body text."*
        if !families.contains(&default_family.as_str()) {
            families.push(default_family.as_str());
        }
        // ⚠️ **`known` is not `!axes.is_empty()`, and that distinction is the
        // whole care in this fix.** `family_axes_locked` answers an empty list
        // both for a family fontique cannot find **and** for one it can find
        // whose default face declares no axes — a static face, or one whose blob
        // will not load or parse. Only the first may inherit; asking for Inter's
        // `opsz` on a **static** face the user really did ask for is requesting a
        // variation that font does not carry.
        //
        // ⚠️ **The guard is about the *request*, not demonstrably about the
        // ink.** A `FontVariation` naming a tag with no matching `fvar` axis is
        // very likely dropped by the shaper, which was not measured — so a reader
        // who tries the `!axes.is_empty()` spelling may well find it renders
        // identically. It is still wrong, and the reason is that the two states
        // are different questions rather than that the pixels differ.
        //
        // ⚠️ **`known` asks whether the family is *listed*, not whether its face
        // can be read.** A family that resolves but whose face will not load is
        // `known`, inherits nothing, and reproduces exactly the defect above.
        // Registered faces are memory blobs, so that is not reachable today; it
        // is where to look if it ever is. (`arch-scribe` found both of these
        // while writing D711 — the first draft of this comment said "two
        // different things" over five early returns, which is the
        // count-in-a-comment shape one paragraph after fixing one.)
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
                // Unknown: parley will shape this run with the fallback face, so
                // the axis settings have to be the fallback's or the two spellings
                // of the same face measure differently.
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

    // **Not `Layout::set_text_indent`.** parley's own indent and a per-line
    // `line_x` are two mechanisms on the same edge — `resolve_indent` decides
    // applicability per line and `break_next` then subtracts the amount from that
    // line's max advance itself — so using both double-counts the indent and
    // shrinks the measure twice. All of it is ours now, which is also what makes
    // the amount vary by paragraph (`Paragraphs`).
    let paras = Paragraphs::new(parts);

    // **On a rail the box is inert, and both halves of that are here** (§15 D405).
    // A path's own length is the measure, so wrapping at the box width would break
    // lines against a number that has nothing to do with where the text runs out;
    // and alignment is applied by [`bend`] as an offset *along the rail*, so
    // letting parley centre the line inside the box first would centre it twice,
    // once against each measure. `TextSizing` and `paragraph.align` both keep their
    // stored values — this reads them differently, it does not overwrite them — so
    // detaching the path restores exactly the layout the node had before.
    let on_rail = parts.on_path.is_some();
    let wrap_width = if on_rail {
        None
    } else {
        parts.sizing.wrap_width()
    };
    let height_limit = match (parts.sizing, parts.block.overflow) {
        // A fixed box only truncates when it has been told to clip or ellipsize;
        // `Visible` is the state that lets text hang out of its frame, which is
        // an ordinary thing to want while a layout is being worked out.
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
    // **The second break pass optical margin alignment needs** (§15 D830), run
    // only for a node that asked for it — everything else pays one `paras` scan.
    //
    // ⚠️ **The pass is not free and it is not re-shaping either.** Breaking is a
    // walk over clusters that are already shaped, so this is the cheaper half of
    // laying out twice, and it happens on the same `Layout` — `break_lines()`
    // hands out a fresh breaker that starts from the beginning.
    //
    // 🚨 **The second pass can break somewhere the first did not, so it is
    // repeated until every line is corrected by its own bearings** (§15 D855,
    // `[X3-L1-01]`). Every line's measure grows by its two bearings, so a word
    // can cross a line boundary — and the offsets are read per *line index*, so
    // from the first line that changed, each was corrected by another line's
    // glyphs. D830 accepted that as *"only the optical nicety is approximate"*,
    // and measured it was worse than approximate: at a 160-wide start-aligned
    // measure the corrected edge was **58% raggeder than no correction at all**,
    // and a justified block left its box at 12 of 181 widths. The maintainer
    // ruled it fixed (2026-09-23).
    //
    // **Settled a line at a time, from the top.** After each pass, find the first
    // line whose bearings are not the ones it was given. Every line above it was
    // given the same numbers as last pass and so broke the same way; the
    // mismatched line therefore *starts* where it did, which makes its left
    // bearing exact, and only its right bearing — which depends on where the line
    // ends — was a guess. So it gets the one it actually has, and the lines below
    // it get this pass's readings as their next guess. The first mismatch only
    // ever moves down, which is what bounds the loop.
    //
    // ⚠️ **A line can cycle** — guess `a` ends it on a glyph with bearing `b`,
    // and guess `b` ends it on one with `a` — and a first version that only
    // iterated to a whole-layout fixed point never settled at 160 for exactly
    // that reason. A line that comes back to a guess it has already tried is
    // **pinned**: its left bearing (exact) and a right bearing of zero, which
    // hangs nothing, so its ink ends *inside* the nominal edge rather than past
    // it — never worse than the setting being off, and the start edge still
    // corrected. The cap is a belt; past it the node is laid out uncorrected.
    //
    // **The auto-width box keeps the uncorrected width** (§15 D855,
    // `[X3-L2-01]`), which is D830's *"the node's box does not move"* made true of
    // the width and not only of the start edge: a start-shifted line pulled
    // parley's width in by its left bearing, a different amount per node, so two
    // labels right-aligned by their boxes moved apart when the setting was on.
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
            // Above `j` nothing changed; keep what those lines were given, so a
            // pinned line stays pinned. From `j` down, this pass's readings.
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
        // ⚠️ **Redundant today, and measured to be so rather than assumed.**
        // Removing this line alone changes nothing any test can see, because the
        // wrap gate above leaves parley with no measure to align *within* — an
        // alignment with no container width is a no-op. Removing **both** is what
        // produces the double-count: a centred 300-wide node on a 600-long rail
        // lands at 395.2 instead of 272.6, which is the two offsets summed.
        //
        // Kept because the two gates are one decision — "the box is inert on a
        // rail" — and a reader who gave railed text a wrap width for some later
        // reason would silently re-arm the double-count if this were not here.
        // Stated as redundant so nobody reads it as load-bearing and rewrites the
        // gate above around it.
        if on_rail {
            Alignment::Start
        } else {
            parley_alignment(parts.paragraph.align, parts.paragraph.direction)
        },
        AlignmentOptions {
            // **Deliberately true.** The default is `false`, which silently
            // start-aligns any line wider than its box — so a centred label
            // whose text outgrew its frame jumps to the left edge, which reads
            // as the alignment control having stopped working.
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
            // The name is the tag here and localized in `family_axes`; everything
            // else about an axis, the guard included, is
            // [`FontAxis::from_fvar`]'s.
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
/// A paragraph is the text between hard breaks — what parley reports as
/// [`BreakReason::Explicit`]. [`ParaSpans`] is keyed by byte range, so mapping a
/// paragraph to its style needs the byte it starts at, and **that table has to be
/// built by scanning the text**: `BreakLines` will not hand back a committed
/// line's `text_range` while it is still breaking, and by the time
/// `layout.lines()` can be read the measure has already been chosen. What the
/// breaker *does* hand back is each line's break `reason`, so the two halves meet
/// at a paragraph **index** — this type turns the text into a table, and
/// [`break_lines`] and [`y_map`] count `Explicit` breaks into it.
///
/// Two mechanisms on one edge, in other words, which is the shape of thing worth
/// a test rather than a comment: `paragraph_starts_agree_with_parleys_hard_breaks`
/// is what stops them drifting.
struct Paragraphs<'a> {
    /// One resolved style per paragraph, or **empty when nothing is overridden**
    /// and every paragraph is `default` — the ordinary case, and not worth an
    /// allocation on a path that runs on every keystroke.
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
                // **Clamped, so a trailing hard break's empty paragraph inherits
                // from the left.** `"a\n"` is two paragraphs and the second one
                // owns no bytes, so a span can neither be set on it nor resolve at
                // its start; reading the byte before it is the same rule the
                // character scope's boundary follows, and it is what makes Enter
                // at the end of a paragraph carry that paragraph's indent on.
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

    /// The style of paragraph `index`, counting from zero.
    ///
    /// Total rather than fallible: past the end is the node default, which is what
    /// a paragraph the table did not predict should draw as.
    fn style(&self, index: usize) -> &ParagraphStyle {
        self.styles.get(index).unwrap_or(self.default)
    }

    /// Whether **any** paragraph asks for optical margin alignment — the gate on
    /// the second break pass (§15 D830).
    ///
    /// **The node default has to be asked separately from the overrides**, and
    /// that is the whole of why this is a method rather than a `styles.iter()` at
    /// the call site: `styles` is deliberately *empty* when nothing is overridden
    /// (see the field), so a walk over it alone answers `false` for the ordinary
    /// node that simply has the setting on.
    ///
    /// ⚠️ **The second clause is inert today** (§15 D855, `[X3-L6-03]`): no
    /// `ParaAttr` reaches `optical_margins`, so every entry of `styles` carries
    /// the default's value. Kept, because the day a paragraph attribute does it
    /// is the right answer — and pinned by
    /// `no_paragraph_attribute_reaches_optical_margins`, whose exhaustive `match`
    /// is what makes that day a compile error rather than a surprise.
    fn any_optical_margins(&self) -> bool {
        self.default.optical_margins || self.styles.iter().any(|s| s.optical_margins)
    }
}

/// The byte each paragraph starts at, in content order. Never empty — an empty
/// node is one empty paragraph.
///
/// **The break set is parley's, not CSS's**: `layout::data::to_whitespace` maps
/// `\n`, `\r`, U+2028 and U+2029 to `Whitespace::Newline`, and a newline cluster
/// is what raises `BreakReason::Explicit`.
///
/// **`\r\n` is two breaks, with an empty paragraph between them.** This was
/// written to coalesce it — CRLF is one grapheme cluster under UAX #29, so one
/// cluster and one break was the obvious reading — and
/// `paragraph_starts_agree_with_parleys_hard_breaks` failed on its first run:
/// parley clusters the CR and the LF separately, so it breaks twice. Reading it
/// the "correct" way would put every paragraph of a CRLF document on its
/// neighbour's indent. (The same two breaks are why a CRLF document already gets
/// two paragraph gaps from `y_map`, which predates this and is parley's line
/// structure rather than ours to reinterpret — the caret follows those lines.)
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
/// hard breaks — **what a paragraph-scoped write acts on**.
///
/// A paragraph attribute cannot apply to half a paragraph, so a selection has to
/// grow to whole ones before it can key a [`ParaSpans`] entry, and a bare caret
/// names exactly one paragraph. `pub` so that the panel, [`TextEdit`] and anything
/// else asking "which paragraph is this?" ask the same code: the hard-break set is
/// parley's ([`paragraph_starts`]) and nothing outside this module has any business
/// restating it.
///
/// Two edges worth naming, because both are places a reasonable reading is wrong:
///
/// - **A selection ending exactly on a boundary does not reach the paragraph after
///   it.** Selecting a whole paragraph *including* its newline puts `end` on the
///   next paragraph's first byte, and taking that literally would style a
///   paragraph the user can see is not selected. A bare caret in the same position
///   is a different thing and does belong to the paragraph after it, which is why
///   the empty case is answered separately rather than by one formula.
/// - **A paragraph with no bytes of its own reaches back over its own break.** The
///   one after a trailing newline owns nothing, so its range would be empty,
///   [`crate::typography::Spans::set`] would no-op on it and the control would be
///   silently dead — on
///   exactly the paragraph a user has just made with Enter and wants to set up
///   before typing. The break byte it reaches back over is the same byte
///   [`Paragraphs`] clamps that paragraph's lookup to, so the two rules are one
///   rule read from either end.
pub fn paragraph_bounds(content: &str, range: Range<usize>) -> Range<usize> {
    let len = content.len();
    let starts = paragraph_starts(content);
    let begin = starts
        .iter()
        .rev()
        .find(|s| **s <= range.start.min(len))
        .copied()
        .unwrap_or(0);
    // The last byte the range actually covers. A caret covers none and belongs to
    // the paragraph it sits in; a selection stops short of the byte at its end.
    let last = if range.is_empty() {
        range.start.min(len)
    } else {
        range.end.min(len).saturating_sub(1)
    };
    let end = starts.iter().find(|s| **s > last).copied().unwrap_or(len);
    if begin < end {
        return begin..end;
    }
    // An empty trailing paragraph. `next_back` rather than `begin - 1` because the
    // break may be U+2028, which is three bytes and not a byte to land inside.
    let back = content[..begin]
        .chars()
        .next_back()
        .map(char::len_utf8)
        .unwrap_or(0);
    begin - back..end
}

/// Which paragraphs `range` touches, as indices into [`paragraph_starts`].
///
/// **[`paragraph_bounds`]' rule read in index space, and it has to be indices** for a
/// caller that writes to the paragraphs one at a time: the empty paragraph a trailing
/// break leaves owns no bytes, so the only range a span can be set on for it is the
/// break byte *before* it — the same byte the paragraph above ends on. Two paragraphs
/// that share a byte cannot be told apart by one, so a loop that intersected each
/// paragraph's writable range with the selection's would touch both.
///
/// The two answers agree paragraph for paragraph, which
/// `the_paragraphs_touched_are_the_ones_paragraph_bounds_spans` is the tripwire for.
fn paragraphs_touched(content: &str, range: Range<usize>) -> Range<usize> {
    let starts = paragraph_starts(content);
    let len = content.len();
    // Total: `partition_point` counts the starts at or before a byte, and there is
    // always at least one (an empty node is one empty paragraph), so the subtraction
    // cannot underflow.
    let index_of = |byte: usize| starts.partition_point(|s| *s <= byte).saturating_sub(1);
    // The last byte the range actually covers — a caret covers none and belongs to
    // the paragraph it sits in, a selection stops short of the byte at its end. Both
    // halves are `paragraph_bounds`', which is the point.
    let last = if range.is_empty() {
        range.start.min(len)
    } else {
        range.end.min(len).saturating_sub(1)
    };
    index_of(range.start.min(len))..index_of(last) + 1
}

/// Where one line starts and how much measure it gets: `(line_x,
/// line_max_advance)` in parley's terms, which become the line's
/// `inline_min_coord` and `inline_max_coord`.
///
/// Per *line* rather than per paragraph because the first-line indent lands on
/// one line of the paragraph and [`ParagraphStyle::hanging`] chooses which.
fn line_geometry(
    style: &ParagraphStyle,
    font_size: f64,
    max_advance: f32,
    first_line: bool,
) -> (f32, f32) {
    // `start_edge`, not `indent_start`, so a nested item's lines and its marker are
    // placed by one function (see [`ParagraphStyle::start_edge`]).
    let start = style.start_edge(font_size);
    let end = style.indent_end.resolve(font_size);
    // parley's `resolve_indent` rule, ours now: the amount lands on the
    // paragraph's opening line, and `hanging` moves it to every other one.
    let extra = if first_line != style.hanging {
        style.indent.resolve(font_size)
    } else {
        0.0
    };
    let x = (start + extra) as f32;
    // **The right edge stays at `max_advance - end` whatever the start edge
    // does**, which is what lets a negative (outdenting) first-line indent hang
    // into the margin instead of dragging the wrap point left with it. That the
    // result may exceed the layout's own width is why `break_lines` hands parley
    // an infinite `layout_max_advance`.
    //
    // Clamped at zero for the degenerate case — indents together wider than the
    // box — where parley would otherwise place one cluster per line off a
    // negative measure. One cluster per line is the honest drawing of that; a
    // negative measure is not a measure.
    let measure = (max_advance - end as f32 - x).max(0.0);
    (x, measure)
}

/// Each line's left and right **side bearings**, in layout px — the gap between
/// where a line's ink starts and where its advance origin sits, and the same at
/// the far end (§15 D830).
///
/// Read off a layout that has already been broken once, which is why optical
/// margin alignment costs a second break pass: the correction depends on the
/// first and last glyph of a line, and which glyphs those are is what breaking
/// decides.
///
/// ⚠️ **A glyph with no ink contributes nothing rather than its own bearing.** A
/// line ending in a space has a last glyph whose outline is empty, and its
/// "right bearing" would be the whole advance — which would hang the line a space
/// past the measure. The walk steps over empty glyphs from each end and gives up
/// at zero, which is also the answer for a blank line.
///
/// ⚠️ **The font borrow never leaves the call that draws one glyph.** A
/// `RunFace` borrows the blob it was built from; carrying one out to a `Vec`
/// alongside the glyphs would need the blob to outlive the run, and the only way
/// to write that is a `transmute` to `'static`. This module's one `unsafe` budget
/// is spent on nothing, and it should stay that way — so what is collected to
/// walk a line backwards is its runs and glyph *positions*, and a face is built
/// for the one glyph being asked about.
///
/// 🚨 **It outlines the two glyphs it needs per line, not every glyph** (§15
/// D855). It used to draw every glyph of every line to find the first and last
/// with ink, on the per-keystroke `shape` path — measured in release on a
/// 5,760-glyph paragraph, best of 20: **2.26 ms with the setting off, 7.93 ms
/// on**, ~82% of the difference being outlines whose answer was thrown away.
/// Now the walk goes forward from the start until one glyph has ink and backward
/// from the end until one does, which on text is one outline at each end.
/// Re-measured after, release, best of 20, a 4,680-glyph paragraph at 400 wide:
/// **1.80 ms off, 2.79 ms on — 1.55× against the 3.5× before**, and that
/// includes the extra passes the settling loop in `shape` now makes. (A
/// different fixture from the finding's, so the ratio is the comparison and the
/// milliseconds are not.)
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
        // The first inked glyph's left bearing, walking forward…
        let lsb = runs
            .iter()
            .find_map(|r| {
                r.positioned_glyphs()
                    .find_map(|g| inked(r, g).map(|b| (b.x0 - f64::from(g.x)) as f32))
            })
            .unwrap_or(0.0);
        // …and the last one's right bearing, walking back. A run's glyphs are
        // collected to be walked in reverse; that is positions, not outlines.
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
/// Returns whether anything was left unplaced. **Incremental rather than
/// `break_all_lines`**, for two reasons that arrived separately: a limit can only
/// be honoured by asking for one line at a time and
/// [`parley::BreakLines::revert`]ing the one that went too far — the line that
/// overflows has to be un-committed, not merely ignored, or its height is still
/// in the layout — and per-paragraph geometry has to be set *before* each
/// `break_next`, which is the same hook. `break_all_lines` is
/// `break_lines().break_remaining(w)`, i.e. this loop with one measure, so
/// there is nothing left for a fast path to be faster than.
///
/// ⚠️ **This is where hyphenation would go, and it was declined on cost rather than
/// on possibility** (§15 D399). The two calls a hyphenation retry needs are already
/// here for other reasons — `set_line_max_advance` below for the paragraph indent,
/// and `revert()` for the height limit — so charging a hyphen to the line it ends is
/// re-breaking that line at `measure − hyphen`, in this loop, with parley's public
/// API. parley already breaks at `U+00AD` and the glyph is inkless, so the *breaking*
/// half is free; what was priced and declined is the offset map D80 specifies and a
/// second layout pass. **Do not re-derive this as a wall** — `roadmap.md` §0 carries
/// the decision and D399 carries the numbers.
///
/// 🚨 **Hyphenation is no longer alone there. Tab stops, columns and widow/orphan
/// control joined it on 2026-09-22** (§15 D825, `roadmap.md` §0), and they are on the
/// list for the same kind of reason: not one of them is short of a dependency, so
/// each is a feature of *this* loop and its neighbours rather than a wall, and each
/// is a build nobody has asked for. **Widow/orphan control is the one that would
/// live here**, as a second pass over the lines this loop yields; tab stops are a
/// layer over `Whitespace::Tab`, which parley classifies and does not lay out; and
/// columns are a change to what a paragraph's box *is*, which is above this function
/// entirely.
///
/// ⚠️ **Justify-all is the exception and is *not* a non-goal — it is blocked
/// upstream**, which is a different state with a different trigger and has been
/// mistaken for a price twice. `TextAlign::Justify` is wired and `JustifyLast` ships
/// three readings of the last line that are ours; what is missing is parley's
/// `align_impl` hard-coding a skip of `BreakReason::None | Explicit`, with `align`,
/// `LayoutData` and `ClusterData::advance` all `pub(crate)`. The trigger is a patch
/// upstream, not a decision here (§15 D825, `roadmap.md` *Later · Parked*).
///
/// 🚨 **This run was separated from its item on 2026-09-22 and restored the same
/// hour** (§15 D830). An `Edit` adding `optical_offsets` was anchored on
/// `fn break_lines(`'s signature, which put the new item between this run and the
/// function it describes — and because both runs are `///` with nothing between
/// them they **merged**, leaving `break_lines` with no doc at all and this prose
/// sitting on a function about side bearings. ⚠️ **The `///`-run length ranking
/// could not see it**: the merged run came to about 38 lines against a floor of
/// 58, which is exactly the case `CLAUDE.md` calls *"a clean reading says nothing
/// about the edit you just made."* **Habit 2, the neighbour grep, is what caught
/// it**, run as a routine over every item the change inserted.
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
    // **`INFINITY`, not the wrap width, and it is an assertion in parley that
    // says so.** `break_next` opens with `layout_max_advance == f32::INFINITY ||
    // line_max_advance - layout_max_advance < 1.0`, so with a finite layout
    // maximum a line asking to be *wider* than the box panics rather than
    // misdraws — and an outdented first line is exactly that. Nothing else reads
    // the value: this assertion, the `>= f32::MAX` test that back-fills an
    // unbounded layout's `inline_max_coord` (which `f32::MAX` still satisfies for
    // an auto-width node), and a getter we never call.
    breaker.state_mut().set_layout_max_advance(f32::INFINITY);
    let mut lines = 0_u32;
    let mut truncated = false;
    // Which paragraph the line about to be broken belongs to, and whether it
    // opens that paragraph.
    let mut paragraph = 0_usize;
    let mut first_line = true;
    loop {
        let (mut x, mut measure) =
            line_geometry(paras.style(paragraph), font_size, max_advance, first_line);
        // **Optical margin alignment, and the reason it is one adjustment rather
        // than four** (§15 D830). Start the line a left bearing earlier and widen
        // its measure by both bearings: parley then aligns or justifies into the
        // widened measure, so *every* alignment falls out of the same two numbers.
        // Start-aligned, the line begins at `x - lsb`, putting its ink on the
        // nominal edge; end-aligned, parley pushes the advance end to
        // `nominal_end + rsb`, putting the ink there instead; centred, the two
        // corrections apply symmetrically and the *ink* is what gets centred; and
        // justified, the line stretches to the widened measure, so its ink spans
        // exactly the nominal one. **No arm of this reads `align` at all**, which
        // is what stops the four drifting apart.
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
            // Neither of the other two yields can arise here: they mean a line
            // height limit was exceeded or an out-of-flow inline box was met, and
            // we set no `line_max_height` and place no inline boxes. Stopping is
            // the safe reading anyway — both leave the line *uncommitted*, so
            // resuming without changing the geometry would ask for the same line
            // again forever.
            _ => break,
        };
        lines += 1;
        let too_tall = height_limit.is_some_and(|h| breaker.committed_y() > h);
        // Never revert the *first* line: a box shorter than one line still shows
        // that line (clipped), where an empty layout would lose the caret and
        // the node's identity with it.
        if too_tall && lines > 1 {
            breaker.revert();
            truncated = true;
            break;
        }
        if max_lines > 0 && lines >= max_lines {
            truncated = !breaker.is_done();
            break;
        }
        // The next line opens a new paragraph exactly when this one ended at a
        // hard break — the same key `y_map` builds its spacing table on.
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
/// **Computed once, here, and stored on the shaped layout** — because both
/// boundaries read it. It was briefly split, with the constant half applied
/// inside `extract` on a local clone; the drawing was right and the *hit test*
/// silently kept a zero shift, so a click in a bottom-aligned box landed a line
/// out. `a_click_in_a_bottom_aligned_box_lands_on_the_line_under_the_pointer` is
/// the guard.
fn y_map(layout: &Layout<RunIndex>, parts: TextRef<'_>, paras: &Paragraphs<'_>) -> YMap {
    let mut breaks = Vec::new();
    let mut cumulative = 0.0;
    // **A running sum of line heights, not `block_min_coord`.** That metric is
    // the line's *ink* extent — for a 10px line box it measured −1 to 11 — so
    // keying the table on it put each paragraph's shift one baseline early and
    // moved the line *above* the break as well as the ones below it.
    let mut top = 0.0_f64;
    let mut previous_was_hard_break = false;
    let mut paragraph = 0_usize;
    for (index, line) in layout.lines().enumerate() {
        if previous_was_hard_break {
            paragraph += 1;
            // **The paragraph *below* the break owns the gap.**
            // `ParagraphStyle::spacing` is space-before, so the value that opens
            // this gap belongs to the paragraph it is above — which is what makes
            // the node-level default mean what it always meant (a gap at every
            // hard break, none over the opening line) while still letting one
            // paragraph ask for its own.
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
        // An emergent box already *is* the visible extent, and `TextLayout::origin`
        // carries the inset — so the glyphs keep their own coordinates and trim
        // moves no ink at all.
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
///
/// A struct rather than a tuple because two of the three are a `Size`, and a call
/// site that swapped them would produce a box that is right almost everywhere —
/// trim is the only thing that separates them, and it is off on most nodes.
struct NodeBox {
    origin: Vec2,
    size: Size,
    /// The box **before trim**, always at the local origin.
    ///
    /// Equal to `size` exactly when trim did nothing to the box, which is every
    /// untrimmed node *and* every `Fixed` one — `Fixed` is the box the user typed
    /// and trim does not reinterpret it (§15 D78). That equality is what the canvas
    /// tests before drawing the line box as a second outline, so it has to be exact
    /// rather than nearly true: reporting a line box for a `Fixed` node would draw
    /// a dashed rectangle round its *content*, which is a different statement.
    untrimmed: Size,
}

/// The node's box and its offset from the local origin, plus the vertical shift
/// that puts the text inside it.
///
/// Mutates `ymap.base` rather than returning a third value, because the shift
/// and the box are one decision: the box says where the text may sit and the
/// base says where in it the text actually sits.
///
/// `natural_width` is the layout's width **before** optical margin alignment
/// shifted any line (§15 D855), when it ran: an auto-width box is sized by it, so
/// the setting moves ink inside the box and never the box itself.
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
        // An emergent box: it *is* the visible extent, so trimming tightens it
        // around ink that does not move — which is why the origin carries the
        // top inset instead of the glyphs being shifted up by it.
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
        // An authored box: `Fixed(size)` is the box the user typed and trim does
        // not reinterpret it. What trim changes here is the *datum* vertical
        // alignment measures — the cap-height extent rather than the line box. So
        // there is nothing untrimmed about it, and it says so by answering `size`.
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
/// Reads the *first* line's rise and the *last* line's drop only — interior
/// leading is untouched, which is what "trim the box" means rather than
/// "re-space the lines".
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

/// One run of a text node as an **exporter** needs it: the characters and the
/// style they carry.
///
/// The counterpart of [`GlyphRun`] for a consumer that emits *text* rather than
/// glyphs. It owns its substring rather than reporting a byte range because the
/// characters have already been case-transformed — slicing a node's `content` by a
/// range would give an exporter the text as typed, not the text as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportRun {
    /// The characters, already case-transformed exactly as the canvas drew them.
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
/// **For the exporters, and deliberately not on the render path.** SVG needs one
/// `<text>` per line at its own baseline with one `<tspan>` per run (§15 D81), and
/// none of that is derivable from the cached [`TextLayout`]: `GlyphRun` carries
/// glyph *ids*, which cannot be mapped back to characters through ligatures and
/// reordering. Widening the cached layout to carry text would put a `String` per run
/// on every relayout to serve a consumer that runs once, so this re-shapes instead —
/// permitted because export is not a hot path (§5.9), and free when nobody exports.
///
/// **The runs come from intersecting the style runs with the line's own range, not
/// from the glyph runs.** parley has no per-`GlyphRun` byte range at all — only
/// `Run::text_range`, which covers the whole run across every line it was broken
/// over, so reading it per glyph run reports the same span three times for a
/// three-line paragraph. `Line::text_range` plus the style-run boundaries is fully
/// determined and needs nothing from the brush. It is sound to compare those ranges
/// against the laid-out string because [`cased_text`] preserves byte length by
/// construction (`case_char` refuses any transform that would change it, with a
/// `debug_assert` on the total), so the cased text and `content` share one index
/// space.
pub fn export_lines(parts: TextRef<'_>) -> Vec<ExportLine> {
    let shaped = shape(parts);
    // Recomputed rather than carried out of `shape`: it is a pure function of the
    // content and the runs, so keeping it out of `Shaped` keeps the allocation off
    // the cached path.
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
        // **A marker is a line of its own here**, not a run inside this one: SVG
        // tspans flow, so a marker sharing the line's `<text>` would sit against the
        // first word instead of out in the gutter. Its x is the left edge because
        // that is what `<text x>` means, which is the one thing this consumer has to
        // work out for itself (`ListMarkers::at` gives the right edge).
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
        // The same justification shift `extract` applies, for the same reason: the
        // last line of a justified paragraph is laid out short and then moved.
        let dx = justify
            .filter(|_| index + 1 == line_count || line.break_reason() == BreakReason::Explicit)
            .map(|j| justify_shift(&line, j))
            .unwrap_or(0.0);
        // The leftmost glyph run is the line's left edge; `offset` already carries
        // whatever `Layout::align` did.
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
            // Guarded rather than indexed: a boundary that is not on a char edge
            // would panic, and that is not worth taking an export down for.
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
    // **The truncation ellipsis, which the canvas drew and this did not**
    // (§15 D506, `[S5.2-L1-03]`). `extract` appends it and this walk had no
    // ellipsis logic of any kind, so a truncated node exported as the clipped
    // text with nothing to say it was clipped — the canvas reading
    // *"one two three four five …"* and the drawing beside it *"one two three
    // four five"*.
    //
    // **Appended to the last line's runs rather than placed**, which is the one
    // difference from `extract`'s version and is the point: SVG `<tspan>`s flow,
    // so the ellipsis lands after the text it follows without this walk computing
    // an `x` at all. `ellipsis_run`'s clamp against the wrap width has no
    // counterpart here for the same reason — flow cannot overshoot a measure it
    // is not laying out against.
    //
    // The style is the one at the last **visible** byte, exactly as D490 made
    // `extract` take it: `shaped.runs` is ascending over the whole string, so the
    // style at the end of the *content* is precisely the type truncation removed.
    // `rev().find(start < end)` rather than a containment test, because the last
    // visible byte can fall in a gap between two spans.
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
    // The one map, exactly as the hit test will read it (see [`y_map`]).
    let ymap = &shaped.ymap;

    let justify = (parts.paragraph.align == TextAlign::Justify)
        .then_some(parts.paragraph.justify_last)
        .filter(|j| *j != JustifyLast::Start);

    let mut runs: Vec<GlyphRun> = Vec::new();
    let mut decorations: Vec<DecorationInk> = Vec::new();
    // Which of those bands are underlines, in the order they were pushed — the one
    // thing `skip_ink` cannot recover from the geometry. See its doc.
    let mut underlines: Vec<usize> = Vec::new();
    let mut last_line_end: Option<(f64, f64, usize)> = None;
    // Where the **last** line's ink starts in `runs` and `decorations`, and how
    // far left it reaches — the three things the ellipsis needs in order to make
    // room for itself by moving the line rather than by sitting on top of it
    // (§15 D610, `[S5.2-L1-01]`). `usize::MAX` is "no last line yet"; a `line_count` of zero
    // never enters the loop and the ellipsis block below never runs.
    let mut last_line_from = (usize::MAX, usize::MAX, f64::INFINITY);
    // The first run's own decoration metrics — see `TextLayout::decoration_sizes`.
    // Taken here rather than beside the two `decoration_ink` calls below, because
    // those only run for a decoration that *exists*, and the whole point of this is
    // to answer for one that does not yet.
    let mut decoration_sizes: Option<(f64, f64)> = None;

    // **List markers, collected in the walk that is already here.** A marker goes on
    // its paragraph's *first* line, so it needs the paragraph index and that line's
    // baseline — and finding the paragraph means counting `BreakReason::Explicit`,
    // which `break_lines` and `y_map` each already do. A third walk would be a third
    // counter to keep agreeing on one edge (§15 D163), so this rides the existing
    // one; only the shaping happens afterwards, where the engine borrow is over.
    let markers = ListMarkers::of(parts);
    let mut paragraph_heads: Vec<(usize, f64)> = Vec::new();
    let mut paragraph = 0_usize;
    let mut first_line = true;

    let line_count = shaped.layout.len();
    let mut baselines: Vec<f64> = Vec::with_capacity(line_count);
    for (index, line) in shaped.layout.lines().enumerate() {
        // **Every line, not only a paragraph's first** (§15 D355) — the marker
        // walk below wants one per paragraph and the snap targets want one per
        // line, so this is the same `to_local` asked one level finer rather than
        // a second walk. A baseline is the one y `to_local` may be handed
        // (§15 D166), which is why both reads spell it this way.
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
            // The brush *is* the style-run index, and `DEFAULT_RUN` falls out of
            // range on purpose: an inheriting run was never pushed, so it carries
            // the defaults' brush and must resolve to the defaults.
            let style = shaped
                .runs
                .get(glyph_run.style().brush as usize)
                .map(|(_, s)| s)
                .unwrap_or(parts.style);
            // Baseline shift is ours: a draw offset, not a metric, so the line
            // box does not grow and neighbouring baselines stay aligned.
            let shift = style.baseline_shift.resolve(style.font_size);
            let baseline = ymap.to_local(f64::from(glyph_run.baseline())) - shift;
            let glyphs: Vec<Glyph> = glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x + dx as f32,
                    // The glyph's own y is the baseline parley placed it on;
                    // re-basing on the mapped baseline moves the whole run.
                    y: (f64::from(g.y) - f64::from(glyph_run.baseline()) + baseline) as f32,
                    // Flat here always: `bend` is a pass over the finished layout
                    // and is the only thing that ever writes this.
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
                // **The toggle is applied here, by not listing the band.** `skip_ink`
                // is handed the indices of the bands that skip, so a run whose
                // underline has it switched off simply never reaches the list and
                // keeps the empty `gaps` every band is built with — no second flag to
                // read downstream, and the outlining pass is skipped along with it
                // (§15 D357).
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
                // ⚠️ **The third field was `runs.len()` and read by nobody** — the
                // `_` in the pattern below was the tell (§15 D490,
                // `[S5.2-L1-02]`). It is the last **visible byte** now, which is
                // what the ellipsis needs to know whose type it is set in.
                //
                // `line.text_range()`, not `run.text_range()`: a parley *run*
                // spans every line it was broken over, which is the trap
                // `export_lines` documents. The line is the thing that ends here.
                last_line_end = Some((x1, baseline, line.text_range().end));
                last_line_from.2 = last_line_from.2.min(x0);
            }
            runs.push(GlyphRun {
                // The exact blob parley resolved (Inter or an app-registered
                // family); a cheap shared handle, no byte copy.
                font: run.font().clone(),
                font_size: run.font_size(),
                coords: run.normalized_coords().to_vec(),
                glyphs,
                color: style.color,
            });
        }
    }

    // The end of the last line's ink, taken **before** the markers are appended.
    // A marker is pinned to the block indent and deliberately does not follow the
    // paragraph's alignment ([`marker_run`]), so the ellipsis shift below must not
    // move one — and a marker on the last line would otherwise be inside the range.
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
        // **The style at the last byte that is drawn, not at the end of the
        // content** (§15 D490, `[S5.2-L1-02]`). `shaped.runs` is a range table in
        // ascending byte order over the *whole* string, so `.last()` answered with
        // the style of the text that truncation removed — a character span
        // anywhere in the invisible tail restyled the ellipsis. Measured: a
        // `Size(40.0)` span over bytes past the one visible line put a **40pt `…`
        // in a 12.1-unit-tall box**, moved 16.5 units left, for a span set on text
        // that is not on screen.
        //
        // `rev().find(start < visible_end)` rather than a containment test: the
        // last visible byte can fall in a gap between two spans, and the run that
        // begins before it is the one whose type the reader last saw.
        let style = shaped
            .runs
            .iter()
            .rev()
            .find(|(r, _)| r.start < visible_end)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| parts.style.clone());
        // 🚨 **The line makes room for the ellipsis; the ellipsis does not sit on
        // the line** (§15 D610, `[S5.2-L1-01]`). This used to clamp the glyph's own x to
        // `w - advance`, which keeps it inside the box — the stated aim — and says
        // nothing about the **ink**: under `Center` and `End` the line's right edge
        // reaches the box edge by construction, so the clamp always bit and pushed
        // the `…` back over the last characters by its own advance. Measured in
        // release on one 10pt `AutoHeight(120.0)` node at `max_lines = 1`: body
        // glyphs at or past the ellipsis origin were **0** for `Start`, **1** for
        // `Center`, **2** for `End`. T2's *a bound that bounds the wrong quantity*,
        // from the drawing side.
        //
        // **Shifting the whole last line left is what a browser does** with
        // `text-align: right; text-overflow: ellipsis` — the ellipsis lands where
        // the line used to end and the text is pushed off the edge it was flush
        // against. Its two virtues over the other candidate fix, dropping trailing
        // clusters: it changes **no visible string**, so no document already using
        // the feature loses a character; and it is a uniform translation, so it is
        // right for a bidi line, where "the trailing clusters" are at the *left*
        // and a naive `x >= limit` drop would eat the beginning of the text.
        //
        // ⚠️ **`Start` is untouched and is the control.** Its line ends short of
        // the box, so `over` is negative and nothing moves — which is what makes a
        // test asserting all three alignments say something.
        //
        // ⚠️ **What this does not fix**: the ellipsis is placed at the line's
        // right edge for every direction, and an RTL line's logical end is at its
        // *left*. That is a second defect at the same call site and it is
        // pre-existing; `parley_alignment` is where the direction is known.
        let mut at = x;
        if let Some(w) = parts.sizing.wrap_width() {
            let advance = marker_advance(&style, ELLIPSIS);
            // Never past the line's own left edge: a box narrower than the
            // ellipsis would otherwise drag the ink out of the node entirely, and
            // one glyph clipped at the right is the better failure.
            let over = (x + advance - w).min(last_line_from.2).max(0.0);
            if over > 0.0 {
                let (from_run, from_decor, _) = last_line_from;
                for run in &mut runs[from_run..last_line_to] {
                    for g in &mut run.glyphs {
                        g.x -= over as f32;
                    }
                }
                // ⚠️ **The bands move and their `gaps` do not need to**, which is
                // only true because of where this sits: `skip_ink` runs *below*
                // this block and every band is still carrying the empty `gaps` it
                // was built with. Move this above `skip_ink` and the gaps are
                // stale by exactly `over`.
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

    // **After the markers and the ellipsis, because it reads `runs` and they are
    // ink too.** Neither can reach a band in practice — a marker draws in the
    // gutter and an ellipsis begins where the last run's band ends — but the rule
    // "every glyph this node draws" needs no exception, and a band clamps its gaps
    // to its own `x` range anyway.
    skip_ink(&runs, &mut decorations, &underlines);

    let mut out = TextLayout {
        size: shaped.size,
        origin: shaped.origin,
        untrimmed: shaped.untrimmed,
        runs,
        decorations,
        truncated: shaped.truncated,
        // Zero for an empty node, which has no line to measure — the panel reads
        // that as "nothing resolved" and falls back to what it did before.
        line_height: shaped
            .layout
            .lines()
            .next()
            .map_or(0.0, |l| f64::from(l.metrics().line_height)),
        decoration_sizes,
        baselines,
        warp: None,
    };
    // **Last, and after `skip_ink` on purpose** — see [`bend`]'s doc for why the
    // whole pipeline stays flat until here.
    if let Some(warp) = warp_for(parts, shaped) {
        bend(&mut out, warp);
    }
    out
}

/// How far a justified paragraph's last line moves. The line is not re-spaced,
/// only translated — which is what makes centre and end affordable where
/// "justify all" is not.
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
/// `parley` is asked for the *geometry* (its own resolved offset and size, which
/// already fold in the run's font metrics and any override we pushed) and our
/// own attribute for the two things it cannot hold: the line style and the
/// colour.
///
/// **`run_color` is the run's own ink, and an uncoloured decoration takes it.**
/// Three levels, resolved here because this is the one place all three are in
/// scope: the decoration's own colour if it has one, else the colour of the run it
/// sits under, else `None` for the node's fill stack. An underline under a red word
/// is red — which is what "absence means the text's own colour" has always
/// promised, and what stopped being true the moment a *run* could have a colour of
/// its own.
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
    // Offsets are measured from the baseline **upwards** — parley's convention,
    // inherited from `post.underlinePosition`, so an underline's own offset is
    // negative and the band's top is `baseline - offset`.
    let offset = f64::from(resolved.offset.unwrap_or(metric_offset));
    let size = f64::from(resolved.size.unwrap_or(metric_size)).max(0.0);
    let top = baseline - offset;
    Some(DecorationInk {
        band: Rect::new(x0.min(x1), top, x0.max(x1), top + size),
        style: ours.style,
        color: ours.color.or(run_color),
        // Filled in by [`skip_ink`] once every run is shaped, and only for the
        // bands that are underlines: this function is called for both kinds and
        // cannot tell which it is building.
        gaps: Vec::new(),
    })
}

/// The clearance a broken band leaves either side of the ink it steps around, as
/// a multiple of the band's own thickness. See [`skip_ink`].
///
/// **A multiple rather than a length, because this is text-local space.** A floor
/// in device pixels would be a different gap at every zoom level and a different
/// one again at 150% display scaling; a thickness is already proportional to the
/// type size, so one number holds from 8pt to 800.
const SKIP_CLEARANCE: f64 = 1.0;

/// How short a fragment of rule has to be to count as dirt rather than as
/// underline, as a share of the band's thickness. See [`absorb_specks`] for the
/// measured distribution this sits in the middle of, and for why it is a half rather
/// than the whole it started as.
const SKIP_SPECK: f64 = 0.5;

/// Flattening tolerance for the glyph outlines [`skip_ink`] measures, in
/// text-local units.
///
/// Coarser than the render boundary's own `TOLERANCE` on purpose. What comes out
/// of the flattening is an x-interval per contour, widened by a whole thickness
/// either side, so an eighth of a unit of chord error cannot move a gap edge
/// anywhere a reader could find it — and the flattening is the one part of this
/// whose cost scales with the glyph count.
const SKIP_TOLERANCE: f64 = 0.125;

/// Break each underline band around the glyph ink it would otherwise cross —
/// CSS's `text-decoration-skip-ink: auto`, which is what a reader expects, what
/// every browser draws, and what the canvas said out loud that it did *not* do
/// until this landed (§15 D356).
///
/// **Underlines only, and that is css-text-decor-4's rule rather than a shortcut.**
/// Skipping applies to underlines and overlines; a `line-through` is *meant* to
/// cross the letters, so a strikethrough's [`DecorationInk::gaps`] stays empty.
/// That is why this is handed the indices of the bands that skip instead of
/// looking at the bands themselves: the geometry cannot tell the two apart — a
/// strikethrough over a superscript run sits exactly where an underline over the
/// run beneath it would — and [`extract`]'s two calls are the only place that
/// knows which it asked for. `ondin_export::svg` has been stating the same rule
/// since 2026-08-21 and now agrees with the renderer about the answer as well as
/// the question (§15 D281).
///
/// **And it is a per-run setting, applied by the same list.** `Decoration::skip_ink`
/// defaults to on; a run that switches it off is left out of `underlines` by
/// [`extract`], so this function never sees it and its band keeps the empty
/// [`DecorationInk::gaps`] it was built with (§15 D357). Which means the whole cost
/// below is not paid at all for a node that has asked for a continuous rule.
///
/// **Whole-node, not per-run.** That is the granularity [`outline`] already works
/// at and the right one here: a band is broken by the ink that crosses it, and a
/// run boundary does not change that answer. Ink from a *neighbouring* band's run
/// is clamped away by the band's own `x` range, so the two do not bleed.
///
/// ## Why the interruptions live in the layout, and are intervals
///
/// This is the decision the roadmap entry owed its next reader. Two reasons, and
/// the second is the one that decides it.
///
/// A [`TextLayout`] is *already* the cached, dirty-set-invalidated product this
/// work needs to sit in (§5.9). Computing the gaps in [`extract`] costs one
/// outlining pass per relayout, on the nodes that carry an underline, beside the
/// parley shaping that relayout has just paid for — a fraction of it. Computing
/// them in the scene walk would cost it per *frame*, on every underlined node,
/// which is the cost the roadmap entry existed to warn about.
///
/// And the obvious mechanism — `boolean::evaluate(Subtract, &[band, glyphs])` —
/// **subtracts from the wrong shape.** The four line styles are built at the render
/// boundary (§6.3), so the only band that exists up here is the plain rectangle,
/// and a boolean run down there would be cutting a path whose dash phase and wave
/// crests are already baked. That puts a `flo_curves` fold inside a `catch_unwind`
/// on the frame path, per underlined node, where nothing else in the codebase runs
/// one uncached (`Resolved::boolean`, `renderer::reevaluate_booleans`). An
/// **interval** is style-independent: "the ink is here along the band" is equally
/// true of a solid rule, a dotted one and a ribbon, so it is the one form of this
/// answer that can be computed once, here, and still leave the pattern to be built
/// there.
/// ## What it costs, measured
///
/// The same 816-glyph paragraph twice — once on one line, once wrapped to 300 so it
/// carries 24 bands instead of 1. Release profile, whole `layout()` calls, best of
/// two runs of 200, milliseconds:
///
/// | | 1 band | 24 bands |
/// | --- | --- | --- |
/// | no underline at all | 0.25 | 0.26 |
/// | outline per glyph *occurrence* | 1.87 | 1.77 |
/// | memoized, no per-glyph box reject | 0.32 | 0.72 |
/// | as written | 0.34 | 0.40 |
///
/// The last row moved by 0.02 and 0.04 when [`raw_crossings`] gained its three
/// scanlines — three more passes over a crossing glyph's edges, and almost free,
/// because the box reject above means almost no glyph is a crossing glyph.
///
/// ⚠️ **The first version of this cost six times the rest of the layout** — which is
/// not what "it rides the relayout" was supposed to mean, and it was not the
/// flattening. It was doing the same work over and over, twice:
///
/// - **Each distinct glyph is outlined once.** Eight hundred letters are about thirty
///   different ones, so the ink is memoized per (face, instance, size, glyph) and
///   *placed* by adding the pen position — which is why [`GlyphInk`] is glyph-local.
///   Worth 1.87 → 0.32.
/// - **A band rejects a glyph on its bounding box**, four comparisons, before reading
///   any edge. Worth nothing at all on one band and 0.72 → 0.36 on twenty-four, which
///   is the whole point of it: without it the per-band pass multiplies by the node,
///   and a fully underlined page is the case that hurts.
///
/// What is left is Θ(bands × glyphs), and only on a node that carries an
/// underline.
///
/// 🚨 **The sentence here read *"under a third of the layout it rides on"* and is
/// false at scale** (§15 D769, `[S5.2-L4-06]`). It was measured on the table above,
/// which tops out at 816 glyphs and 24 bands. At **22,528 glyphs over 366 bands**
/// the scan is **6.6 ms of a 12.7 ms layout — 52%**, because the per-*run* reject
/// tests against `reach`, the **union** of every band's extent: on a multi-line
/// underlined paragraph that union spans the whole node, so the reject rejects
/// nothing and `raw_crossings` walks the full glyph list once per band. The
/// four-comparison box test makes each step cheap; it does not remove the product.
///
/// ⚠️ **A measurement is about the fixture it was taken on**, and this one was
/// quoted as a property of the function. The figure is left as a complexity class
/// rather than replaced with a bigger number, because the next fixture will be
/// bigger again.
///
/// ⚠️ **It is *not* paid twice per keystroke**, which both `[S5.2-L4-06]` and
/// `review/index.md` say: that cites `[A4-L4-01]`, **fixed** by §15 D592 — a
/// keystroke shapes the edited node once, and `canvas.rs` says so at the site.
/// `TextEdit::reshape` calls `shape()`, not `extract`, so `skip_ink` runs once.
///
/// **Left as it is, deliberately.** The trigger is ~4,500 words underlined end to
/// end in a *single* node; bucketing `placed` by line is ~20 lines, and the honest
/// guard for it is a byte-identical snapshot of `gaps` before and after — the
/// existing test asserts *properties* (`!gaps.is_empty()`, `total < width * 0.6`)
/// and would stay green through a float-level drift.
fn skip_ink(runs: &[GlyphRun], decorations: &mut [DecorationInk], underlines: &[usize]) {
    if underlines.is_empty() {
        return;
    }
    // The union of the bands' vertical reach. This is what lets a whole run be
    // rejected without outlining it, which is the difference between outlining one
    // line of a paragraph and outlining all twenty.
    let mut reach = (f64::INFINITY, f64::NEG_INFINITY);
    for &i in underlines {
        let (top, bottom) = ink_extent(&decorations[i]);
        reach = (reach.0.min(top), reach.1.max(bottom));
    }

    // The distinct glyph inks, and one entry per glyph the node draws pointing into
    // them. `sigs` is the (face, instance, size) each run shaped with, found by
    // linear scan: a node has a handful of those at most, and an *index* is what
    // keeps the memo key exact — a hash of the coordinates would be one collision
    // away from drawing one run's glyph with another's ink.
    let mut inks: Vec<GlyphInk> = Vec::new();
    let mut sigs: Vec<(*const u8, u32, u32, &[i16])> = Vec::new();
    let mut memo: rustc_hash::FxHashMap<(usize, u32), usize> = Default::default();
    let mut placed: Vec<(usize, f64, f64)> = Vec::new();
    let mut path = BezPath::new();
    for run in runs {
        let Some(first) = run.glyphs.first() else {
            continue;
        };
        // A glyph's ink stays within two ems of its own baseline in any face worth
        // shaping with — ascenders reach about one em and descenders a third — so
        // this rejects the lines no band can touch without paying to outline them.
        // Conservative in the *safe* direction: a face with a glyph past two ems
        // loses a gap it should have had, which is exactly what the canvas drew
        // before this function existed.
        let span = f64::from(run.font_size) * 2.0;
        let y = f64::from(first.y);
        if y + span <= reach.0 || y - span >= reach.1 {
            continue;
        }
        let Some(face) = RunFace::of(run) else {
            continue;
        };
        // The blob's *address* stands for the face: `FontData` is a shared handle,
        // so two runs either point at the same bytes or at different ones, and
        // nothing here outlives the layout that made it.
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
                // Upright, and not because a glyph here is: `skip_ink` runs before
                // `bend` (see its doc), so every `rot` it could read is still zero
                // and the whole point of this memo is that one letter's ink is
                // outlined once for every place it appears.
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

/// One glyph's flattened ink, in **glyph-local** space — the outline as
/// [`outline`] would draw it with the pen at the origin, so placing it is an
/// addition and the same ink serves every occurrence of that glyph.
///
/// See [`skip_ink`]'s measurements for why this is memoized rather than built per
/// glyph in reading order.
struct GlyphInk {
    /// Flattened edges as `[x0, y0, x1, y1]`, y already mirrored the way the rest
    /// of the outlining mirrors it.
    edges: Vec<[f64; 4]>,
    /// The box those edges cover. **This is what makes the per-band pass cheap** —
    /// a band rejects a whole glyph on four comparisons instead of walking thirty
    /// edges, so twenty bands on one paragraph cost twenty rejects and not twenty
    /// walks. Empty for a glyph with no ink, in the form that fails every overlap
    /// test rather than the form that passes them all.
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

/// The skrifa side of one [`GlyphRun`] — the face, the instance and the size,
/// resolved once for the whole run.
///
/// **A struct only because a `DrawSettings` is consumed per glyph and these three
/// are not.** This is the body [`append_run_outline`] used to hold inline, lifted
/// out when [`skip_ink`] became a second caller that wanted the same face with a
/// different pen position.
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
            // The instance parley shaped with, in skrifa's own units: `coords` is
            // already normalized F2Dot14 bits (that is what `normalized_coords`
            // hands over), so this is a re-wrap and not a conversion. Getting it
            // wrong would draw the default instance — a variable font's weight axis
            // silently ignored.
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
        // The pen borrows `out`, so the "is a contour still open" answer is carried
        // out of the block rather than asked while it is alive.
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
            // A fresh `DrawSettings` per glyph because `draw` consumes it; the two
            // borrowed pieces are cheap and the location is shared.
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

/// The band's true vertical extent — what the render boundary will actually
/// paint, which is the band itself for three of the four styles and taller for
/// the fourth.
///
/// ⚠️ **A wavy underline's ribbon is three thicknesses tall, not one.**
/// `scene::wave` swings one thickness either side of the band's middle and is half
/// a thickness thick, so its ink reaches `1.5t` from the centre where the band
/// reaches `0.5t`. Gaps measured against the band alone would let a crest run
/// straight through a descender the band had been broken for — the one case where
/// the drawing is bigger than the geometry it was derived from.
///
/// This is the one number in core that belongs to the render boundary. It is
/// stated in both places, and `scene::tests::a_wavy_ribbon_stays_inside_the_reach_core_assumes`
/// is what stops the pair drifting apart in silence.
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
/// ⚠️ **The closing edge is emitted by hand.** `kurbo::flatten` passes `ClosePath`
/// through untouched, so the segment from the last point back to the subpath's
/// start never arrives as a `LineTo` — and in a glyph that segment is a real
/// contour edge, very often the flat side of a stem. Dropping it loses the gap
/// under exactly the letters this feature exists for.
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

/// Where the band is interrupted — the **final** answer, clearance, speck rule, air
/// and all, which is what makes `scene::kept_spans` a plain complement.
///
/// ## Structure first, air second, and that order is the whole design
///
/// 1. **The break structure, decided with no clearance at all.** The band breaks
///    where glyph ink actually crosses it; [`absorb_specks`] then closes the
///    fragments that are not rules — a hole inside one letter, or anything under half
///    a thickness.
/// 2. **Then the air**, by [`give_air`]: every gap grows by up to a thickness on each
///    side, taking only what the bar beside it can spare above a floor of one
///    thickness.
///
/// **Nothing that exists can be taken away by the air**, which is what the order
/// buys. Get it the other way round — widen the crossings first, then ask which
/// fragments are big enough to draw — and a bar's width is `bearing − clearance`, so
/// there is always a type size at which a bar crosses whatever minimum is set and the
/// drawing **pops between two shapes**. That is not hypothetical: it is what this
/// function did for two commits, and an underlined `g` lost one of its two marks at
/// 15 and 16pt. Moving the minimum from a whole thickness to a half moved the pop to
/// 24pt rather than removing it, which is the measurement that settled the order.
///
/// The three numbers are all the band's own thickness or a share of it, so the
/// drawing holds at every type size and zoom: full clearance where there is room, a
/// square of ink where there is not (`tests::air_narrows_a_bar_and_never_removes_one`,
/// `tests::a_narrow_bands_rule_never_vanishes_and_keeps_its_width_at_every_size`).
///
/// A band whose ink leaves nothing that qualifies keeps its widest raw fragment
/// anyway — one mark rather than none, which is what a lone `j` gets — and a band
/// whose ink really does cover it end to end shows nothing, because then there is
/// nothing to show.
///
/// ## And the clearance is horizontal only
///
/// The vertical test is the ink as drawn. Padding the band vertically as well is the
/// obvious spelling, and what it spends is the clear air between an underline and the
/// *overshoot* of a round letter — the percent of an em by which an `o` or an `e`
/// dips below the baseline it sits on. Measured on Inter (and proportional, so the
/// type size drops out): that air is **1.31 thicknesses**, so a whole thickness of
/// vertical padding survives this face with 0.31 to spare and 1.4 of it breaks the
/// rule under every round letter — see
/// `tests::an_underline_with_nothing_in_its_way_is_one_unbroken_bar`, which pins the
/// margin for exactly this reason. A face whose `underlinePosition` is half as deep
/// has half the air. So the vertical test stays exact, where the answer is a fact
/// about the drawing rather than a bet on the face, and the widening is what gives
/// the gap its air.
fn crossings(
    inks: &[GlyphInk],
    placed: &[(usize, f64, f64)],
    ink: &DecorationInk,
) -> Vec<(f64, f64)> {
    let t = ink.band.height();
    if t <= 0.0 || ink.band.width() <= 0.0 {
        return Vec::new();
    }
    // **The structure first, and it is decided without the clearance**, so that no
    // bar exists or not according to how much air it was given.
    let (bare, letters) = raw_crossings(inks, placed, ink, 0.0);
    let structure = absorb_specks(&bare, ink.band, t, &letters);
    if fragments(&structure, ink.band).is_empty() {
        // Nothing the glyph ink leaves is a rule. Keep the widest raw fragment
        // anyway — one short mark is what "this text is underlined" looks like when
        // there is no room for a rule — or nothing at all if the ink really does
        // cover the band end to end.
        return match fragments(&bare, ink.band)
            .into_iter()
            .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
        {
            Some(widest) => complement(&[widest], ink.band),
            None => bare,
        };
    }
    // Then the air, which can no longer take a bar away — only narrow it, and not
    // past a square of ink. **That is where "a bar shorter than the band is thick is
    // not a bar" belongs**: as the limit on what air may take, not as a veto on what
    // exists, which is the whole difference between a drawing that pops and one that
    // does not.
    give_air(&structure, ink.band, t * SKIP_CLEARANCE, t)
}

/// Grow every gap by up to `clear` on each side, **taking only what the bar it eats
/// into can spare** above `floor`.
///
/// ⚠️ **This is the shape of the fix, and the shape of the bug it fixes.** For two
/// commits the clearance was applied to the crossings *before* anything decided which
/// fragments were rules, so a bar's width was `bearing − clearance` and a bar could be
/// shrunk to nothing by the air around it. Every minimum width then has a type size
/// where a bar crosses it and the rule pops between two drawings: at a whole
/// thickness that size was 15–16pt for an underlined `g`, and moving the threshold to
/// half a thickness moved the pop to **24pt** rather than removing it. Deciding the
/// structure first and giving air second is what makes the drawing continuous — a
/// marginal bar stops getting air instead of ceasing to exist.
///
/// The arithmetic: a bar with two gaps beside it may be eaten from both, so each side
/// takes at most half of what it can spare; a bar against the band's own edge has
/// only one gap beside it and can spend everything on that one. For an ordinary bar,
/// `spare` is far more than `2 · clear` and every gap gets the full clearance, which
/// is why nothing about ordinary copy changes.
fn give_air(gaps: &[(f64, f64)], band: Rect, clear: f64, floor: f64) -> Vec<(f64, f64)> {
    if clear <= 0.0 || gaps.is_empty() {
        return gaps.to_vec();
    }
    // The band as an alternating run of bars and gaps, so each gap can look at its
    // neighbours without matching coordinates back up by equality.
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
    // What each bar can give up, per gap beside it.
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
/// and the x-box of every letter that crossed it. Named because the pair is a
/// mouthful, and both halves are `(x0, x1)` intervals in the band's own space.
type Crossings = (Vec<(f64, f64)>, Vec<(f64, f64)>);

/// One crossing, widened by the clearance and clamped to the band — `None` if
/// nothing of it lands inside.
fn widened(x0: f64, x1: f64, band: Rect, clear: f64) -> Option<(f64, f64)> {
    let (a, b) = ((x0 - clear).max(band.x0), (x1 + clear).min(band.x1));
    (b > a).then_some((a, b))
}

/// The clearance-widened, merged x-spans of `ink`'s band that glyph ink crosses,
/// **and the placed x-box of every letter that contributed one**.
///
/// [`crossings`] calls this twice, which is the whole reason `clear` is an argument.
/// The boxes come back with the spans because they are the same walk: they are what
/// tells a fragment of rule showing through a hole *inside* a letter from one lying
/// in the bearing *beside* it, and [`absorb_specks`] needs that distinction.
///
/// ⚠️ **Two measurements of the same thing, and the first one is the one that is
/// right.** Three scanlines through the band give the *filled* ink — pairs of
/// crossings, so the span between a stem's two sides is covered — and then every
/// edge's own x-extent inside the band is added on top, which catches a thin feature
/// that slips between the rows (the tip of a descender, a hairline diagonal).
///
/// **The edge extents alone were the original implementation and they were wrong.** A
/// vertical edge crossing the band has an x-extent of *zero*: a `p`'s stem is two
/// vertical edges and the ink between them was never measured at all. It looked
/// correct only because the clearance dilated each edge into a span 2·`clear` wide,
/// so for a stem narrower than that the two spans overlapped and covered it by
/// accident. Two consequences, and both are real: at **zero** clearance — which
/// [`crossings`] now asks for — a `p` produced no gap whatever, and at any clearance
/// a stem *wider* than `2 · clear` would have had the underline drawn straight
/// through its middle. `tests::a_bold_stem_is_one_gap_and_not_two` pins the second,
/// which is a bug that was in the tree and was never seen because Inter's regular
/// stem at 40px is 3.4 units against a 5.5-unit dilation.
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
        // The whole glyph, on its box, before any of its edges are read. Written in
        // the placed space rather than by offsetting the band, so the two rejects
        // and the arithmetic below all speak one coordinate system.
        let (gx0, gy0) = (glyph.bounds.x0 + dx, glyph.bounds.y0 + dy);
        let (gx1, gy1) = (glyph.bounds.x1 + dx, glyph.bounds.y1 + dy);
        if gy1 <= top || gy0 >= bottom || gx1 + clear <= band.x0 || gx0 - clear >= band.x1 {
            continue;
        }
        let before = spans.len();
        // **The filled ink, three scanlines of it.** A stem's crossing is the space
        // *between* its two sides, and an edge on its own cannot say that — see the
        // ⚠️ note on this function.
        for row in [top, (top + bottom) * 0.5, bottom] {
            hits.clear();
            for &[ex, ey, fx, fy] in &glyph.edges {
                let (ax, ay, bx, by) = (ex + dx, ey + dy, fx + dx, fy + dy);
                // Half-open in y, which is what makes a shared vertex count once and
                // leaves an even number of crossings on a closed contour.
                if (ay <= row) == (by <= row) {
                    continue;
                }
                hits.push(ax + (bx - ax) * (row - ay) / (by - ay));
            }
            hits.sort_by(f64::total_cmp);
            for pair in hits.chunks_exact(2) {
                if let Some(s) = widened(pair[0], pair[1], band, clear) {
                    spans.push(s);
                }
            }
        }
        for &[ex, ey, fx, fy] in &glyph.edges {
            let (ax, ay, bx, by) = (ex + dx, ey + dy, fx + dx, fy + dy);
            let (ymin, ymax) = if ay <= by { (ay, by) } else { (by, ay) };
            // Strict, so ink that merely *touches* the band's edge does not break it.
            if ymax <= top || ymin >= bottom {
                continue;
            }
            // The x extent of the part of this edge that is inside the band's rows.
            // The clamp is what makes an edge lying wholly inside them answer with
            // both its own endpoints rather than with a projection off the end of
            // itself.
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
        // Only a letter that actually broke the band: one whose ink is above it has
        // nothing to say about which fragments are holes in a letter.
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

/// What is left of `band` once `gaps` are taken out of it — the same complement
/// `scene::kept_spans` takes, computed here because the rules in [`crossings`] are
/// all about how wide these come out.
///
/// `gaps` must be sorted and non-overlapping, which is what the merge at the end of
/// [`raw_crossings`] guarantees.
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

/// `gaps`, grown to swallow every fragment of rule that is not a rule. **Two kinds,
/// and they are two different questions.**
///
/// 1. **A fragment inside one letter's own ink box is a hole in the letter**, not a
///    piece of underline, whatever its width: it is the band showing through the
///    loop of a `g` or between the two diagonals of a `y`, and it reads as dirt
///    caught in the letter. `letters` is the box of every glyph that broke this
///    band; a fragment contained in one of them is closed.
/// 2. **A fragment shorter than half the band's thickness is too short to read** —
///    what a clamp leaves at the band's edge when a word opens with a descender, or
///    between two crossings that nearly meet. `gypsy` at 40px opens with `0.18` and
///    `0.51` units against a thickness of 2.73.
///
/// Absorbing either **is** merging the gaps on both sides of it, which is the same
/// decision said the other way round.
///
/// ⚠️ **The share is a half and not a whole, and the reason is measured.** "A bar
/// shorter than the band is thick is not a bar" was this rule for two commits, and a
/// full thickness turned out to sit exactly on top of the commonest case in the
/// distribution: an underlined `g`'s side bearing in Inter is **1.02 to 1.06
/// thicknesses** at most sizes, so a 2% wobble in the letterform flipped it in and
/// out and the rule popped between two drawings at 15 and 16pt. The measured values
/// at 40px fall in two clumps — dirt at `0.07t` and `0.19t`, then real bearings from
/// `0.75t` (a `j`) to `1.23t` — and half a thickness is the empty middle of that. It
/// is rule 1 that made the room: with holes inside letters closed structurally, this
/// one no longer has to reach up past the `0.58t` slivers in a `g`'s tail, which is
/// what forced it so high in the first place.
///
/// ⚠️ **Moving it did not fix the pop, and nothing about a threshold could have.**
/// Half a thickness moved the size at which an underlined `g` changed shape from
/// 15–16pt to 24pt; what removed it was applying this rule to the crossings *before*
/// any clearance widens them, so that no bar's existence depends on the air around
/// it. See [`give_air`]. This rule is here for the dirt, and only for the dirt.
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
/// **Built once and asked by paragraph index, because two passes walk these lines.**
/// `extract`'s ink is a [`GlyphRun`] in the cached layout; `export_lines`'s is an
/// [`ExportLine`] of its own, since SVG needs the marker as a separate `<text>` at
/// its own x rather than as a `<tspan>` that would flow (§15 D81). Keeping the
/// decision here is what stops the two disagreeing about which paragraph is an item,
/// what number it carries, or where its gutter is — *a marker that draws on the
/// canvas and is missing from the export is exactly what a second path invites.*
struct ListMarkers<'a> {
    parts: TextRef<'a>,
    paras: Paragraphs<'a>,
    /// The byte each paragraph starts at, so an item's own character style can be
    /// resolved there.
    starts: Vec<usize>,
    /// The marker and its ordinal per paragraph — see [`Self::ordinals`].
    ordinals: Vec<Option<(ListMarker, usize)>>,
}

impl<'a> ListMarkers<'a> {
    /// `None`, and no allocation at all, when nothing in the node asks for a marker
    /// — which is every text node until somebody makes a list.
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
    /// **Consecutive paragraphs belong to one list only if they ask for the same
    /// marker at the same level**, and that is the whole numbering rule — the only
    /// thing this model has in place of markup. There is no `<ol>` to be inside, so a
    /// run of items is bounded by the first neighbour that disagrees: a paragraph with
    /// no marker, or one asking for a different one. Each run counts from one, so a
    /// bullet between two numbered items makes three lists rather than a gap in one.
    ///
    /// Unordered markers are counted too and simply ignore the number, which keeps
    /// this one rule rather than two.
    ///
    /// **The stack is what makes nesting restart per parent** ([`ParagraphStyle::level`]).
    /// One counter per open level, and going back *out* truncates the deeper ones — so
    /// `1, (a, b), 2, (a, b)` numbers both sublists from `a` while the outer run counts
    /// through, which is the behaviour `<ol>` gets from being a container. It also
    /// answers the case a flat run cannot: an item at the outer level resumes its own
    /// count rather than restarting, because the paragraphs between it and its
    /// predecessor never touched level 0's counter.
    fn ordinals(paras: &Paragraphs<'_>, count: usize) -> Vec<Option<(ListMarker, usize)>> {
        let mut out = Vec::with_capacity(count);
        // The marker each open level is counting, and how far it has got. `None` at a
        // level is one skipped on the way in — a level-2 item under a level-0 one —
        // which has no count of its own because it has no paragraph, and must not lend
        // one to a level-1 item arriving later.
        let mut open: Vec<Option<(ListMarker, usize)>> = Vec::new();
        for index in 0..count {
            let style = paras.style(index);
            let Some(marker) = style.marker else {
                // **A paragraph that is not an item ends every list**, at every level
                // — the rule level 0 has always followed, read over the stack. It is
                // what makes a continuation paragraph inside an item restart the
                // numbering after it, which is a real limitation of a model with no
                // container and is stated in `ParagraphStyle::level`.
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
                // Either this level had not opened, or it was a different list. Both
                // start this marker's own count at one.
                _ => Some((marker, 1)),
            };
            out.push(*counter);
        }
        out
    }

    /// What paragraph `index` draws: the marker text, the character style to set it
    /// in, and the x its **right edge** lands on.
    fn at(&self, index: usize) -> Option<(String, TextStyle, f64)> {
        let (marker, n) = self.ordinals.get(index).copied().flatten()?;
        // **The item's own type, resolved at its first byte** — so a 40pt item gets a
        // 40pt bullet and a red one a red bullet, and recolouring or resizing a list
        // item takes its marker with it. Clamped like `Paragraphs::new`'s own lookup,
        // for the empty paragraph a trailing newline makes.
        let last_byte = self.parts.content.len().saturating_sub(1);
        let start = self.starts.get(index).copied().unwrap_or(0).min(last_byte);
        let style = self.parts.spans.resolve(self.parts.style, start);
        // **The gutter is the paragraph's own start edge**, resolved against the
        // *node's* font size because that is what `line_geometry` resolved it
        // against. Resolving an `Em` indent against the item's own size instead would
        // put the marker and its text on two different gutters. Nesting rides along
        // in `start_edge`, which is the same function `line_geometry` calls.
        let right = self
            .paras
            .style(index)
            .start_edge(self.parts.style.font_size);
        Some((marker.text(n), style, right))
    }
}

/// A list marker's own advance, for a caller that needs to right-align it itself.
///
/// `export_lines` is that caller: SVG's `<text x>` names the ink's **left** edge.
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
        // Flat, like the two shapers below it: a marker is a fragment measured to
        // be *placed into* the flat layout, and `bend` maps that layout afterwards.
        // Bending it here would bend it twice.
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    })
    .width()
}

/// A shaped list marker, right-aligned so its own right edge lands on `right`.
///
/// **Right-aligned, so `1.` and `10.` put their text in the same column** — which
/// is the reason a list marker is not an in-flow parley inline box even setting
/// aside the caret (see [`ParagraphStyle::marker`]): an in-flow box makes each
/// item's text start at a different x, and the misalignment grows exactly when a
/// list gets long enough to notice.
///
/// **There is no gap constant.** The right edge lands on the paragraph's start edge
/// and the sidebearings of `.` and of the first letter are the gap; the user sizes
/// the gutter with `indent_start`, which is a control they already have. One fewer
/// number chosen here.
///
/// The marker does **not** follow the paragraph's alignment or its first-line
/// indent: it is pinned to the block indent, so a hanging indent and a marker do
/// not fight over the same edge. A list is a start-aligned construct, and saying so
/// is better than a marker that drifts.
fn marker_run(style: &TextStyle, text: &str, right: f64, baseline: f64) -> Option<GlyphRun> {
    let spans = CharSpans::default();
    let para_spans = ParaSpans::default();
    // `ParagraphStyle::default()` carries no marker, so this cannot recurse — and
    // it would not anyway, since markers are emitted by `extract` and this shapes.
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
    // The item's own ink, so a per-run colour reaches the marker. `extract_flat`
    // leaves this `None` for the font picker, which wants the chrome's colour.
    run.color = style.color;
    Some(run)
}

/// The truncation mark, named once because two functions must measure and shape
/// the **same** string: `extract` asks [`marker_advance`] how much room to make
/// and [`ellipsis_run`] shapes what goes in it (§15 D610, `[S5.2-L1-01]`). Two spellings of
/// one glyph is a gap that opens silently.
const ELLIPSIS: &str = "…";

/// A shaped `…` placed at `at`, the end of a truncated line.
///
/// Shaped on its own rather than appended to the content, so the ellipsis never
/// enters the byte space the caret and the spans live in.
///
/// ⚠️ **It no longer decides where it goes** (§15 D610, `[S5.2-L1-01]`). It took the wrap
/// width and clamped itself to `w - advance`, which kept it in the box and put it
/// on top of the text; the caller makes room by moving the line instead, and the
/// argument for that is at the call site.
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
    // The ink of the type it follows, exactly as `marker_run` does it three
    // functions above (§15 D490). `extract_flat` leaves this `None` for the font
    // picker, which wants the chrome's colour — so both of its other callers owe
    // this line, and only one of them was paying it: an ellipsis after a red run
    // fell through to the node's fill stack.
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
            // A family-list preview draws in the picker's own text colour, which
            // the chrome supplies; a run colour would be a document value leaking
            // into a control.
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

/// How many text layouts this thread has shaped since it started. Never
/// decreases (§15 D590).
///
/// ⚠️ **Counted in `shape` and not in [`layout`]**, because those are not the
/// same population: `TextEdit::reshape` shapes without going through `layout` at
/// all, and it is exactly half of the pair `[A4-L4-01]` is about. A counter on
/// `layout` reports **0** for a keystroke.
///
/// **The counter `Resolved`'s cache is about**, and the only way to assert that a
/// commit did not do work it had no reason to do. `Resolved::update` used to
/// re-shape every text node in a moved subtree for a *translate* — a change that
/// cannot move a glyph — and no assertion anywhere could see it: the resulting
/// layouts are identical, so the differential test that owns `update`'s
/// correctness is green either way. Reading this **either side of a commit** says
/// how much shaping it cost, which is the question.
///
/// **A count and not a flag**, and thread-local rather than global, for
/// [`crate::boolean::failures`]'s two reasons exactly: a monotonic number lets
/// brackets nest without treading on each other, and parley's engine is
/// thread-local here anyway, so a shared counter would have the renderer's
/// preview thread and the edit path reporting each other's work.
///
/// Cheap enough to leave in a release build — one non-atomic increment against a
/// shape measured at 1.17 ms for 4,000 characters.
pub fn shapes() -> u64 {
    SHAPES.with(std::cell::Cell::get)
}

/// Local (untransformed) box of a text node's content. Thin wrapper over
/// [`layout`] for geometry/hit-testing that doesn't need glyphs.
pub fn measure(parts: TextRef<'_>) -> Rect {
    layout(parts).bounds()
}

/// A family name shaped in the family it names — one row of the font picker.
///
/// Positions are in strip-local space: `x` along the run from 0, `baseline` down
/// from the top of the line box.
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
/// **A preview that silently falls back is worse than no preview**, because the
/// whole point of the row is to show what the typeface looks like: a row reading
/// "Bungee Shade" in Inter is a lie about a font the user is choosing by eye. So
/// this is deliberately all-or-nothing, and the three ways it declines are the
/// three ways parley can hand back something that is not the family asked for:
///
/// - the family has no registered faces (the bytes have not arrived yet), so
///   every glyph would come from the bundled fallback;
/// - the name needed more than one face to draw, or came back drawn by a blob
///   that is not this family's — parley's own fallback stepping in for
///   characters the face does not carry;
/// - a glyph came back as `.notdef`, which is what an icon or symbol font does
///   with a Latin name. Those families exist in the catalog in numbers, and a
///   row of empty boxes reads as a broken list rather than as a symbol font.
///
/// In all three the caller draws the name in the UI font instead.
pub fn family_preview(family: &str, label: &str, size: f64) -> Option<FamilyPreview> {
    // The blob this family's default face is made of, which is what the shaped
    // run has to have come from. Taken before shaping so the borrow is over.
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
            // The font's own leading: a preview row is one line of the typeface
            // as the typeface sets it, and it is what keeps ink inside the box.
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
/// This is the model behind in-canvas text editing (§9.3). It lives in core,
/// not in the app, because everything it needs — shaping, cluster boundaries,
/// bidi-aware caret movement — is parley's, and parley is core's dependency
/// alone. The app owns *when* an editing session starts and ends and commits
/// exactly one transaction when it does; this type owns what happens to the text
/// in between, and never touches the `Document`.
///
/// Coordinates are text-local (origin at the node's local origin, y down),
/// matching [`Glyph`] positions, so callers map through the node's world
/// transform — and **every point goes through the session's [`YMap`]**, which is
/// what keeps a click landing on the character the eye sees when paragraph
/// spacing, trim or vertical alignment have moved the ink.
pub struct TextEdit {
    content: String,
    style: TextStyle,
    spans: CharSpans,
    para_spans: ParaSpans,
    paragraph: ParagraphStyle,
    block: BlockStyle,
    sizing: TextSizing,
    /// The rail, carried for the whole session.
    ///
    /// **A session on bent type has to shape bent**, because everything this type
    /// answers — the caret, the selection highlight, the hit test — is measured
    /// against the layout it produces. Editing a node whose rail was dropped here
    /// would put the caret where the text used to be for as long as the session
    /// lasted, and put it back on commit.
    on_path: Option<BezPath>,
    /// And which way round it runs, for the same reason.
    on_path_flip: bool,
    /// And where along it the type starts, for the same reason again.
    on_path_offset: f64,
    shaped: Shaped,
    /// The bend, rebuilt beside [`Self::shaped`] (§15 D407).
    ///
    /// **Cached rather than asked of a layout, because this type never builds
    /// one.** Everything a session answers — the caret rectangle, the selection
    /// quads, and every pointer question through [`Self::to_layout`] — is computed
    /// against parley's own *flat* layout, so a session on railed type was drawing
    /// its caret and reading its clicks in a space the ink had left. Reported as
    /// the caret "editing the ghost of the old position", which is exactly what it
    /// was doing.
    ///
    /// ⚠️ **`TextLayout::warp` is the wrong door here even though it holds the same
    /// value.** Getting it means `extract`, which re-derives every glyph run and
    /// allocates them — a caret is asked for on every frame of a blink, and a drag
    /// asks a pointer question per frame on top.
    warp: Option<PathWarp>,
    selection: Selection,
    /// Attributes set on an empty caret, waiting for the next character typed.
    ///
    /// The one exception to the boundary rule (see `typography.rs`): asking for
    /// bold with nothing selected has to mean something, and the only thing it
    /// can mean is "the next thing I type". Cleared by any caret movement, so it
    /// cannot leak to a different place in the text.
    pending: Vec<crate::typography::CharAttr>,
    /// The x a vertical move aims at, in parley's layout space and **corrected**
    /// the way [`Self::caret_rect`] corrects it. Kept across consecutive Up/Downs
    /// so that walking through a short line does not pull the caret in with it.
    ///
    /// parley keeps one of these itself (`Selection::h_pos`) and it cannot be used:
    /// it is private, and it is seeded from `Cursor::geometry`, which measures in a
    /// frame that omits the line's start edge — see [`Self::move_line`].
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

    /// Rebuild the bend beside the shaping.
    ///
    /// **Called from every place that writes [`Self::shaped`]**, which is the whole
    /// discipline: the warp reads the shaped width and the first line's baseline,
    /// so a re-shape that left it alone would put the caret against the *previous*
    /// string's alignment — visible as the caret drifting a little further off the
    /// ink with every character typed on a centred rail.
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

    /// The paragraph overrides as editing has left them — re-based over every
    /// edit of the session, and committed in the same operation as the content
    /// for the reason [`crate::Operation::SetText`] gives.
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
    /// snapped outward to hard breaks ([`paragraph_bounds`]).
    ///
    /// **Read by the panel rather than recomputed there**, because a live session's
    /// content is a commit ahead of the document's — snapping the selection against
    /// the *document's* string would name the wrong paragraph the moment anything
    /// had been typed. Empty only for an empty node, where there is no paragraph to
    /// distinguish from the node and the panel writes the defaults instead.
    pub fn paragraph_range(&self) -> Range<usize> {
        paragraph_bounds(&self.content, self.selected_range())
    }

    /// Apply one paragraph attribute to the paragraph(s) the selection touches.
    ///
    /// The paragraph twin of [`Self::style_selection`], and deliberately with no
    /// `pending` counterpart: a caret is *in* a paragraph, so there is always
    /// something to write to and nothing to hold for the next character typed.
    pub fn style_paragraph(&mut self, attr: crate::typography::ParaAttr) {
        let range = self.paragraph_range();
        self.para_spans.set(range, attr, &self.paragraph);
        self.reshape_keeping_selection();
    }

    /// Nest or un-nest the list items the selection touches: `+1` for `Tab`, `-1`
    /// for `Shift`+`Tab`. Returns whether anything moved.
    ///
    /// **Per paragraph and relative, where [`Self::style_paragraph`] is one absolute
    /// value over the whole range.** That difference is the feature: select an item
    /// and the sublist under it, press `Tab`, and they must become levels 1 and 2
    /// rather than both becoming 1 — a uniform write flattens exactly the structure
    /// the gesture is for. Each paragraph is clamped to `0..=MAX_LIST_LEVEL`
    /// separately, so a selection with one item already at the floor still moves the
    /// rest.
    ///
    /// **Only paragraphs with a marker move.** A level indents any paragraph
    /// ([`ParagraphStyle::level`]), but `indent_start` is zero until something opens a
    /// gutter, so nesting a plain paragraph would usually do nothing visible at all —
    /// and the panel's own Level field is reachable only through an item, so letting
    /// the keyboard reach further would make a state the card cannot show. A run of
    /// items with a plain paragraph among them moves the items and leaves it.
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
            // The same clamped byte `Paragraphs::new` resolves this paragraph's style
            // at, so the level read here is the one the layout is using.
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
            // **`paragraph_bounds` for the write, not `start..next_start`** — it is
            // what makes the empty paragraph a trailing break leaves reach back over
            // that break, which is the only range a span can be set on at all there.
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
    /// ⚠️ **The bare-caret half has no caller in the app** (§15 D733,
    /// `[S6.2-L3-08]`). This function is `pending`'s only door and has exactly one
    /// production caller — `panels::typography::apply_char_attrs`'s
    /// `subject.partial` arm — and `TypeSubject::read` sets `partial` **only** for
    /// a **non-empty** selection. So `range.is_empty()` is unreachable from the
    /// panel: a bare caret takes the *"anything else"* route instead and writes
    /// the node's defaults.
    ///
    /// **The feature stays** — `architecture.md` §5.4's boundary-rule bullet
    /// designs it (*"typing at the edge of a span inherits from the character to
    /// the left, unless the caret was explicitly restyled while empty"*), the two
    /// tests below pin it,
    /// and it is what a caret-then-type gesture would need the day one is wired
    /// up. ⚠️ **§15 D217 is *not* the entry that designs it** — it is the
    /// clipboard entry, and it *depends* on the feature (`insert` consumes
    /// `pending` where a `cut` must not) rather than deciding it. A
    /// wrong-but-resolving citation, caught by `arch-scribe` while writing D733.
    /// What is not allowed to stay is prose elsewhere claiming it *is* wired
    /// up: `typography`'s module doc said so until 2026-09-10. **An unreachable
    /// arm is cheap; an unreachable arm the record calls live is what sends a
    /// reader looking for a mechanism that is not there.**
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

    /// Rewrite one attribute across the selection **run by run**, each run's new
    /// value computed from its own old one (§15 D752).
    ///
    /// 🚨 **[`Self::style_selection`] writes one value over everything, and for a
    /// list-valued attribute that is a flatten.** `CharAttr::Features` and
    /// `CharAttr::Variations` each carry a whole list, so a panel that wants to
    /// change *one tag* has to read a list, edit it and write it back — and over a
    /// selection whose runs disagree, the list it read came from the first run.
    /// Writing it back put the first run's every other setting onto all of them:
    /// toggle `liga` over a range half of which had `tnum` on, and the `tnum` was
    /// silently gone. `[S6.2-L1-05]`, and the maintainer's ruling was *"write only
    /// the tag you named and leave each run's other features alone."*
    ///
    /// **`f` sees each run's own value**, so the merge is the caller's and the
    /// boundaries are ours. A run that `f` leaves unchanged still gets written —
    /// `Spans::set` collapses a value equal to the node default rather than
    /// storing it, so this does not grow the span list, and the alternative is
    /// making every caller compare.
    ///
    /// ⚠️ **A bare caret does nothing here, deliberately.** `style_selection`'s
    /// empty-range arm holds the attribute as `pending` for the next character
    /// typed; there is no *run* to compute from at a point, and quietly falling
    /// back to the caret behaviour would make one function answer two questions.
    /// The panel's only caller is the partial arm, which is a non-empty selection
    /// by construction.
    pub fn style_selection_per_run(
        &mut self,
        kind: crate::typography::CharAttrKind,
        f: impl Fn(&crate::typography::CharAttr) -> crate::typography::CharAttr,
    ) {
        let range = self.selected_range();
        if range.is_empty() {
            return;
        }
        // Collected first: `runs_of` borrows the spans and the loop writes them.
        let runs = self.spans.runs_of(&self.style, kind, range);
        for (at, old) in runs {
            self.spans.set(at, f(&old), &self.style);
        }
        self.reshape_keeping_selection();
    }

    // --- editing ---------------------------------------------------------

    /// Replace the selection (or insert at the caret) with `s`.
    ///
    /// **Newlines are normalized on the way in**, which is what makes this the one
    /// door: parley maps `\r` and `\n` each to `Whitespace::Newline` *separately*, so a
    /// `\r\n` pair raises **two** explicit breaks and every line of pasted Windows text
    /// comes out double-spaced. Measured rather than assumed — `"a\r\nb"` shapes to the
    /// same three lines as `"a\nb\nc"`, which
    /// `a_crlf_pair_is_one_line_break_not_two` now pins.
    ///
    /// **Here and not in [`Self::new`]**: this is the *input* door, where a clipboard,
    /// a drop or a keystroke arrives, and normalizing what arrives is what every text
    /// editor does. `new` is handed the **document's** content, and rewriting that
    /// would silently put the editor and the document out of step.
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
        // A caret restyled while empty applies to exactly what was typed next.
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

    /// Everything a caret move invalidates, in one place so that adding a third
    /// thing does not mean finding eight call sites.
    ///
    /// A pending style belongs to the position the caret was at (see
    /// [`Self::pending`]), and the sticky vertical column belongs to the column it
    /// was in. [`Self::move_line`] is the one mover that must **not** call this,
    /// and says why.
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

    /// One line up or down, aiming at the column the caret is *visually* in.
    ///
    /// **Not `Selection::previous_line`/`next_line`, and for the same reason
    /// [`Self::caret_rect`] needs correcting — but worse.** parley seeds its sticky
    /// column from `Cursor::geometry`, which omits the line's start edge, and then
    /// lands the caret with `Cursor::from_point`, which reads it. The two frames
    /// differ by exactly that edge, so the landing point came out one indent to the
    /// left of where the caret visually was — *within* one indented paragraph as
    /// much as across two, and for an indent wider than the first word that means
    /// jammed against the line start on every press. The column has to be measured
    /// in the frame it is going to be spent in.
    ///
    /// The sticky column is ours for the same reason: `Selection::h_pos` is private
    /// and seeded from the wrong frame, so there is nothing to correct from outside.
    fn move_line(&mut self, delta: isize, extend: bool) {
        // **`pending` alone, deliberately not `caret_moved`.** The sticky column is
        // the one thing a vertical move must carry rather than clear — that is what
        // stops walking through a short line from pulling the caret in with it.
        self.pending.clear();
        let index = self.caret_line_index();
        let last = self.shaped.layout.len().saturating_sub(1);
        let x = self.sticky_x.unwrap_or_else(|| self.caret_x());
        self.sticky_x = Some(x);
        let target = index.checked_add_signed(delta).filter(|t| *t <= last);
        let Some(line) = target.and_then(|t| self.shaped.layout.get(t)) else {
            // Past either end. parley moves to the far end of the text rather than
            // leaving the caret where it is, and that is the behaviour to keep — the
            // column rides along, so coming back lands where it left.
            self.selection = if delta < 0 {
                self.selection.line_start(&self.shaped.layout, extend)
            } else {
                self.selection.line_end(&self.shaped.layout, extend)
            };
            return;
        };
        let m = line.metrics();
        // parley's own probe point for "inside this line", from `move_to_line`.
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
    /// **The inverse half of the one translation** (see [`YMap`]): every pointer
    /// question comes through here and every drawn y goes back out through
    /// `YMap`'s local direction, so the ink and the hit test agree about where a
    /// spaced, trimmed or vertically-aligned line is.
    ///
    /// **"Cannot disagree" is what this said, and for a while it was one
    /// qualification too strong.** The local direction has two faces — `to_local`
    /// for a baseline and `to_local_on_line` for the ink coordinates a *rectangle*
    /// is made of — and this one used to be neither, subtracting a shift from a
    /// coordinate that might be sitting in a gap where no line is. It now resolves
    /// the **line** first and asks `YMap` for that line's shift, so it is the
    /// tiling's own inverse; [`Shaped::layout_y`] has the whole of why, and what
    /// remains approximate is parley's (§15 D166).
    /// ⚠️ **The rail is un-bent *first*, and that ordering is the whole of it**
    /// (§15 D407). On a railed node the local space the pointer arrives in is the
    /// bent one, so `PathWarp::nearest` maps it back to the flat plane parley
    /// reasons in; only then does `YMap`'s own inverse run, because paragraph
    /// spacing, trim and vertical alignment are all *flat* offsets that the bend
    /// was applied on top of. Doing the two the other way round asks `layout_y` a
    /// question about a `y` that has been through a curve.
    fn to_layout(&self, p: Point) -> (f32, f32) {
        let p = match &self.warp {
            Some(w) => {
                let (x, y) = w.nearest(p);
                // Into the lap the text is on — see `shortest_way_round`.
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
    /// **Shifted by the line's own start edge, which parley's caret geometry
    /// leaves out.** `parley::Cursor::geometry` measures from
    /// `Cluster::visual_offset`, which sums `line.metrics.offset` and the cluster
    /// advances and never reads `inline_min_coord` — the per-line start edge a
    /// paragraph indent lives in (§15 D163). Everything else in parley that has an
    /// opinion about x *does* read it: `Selection::geometry` adds it, so the
    /// highlight was right; `Cursor::from_point` adds it, so the hit test was
    /// right; and the glyph runs are positioned with it, so the ink was right. The
    /// caret alone drew in an un-indented frame.
    ///
    /// **A regression that arrived with per-paragraph measure rather than a gap in
    /// it.** Before D163 the indent went in through `Layout::set_text_indent`,
    /// which lands in `line.indent` and thence in `metrics.offset` — which
    /// `visual_offset` *does* include — so the caret followed a first-line indent
    /// for free and stopped when the amount moved to `line_x`. Reported from use as
    /// the cursor sitting where the text used to be.
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
    /// By the caret's own `y0`, which *is* that line's `block_min_coord` —
    /// `cursor_rect` copies it in without arithmetic. Spelled out here because the
    /// `Layout::line_for_offset` parley uses for the same job is `pub(crate)`.
    ///
    /// **The last line at or above the caret, not the first line containing it**,
    /// and that distinction is a bug this had: `block_min_coord..block_max_coord`
    /// spans ascent to descent, so the line boxes **overlap** whenever the line
    /// height is tighter than the two together — which is the ordinary case, Inter
    /// at 20pt being 24.4 of type in a 20pt box. A containment search then returns
    /// the line *above* the one that owns the caret, and Up from the last line of
    /// three landed on the first. Testing one monotonic edge has no such ambiguity.
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
    /// canvas draws (§15 D407).
    ///
    /// **The bent counterpart of [`Self::caret_rect`], and the one every caller
    /// should reach for.** A caret on a rail is a short segment across the curve,
    /// not an axis-aligned rectangle, so a rect cannot express it and the canvas
    /// drew one at the flat position for a day — the reported "cursor shows on the
    /// original position of the text, before it was set to follow the path".
    /// Ordinary horizontal type comes back as exactly one quad, the corners of the
    /// rectangle this used to be.
    pub fn caret_quads(&self, width: f64) -> Vec<[Point; 4]> {
        self.warped(self.caret_rect(width))
    }

    /// The selection highlight as convex quads in text-local space. Empty when the
    /// selection is just a caret. [`Self::caret_quads`]'s twin, for the same
    /// reason.
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

    /// Highlight rectangles for the selected range, in text-local space. Empty
    /// when the selection is just a caret.
    ///
    /// ⚠️ **Flat, always** — [`Self::selection_quads`] is what follows a rail, and
    /// is what a canvas wants. This stays because the vertical-movement arithmetic
    /// above reasons in parley's own space and would have to un-bend to use the
    /// other.
    pub fn selection_rects(&self) -> Vec<Rect> {
        self.selection
            .geometry(&self.shaped.layout)
            .into_iter()
            .map(|(b, line)| self.to_local_rect(b, line))
            .collect()
    }

    /// A parley rectangle in text-local space.
    ///
    /// **The line has to be named**, not inferred from the y: a rectangle's edges
    /// are *ink* coordinates and [`YMap::shift_for_line`] has the whole of why
    /// deciding a paragraph's spacing from one of those is wrong.
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

    /// The selected text, empty when the selection is a bare caret — the whole of
    /// what a copy takes.
    ///
    /// A slice rather than a `String`, so the caller decides whether it needs to
    /// own one. Indexing cannot panic: the range comes from the same selection the
    /// content was shaped with, and every mover goes through parley's cluster
    /// boundaries.
    pub fn selected_text(&self) -> &str {
        &self.content[self.selected_range()]
    }

    /// Remove the selection and hand it back — the model half of a cut.
    ///
    /// `None` for a bare caret, which is what lets a caller dim its *Cut* row
    /// rather than offer one that silently does nothing, and what stops a cut with
    /// no selection putting an empty string on the system clipboard.
    ///
    /// **[`Self::replace`], not `insert("")`.** The two differ in exactly one way
    /// that matters here: `insert` takes [`Self::pending`], because a pending
    /// style is waiting for *the next thing typed*. A cut is not that, so it must
    /// not consume it — the sequence "ask for bold, then cut the word before it"
    /// has to leave the bold still pending.
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
        // The column the caret was aiming at described a layout that no longer
        // exists. Also true of `reshape_keeping_selection`, where the *lines* may
        // have moved even though the bytes did not.
        self.sticky_x = None;
    }

    /// Re-shape after a change that moved no text, **keeping the selection**.
    ///
    /// [`Self::reshape`]'s counterpart, and the distinction is the whole point:
    /// `from_byte_index` builds a *collapsed* caret, so re-shaping through it throws
    /// any selection away. That is right after an edit — the text the selection
    /// covered is gone — and wrong after a **restyle**, where every byte is still
    /// there and the user is still pointing at it.
    ///
    /// Reported as "selecting a portion of the text and applying a property
    /// deselects the text", with the consequence that made it worth reporting: you
    /// do not notice, so the *next* property lands on the whole node. Both callers
    /// are restyles ([`Self::style_selection`] and [`Self::restyle`]), which is why
    /// applying any character attribute from the panel dropped the selection.
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

/// `s` with every `\r\n` pair and every lone `\r` collapsed to a single `\n`, or `None`
/// when it holds no `\r` and is already its own answer.
///
/// The `None` is not an optimisation for its own sake: [`TextEdit::insert`] is called
/// once per character typed, and the overwhelmingly common case is one keystroke with
/// nothing to rewrite. Handing back "nothing to do" keeps that case allocation-free.
///
/// A lone `\r` becomes a newline rather than being dropped, because it **is** a break —
/// parley already treats it as one, and classic-Mac and some serial sources still
/// produce them.
fn normalized_newlines(s: &str) -> Option<String> {
    if !s.contains('\r') {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            // The `\n` of a `\r\n` pair is consumed, so the pair is one break.
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
/// The owned twin of [`TextRef`], for the two callers that cannot borrow from a
/// `NodeKind`: [`TextEdit`], which outlives the document read that produced it,
/// and the app's default-style factory.
#[derive(Clone, Debug, PartialEq)]
pub struct TextParts {
    pub style: TextStyle,
    pub spans: CharSpans,
    pub para_spans: ParaSpans,
    pub paragraph: ParagraphStyle,
    pub block: BlockStyle,
    pub sizing: TextSizing,
    /// The rail, owned. See [`TextRef::on_path`].
    ///
    /// **Owned here even though a live edit session never changes it**, because
    /// the alternative is a `TextParts` that lays out flat while the node it was
    /// taken from is bent — and this is exactly what a `TextEdit` session shapes
    /// through, so the caret and the selection highlight would be measured
    /// against the wrong geometry for as long as the session lasted.
    pub on_path: Option<BezPath>,
    /// And which way round it runs. See [`TextRef::on_path_flip`].
    pub on_path_flip: bool,
    /// And where along it the type starts. See [`TextRef::on_path_offset`].
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
    //! ⚠️ **Names in this module's prose are written in plain backticks, never as
    //! `[links]`** (§15 D319). `cargo doc` builds without the `test` cfg, so this
    //! module is simply absent from what rustdoc reads: an intra-doc link here is
    //! decoration **no gate can validate**, and it rots silently when the item it
    //! names is renamed or deleted. Swept 2026-09-15 — 26 links over 21 targets,
    //! every one of which was resolved by hand first, all converted.
    //!
    //! **Re-check after a session that writes a lot of test prose**: grep the added
    //! doc-comment lines of your own diff for an opening bracket-backtick, then ask
    //! of each hit whether its item, or the module around it, is `cfg(test)`.
    //! `CLAUDE.md` carries the command under *Build / test*.

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

    /// **Styling a selection must not deselect it.** Reported from use: apply a
    /// colour to a selected word, the selection silently vanishes, and the next
    /// colour you pick lands on the *whole node* — which is the half that makes it
    /// worth a test rather than a nicety.
    ///
    /// The cause was one shared helper: `reshape` rebuilds the selection with
    /// `Selection::from_byte_index`, which is a collapsed caret. Right after an edit,
    /// where the selected text is gone; wrong after a restyle, where every byte is
    /// still there.
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

        // And a *second* attribute still lands on the same range rather than on
        // everything, which is the reported consequence.
        edit.style_selection(CharAttr::Size(30.0));
        assert_eq!(edit.selected_range(), before);
    }

    /// **An auto-height box is as wide as it was authored, not as wide as the ink.**
    /// The inspector's `W` field reads this box — `query::local_box` → `local_bounds`
    /// → here — so this is the field's claim to be showing the user's own number back
    /// to them rather than a measurement of the glyphs. That claim is the reason
    /// auto-height was the worse half of the size row the panel would not draw: the
    /// width is authored state, and nothing named it.
    ///
    /// **Both directions, because one sample cannot tell an authored width from a
    /// shaped one.** At 400 the box is wider than the ink and at 30 it is narrower
    /// than a single word, so a `box_of` answering `content_width` — the shape of the
    /// plausible wrong version, since that is what `Auto` gets one line above it —
    /// lands on neither number. The `Auto` case below is the control: it *does* track
    /// the ink, so the assertion above is about the mode rather than about `measure`
    /// handing its own input back.
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

    /// **A span at byte 0 leaked its style onto every run that merely inherited.**
    /// Found while building `export_lines`, and it reaches the *canvas*, because
    /// `extract` resolved a run's style the same way: through the brush parley
    /// reports, read as an index into the style runs. A run whose pushed properties
    /// all equal the node's defaults resolves to the **default's** brush, which
    /// `shape` pushes as `0` — so it claimed to be style run 0 and took style run
    /// 0's baseline shift.
    ///
    /// It hid because it is only wrong when style run 0 is not itself the defaults:
    /// with the span anywhere but the start, index 0 *is* the plain style and the
    /// wrong answer is accidentally the right one. Every existing per-run test put
    /// its span in the middle.
    ///
    /// A shift on "a" alone must move "a" alone. `style_at` resolves by location.
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
        // Exactly one run is lifted; the rest sit on the unshifted baseline.
        let lifted = ys.iter().filter(|y| **y < ys[ys.len() - 1] - 1.0).count();
        assert_eq!(
            lifted, 1,
            "only the shifted run may move — every run took the shift: {ys:?}"
        );
    }

    // --- export_lines -----------------------------------------------------

    /// **A wrap becomes lines, which is the half `--svg` could never express.**
    /// The writer emits one `<text>` per line at its own baseline; the cached
    /// `TextLayout` has no line structure at all, so this is the seam that gives it
    /// one. Both lines' text together must be the content, and the second must sit
    /// lower than the first.
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

    /// **The truncation ellipsis is on the canvas and was absent from the export**
    /// (`[S5.2-L1-03]`, §15 D506).
    ///
    /// `extract` appends an ellipsis run when `shaped.truncated && overflow ==
    /// Ellipsis`; `export_lines`, the exporter's own walk of the same shaped
    /// layout, had **no ellipsis logic of any kind** and never read
    /// `shaped.truncated`. So the drawing read *"one two three four five"* where
    /// the canvas read *"one two three four five …"*, with nothing anywhere
    /// admitting the loss — the writer side has no `TextOverflow` handling either,
    /// so it was not made up downstream.
    ///
    /// The two walks were built to agree: `ListMarkers`' own doc says a marker
    /// *"that draws on the canvas and is missing from the export is exactly what a
    /// second path invites"*, and `ListMarkers` exists so the two cannot disagree
    /// about markers. The ellipsis is the same class of node-level ink and got no
    /// such treatment.
    ///
    /// **`Clip` is the control**, and it is the one that matters: the ellipsis is
    /// conditional on the overflow setting, not on truncation, so a version that
    /// appended it whenever the node was truncated would pass a `truncated`-only
    /// assertion.
    ///
    /// ⚠️ **Asserted against the canvas rather than against `"…"`**, so the claim
    /// is *the two walks agree* rather than *this walk emits a character*: the
    /// canvas's own run count is read from `extract` on the same `TextParts`.
    ///
    /// **Flip run**, the append removed: fails on *"the export ends in the
    /// ellipsis the canvas drew"* with the text ending in `"five "` — the
    /// predicted site.
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

    /// **A styled span comes back as its own run, carrying its own style.** This is
    /// what a `<tspan>` needs and what one `<text>` per node cannot say: the writer
    /// reads `style` off each run rather than the node's defaults.
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

    /// **The ranges parley reports index the *cased* string, not `content`.** So a
    /// run's text has to come out transformed, exactly as the canvas drew it —
    /// slicing the node's own content by those ranges is what would silently
    /// mis-slice, and on a run whose case changes the length it would panic or cut a
    /// character in half.
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

    /// The picker's row for a family that *is* registered: real glyphs, a real
    /// width, and a baseline inside the line box.
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

    /// **The preview declines rather than lying.** An unregistered family shapes
    /// perfectly well — parley falls back to Inter — so the tempting
    /// implementation returns a row that reads "Bungee Shade" set in Inter, about
    /// a typeface the user is choosing by eye.
    #[test]
    fn an_unregistered_family_has_no_preview_rather_than_a_fallback_one() {
        assert!(
            family_preview("No Such Family At All", "No Such Family At All", 15.0).is_none(),
            "a family with no faces must not preview in the fallback face"
        );
    }

    /// The icon-font case, and the reason the `.notdef` guard exists. The family
    /// is registered and shapes without complaint — it just has nothing to draw
    /// these characters with, so what comes back is a row of empty boxes. A
    /// picker full of those reads as a broken list.
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

    /// The eviction contract the app's preview cache depends on (§5.4a): a
    /// preview-only family can be un-registered, and re-registered later when the
    /// user scrolls back to it.
    ///
    /// **Fontique keeps the family *name* after the last face is removed**, which
    /// is why `is_family_available` asks about faces — asking about the name
    /// answered "yes" for a family that could no longer draw anything, and the
    /// picker would then have drawn a row in the fallback face believing it was
    /// the real one.
    ///
    /// Inter is the subject only because it is the one family core bundles; the
    /// test puts it back before it ends, so nothing downstream is left without a
    /// fallback face (tests share a thread under `--test-threads=1`).
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
            // The old fixture's 1.0 line height, so the height arithmetic below
            // stays about the font size rather than about Inter's metrics.
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

    /// **A family the machine does not have measures the same as the fallback it
    /// is actually shaped in** (§15 D711, `[S5.1-L1-06]`).
    ///
    /// The module doc promises *"unknown families fall back to Inter"* and the
    /// **face** did — `push_style` names the fallback as the second entry of the
    /// family list. The **axes** did not, so an unknown family shaped Inter at
    /// its `opsz` default of 14 where `"Inter"` shapes it at the clamped 32.
    /// Measured in release at 96pt: 726.609 against 670.500, **8.4% wider** —
    /// and it snaps to the narrower one the moment the family becomes available,
    /// which is a reflow of a document nobody edited.
    ///
    /// ⚠️ **96pt, because the defect is optical sizing and `opsz` is what a
    /// large size changes.** At 20pt the clamped value is near the default and
    /// the two spellings measure almost the same, so a fixture at body size
    /// passes whichever way the code goes.
    ///
    /// ⚠️ **The width is compared against `"Inter"` rather than to a literal**,
    /// for the reason the neighbouring tests give: a number here is a
    /// measurement of the bundled face's version, and the claim is that the two
    /// *agree*.
    ///
    /// ⚠️ **Flip, run:** restoring `axes_for`'s `.unwrap_or(&[])` — so an
    /// unknown family contributes no axes — fails the second assertion at
    /// **780.5625 against 719.625**, 8.5% wider. Those are *debug* numbers and
    /// the finding's 726.609/670.500 are release ones; the absolute widths
    /// differ between profiles and the **ratio** does not, which is why the
    /// assertion compares the two spellings rather than either against a
    /// literal. The first assertion is the control and stays green under the
    /// flip, so the test is about the fallback rather than about shaping at all.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
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

    /// **`decoration_sizes` reports what the face is drawing, and it is not zero**
    /// (`[S6.3-L1-01]`, §15 D570).
    ///
    /// The panel's thickness chip seeds from this, and it seeded `Px(0.0)` before
    /// this field existed — which erases the line, `decoration_path` and
    /// `crossings` each refusing a band of zero height. So "not zero" is the whole
    /// contract, and the second half of it is that the number **scales with the
    /// type**: a constant would be a metric in name only.
    ///
    /// ⚠️ **Inter's two numbers are equal**, so this test cannot tell the underline
    /// metric from the strikethrough one and does not claim to. They are two fields
    /// because `run.metrics()` reports two, not because a measurement here separated
    /// them — the one bundled face happens to give `underline_size ==
    /// strikethrough_size`, and a test asserting they differ would be asserting a
    /// fact about Inter.
    ///
    /// **Flip run**, `decoration_sizes` set to `None` unconditionally in `extract`:
    /// fails on *"a shaped node reports the face's own thicknesses"*.
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

    /// **The number the chip seeds draws ink, and zero does not** — the two halves
    /// of `[S6.3-L1-01]` in one assertion pair, at the layer the panel cannot see.
    ///
    /// This is what makes the seed a *fix* rather than a different arbitrary value:
    /// `decoration_ink` builds the band as `top..top + size`, and a zero `size` is a
    /// zero-height rectangle that `render::scene::decoration_path` and `crossings`
    /// each open by refusing.
    ///
    /// **Flip run**, the assertion's `Px(seed)` replaced by `Px(0.0)`: fails on *"the
    /// seeded thickness draws a band"* — which is the old behaviour, stated as a
    /// test.
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
        // The control: the seed is what "the font decides" was already worth, so
        // leaving auto has to change the *unit* and not the band.
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
        // Em(1.0) line height: each line box is exactly font_size tall.
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
        // The reason % is the default unit: tracking has to survive a type
        // token being used at two sizes.
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
        // Inter's own line height is a little over its em, and in any case not
        // either of the two above.
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

    /// How much of each glyph's own advance keeps its underline, given a laid-out
    /// single-run node — the shared fixture for the skip-ink tests below.
    ///
    /// Per glyph rather than per band because a *total* proves almost nothing here:
    /// a rule that skipped a fixed 20% of every band would match one, and the
    /// question skip-ink actually answers is **which letters**.
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

    /// **The band breaks under the descenders and nowhere else.** Skip-ink, the
    /// feature (§15 D356): "gypsy" at 40px is `g y p s y`, four of whose five
    /// letters put ink through an underline and one of which does not.
    ///
    /// The measured fractions of each letter's own advance that keep their rule,
    /// Inter at 40px: **g 0.03, y 0.35, p 0.62, s 0.99, y 0.37.** The `p` is the
    /// interesting one and the reason this asserts per glyph rather than per band —
    /// its bowl is above the band, so only the stem breaks the rule and most of the
    /// letter keeps it. A gap "under the p" would have been three times too wide and
    /// no total would have said so.
    ///
    /// The thresholds are deliberately loose either side of that: *some* break for a
    /// descender, *almost none* for the `s`, and *most of it left* for the `p`. The
    /// exact fractions are a function of `SKIP_CLEARANCE`, which is a value to be
    /// tuned by eye, and a test that pinned them would go red for a tuning rather
    /// than for a defect.
    ///
    /// ⚠️ Flipped by returning early from `skip_ink`: fails at the `g`, the first
    /// descender, which is where it was predicted.
    ///
    /// ⚠️ **Two flips that do *not* bite, and both are findings rather than failed
    /// experiments.**
    ///
    /// Taking each edge's whole x range instead of only the part inside the band's
    /// rows — the correctness `crossings` spends four lines on — moves `g` from 0.03
    /// to 0.00 and `y` from 0.353 to 0.347, and leaves `p` and `s` untouched. The
    /// reason is `SKIP_TOLERANCE`: flattening at an eighth of a unit leaves edges too
    /// short for the clip to have much to remove, and only a *straight* edge survives
    /// flattening long enough to matter — Inter's `y` tail, worth 0.7% of a letter.
    /// So the clip is not what makes this test pass; what it buys is that the answer
    /// does not depend on the tolerance, which is worth keeping and is *not* what this
    /// test proves.
    ///
    /// Dropping `flatten_edges`' `ClosePath` arm changes nothing at all here. Of the
    /// seven contours in `gypsy` exactly one leaves a non-degenerate closing edge, and
    /// its x range is inside a gap other edges had already opened. Inter is a `glyf`
    /// face and TrueType contours are cyclic — the final segment back to the start is
    /// emitted like any other — so the arm is there for a CFF face, where the close
    /// really is implicit, and **no bundled font can test it**.
    ///
    /// Which leaves the `p` assertion below with no one-line flip that reaches it: it
    /// is aimed at a *re-implementation* — per glyph, or per contour, which is the
    /// shape this was nearly written as — and says in one number why that shape is
    /// wrong.
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
        // **And the p keeps most of its rule**, which is the assertion that says
        // this is a contour test and not a bounding-box one: its bowl is above the
        // band and only the stem crosses. Skipping the glyph, or the whole x range
        // of any contour that touches the band, answers 0.0 here.
        assert!(
            kept[2] > 0.4,
            "only the p's stem crosses the band, not its bowl: kept {kept:?}"
        );
        // And what is left is still an underline rather than four crumbs.
        let total: f64 = tl.decorations[0].gaps.iter().map(|(a, b)| b - a).sum();
        assert!(
            total < tl.decorations[0].band.width() * 0.75,
            "gaps {:?} of a band {:?}",
            tl.decorations[0].gaps,
            tl.decorations[0].band
        );
    }

    /// **A strikethrough is never skipped**, which is css-text-decor-4's rule and
    /// not an omission: `line-through` is *meant* to cross the letters.
    ///
    /// The same word as the test above, so the difference is the decoration and
    /// nothing else — and its band sits in the middle of the x-height, where every
    /// one of those five letters has ink. If this ever answers a gap it will answer
    /// five.
    ///
    /// ⚠️ Flipped by pushing the strikethrough's index into `extract`'s `underlines`
    /// beside the underline's: this fails with **six** gaps over five letters — the
    /// `s` alone breaks the band twice, which is what a strikethrough would look
    /// like if this rule were ever "improved" into symmetry with the underline.
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

    /// **An underline told not to skip keeps one unbroken bar** — the toggle
    /// (§15 D357), on the same word as the test two above, so the only difference
    /// between a band with four gaps and a band with none is `Decoration::skip_ink`.
    ///
    /// The control comes first and is the whole reason this is not vacuous: `gypsy`
    /// with the flag left alone has to answer *some* gap, or the second half is
    /// asserting that nothing happened in a fixture where nothing was going to.
    ///
    /// **The second arm is the one worth having**, and it is about *which* underline
    /// is asked. Skip-ink is a per-run setting resolved through the spans like every
    /// other character attribute, so a span over `gy` that switches it off leaves the
    /// run beside it still breaking — which is the assertion that goes red if the flag
    /// is ever read off the node's own `TextStyle` instead of the run's resolved one.
    /// That is a plausible slip rather than a hypothetical: `extract` has both in
    /// scope on the line that pushes the index, one named `parts.style` and one
    /// `style`.
    ///
    /// ⚠️ **Two flips, and they bite in two different places — which is the argument
    /// for both arms being here.** Pushing the index unconditionally, the whole of
    /// the toggle, fails on the first arm's `gaps` assertion with four gaps over
    /// `gypsy`. Reading `parts.style.underline` for the flag instead of the run's
    /// fails **only** on the span arm's first assertion, `gy` coming back with two
    /// gaps: the whole-node arm sets the flag *on the node's own default*, so a
    /// version that reads that default answers it correctly and arm one stays green.
    /// So the first arm cannot tell the two implementations apart and the second one
    /// can, which is what a per-run setting needs a per-run fixture to say. Both
    /// sites were predicted; the second was predicted wrongly first — as the
    /// control — and this is what running it said.
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

        // The span arm: the node default skips, `gy` does not.
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

    /// **A word with nothing below the baseline keeps one unbroken rule** — the
    /// case that stops skip-ink being a bug of its own.
    ///
    /// This is what the horizontal-only clearance buys. The obvious spelling grows
    /// the band vertically too, and a round letter's overshoot dips *below* the
    /// baseline: get that wrong and every `o`, `e` and `c` breaks its own underline,
    /// which is a worse drawing than the one skip-ink was built to fix.
    ///
    /// **The margin, measured rather than assumed** (Inter, and proportional so the
    /// type size drops out): `onmax`'s deepest ink is 0.47px below a baseline whose
    /// band starts 4.06px below it, at a thickness of 2.73 — so the clear air
    /// between the ink and the band is **1.31 thicknesses**.
    ///
    /// ⚠️ **The obvious flip does not bite, and that is the finding.** Padding the
    /// band up and down by a whole thickness in `crossings` leaves this test green,
    /// because 1.0 < 1.31; at 1.4 it fails, and it fails under the `o` and the `a`
    /// (`[(6.4, 15.5), (82.6, 89.9)]`), which is the drawing this paragraph is
    /// describing. So the test does not prove the rule has to be horizontal-only —
    /// what it pins is the *margin*, and the margin is what says the choice is
    /// face-dependent rather than free: Inter would spend 70% of it to buy nothing,
    /// and a face whose `underlinePosition` is half as deep would spend all of it.
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
        // The fixture is in the state the assertion is about: the deepest ink really
        // is above the band, by the margin quoted above.
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

    /// **A wavy underline skips what its *crests* reach, not what its band does.**
    ///
    /// `scene::wave` swings a thickness either side of the band's middle, so its ink
    /// is three thicknesses tall where the band is one. `ink_extent` is the one place
    /// in core that knows that number, and this is the test that the knowing is worth
    /// something: over the same word, the wavy band's gaps must cover strictly more
    /// than the solid band's, and everything the solid one broke.
    ///
    /// ⚠️ Flipped by giving the `Wavy` arm of `ink_extent` the band's own `0.5t`
    /// reach: the two gap lists come out *identical*, and the first assertion fails.
    ///
    /// ⚠️ **Strict containment used to be the second assertion and is no longer true.**
    /// It held while air was a fixed dilation of every crossing; now air is rationed by
    /// the bar it eats into (`give_air`), and a wavy band's crossings are taller and
    /// therefore wider, so its bars are narrower and can spare *less* air. Measured,
    /// the wavy gap that answers the solid `23.62..35.59` starts at `23.99` — 0.37
    /// units inside it. So this asserts overlap and a bigger total instead, which is
    /// the honest reading of "the crests break more of the band", and the paragraph
    /// stays because the reason containment fell is worth more than the assertion was.
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

    /// The bars of rule an underlined `word` keeps at `size`, in order — the
    /// complement `scene::kept_spans` will take, which is what actually gets painted.
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

    /// **Air narrows a bar and never removes one.** The whole of `give_air`, in
    /// widths measured in thicknesses so the numbers are scale-free, at 40px:
    ///
    /// | word | bars |
    /// | --- | --- |
    /// | `page` | 0.90, 12.86, 8.07 |
    /// | `gypsy` | 1.00, 1.00, 2.65, 11.85, 2.74 |
    /// | `g` | 1.00, 1.00 |
    /// | `j` | 0.95 |
    ///
    /// Three things to read off it. A long bar keeps its full clearance and is
    /// untouched — `page`'s 12.86 and 8.07, which is why ordinary copy is unaffected
    /// by any of this. A bar the clearance would erase is **held at one thickness**
    /// instead: every 1.00 in the table is a bar against that floor, and `g`'s two
    /// marks either side of its tail are the case the whole rework is for. And a bar
    /// that is *already* narrower than the floor gets no air at all rather than being
    /// padded out to it — `page`'s leading 0.90 and the `j`'s 0.95 are their own raw
    /// bearings, untouched.
    ///
    /// ⚠️ Flipped by removing the floor (passing `0.0`) — the drawing this replaced —
    /// and the **predicted site was wrong for the third time in the same direction**:
    /// the `g` was going to lose both its bars, and it does, but `page` fails first,
    /// because its leading 0.90 bar is eaten whole and the list comes back
    /// `[35.18, 22.06]`. Three flips now, three times the plain word has caught a
    /// change to the air before the interesting word got its turn. The lesson is
    /// filed rather than re-learnt: *put the boring fixture first and expect it to be
    /// the one that fails.*
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

    /// ⚠️ **A stem's crossing is the ink between its two sides, and one edge cannot
    /// say that.** A `p`'s descender is bounded by two *vertical* edges, whose
    /// x-extent inside the band is zero — so measuring crossings edge by edge misses
    /// the stem entirely, which is what the three scanlines in `raw_crossings` are
    /// for.
    ///
    /// This is a real defect that sat in the tree behind the clearance: dilating each
    /// edge by a whole thickness made the two zero-width spans overlap and cover the
    /// stem *by accident*, so it only surfaced when the structure began to be decided
    /// at **zero** clearance and every `p` in the document stopped breaking its
    /// underline.
    ///
    /// ⚠️ Flipped by deleting the scanline loop: the `p` comes back with **no gap at
    /// all** and the first assertion fails. The other half of the old bug — a stem
    /// wider than twice the clearance, which would have had the rule drawn straight
    /// through it — **is not reachable with Inter at any weight**: measured at 200px
    /// and weight 900, the stem is 22.1 units against a 50.8-unit dilation, so it is
    /// recorded here rather than tested.
    #[test]
    fn a_stems_crossing_is_the_ink_between_its_sides() {
        let mut s = style(40.0);
        s.underline = Some(Decoration::default());
        let tl = layout_with("p", &s, &TextSizing::Auto);
        let ink = &tl.decorations[0];
        assert_eq!(ink.gaps.len(), 1, "the p's stem breaks the band: {ink:?}");
        let (a, b) = ink.gaps[0];
        let t = ink.band.height();
        // Two vertical sides a stem apart, plus a thickness of air each side. Wider
        // than the air alone is what says the ink between them was measured.
        assert!(
            b - a > 2.0 * t,
            "the gap must cover the stem and not just the air: {} against {t}",
            b - a
        );
        // And it is one gap rather than two, which is what an edge-by-edge
        // measurement would leave once the sides stopped overlapping.
        assert!(
            fragments(&ink.gaps, ink.band).len() == 2,
            "one gap, two bars: {:?}",
            fragments(&ink.gaps, ink.band)
        );
    }

    /// **The rule never vanishes, at any size.** That is the invariant the tiers in
    /// `crossings` exist for, and the one the user asked for: a lone `j` shows
    /// something. Eleven sizes from 6pt to 400 × five pathological words, none of
    /// which kept a single bar before the tiers were written.
    ///
    /// **And the drawing keeps its width as the size changes**, which is the second
    /// half of what the floor buys and the reason the floor is a thickness.
    ///
    /// ⚠️ **This is the third statement of that claim and the first true one.** It was
    /// first written as "the tiers are scale-invariant, because every quantity
    /// compared is linear in the size". That was wrong: parley rounds glyph advances
    /// *and* Inter's `opsz` axis changes the letterform outright, so a bearing
    /// measured in thicknesses really does drift — a lone `j`'s runs from about 2.0
    /// thicknesses at 6pt to 1.07 at 400, and an underlined `g` used to lose one of
    /// its two marks at 15 and 16pt as a bearing crossed the threshold.
    ///
    /// What makes the *drawing* steady is not the geometry but the floor: whatever the
    /// bearing does, air stops being taken at one thickness. So the `j`'s mark
    /// measures **1.00t from 6pt to 24pt** and 0.94–0.95t above that, the second range
    /// being where the raw bearing has itself fallen below the floor and simply keeps
    /// its own width. A width that is steady *because a rule holds it there* is a
    /// different claim from one that is steady because the inputs are proportional,
    /// and it is the one worth pinning: it survives a face whose bearings behave
    /// nothing like Inter's.
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
            // The one-letter case, where the mark's width is held by the floor rather
            // than by anything about the letter.
            let (j, t) = bars_of("j", size);
            assert_eq!(j.len(), 1, "one mark at {size}pt: {j:?}");
            let ratio = j[0] / t;
            assert!(
                (0.93..=1.01).contains(&ratio),
                "the j's mark is about a thickness wide at {size}pt: {ratio}"
            );
            // And `g` keeps both of its, which is the case this rework exists for and
            // the one that used to vanish at 15 and 16pt.
            let (g, _) = bars_of("g", size);
            assert_eq!(g.len(), 2, "two marks on a g at {size}pt: {g:?}");
        }
    }

    /// **Every band gets its own gaps, and line 19's rule is not broken by line 18's
    /// descenders.** Also the fixture the cost table in `skip_ink`'s doc is measured
    /// on: one paragraph, wrapped, every line underlined.
    ///
    /// This is what the per-glyph bounding-box reject is *for*, and that reject is an
    /// optimisation with a wrong answer available to it — a glyph rejected for the
    /// wrong band loses a gap, and a glyph let through by a box that is too tall puts
    /// one where there is no ink. Neither can show up on the single-line fixtures
    /// above, because there is only one band there to be wrong about.
    ///
    /// ⚠️ **It takes *both* y tests removed to break it, and that is the finding.**
    /// Dropping the `gy` half of the box reject alone leaves this green, because the
    /// per-edge test in `crossings` catches the other lines' edges one level down;
    /// disabling that per-edge test alone leaves it green too, because the box reject
    /// caught the glyph before any of its edges were read. With both gone the first
    /// band loses its whole width — `293.8 of 293.8` — which is the drawing this test
    /// is named for. So the two are redundant *for correctness* on this fixture, and
    /// only one of them is redundant for speed (the table in `skip_ink`).
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
            // Each band's gaps came from *its own* line: the whole paragraph's
            // descenders are far more ink than one line's band can lose.
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
        // Find a glyph on the second line: its y is greater.
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
        // `align_when_overflowing` defaults to false in parley, which
        // start-aligns an overflowing line — reads as the control breaking.
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

    /// **The middle setting of a three-value control does the middle thing**
    /// (`[S5.2-L6-05]`, §15 D497).
    ///
    /// ⚠️ **`JustifyLast::Center` was executed by no test in the workspace.** The
    /// Type panel paints all three values (`panels/typography.rs`, iterating
    /// `JustifyLast::ALL`) and writes the chosen one straight into a
    /// `SetParagraphStyle`, so it ships; the only other `Center` in the tree is a
    /// serialization round-trip that lays nothing out. Replacing `Center => free /
    /// 2.0` with `Center => free` — shipping a control whose middle setting
    /// silently does what its right-hand one does — left the whole workspace green.
    ///
    /// **Asserted as an ordering plus a midpoint**, which needs no magic number:
    /// `start < centre < end`, and `centre` within a point of `(start + end) / 2`.
    /// The ordering alone would pass a `Center` arm returning `free * 0.9`.
    ///
    /// **Both callers of `justify_shift`, because there are two and a divergence
    /// between them would be invisible**: `extract`, which is the canvas, and
    /// `export_lines`, which is the SVG walk. They measure different things —
    /// glyph `x` against `ExportLine::x` — so the assertion is made twice rather
    /// than shared.
    ///
    /// **Flip run**, `Center => free`: fails on *"centre is short of end"*, the
    /// predicted site, in the `extract` half — the first of the two, which is why
    /// the export half is asserted after rather than before.
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

        // The SVG walk's own copy of the same shift.
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

    /// **A fixed box told to clip stops at the line that would overflow it**
    /// (`[S5.2-L6-04]`, §15 D500).
    ///
    /// ⚠️ **The whole height-limit half of `break_lines` was exercised by
    /// nothing.** `height_limit` is `Some` only for `TextSizing::Fixed` paired
    /// with `Clip` or `Ellipsis`, and no test in the workspace built that pair
    /// with content that overflows: the only overflow-carrying test is
    /// `ellipsis_adds_a_glyph_where_clip_does_not`, which uses
    /// `AutoHeight(60.0)` and reaches truncation through `max_lines` instead, and
    /// the only other `Clip`/`Ellipsis` in a test is a serialization round-trip of
    /// the field. So `breaker.revert()`, the `truncated = true` that arms the
    /// ellipsis, and the *"never revert the first line"* rule were all asserted by
    /// nothing. `let too_tall = false && …` left the whole workspace green.
    ///
    /// `Visible` is the control and it is the right one: the same box and the same
    /// content, differing only in the setting under test, so the assertion is
    /// about the limit rather than about the fixture being small.
    ///
    /// **Flip run**, `too_tall` forced to `false`: fails on *"and `Clip` says it
    /// stopped"*. **The predicted site was the line-count assertion below it and
    /// that was wrong** — `truncated` is checked first, and it is the earlier
    /// symptom: the flag arms the ellipsis, so a wrong answer there is visible
    /// before any line is counted.
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

    /// **A box shorter than one line still shows that line** (`[S5.2-L6-04]`,
    /// §15 D500) — the `&& lines > 1` clause beside the revert.
    ///
    /// Its own comment names what dropping it costs: *"an empty layout would lose
    /// the caret and the node's identity with it"*, which is a node that cannot be
    /// selected back out of. One clause, defended by nothing until now.
    ///
    /// **Two flips run and both bite here, at the same assertion and from
    /// opposite sides** — which is what says this fixture is on the boundary
    /// rather than merely near it. `&& lines > 1` removed: **0** baselines against
    /// 1, the node emptied. `too_tall` forced to `false`: **3** against 1, the
    /// limit ignored altogether. The `truncated` assertion beside it survives the
    /// first flip, a layout that reverted its only line being truthfully
    /// truncated — **the flag is not the interesting half; the surviving line
    /// is.**
    #[test]
    fn a_box_shorter_than_one_line_still_shows_that_line() {
        let d = style(10.0);
        let mut p = parts(&d, &TextSizing::Fixed(Size::new(120.0, 4.0)));
        p.block.overflow = TextOverflow::Clip;
        // ⚠️ **The content has to overflow the *width* too.** A first attempt used
        // a string that fits on one line at this measure, so the breaker never
        // reached a second line, never reverted, and reported `truncated` false —
        // the fixture asserting nothing about the clause it names.
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

    /// **An ellipsis is set in the type it follows, not in the type it replaced**
    /// (§15 D490, `[S5.2-L1-02]`).
    ///
    /// `shaped.runs` is a range table in ascending byte order over the **whole**
    /// content, so `.last()` answered with the style at the end of the *string* —
    /// which, on a truncated node, is precisely the text that is not drawn. A
    /// character span anywhere in the invisible tail therefore restyled the
    /// ellipsis: a `Size(40.0)` span past the one visible line put a **40pt `…` in
    /// a 12.1-unit box**, moved 16.5 units left.
    ///
    /// **The control is the same node with no span**, and it is what makes the
    /// assertion mean "unchanged" rather than "10.0 by coincidence". The **box** is
    /// asserted to be the same in both runs, which is what makes the size mean
    /// something visible: it is measured from the drawn line and does not move, so
    /// the 40pt ellipsis was not a large box with a large glyph in it — it was a
    /// glyph four times the height of the line it sat on.
    ///
    /// ⚠️ **That sentence claimed a box assertion this test did not make**, until
    /// `arch-scribe` read the two against each other: the closure returned a font
    /// size and nothing else. **Test prose is the one class of drift no gate in
    /// this project can see** (§15 D319), and it was committed by the session that
    /// had spent the day on that class. Written rather than struck, because the
    /// claim was worth making — and the first attempt at writing it asserted
    /// `font_size < box_height`, which ordinary type does not satisfy.
    ///
    /// ⚠️ **This region had exactly one assertion with teeth before now, and it
    /// counts glyphs.** The review flipped the style source to `parts.style` — the
    /// node default, ignoring every span — and the whole workspace stayed green;
    /// the neighbouring test above passed because *"is there an ellipsis at all"*
    /// is all it asks. **Nothing asserted what the ellipsis looks like or where it
    /// goes.**
    ///
    /// ⚠️ **Flip-check, run: the lookup back to `shaped.runs.last()`.** Fails on the
    /// font size at 40 against 10 — the predicted site.
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
        // ⚠️ **The box is what makes the size assertion mean something visible.**
        // It is measured from the *drawn* line and does not move — 10 units under
        // both spans — so the 40pt ellipsis was not a large box with a large glyph
        // in it, it was a glyph four times the height of the line it sat on.
        //
        // ⚠️ **This was written as `font_size < box_h` first, and that is not an
        // invariant.** A 10pt font in a 10-unit line box fails it, correctly: an em
        // is not a line box, and the assertion would have been demanding something
        // ordinary type does not satisfy. What is real is that the box is *the
        // same* in both runs, which is what says the difference is entirely in the
        // ellipsis.
        assert!(
            (styled_box - box_h).abs() < 0.01,
            "the box moved from {box_h} to {styled_box}, so the truncated tail is \
             reaching the layout as well as the ellipsis"
        );
    }

    /// **An ellipsis never sits over the text it ends** (§15 D610, `[S5.2-L1-01]`).
    ///
    /// 🚨 **The clamp kept the glyph in the box and said nothing about the ink.**
    /// `ellipsis_run` placed itself at `x.min(w - advance)`, where `x` is the last
    /// visible line's right edge — and under `Center` and `End` that edge reaches
    /// the box edge *by construction*, so the `min` always bit and pushed the `…`
    /// back over the last characters by its own advance. Measured in release on
    /// this very fixture: body glyphs at or past the ellipsis origin were **0**
    /// for `Start`, **1** for `Center`, **2** for `End`. The clamp's own comment
    /// states the aim — *"past the wrap width it would be the one glyph hanging
    /// out of a node that was truncated precisely to fit"* — which is a claim
    /// about the **box** while what fails is the relationship to the **ink**.
    ///
    /// **`Start` is the passing control and it is in the same assertion loop**,
    /// which is what stops this reading as "ellipses are fine now": it was already
    /// correct, its line ends short of the box, and the fix must leave it alone.
    ///
    /// ⚠️ **The region had no assertion about placement in either direction.** The
    /// review replaced the whole clamp with `let at = x;` and the entire workspace
    /// stayed green — `ellipsis_adds_a_glyph_where_clip_does_not` counts glyphs and
    /// reads no position, no size and no colour, and
    /// `an_ellipsis_is_set_in_the_type_it_follows` reads the size.
    ///
    /// **Flip-check, run**, against the plausible wrong version rather than
    /// against nothing — the old clamp, restored as `at = x.min(w - advance)` with
    /// the line shift removed. It fails at `Center` first with *"1 of the line's
    /// glyphs are at or past the ellipsis"*, and at `End` with 2. ⚠️ **The
    /// predicted site was `End`**, on the reasoning that it overlaps by the most;
    /// the loop reaches `Center` first and both are wrong by the same rule, so the
    /// order of the fixtures decided it and not the size of the defect.
    ///
    /// ⚠️ **The second assertion is what stops the fix being "let it hang".**
    /// Dropping the clamp entirely also passes the overlap test and puts the glyph
    /// outside the wrap width — where a `Fixed` box clips it away and the node's
    /// own bounds, measured from `shaped.size`, do not contain it. Both halves are
    /// asserted, so neither one-sided answer is green.
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

            // The ellipsis is the run `extract` appends last; everything before it
            // is the line's own ink.
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

            // And the clamp's own aim, which the fix must not trade away.
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
        // The whole point of `TextLayout::origin`: toggling trim must not shift
        // a text node in any saved file.
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

    /// **The line box comes back beside the trimmed one, and contains it.**
    ///
    /// This is what the canvas draws as a dashed second outline once trim is on by
    /// default, so the leading the trim removed is visible rather than merely
    /// absent. Two things have to hold for that drawing to be honest: the untrimmed
    /// box has to be the box the node *would* have had — which is
    /// `trimmed + trim_top + trim_bottom`, not the trimmed one — and the trimmed box
    /// has to sit inside it. The plausible wrong version is `untrimmed: size` for the
    /// `Auto` arm, i.e. forgetting to add the insets back, and it makes
    /// `untrimmed_bounds` silently equal `bounds` so the canvas draws nothing at
    /// all — a bug that looks exactly like "the feature is off".
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
        // It is the box the node would have had untrimmed — measured against the
        // node that really is untrimmed, rather than against arithmetic on itself.
        assert!(
            (line.height() - plain.size.height).abs() < 1e-9,
            "line box {} should be the untrimmed height {}",
            line.height(),
            plain.size.height
        );
        assert_eq!(line.y0, 0.0, "the line box starts at the local origin");
        // And the tight box is inside it, which is what makes one a padding of the
        // other rather than two rectangles that happen to be near each other.
        assert!(
            line.contains_rect(tl.bounds()),
            "{:?} does not contain {:?}",
            line,
            tl.bounds()
        );
        assert!(tl.bounds().y0 > line.y0, "trimmed from the top");
        assert!(tl.bounds().y1 < line.y1, "and from the bottom");
    }

    /// **Nothing to draw when trim changed nothing** — the equality the canvas tests
    /// instead of asking what the trim setting is.
    ///
    /// Both arms matter and for different reasons. An untrimmed node is the obvious
    /// one. A `Fixed` node is the one that would go wrong: its trim setting is live
    /// and `trim_insets` really does return a non-zero pair for it, but the box is
    /// the one the user typed and trim only moves the vertical-alignment datum
    /// (§15 D78) — so reporting a line box here would draw a dashed rectangle round
    /// the node's *content*, which is a different statement about a different thing.
    #[test]
    fn a_node_whose_box_trim_did_not_change_reports_no_line_box() {
        let mut d = style(40.0);
        d.line_height = None;

        // Trim off, box emergent: the line box *is* the box.
        let none = layout_with("Hxy", &d, &TextSizing::Auto);
        assert_eq!(none.untrimmed_bounds(), none.bounds());

        // Trim on, box authored. `Fixed` is deliberately smaller than the line box
        // here, so a leak would be visible as a *larger* rectangle rather than
        // hidden by the two happening to match.
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
        // There is no free space to distribute, so all three must agree —
        // otherwise the control appears to do something and does not.
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

    /// The runs a layout holds, as `(first glyph x, glyph ids)` — enough to tell a
    /// marker from the text it precedes and to say where it sits.
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

    /// **A marker is ink and not content**, which is the whole of why lists cost so
    /// little here (§15 D169). Three claims in one, because they are one decision:
    /// the content is untouched, the marker draws to the *left* of the text column,
    /// and the byte space the caret lives in has not moved.
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
        // The text sits at the block indent; the marker's right edge lands on it, so
        // the marker starts to the left of that and after zero.
        let (text_x, _) = runs[0];
        let (marker_x, ref ids) = runs[1];
        assert_eq!(text_x, 30.0, "the text is at `indent_start`");
        assert!(
            marker_x < text_x && marker_x > 0.0,
            "the marker sits in the gutter, not on the text and not off the box: {runs:?}"
        );
        assert_eq!(ids.as_slice(), &[1462], "Inter's U+2022 (see ListMarker)");
    }

    /// **`1.` and `10.` put their text in the same column, and their markers do
    /// not** — the reason the marker is right-aligned rather than laid out in flow.
    #[test]
    fn ordered_markers_are_right_aligned_so_the_text_column_holds() {
        let d = style(20.0);
        // Nine items so the tenth is two digits.
        let content = (1..=10).map(|_| "x").collect::<Vec<_>>().join("\n");
        let e = TextEdit::new(&content, list_parts(&d, ListMarker::Decimal));
        let tl = e.text_layout();
        // Ten text runs then ten markers, in that order — `extract` appends the
        // markers after the line walk.
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

    /// The numbering rule, which is the only thing standing in for markup: a run of
    /// items is bounded by the first neighbour that asks for something else.
    ///
    /// **Both boundaries, because they are not the same code path** — the first
    /// version of this test asserted only the no-marker one and passed against a
    /// `list_ordinals` that never restarted on a *changed* marker, since a
    /// marker-less paragraph resets the run through the other arm.
    #[test]
    fn numbering_restarts_where_the_marker_changes_or_stops() {
        let d = style(20.0);
        // Five paragraphs; the third is the one that interrupts.
        let content = "a\nb\nc\nd\ne";
        let third = 4..5;

        // The marker glyphs of each item, in paragraph order.
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

        // Stopping: the third paragraph is not an item at all.
        let m = markers(ParaAttr::Marker(None));
        assert_eq!(m.len(), 4, "four items, the third opted out: {m:?}");
        assert_ne!(m[0], m[1], "1. is not 2.");
        assert_eq!(m[0], m[2], "the run restarts after the gap");
        assert_eq!(m[1], m[3]);

        // Changing: the third paragraph is an item, of a different list.
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

    /// **A marker sits on its paragraph's own baseline**, which is what makes
    /// paragraph spacing, box trim and vertical alignment all follow for free rather
    /// than each needing a term (§15 D169).
    ///
    /// `docs/roadmap.md` expected `trim_insets` to want work once markers existed. It does
    /// not, and this is why: trim folds the *face metrics* of the first and last
    /// line's runs, and a marker is set in the same face at the same size as the text
    /// it sits beside — so it adds no metric that was not already in the fold. That
    /// held only because the marker stopped being a parley line item; under the
    /// inline-box design it would have entered `line.runs()` and the guess would have
    /// been right.
    #[test]
    fn a_marker_shares_its_paragraphs_baseline_through_the_y_map() {
        let d = style(20.0);
        let mut p = list_parts(&d, ListMarker::Decimal);
        // Both of the things that move text after the lines are broken, at once: a
        // 40pt gap between paragraphs and a trim that tightens the box.
        p.paragraph.spacing = Length::Px(40.0);
        p.block.trim = BoxTrim::CapToBaseline;
        let tl = TextEdit::new("a\nb\nc", p).text_layout();
        // Three text runs then three markers, appended after the line walk.
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
        // And the gap really is in those numbers, so this is not passing on a
        // degenerate layout where every baseline is the same.
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
        // Bijective base 26 — 26 is `z` and 27 is `aa`, not `a` again.
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

    /// One paragraph of one letter per entry in `levels`, each at the level given.
    ///
    /// The span is anchored on each paragraph's **first byte**, which is the byte
    /// `Paragraphs::new` resolves that paragraph's style at — a span covering some
    /// other byte inside it sets a value nothing reads.
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

    /// Every string the exporter emits, in order — markers and text interleaved.
    ///
    /// **The readable form of the numbering, and the second path that reads it.**
    /// `run_starts` can only compare one marker's glyphs against another's; this says
    /// `1.` outright. It also puts these assertions on `export_lines` rather than on
    /// the cached layout, which is the path §15 D169 had to keep in step by hand.
    ///
    /// A text run carries its own hard break, so the expected vectors below read
    /// `"a\n"` — the marker before each line, then the line, newline included.
    fn export_texts(parts: TextRef<'_>) -> Vec<String> {
        export_lines(parts)
            .into_iter()
            .flat_map(|l| l.runs.into_iter().map(|r| r.text))
            .collect()
    }

    /// **A level steps the start edge by one more gutter, and takes the marker with
    /// it.** Both halves, because they are two readers of one number
    /// (`ParagraphStyle::start_edge`) and the plausible wrong version — the multiply
    /// in `line_geometry` alone — moves the text and leaves the marker in the outer
    /// gutter, which is the one thing a nested list must not look like.
    #[test]
    fn a_nested_item_puts_its_text_and_its_marker_one_gutter_further_in() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 1]);
        let tl = TextEdit::new(&content, p).text_layout();
        let runs = run_starts(&tl);
        assert_eq!(runs.len(), 4, "two items and two markers: {runs:?}");
        let (text, markers) = runs.split_at(2);
        // `list_parts` sets a 30px `indent_start`, so the levels are 30 and 60.
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

    /// **A nested run counts from one and its parent counts through it** — the rule
    /// `<ol>` gets from being a container and this gets from a stack of counters.
    ///
    /// Two claims, and the second is the one a flat run cannot express: item `d`
    /// follows two nested items and is still `2.`, because nothing between it and `a`
    /// touched level 0's count.
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

    /// A level opened deeper than the one above it — level 2 under level 0 — leaves
    /// level 1 with **no** count of its own, so the first level-1 item is `1.` and not
    /// `2.`.
    ///
    /// Worth a test of its own because the tidier implementation is wrong here: filling
    /// the skipped levels with the same `(marker, 1)` the deep one gets makes level 1
    /// look like a run already under way, and its first item numbers itself second.
    #[test]
    fn a_level_skipped_on_the_way_in_lends_no_count_to_one_arriving_later() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 2, 1]);
        let texts = export_texts(p.as_ref(&content));
        assert_eq!(texts, vec!["1.", "a\n", "1.", "b\n", "1.", "c"]);
    }

    /// **A paragraph that is not an item ends every list, at every level** — the rule
    /// level 0 has always followed, now read over the stack.
    ///
    /// The discriminating item is `d`: if the break cleared only its own level and
    /// deeper, level 0's counter would survive it and `d` would be `2.`.
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

    /// The same list at two levels is two lists, even asking for the same marker.
    ///
    /// The level test the marker rule cannot cover: `numbering_restarts_where_the_marker_changes_or_stops`
    /// bounds a run by a *different* marker, and this bounds one by the same marker at
    /// a different depth.
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

    /// **`Tab` shifts each item by one and keeps the shape of the selection.** The
    /// wrong version — one absolute value over the paragraph range, which is what
    /// `style_paragraph` does — flattens a parent and its sublist onto the same level,
    /// which is exactly the structure the gesture exists to build.
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

    /// **A caret moves its own paragraph and nothing else**, which is the common case:
    /// no selection, one item, one press.
    #[test]
    fn tab_with_a_bare_caret_nests_the_paragraph_it_sits_in() {
        let d = style(20.0);
        let (content, p) = levelled(&d, ListMarker::Decimal, &[0, 0, 0]);
        let mut e = TextEdit::new(&content, p);
        // The caret opens at the end of the content, so one line up is the middle
        // paragraph of "a\nb\nc". Asserted, because a test whose fixture is not in the
        // state it claims is a test about nothing.
        e.move_up(false);
        assert_eq!(
            paragraphs_touched(e.content(), e.selected_range()),
            1..2,
            "the caret is in the second paragraph"
        );
        assert!(e.step_list_level(1));
        assert_eq!(levels(&e), vec![0, 1, 0]);
    }

    /// The floor and the ceiling are **per paragraph**, so a selection with one item
    /// already at the bound still moves the rest — and `Shift`+`Tab` at level 0 is a
    /// no-op that says so, which is what lets the caller leave the document alone.
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

    /// **Only items move.** A level indents any paragraph, but `indent_start` is zero
    /// until something opens a gutter, so nesting a plain paragraph is usually
    /// invisible — and the panel's Level field is reachable only through an item, so a
    /// keyboard that reached further would make a state the card cannot show.
    #[test]
    fn tab_leaves_a_paragraph_that_is_not_a_list_item_alone() {
        let d = style(20.0);
        let (content, mut p) = levelled(&d, ListMarker::Decimal, &[0, 0, 0]);
        let flat = p.paragraph.clone();
        // The middle paragraph opts out of the marker, mid-run.
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

    /// A text node with no list at all is untouched, and the caller is told so — which
    /// is what lets `Tab` be claimed in a session without leaving an undo step behind.
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

    /// **The empty paragraph a trailing break leaves can be nested**, which is the case
    /// the whole index detour exists for: it owns no bytes, so the only range a span
    /// can be set on is the break byte *before* it — the same byte the paragraph above
    /// ends on. Writing by byte overlap would move both.
    ///
    /// It is also the paragraph a user is most likely to press `Tab` on: Enter at the
    /// end of an item, then `Tab` to start the sublist before typing it.
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

    /// The tripwire between the two mechanisms: `paragraphs_touched` answers in
    /// indices what `paragraph_bounds` answers in bytes, and a selection where they
    /// disagreed by accident would nest a paragraph the panel would not have styled.
    ///
    /// **They disagree in exactly one place, on purpose, and that is why the helper
    /// exists.** Every selection over a fourteen-byte, four-paragraph string agrees —
    /// except a caret in the empty paragraph after the trailing break, where the byte
    /// span reaches *back* over the break and so reads as the paragraph above it. This
    /// test found that by failing rather than by being written for it.
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
                // The byte span's own paragraphs, read back the same way.
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

    /// The baseline of the line holding the `n`th glyph run, in local space.
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
        // The §6 round trip, and the reason the y map is one function used in
        // both directions.
        //
        // **The middle paragraph, not the last.** A click aimed at the *last*
        // line passes with the inverse map removed, because a y below the
        // layout's own height clamps to the final line anyway — a test that
        // passes for the wrong reason. Aiming at the middle discriminates: with
        // no map, local y 58 is past the unspaced layout's 30 and clamps to the
        // third paragraph.
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

    /// **A paragraph gap belongs to the two lines that bound it and to no other**,
    /// which is what `Shaped::layout_y` resolving the line first buys and what
    /// subtracting a shift from the coordinate could not (§15 D166).
    ///
    /// The reported symptom was the last 2.2pt of it — a click in the negative
    /// leading above a spaced paragraph's first line landing on the line below —
    /// and sweeping the whole gap showed that to be the tail of a 40pt fault. Both
    /// halves are asserted here, because the old arithmetic passed neither: with
    /// three 20pt lines 40pt apart it put local y 22.5 through 40 on line 1 while
    /// the pointer was in empty space 20pt above it, local y 42.5 through 57.5 on
    /// line **2**, and local 57.5 — inside line 1's own ascender ink — on line 2
    /// as well.
    #[test]
    fn a_click_in_a_paragraph_gap_lands_on_a_line_that_bounds_it() {
        let mut p = parts(
            &TextStyle {
                font_size: 20.0,
                line_height: Some(Length::Em(1.0)),
                ..style(20.0)
            },
            // Narrow, so the second paragraph wraps and there is a *third* line for
            // a gap click to wrongly reach. One line each would hide the fault
            // behind parley's own clamp to the last line.
            &TextSizing::Fixed(Size::new(60.0, 400.0)),
        );
        p.paragraph.spacing = Length::Px(40.0);
        assert_eq!(
            TextEdit::new("a\nbb bb bb", p.clone()).shaped.layout.len(),
            3,
            "one line, then two wrapped"
        );
        // Line boxes are 20 tall and tile layout space; the 40pt gap opens above
        // line 1, so in local space line 0 is 0..20, the gap is 20..60, line 1 is
        // 60..80 and line 2 is 80..100.
        let line_at = |y: f64| {
            let mut c = TextEdit::new("a\nbb bb bb", p.clone());
            c.click(Point::new(1.0, y));
            c.caret_line_index()
        };
        for (y, want) in [
            (10.0, 0),
            // The gap, splitting at its middle: above goes to the line it hangs
            // under, below to the line it sits over. Never line 2, which the old
            // arithmetic reached by walking a whole gap's worth forward.
            (25.0, 0),
            (40.0, 0),
            (42.5, 1),
            (57.5, 1),
            // The reported 2.2pt: line 1's own ink starts at local 58, two above
            // its box.
            (59.0, 1),
            (70.0, 1),
            (90.0, 2),
        ] {
            assert_eq!(line_at(y), want, "local y {y} should land on line {want}");
        }
    }

    #[test]
    fn a_click_in_a_bottom_aligned_box_lands_on_the_line_under_the_pointer() {
        // Two lines, and the click aimed at the *first* — for the same reason as
        // above: aiming at the last line clamps and would pass without the map.
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
        // ß→SS would break every byte index downstream of it, so the character
        // is left as typed. The measured width proves it was not transformed.
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

    /// **One baseline per line, ascending, and a shifted run does not invent
    /// one** (§15 D355).
    ///
    /// The second half is the whole reason `TextLayout::baselines` is a stored
    /// field rather than a read. `roadmap.md` proposed deriving snap targets from
    /// `Glyph::y`, on that field's own doc saying it sits on the baseline — which
    /// is true of an unshifted run and false of a shifted one, since
    /// `baseline_shift` is a draw offset applied to the glyphs and deliberately
    /// *not* to the line. So the obvious derivation reports a phantom baseline
    /// wherever a superscript is, and reports it as confidently as a real one.
    ///
    /// ⚠️ The fixture puts the shift on the **second** line, so the two halves
    /// cannot pass for each other: a phantom would land between the real
    /// baselines rather than at an end, where a `len()` check alone might still
    /// look right.
    ///
    /// ⚠️ Flipped by filling `baselines` from the glyphs —
    /// `runs.flat_map(|r| r.glyphs).map(|g| g.y)`, sorted and deduplicated —
    /// which is the implementation the roadmap described. It fails on the
    /// **count**, at 3 against 2, and the values say the rest: `[17.0, 29.0,
    /// 37.0]` for a two-line node. 17 and 37 are the real baselines; **29 is the
    /// `2` lifted eight points off the second one**, reported with exactly the
    /// confidence of the other two. A snap target there would pull a shape onto a
    /// line that does not exist.
    #[test]
    fn every_line_has_one_baseline_and_a_shifted_run_does_not_add_another() {
        let d = style(20.0);
        // "x2y" on line two, with the `2` lifted 8px — the shifted glyph is in
        // the middle of the node, not at either end of it.
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

        // And the shifted glyph really is off its line's baseline, or the case
        // this test is about did not occur in the fixture.
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

    /// **An empty node has no baselines**, which is the arm a `[0]` would panic
    /// on and the one a snap-target reader hits first on a text node the user has
    /// just created and not typed into.
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

    /// **The variant list has to reach the family's second file.**
    ///
    /// Inter ships as two variable faces and names its italic cuts in the italic
    /// one's `fvar`; neither face carries an `ital` axis, so nothing in the
    /// coordinates distinguishes them. Reading only the default face therefore
    /// offered nine upright cuts and no italic — and because the panel replaces
    /// its bold/italic pair with this list as soon as the list has two entries,
    /// that left **no control anywhere in the app** that could italicize the one
    /// family the app always has.
    #[test]
    fn inters_variant_list_offers_both_slopes() {
        let variants = family_variants("Inter");
        let names: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();
        for want in ["Regular", "Bold", "Italic", "Bold Italic"] {
            assert!(names.contains(&want), "no {want:?} in {names:?}");
        }
        // The pair that matters is one weight in both slopes: that is what the
        // dropdown could not express.
        for italic in [false, true] {
            assert!(
                variants
                    .iter()
                    .any(|v| v.weight == 700 && v.italic == italic),
                "no bold at italic={italic} in {variants:?}"
            );
        }
    }

    /// 🚨 **The arm serving nearly every *static* system font, asserted for the
    /// first time** (§15 D608, `[S5.1-L6-05]`).
    ///
    /// `family_variants` returns early whenever the family has named instances, and
    /// core registers no system fonts (`Engine::new`, `system_fonts: false`), so the
    /// only families it ever holds under test are the two **variable** Inter files —
    /// and every fixture in the tree, `inters_variant_list_offers_both_slopes` above
    /// included, therefore takes the other arm. The static arm's sort, its dedup and
    /// all nine of `face_name`'s weight buckets could be changed with every gate
    /// green. G9.
    ///
    /// ⚠️ **This tests the two *decisions*, not the parley query above them** —
    /// `ordered_cuts`'s doc says which half is still on trust and why (there is no
    /// registered static face to reach it with). Asserting a decision that was
    /// unreachable is worth more than leaving the whole arm dark, and pretending
    /// otherwise is what the finding is about.
    ///
    /// Flip-check, run, both at the one order assertion — the fixture's comment
    /// below is the part worth reading, because the first version of it could not
    /// see the sort flip at all:
    ///
    /// - the sort tuple reversed to `(v.weight, v.italic)` gives
    ///   `[(400, false), (400, true), (700, false)]` — the light italic interleaved
    ///   between the two uprights;
    /// - the `dedup_by` disabled gives `[(400, false), (700, false), (700, false),
    ///   (400, true)]` — the doubled cut offered twice.
    #[test]
    fn a_static_family_offers_one_variant_per_cut_in_weight_order() {
        let cut = |weight: u16, italic: bool| FontVariant {
            name: face_name(weight, italic),
            weight,
            italic,
            coords: Vec::new(),
        };
        // Deliberately out of order, with one cut registered twice — which is what
        // a family shipping the same face under two files looks like.
        //
        // ⚠️ **A *light italic* is what makes the sort order visible, and the first
        // draft of this fixture had none.** With only a 400-upright and a
        // 700-anything, `(italic, weight)` and `(weight, italic)` produce the same
        // list, so the flip that reverses the tuple was **green** — the fixture
        // could not tell the shipped rule from the wrong one. The italic has to
        // sort *before* an upright on weight and *after* it on slope, or the two
        // keys agree and the test is about nothing.
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

    /// `face_name`'s nine weight buckets and its one special case, none of which
    /// had a test caller (§15 D608, `[S5.1-L6-05]`) — it has exactly one
    /// production caller and that is the static arm above.
    ///
    /// **The boundaries, not the middles.** Every bucket is a range, and a range is
    /// only wrong at its ends: `149`/`150` and `749`/`750` are where an off-by-one
    /// in the table shows up, and a test sampling `100, 200, 300…` would miss it.
    ///
    /// ⚠️ **`("Regular", true)` is *"Italic"*, not *"Regular Italic"*** — the one
    /// case that is not the pattern, and the one a rewrite would flatten.
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

    /// **`[S5.1-L1-01]`'s loss: a font can simply contain a reversed axis
    /// range, and `f64::clamp` is an `assert!`.**
    ///
    /// A `.ttf` whose `fvar` `opsz` record carries `minValue = 200` against
    /// Inter's own `maxValue = 32` registers cleanly — `skrifa` returns the raw
    /// Fixed values and fontique's parse is not a filter — and the next
    /// `text::layout` of *any* text node panicked with *"min > max, or either
    /// was NaN"*, **in release as well as in debug**. The guard on that path is
    /// `optical_size_is_auto`, which is the default, so every ordinary text node
    /// was armed the moment such a family was registered; and it fires through
    /// the library's cover rasterizer too, which is the seam the loader
    /// Criticals also land on.
    ///
    /// ⚠️ **Two assertions for two different failures.** A reversed pair is what
    /// was measured; a `NaN` end is the same panic by another road, and `f64::min`
    /// and `f64::max` *swallow* `NaN` silently — so ordering alone would have
    /// left the second one open and looked complete.
    ///
    /// The default is pulled inside the range too, since it is the value an axis
    /// sits at when nothing else says, and `fvar` does not promise it is between
    /// the ends any more than it promises the ends are in order.
    ///
    /// ⚠️ **Built through `from_fvar` rather than by patching a real font in
    /// memory**, which is how the finding measured it: the patch proves the file
    /// gets through fontique, and this proves what happens after. The two halves
    /// belong to different layers and only this one is cheap enough to keep.
    ///
    /// **Two flips, both run, and both were mis-predicted in the same
    /// direction.** Returning `(a, b)` unordered was expected to fail at *"the
    /// ends come back in order"*; it fails one line earlier, inside `from_fvar`
    /// itself, with `min > max, or either was NaN. min = 200.0, max = 32.0` — the
    /// finding's own panic, verbatim, because the constructor pulls the default
    /// inside the range and reaches `f64::clamp` before this test sees anything.
    /// Routing the `NaN` arms through `f64::min`/`f64::max` fails the same way at
    /// `min = NaN, max = NaN`, which is the measurement behind the note above:
    /// those two functions return `NaN` for a pair of them rather than filtering
    /// it out. ⚠️ **Better failure messages than the ones predicted, and by
    /// accident** — the constructor is a second reader of its own range, so the
    /// guard is load-bearing one line before anything asserts on it.
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

        // The default is brought inside too — `fvar` does not promise it is
        // between the ends any more than it promises the ends are in order.
        assert_eq!(axis(20.0, 30.0).default, 20.0);
        // And an ordinary axis is untouched, which is what says none of this
        // changed the normal answer.
        let ok = axis(8.0, 100.0);
        assert_eq!((ok.min, ok.default, ok.max), (8.0, 14.0, 100.0));
        assert_eq!(ok.clamp(16.0), 16.0);
    }

    #[test]
    fn inter_offers_features_including_tabular_figures() {
        // Read off the face, not off a table of ours — which is the point, and
        // which this test found: Inter ships no `liga` at all (its default
        // ligatures ride `calt`), so a hardcoded checkbox list would have offered
        // a switch that shapes to nothing.
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

    /// **A `ssXX`/`cvXX` row is named by the font, and this is the read that
    /// proves it.** Bundled Inter labels all eight of its stylistic sets and all
    /// fourteen of its character variants in its `name` table, so the panel can
    /// say "Open digits" and "Alternate one" where it used to say `ss01` and
    /// `cv01` — and `cv01`–`cv14` had no name at all before, since the old table
    /// stopped at `ss10`.
    ///
    /// Three separate things are asserted because three separate things can
    /// break:
    ///
    /// - the strings come through at all (the `FeatureParams` → `name` chain);
    /// - **no other tag gets one**, which is the gate on the two font-defined
    ///   ranges. Without it read-fonts parses `ssty` as a stylistic set, because
    ///   it dispatches on the first two bytes of the tag;
    /// - **the tooltips stay `None`**. Inter leaves them NULL, and a NULL name id
    ///   is `0` — the copyright notice, which every font has. So a missing
    ///   `feature_string` range check does not show *nothing*, it shows the
    ///   foundry's copyright on hover.
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

    /// **What `Auto` is worth, which is the whole reason the field could not say.**
    /// A style with no `line_height` takes the face's own metrics, so the number is
    /// only knowable from a shaped layout — and an authored one has to come back
    /// exactly, or a panel seeded from it would move the ink it was reading.
    #[test]
    fn a_layout_reports_the_line_height_its_first_line_actually_got() {
        // **`line_height: None`, which the fixture's `style` does not give**: it
        // sets `Em(1.0)` so the other height arithmetic is about the font size, and
        // a first version of this test read that back as 20.0 and called it Inter's
        // leading. `MetricsRelative(1.0)` is the thing being measured here.
        let mut s = style(20.0);
        s.line_height = None;
        let auto = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!(
            auto > 20.0 && auto < 30.0,
            "Inter's own leading on 20px, not the em: {auto}"
        );
        // Authored, and exact — this is the number the panel writes back.
        let mut s = style(20.0);
        s.line_height = Some(crate::typography::Length::Em(2.0));
        let authored = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!((authored - 40.0).abs() < 1e-9, "200% of 20px: {authored}");
        // **Not the block coordinates**, which measure the line's ink and would
        // report less than the box (§15 D147). A 400% line box is far taller than
        // two characters of ink, so the two answers cannot be confused here.
        s.line_height = Some(crate::typography::Length::Em(4.0));
        let tall = layout_with("Hg", &s, &TextSizing::Auto).line_height;
        assert!((tall - 80.0).abs() < 1e-9, "400% of 20px: {tall}");
    }

    #[test]
    fn tabular_figures_make_every_digit_the_same_width() {
        // The single highest-value feature in the list, and the one worth an
        // assertion: proportional 1 is narrow, tabular 1 is not.
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

    /// **The `opsz` rule, asserted on the function that runs it** (`[S6.1-L3-04]`,
    /// §15 D571).
    ///
    /// The axis is handed in by hand here, which is what makes the three ends of
    /// the range explicit and the **Manual** arm assertable at all. Three arms:
    ///
    /// - **Auto** synthesizes the font size, clamped to the axis. A 96pt heading on
    ///   an `8..=32` axis shapes at 32, not at 96, and not at the axis default.
    /// - **Manual** passes the stored coordinate through **raw**, out of range
    ///   included — the disagreement between the two implementations, settled in
    ///   `axis_settings`' own doc.
    /// - **No such axis**, and nothing is synthesized at all.
    ///
    /// ⚠️ **The clamp asymmetry is the point of the second and third assertions
    /// together.** A reader who saw only the Auto arm would "fix" the Manual one to
    /// match it, and nothing else in the workspace would object: the shaper clamps
    /// either way, so no pixel moves — what moves is the file, and the panel's
    /// readout, which §15 D425 says must show a stored value rather than rewrite it.
    ///
    /// **Flip run**, `axis.clamp(style.font_size)` replaced by `style.font_size`:
    /// fails on *"Auto shapes at the axis it has, not at the size it was asked
    /// for"*, `96` against `32`. ⚠️ **Predicted at the *Manual* assertion and wrong
    /// again** — the reasoning was that a raw 96 would also be the value a
    /// pass-through Manual arm produced, which confused the two arms: they read
    /// different inputs and only the Auto one touches the clamp.
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

    #[test]
    fn optical_size_auto_reaches_the_shaper() {
        // 🚨 **This said "Inter has no `opsz`" and that is false** (§15 D711
        // corrected D571, which is where both copies of the sentence came from).
        // The bundled `Inter-Variable.ttf` ships `opsz` **and** `wght`, and the
        // argument needs no font-table read: if it had no `opsz`, `axis_settings`
        // would synthesize nothing for *either* spelling of the family, the two
        // would already have measured alike, and D711's 8.4% would not have
        // existed to fix. `family_axes`' own doc says the same thing three
        // hundred lines up.
        //
        // **The test is unaffected and only the reason was wrong** — it drives
        // `WGHT`, not `opsz`, so it really does assert the plumbing rather than
        // the picture; that is a property of the axis it picks and not of what
        // the face lacks.
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
    //
    // These assert on parley's own **line metrics** rather than on where a glyph
    // landed, because `inline_min_coord`/`inline_max_coord` are literally what
    // `line_x` and `line_max_advance` become — so the assertion states the rule
    // instead of inferring it from ink. The same pair is what the caret,
    // `Selection::from_point` and word selection read, which is why the caret
    // follows a per-paragraph measure for nothing.

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

    /// **The tripwire between the two mechanisms `Paragraphs` needs.**
    ///
    /// `paragraph_starts` scans the text for hard breaks; `break_lines` and
    /// `y_map` count the `Explicit` breaks parley reports. Neither can see the
    /// other, and they only meet at a paragraph *index* — so if they ever disagree
    /// about what a hard break is, every paragraph past the disagreement silently
    /// draws with its neighbour's indent.
    ///
    /// **This failed the first time it ran**, on `"a\r\nb"`: the scanner coalesced
    /// CRLF into one break, because CRLF is one grapheme cluster, and parley breaks
    /// on it twice. That is the whole reason the test exists rather than a comment
    /// asserting the two agree — reading parley's source got this wrong.
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
        // A start indent moves the start edge and nothing else: the right edge is
        // the box's for both paragraphs.
        assert!((groups[0][0].1 - 300.0).abs() < 0.01, "{:?}", groups[0]);
        assert!((groups[1][0].1 - 300.0).abs() < 0.01, "{:?}", groups[1]);
    }

    #[test]
    fn a_paragraph_span_narrows_only_its_own_measure() {
        let d = style(20.0);
        // The same words twice, so the only difference between the two paragraphs
        // is the override.
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

    /// **The panic this is really guarding.** An outdenting first line needs *more*
    /// measure than the box to keep its right edge, and parley's `break_next` opens
    /// with `line_max_advance - layout_max_advance < 1.0` — so with a finite layout
    /// maximum this brings the whole shape down rather than misdrawing. Hand it the
    /// wrap width instead of `f32::INFINITY` and this test panics.
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

    /// **The snap rule, case by case**, because every one of these is a place the
    /// obvious formula gives an answer a user would call a bug.
    #[test]
    fn a_paragraph_write_snaps_outward_to_whole_paragraphs() {
        // "one\ntwo\nsix" — starts at 0, 4, 8; length 11.
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
    /// back over the break that made it — otherwise `Spans::set` no-ops and the
    /// control is dead on exactly the paragraph somebody has just pressed Enter for.
    #[test]
    fn an_empty_trailing_paragraph_reaches_back_over_its_own_break() {
        assert_eq!(paragraph_bounds("a\n", 2..2), 1..2);
        // And it is the *break* that is reached back over, not a byte: U+2028 is
        // three of them and landing inside one would panic on the slice.
        assert_eq!(paragraph_bounds("a\u{2028}", 4..4), 1..4);
        // The paragraph before it is untouched — it resolves at its own first byte.
        assert_eq!(paragraph_bounds("a\n", 0..0), 0..2);
        // An empty node has no paragraph to tell apart from the node itself, and an
        // empty range is how the panel learns to write the defaults instead.
        assert_eq!(paragraph_bounds("", 0..0), 0..0);
    }

    /// The range the snap produces has to be one a span can actually be *set* on,
    /// and be read back by `Paragraphs` as that paragraph's — the two clamps are
    /// stated at opposite ends of the module and this is where they meet.
    #[test]
    fn a_span_over_a_snapped_range_reaches_that_paragraph_and_no_other() {
        let d = style(20.0);
        // The awkward one: three paragraphs, the last empty from a trailing break.
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
        // Three lines: two with text and the empty one the trailing break makes.
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
        // Em(1.0) line height at 10pt: one line box is exactly 10 tall.
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

        // And it is the gap *above the third* paragraph: the first two lines keep
        // their spacing, which is the half a total height cannot tell you.
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
    // Type on a path (§15 D405)
    // -----------------------------------------------------------------------

    /// A rail running left to right along `y = 0`, `len` long.
    fn straight_rail(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((len, 0.0));
        p
    }

    /// A rail turning a quarter circle: it leaves `(0, 0)` heading **right** and
    /// arrives at `(r, r)` heading **down**, so the tangent sweeps 0 → π/2 and
    /// every assertion about direction has a sign it can get wrong.
    fn quarter_rail(r: f64) -> BezPath {
        // The usual circle constant, so the cubic is a quarter circle to within
        // a thousandth of the radius.
        let k = 0.552_284_749_83 * r;
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((k, 0.0), (r, r - k), (r, r));
        p
    }

    /// `on_rail` at a chosen point size, with the **cap-to-baseline trim** a
    /// text-on-path node carries in practice. Both matter to the offset side: the
    /// trim is what decides how far the ribbon reaches from the rail, so it is
    /// what a lost offset costs.
    fn on_rail_at(size: f64, content: &str, rail: BezPath) -> TextLayout {
        let mut p = parts(&style(size), &TextSizing::Auto);
        p.block.trim = BoxTrim::CapToBaseline;
        p.on_path = Some(rail);
        // ⚠️ **Not starting at a corner**, which the maintainer's document does not
        // either (its `on_path_offset` is 0.0573). Where the run begins decides
        // which glyph sits at which corner, and therefore which corner's normal a
        // sampling bug is asked for.
        p.on_path_offset = 0.057_339_449_541_284_41;
        layout(p.as_ref(content))
    }

    fn on_rail(content: &str, rail: BezPath) -> TextLayout {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path = Some(rail);
        layout(p.as_ref(content))
    }

    /// **A one-character span recolours a glyph on a rail and does not move it**
    /// (§15 D489, `[S5.1-L1-03]`).
    ///
    /// Styling one character makes parley split a run of one, and `bend`'s advance
    /// table used to scan each run on its own: for a run of one there is no next
    /// glyph, so the fallback took the `scan` seed of `0.0`, and the placement
    /// arithmetic then rotated the glyph about its **left edge** instead of its
    /// midpoint. Measured before the fix on this exact shape: **0.149 rad, 8.56°,
    /// and 1.35 units** of position.
    ///
    /// ⚠️ **`CharAttr::Color` is chosen because it is metric-neutral.** A `Size` or
    /// a `Weight` span would split the run *and* change the glyph, and then a
    /// difference in position would be the span doing its job. Colour changes the
    /// ink and nothing parley measures, so every difference here is the bug.
    ///
    /// ⚠️ **A curved rail, and it has to be.** `a_straight_rail_reproduces_flat_type`
    /// above cannot see this at all — on a straight rail the tangent is constant,
    /// so rotating about the left edge and about the midpoint give the same answer
    /// and only the position term survives. Every rail fixture in this module used
    /// a single unstyled run on a rail that hides the rotation, which is why the
    /// review's flip of the fallback line left all 374 tests green.
    ///
    /// ⚠️ **Flip-check, run: the advance scan confined to its own run again.**
    /// Fails on the rotation assertion at **−0.0758 rad** — the predicted site,
    /// with the position assertion behind it. The number is smaller than the 0.149
    /// the review measured because that was a 40pt fixture and this is 20pt: the
    /// error is half the glyph's whole advance, so it scales with the type.
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

    /// **A straight rail reproduces flat type, translated so the rail is the
    /// baseline.** This is the identity case, and it is the one assertion that
    /// says the whole map — advance to arc length, tangent to rotation, `y` to
    /// normal offset — is *consistent* rather than merely smooth. A bend that put
    /// every glyph in a plausible-looking wrong place would pass every curved
    /// assertion below and fail this one.
    ///
    /// ⚠️ **The translation is the feature and it is worth stating rather than
    /// subtracting quietly.** Flat type hangs its first baseline an ascent below
    /// the node's origin; on a rail *the rail* is that baseline, so the whole
    /// layout lifts by exactly one ascent to meet it. A designer who draws a curve
    /// and drops text on it means the letters to sit on the line they drew, not an
    /// ascent below it. The first draft of this test asserted raw equality and
    /// failed on `(0, 17)` against `(0, 0)`, which is that lift and not a bug.
    ///
    /// ⚠️ The rail is deliberately longer than the text: a rail exactly as long
    /// would put the last glyph's midpoint at the very end, where the arc-length
    /// solve is clamped, and this test would then be about the boundary rather
    /// than about the map.
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

    /// **The tangent turns the glyphs, and the last one is turned furthest.**
    ///
    /// ⚠️ **The flip that matters here is the sign of the normal, not the
    /// presence of the rotation** — dropping the rotation entirely is the
    /// implementation nobody would write. Negating it (`Vec2::new(d.y, -d.x)` in
    /// `PathWarp::place`) is the one-character slip that *is* plausible, and it
    /// leaves every rotation below unchanged while throwing the descenders and the
    /// second line to the outside of the curve — which is why
    /// `a_second_line_runs_parallel_outside_the_first` is the test that catches it
    /// and this one is not.
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

    /// **A second line runs parallel to the first, one line height to the side a
    /// descender goes.**
    ///
    /// This pins the *normal's* sign, and with it the claim in `bend`'s doc that
    /// the map is of the plane rather than of the line: a second line is one
    /// `line_height` further down in flat space and nothing in `bend` mentions
    /// lines at all, so if it comes out on the right side the arithmetic
    /// generalised on its own.
    ///
    /// **The straight rail is what makes the sign unambiguous**, which is why this
    /// test does not begin with the interesting one. On a horizontal rail "the
    /// side a descender goes" is just larger `y`, with no geometry to reason
    /// about; the curved case then only has to agree with it.
    ///
    /// On `quarter_rail` that side is the **concave** one — the rail leaves the
    /// origin heading right and curves down toward its centre at `(0, r)`, so the
    /// second line sits one line height *inside* the arc. That is correct and is
    /// what "text around the top of a badge" looks like: the second line is nearer
    /// the middle of the badge.
    ///
    /// ⚠️ **Two drafts of this test were wrong, in opposite directions, and only
    /// the flip found either.** The first sorted the distances and asserted that
    /// the largest exceeded the smallest by half a line height — *symmetric*, so
    /// it passed with the normal negated, and so did every other test in this
    /// section. The second fixed that by demanding the second line be **outside**
    /// the arc, which is the wrong side, and passed only against the flipped
    /// build. Both read as careful. The lesson is the one the straight rail now
    /// carries: assert the sign where the geometry cannot be argued about, then
    /// make the curved case agree.
    #[test]
    fn a_second_line_runs_parallel_one_line_height_along_the_normal() {
        let ys = |l: &TextLayout| -> Vec<f64> {
            l.runs
                .iter()
                .flat_map(|run| run.glyphs.iter())
                .map(|g| f64::from(g.y))
                .collect()
        };
        // The unambiguous case: down the page is larger `y`, whatever a curve
        // would have done.
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

        // And the curved case agrees: the same offset, measured along the normal,
        // which here points at the arc's own centre.
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
        // The cubic is a quarter circle to within about a thousandth of `r`, so
        // the first line's baseline is on `r` to well inside this tolerance.
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

    /// **Text longer than its rail is dropped rather than piled up at the end**,
    /// and says so through the field the panel already reads.
    ///
    /// ⚠️ **The plausible wrong version is a clamp, not a crash.** Clamping the
    /// arc-length lookup to the rail's end — which is what `inv_arclen` does if it
    /// is handed an over-long distance and nothing checks first — stacks every
    /// remaining glyph on the last point. That draws a black smudge and reports
    /// `truncated == false`, so both halves of this test are load-bearing.
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

    /// **Alignment is applied along the rail, and applied once.**
    ///
    /// ⚠️ **The bug this is aimed at is double-counting**, not absence: `shape`
    /// deliberately lays a railed node out `Start`-aligned so that `bend` can
    /// offset it, and the plausible wrong version is the one where that gate is
    /// missing and parley centres the line inside the *box* first. That version
    /// still moves the text when the alignment changes, so an assertion that only
    /// checked "centring does something" would pass it. Comparing against the
    /// rail's own half-length is what makes this about the right measure.
    ///
    /// ⚠️ **The sizing here is `AutoHeight` and that is half the fixture.** The
    /// first draft used `Auto`, and removing the gate left it green — under `Auto`
    /// there is no authored width, so parley has no box to align *in* and all
    /// three alignments shape identically. The double-count is only reachable on a
    /// node with a width of its own, which is exactly the node a designer drops
    /// onto a curve after typing a centred paragraph. **A gate can only be tested
    /// through a fixture that reaches it**, and 300 is deliberately nothing like
    /// the rail's 600 so the two measures cannot be confused in the output.
    ///
    /// ⚠️ **The other half is asserting on the *glyphs*, and the second draft did
    /// not.** It read `layout.origin.x`, which is derived from the rail sweep
    /// rather than from where the letters went — so both the wrap gate and the
    /// alignment gate could be deleted with it still green. Reading the leftmost
    /// glyph is what makes this a test of the type's position. The bug that hid
    /// behind the box reading was real and is fixed: the sweep started at flat
    /// `x = 0` instead of at the stretch the glyphs occupy, so an indented or
    /// truncated line got a box adrift from its own text.
    #[test]
    fn alignment_offsets_along_the_rail() {
        let len = 600.0;
        let sizing = TextSizing::AutoHeight(300.0);
        let mut p = parts(&style(20.0), &sizing);
        p.on_path = Some(straight_rail(len));
        let flat_width = layout_with("Ondin", &style(20.0), &TextSizing::Auto)
            .size
            .width;
        // The rail runs along `y = 0` from the origin, so a glyph's `x` *is* its
        // distance along the rail and no unbending is needed to read it.
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

    /// **The box is the bent line box and it holds the ink.**
    ///
    /// The weaker, obvious assertion — "the box is not the flat box" — passes for
    /// a box that is merely wrong, so this asserts containment instead: every
    /// glyph the layout says it draws has to be inside the rect every handle,
    /// snap and export reads.
    ///
    /// ⚠️ **The large-radius case below is deliberately weak, and it is worth
    /// saying so rather than letting it look like coverage** (`[S5.1-L4-02]`).
    /// `[S5.1-L4-02]` asks for this test to gain one, and it has — but the box
    /// this function checks is the **rail's** swept ribbon and not the ink's, so
    /// on a long rail the box covers the whole curve while the words occupy the
    /// first percent of it. Containment is then true however coarsely the sweep
    /// samples, and a large-radius case *here* would be green for any fix at all.
    /// **`the_bent_box_holds_the_rail_itself` is the one with teeth**; this case
    /// is kept because it costs nothing and because a reader who meets the finding
    /// will come looking for it.
    #[test]
    fn the_bent_box_contains_the_bent_ink() {
        for (what, rail) in [
            ("a tight rail", quarter_rail(150.0)),
            // `[S5.1-L4-02]`'s large-radius case: `warp.len` is **18,852** here,
            // two orders longer than the ink, which is the regime the finding is
            // about. ⚠️ **What the sweep does with it depends on which side of
            // `SKETCH` you are on** (§15 D791), so neither density is stated as
            // *the* one: measured, the uniform sweep hits its 4,096 cap and
            // degrades to a ~4.6-unit step where `RAIL_STEP` asks for 2.0, while
            // the curvature rule wants **864** samples — a ~22-unit step, the
            // radius being enormous — and never reaches the allowance at all.
            // **Containment holds either way, which is the point of the note
            // above**: this case is weak, not a guard.
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
            // And the second box the canvas would draw is suppressed rather than
            // drawn somewhere meaningless — `untrimmed_bounds`' own contract.
            assert_eq!(bent.untrimmed_bounds(), bent.bounds(), "{what}");
        }
    }

    /// **The reported box holds the *rail*, at a sampling density far finer than
    /// the sweep's own** — the assertion a cheaper box sweep has to survive
    /// (`[S5.1-L4-02]`).
    ///
    /// `bend`'s box is the rail swept end to end with the flat box's two
    /// verticals carried along it. **`[S5.1-L4-02]` wants that sampling made
    /// cheaper**, and the way a cheaper sweep goes wrong is by stepping *over* an
    /// extremum: the box comes back too small, and a node's handles, snap targets
    /// and W/H fields all shrink with it. So this re-walks the same map at
    /// **200,001 points** — orders finer than any sweep — and asserts the reported
    /// box already contains every one of them.
    ///
    /// ⚠️ **How densely `bend` samples now depends on `SKETCH`, so this doc names
    /// no one density** (§15 D791). The previous uniform sweep took
    /// `warp.len / RAIL_STEP` steps capped at 4,096; the curvature rule on trial
    /// takes a step from each segment's own curvature and samples both ends of
    /// every segment. Measured on this test's own four fixtures — `total_want`
    /// against the uniform sweep's step count — **arch r=12,000: 1,728 against
    /// 4,096** (capped); **spike h=200: 2 against 239**; **spike h=1,000: 2
    /// against 1,014**; **the 341×172 rectangle: 4 against 513**. The polyline
    /// fixtures collapse to two-per-segment because a straight segment has no
    /// curvature to resolve and its extremes are its ends. **This test is written
    /// against the *outcome* and passes either way**, which is the whole reason it
    /// can referee the trial.
    ///
    /// 🚨 **The two rails are here because the smooth one is far more forgiving
    /// than anybody expects, and a test with only that one would license a fix
    /// that breaks the other.** Measured against a deliberately coarsened
    /// **uniform** sweep — these four figures predate `SKETCH` and are readings of
    /// that sweep, not of whatever `bend` is doing when you read this:
    ///
    /// - **On an arch**, the loss is *second order* in the step — roughly
    ///   `interval² / 2r`, because a smooth curve near its extremum is locally
    ///   flat. Coarsening the sweep **64×** at r = 12,000 shrank the box by
    ///   **0.17 units**. A large-radius arc is not a stress case; it is the easy
    ///   case, and the cap already coarsens it to ~9 units with no measurable loss.
    /// - **On a sharp corner**, the loss is *first order* — the apex is a cusp, and
    ///   missing it loses about the step times the turn. The same 64× coarsening on
    ///   `spike_rail` lost **3.27 units at h = 200 and 6.26 at h = 1000**, against
    ///   **0.0000 at today's density**.
    ///
    /// **So the case to protect is a pen-drawn corner, a star point or a
    /// tight-radius rounded rect — not a big circle.**
    ///
    /// **Flipped**, by coarsening the uniform sweep 64×: this fails at *"a sharp
    /// corner"* with `(0, 3.2708)`, the predicted site and the predicted number —
    /// and `the_bent_box_contains_the_bent_ink` stays **green under the same
    /// flip**, large-radius case and all. That is the whole argument for this test
    /// existing: the guard `[S5.1-L4-02]` nominated cannot see the failure its own
    /// fix would cause.
    ///
    /// ⚠️ **That flip is written against the `SKETCH = false` arm and only bites
    /// there**, there being no `steps` to coarsen on the other one. The curvature
    /// rule's equivalent mutation is to raise `BOX_SAG`; its own first draft is
    /// what `BOX_SAG`'s doc records failing `dragging_railed_text_scales_its_rail`
    /// at 0.05.
    ///
    /// ⚠️ **The spike's legs are
    /// deliberately unequal** (`237, 13` rather than a mirror of the first): with
    /// equal legs the apex falls at `s = len/2` and an even step count lands a
    /// sample exactly on it, so the test reports **0.0000 for any coarseness** and
    /// is about nothing. That was the first version of this fixture.
    #[test]
    fn the_bent_box_holds_the_rail_itself() {
        for (what, rail, wraps) in [
            ("an arch past the step cap", arch_rail(12_000.0), false),
            ("a sharp corner", spike_rail(200.0), false),
            ("a sharper, taller corner", spike_rail(1000.0), false),
            // ⚠️ **Small on purpose.** The type has to wrap past a corner and onto
            // the far side, or there is no ink out there for a missing offset to
            // strand — which is what a 341×172 version of this case did: green,
            // and about nothing. The fixture assertion below is what says so.
            ("a closed rectangle", rect_rail(341.0, 172.0), true),
        ] {
            let bent = match wraps {
                // The maintainer's case as closely as a fixture has got it: 24pt,
                // the cap-to-baseline trim a text-on-path node carries, their
                // rail's size, their string, their start offset. ⚠️ **It still does
                // not reproduce their bug** — see the note below the `bounds` call.
                true => on_rail_at(
                    24.0,
                    "Hello world this is Ondin text on rail tool and it's looking good",
                    rail,
                ),
                false => on_rail("Ondin on a curve", rail),
            };
            let b = bent.bounds();

            // **The ink, not only the centreline.** A rail's centreline can sit
            // comfortably inside a box whose **offset** side is missing entirely,
            // because the centreline is not offset at all — so the assertion below
            // cannot see a lost ribbon edge and this one can.
            //
            // 🚨 **Read the next sentence before trusting this test.** It is here
            // because a sketch of the cheaper sweep did exactly that — sampled each
            // segment only at its *start*, and since `place` resolves a corner to
            // **one** segment, a closed rectangle lost a whole edge's outward
            // ribbon: the box came back **17.46 units narrow on the right, sitting
            // exactly on the rail**, with the type hanging outside it. **And this
            // assertion did not catch it.** Four fixtures were tried — 341×172 and
            // 62×40 rectangles, 20pt and 24pt, default and cap-to-baseline trim,
            // with and without a start offset — and every one stayed green against
            // the bug. What caught it was running the **maintainer's own document**
            // through the two sweeps and diffing the boxes.
            //
            // ⚠️ **So this is a guard against the *class* and not a regression test
            // for that bug.** Something about a real document — the font, the
            // string, where the glyphs fall against the corners — decides whether a
            // lost offset strands any ink, and it has not been reduced to a fixture.
            // Do not read a green run here as "the ribbon is whole".
            //
            // ⚠️ **A point of slack, because the box is *trimmed***. `CapToBaseline`
            // and friends measure to the cap and the baseline, so a descender or an
            // ascender overshoot legitimately pokes out by a fraction — 0.29 on the
            // fixture that found this. What is not legitimate is a whole edge.
            let ink = outline(&bent).bounding_box();
            // The type must actually have gone round the rail, or a missing
            // offset side has no ink to strand and this assertion is about
            // nothing. `w.bounds()` is the rail's own box.
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

            // **The fixture is asserted before the assertion it is for.** A rail
            // that failed to build, or a box that came back degenerate, would make
            // every containment below trivially true.
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

    /// **A railed box clears its rail on all four sides, whichever way the rail is
    /// traversed** — the offset-side guard `the_bent_box_holds_the_rail_itself`
    /// could not reduce to a fixture, in the one form where it is deterministic.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg,
    /// so a `[link]` in this module's prose is decoration no gate can validate.)
    ///
    /// That test's own note says the *"17.46 units narrow on the right"* bug
    /// escaped four fixtures and was caught only by running the maintainer's
    /// document through two sweeps: whether a lost offset **strands ink** depends
    /// on the font, the string and where the glyphs fall against the corners. **This
    /// asserts the ribbon rather than the ink**, so it does not need any of that —
    /// the sweep covers the whole rail whatever the text does, and a lost edge is a
    /// margin of exactly zero.
    ///
    /// **Both traversals, because they are different code.** `PathWarp::at` maps
    /// `along` to `len − along` under `PathWarp::flip`, so a sampler that walks
    /// `segs` in rail order is reading each segment's interval **backwards** on a
    /// flipped rail. `[S5.1-L4-02]`'s per-segment sketch did exactly that: its
    /// boundary samples landed mid-edge and its end nudge landed in the *next*
    /// segment, so the right edge never contributed its own normal.
    /// **Unflipped it was byte-identical**, which is why nothing else saw it.
    ///
    /// ⚠️ **The clearance is asserted as *equal on four sides*, not as a number.**
    /// On a rectangle rail the normal is axis-aligned and the offset constant, so
    /// all four are the same — the ascent unflipped and the descent flipped, both
    /// font- and trim-dependent and neither worth pinning (3.0 flipped here). What
    /// does not depend on the font is that none of them is **zero**.
    ///
    /// **Flipped**, by restoring the sketch's original sampling in `bend` —
    /// `let a0 = *s0;` in place of the mirrored binding, **and** dropping the
    /// `a0 + slen * 1e-9` sample so only the forward nudge is left: fails at
    /// *"a square-ish rectangle flip=true"* on the **right** clearance at
    /// `0.0000`, box `x1 = 200.0` against a rail `x1 = 200.0`. That was the
    /// predicted side and the predicted number. **Both fixtures fail** — the 400×20
    /// the same way at `x1 = 400.0`, checked by running it first — and `flip=false`
    /// stays green, as does every other rail test. Toggling `SKETCH` off passes it
    /// too, the uniform sweep having never had the defect.
    ///
    /// 🚨 **Dropping only *one* of those two does **not** fail, and that is worth
    /// knowing before anybody simplifies the fix.** With the mirrored `a0` reverted
    /// but both end nudges kept, every fixture here stays green: straddling each
    /// mirrored boundary from both sides happens to leave a sample inside every
    /// segment of a rectangle, at either aspect ratio. **So these fixtures do not
    /// discriminate the two halves of the repair** — the mirroring is kept because
    /// reading a segment's interval backwards is wrong in general, not because a
    /// test here catches it, and a rail whose segment lengths defeat the straddle
    /// (a three-segment open polyline at roughly 100 : 20 : 10 is one, on paper)
    /// has not been reduced to a fixture. **A green run here is not a licence to
    /// delete the mirroring.**
    #[test]
    fn a_flipped_rails_box_clears_the_rail_on_every_side() {
        // **Two aspect ratios, and *both* have teeth against the defect this is
        // for.** Measured against the original sampling, each fails on the right
        // clearance at `0.0000` under `flip=true`: the 200×100 at `x1 = 200.0` and
        // the 400×20 at `x1 = 400.0`.
        //
        // ⚠️ **What neither of them discriminates is the *mirroring* on its own** —
        // see the 🚨 paragraph in this test's doc. The 20:1 ratio was added on a
        // guess that lopsided segment lengths would defeat the both-ends straddle,
        // and **the guess was wrong**; it is kept as a second shape rather than as
        // a sharper one. ~~"the near-square one does not discriminate … the 20:1
        // rail is the one with teeth"~~ — struck, it was false of both halves.
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

                // The fixture, before the assertion it is for: a degenerate rail
                // or box would make every clearance below trivially true.
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

    /// A half-circle-ish arch: its extremum is **between** the endpoints, unlike
    /// `quarter_rail`, whose bbox is decided by its two ends and which therefore
    /// cannot detect a sweep that steps over anything.
    fn arch_rail(r: f64) -> BezPath {
        let k = 0.552_284_749_83 * r;
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((0.0, -k), (r - k, -r), (r, -r));
        p.curve_to((r + k, -r), (2.0 * r, -k), (2.0 * r, 0.0));
        p
    }

    /// A **closed rectangle** with sharp corners — the rail the text-on-path tool
    /// makes from a rectangle, and the shape that found the offset-side bug the
    /// centreline assertion could not see.
    fn rect_rail(w: f64, h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((w, 0.0));
        p.line_to((w, h));
        p.line_to((0.0, h));
        p.close_path();
        p
    }

    /// A rail with a **cusp** at `(100, -h)` — the shape a pen-drawn corner, a
    /// star point or a tight rounded-rect corner makes, and the one where a
    /// coarser box sweep loses real units rather than fractions of one.
    ///
    /// ⚠️ **Unequal legs on purpose** — see `the_bent_box_holds_the_rail_itself`.
    fn spike_rail(h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, -h));
        p.line_to((237.0, 13.0));
        p
    }

    /// **A warped band follows the rail rather than cutting across it.**
    ///
    /// ⚠️ **This is the test `RAIL_STEP` exists for**, and its flip is the
    /// tempting simplification: map only the endpoints of each edge — which is
    /// what a *flattener* does to a straight line, since there is no curvature to
    /// subdivide by — and the result is a straight ribbon whose midpoint is a long
    /// way off the curve. Asserting on the endpoints would pass that version;
    /// asserting on the middle is the whole point.
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

    /// **The flip does both halves, and doing only one is the bug it is aimed at**
    /// (§15 D406).
    ///
    /// A flip that reversed the *direction* and left the text on the same side
    /// gives upside-down letters reading backwards; one that changed *sides* and
    /// left the direction alone gives right-way-up letters reading backwards. Both
    /// are plausible implementations and both look wrong in a way a screenshot
    /// would catch and an assertion about one axis would not — so this asserts
    /// both, on the same fixture.
    ///
    /// The straight rail is deliberate, for the reason
    /// `a_second_line_runs_parallel_one_line_height_along_the_normal` gives: on a
    /// horizontal rail "the other side" is just the other sign of `y`, and "the
    /// other way" is just the other sign of `x`, with no geometry to argue about.
    ///
    /// ⚠️ **The flip that would catch a half-implementation is not "turn the
    /// feature off"** — it is negating the tangent *without* letting the normal
    /// follow, which is the version somebody writing this by hand would produce.
    /// It leaves the direction assertion green and fails the side one.
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

        // Direction: unflipped the text starts at the rail's origin and runs right;
        // flipped it starts at the far end and runs left.
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

        // Side: the second line is below the rail unflipped and above it flipped —
        // the rail itself being `y = 0`.
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

        // Every glyph is turned half a turn, which is what puts the letters the
        // right way up when read from the other side.
        for g in flipped.runs.iter().flat_map(|r| r.glyphs.iter()) {
            assert!(
                (f64::from(g.rot).abs() - std::f64::consts::PI).abs() < 0.01,
                "a flipped glyph on a straight rail is turned by π: {}",
                g.rot
            );
        }
    }

    /// **The flip survives the rail being taken off and put back**, which is what
    /// the field being *beside* the rail rather than inside it buys (§15 D406).
    ///
    /// The failure it is aimed at is the destructive toggle: with the flag inside
    /// the `Option`, *Detach from path* has nowhere to keep it, so putting the text
    /// back on a curve silently comes back the wrong way round. That is the same
    /// argument `Node::mask_mode` makes beside `Node::mask`, and it is tested here
    /// at the layout because that is where it shows.
    #[test]
    fn the_flip_outlives_the_rail() {
        let mut p = parts(&style(20.0), &TextSizing::Auto);
        p.on_path_flip = true;

        // Off the rail: the flag says nothing and the type is ordinary.
        let flat = layout(p.as_ref("nn"));
        let plain = layout_with("nn", &style(20.0), &TextSizing::Auto);
        assert_eq!(
            flat.size, plain.size,
            "a flip with no rail must be inert, not a second kind of layout"
        );

        // Back on it, and still flipped.
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

    /// **A decoration band warps onto the rail it belongs to, not a baseline away
    /// from it** (§15 D407).
    ///
    /// ⚠️ **This is the test the feature shipped without, and the bug it would have
    /// caught was live for a day.** `PathWarp::place` took *baseline-relative* `y`,
    /// because the glyph loop in `bend` had the baseline to hand and subtracted
    /// it there — while the render boundary handed it a `DecorationInk::band`,
    /// whose coordinates are ordinary text-local ones. On 20pt Inter the underline
    /// came out at **21.94** below a rail it belongs 2.94 below: one whole
    /// baseline, plainly visible, and asserted by nothing.
    ///
    /// The existing warp test did not catch it because its band was built at
    /// `y = 0` — a coordinate that means the same thing in both spaces. **A
    /// fixture on the one value where two conventions agree cannot tell them
    /// apart**, which is why this one takes the band off a real layout instead of
    /// making one up.
    #[test]
    fn a_decoration_warps_onto_the_rail_and_not_a_baseline_below_it() {
        let mut style = style(20.0);
        style.set(CharAttr::Underline(Some(Decoration::default())));
        let mut p = parts(&style, &TextSizing::Auto);

        let flat = layout(p.as_ref("Hxy"));
        let band = flat.decorations.first().expect("an underline").band;
        let baseline = flat.baselines[0];
        // The fixture has to be in the state the bug needs: a band well away from
        // zero, and a baseline well away from zero, or the two conventions agree.
        assert!(
            band.y0 > baseline && baseline > 5.0,
            "the fixture must separate the two spaces: band {band:?}, baseline {baseline}"
        );

        // The rail is `y = 0`, so a correctly warped band's own `y` *is* its
        // distance below the rail.
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
        // And the glyphs it belongs under are on the rail, which is the other half
        // of "these two are in the same space".
        let g = bent.runs[0].glyphs[0];
        assert!(
            f64::from(g.y).abs() < 0.01,
            "the baseline is the rail: {}",
            g.y
        );
    }

    /// **`PathWarp::nearest` is `PathWarp::place`'s inverse**, which is what
    /// lets a click on bent type reach the character under the pointer (§15 D407).
    ///
    /// ⚠️ **Round-tripped rather than checked against hand-computed numbers**, and
    /// on a *curved* rail on purpose: the arithmetic that is easy to get wrong is
    /// converting a Bézier's parameter back to an arc length, and on a straight
    /// segment `t` and arc length are proportional, so a version using `t × len`
    /// passes every straight fixture and is a whole letter out on a curve.
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

    /// **The offset moves the type along the rail, and a closed rail wraps**
    /// (§15 D409).
    ///
    /// ⚠️ **The wrap is not a nicety, it is what makes the offset usable at all.**
    /// Reported as *"it always starts the text at a specific point — with a
    /// triangle, I click on the base and it starts on the right edge at the top"*,
    /// which the offset answers. But a click just *before* the path's own first
    /// point gives an offset near 1.0, and on a rail with an end that leaves almost
    /// no room — the text would be truncated to nothing on a shape that plainly has
    /// a whole circumference of it. A closed rail has no end, so the type carries
    /// on round.
    ///
    /// **Both halves are asserted on the same fixture**, an ellipse, because the
    /// failure is the pair: an implementation that offsets without wrapping passes
    /// the first assertion and truncates on the second.
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

        // ⚠️ **The assertion the wrap is for.** 0.97 puts the text's start within a
        // few units of the rail's own end, so every glyph but the first has to come
        // round the other side.
        let nearly_round = glyphs(0.97);
        assert_eq!(
            nearly_round.len(),
            at_start.len(),
            "a closed rail has no end to fall off — this is what truncates without \
             the wrap"
        );
        // And they really did wrap rather than piling up: the last glyph is past the
        // seam, so it is nowhere near the first.
        let (first, last) = (nearly_round[0], nearly_round[nearly_round.len() - 1]);
        assert!(
            (first - last).hypot() > 20.0,
            "the run must be spread round the seam, not stacked on it: {first:?} to \
             {last:?}"
        );
    }

    /// **An *open* rail still has an end, and an offset moves the text *toward*
    /// it** — the control for the wrap above (§15 D409).
    ///
    /// Without this, "a closed rail wraps" could be written as "every rail wraps",
    /// and a line of text on a short open path would silently fold back over itself
    /// instead of running out.
    ///
    /// ⚠️ **The offset is what makes the flip visible, and the first draft of this
    /// test had none.** With no offset the rail's `start` is zero, and the two
    /// conditions — "the arc length the text has reached is inside the rail" and
    /// "the text has not run longer than the rail" — are the *same inequality*.
    /// Flipping every rail to wrap left it green. **Two rules that coincide at the
    /// origin need a fixture away from it.**
    #[test]
    fn an_open_rail_still_runs_out_and_an_offset_pushes_text_off_it() {
        let word = "Hamburge";
        let flat = layout_with(word, &style(20.0), &TextSizing::Auto);
        // A rail comfortably longer than the word, so nothing is truncated until
        // the offset does it — the whole point being that the *offset* is what
        // pushes the text off the end.
        let rail = straight_rail(flat.size.width * 1.5);

        let whole = on_rail(word, rail.clone());
        let all: usize = flat.runs.iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(
            whole.runs.iter().map(|r| r.glyphs.len()).sum::<usize>(),
            all,
            "the fixture must start with the whole word on the rail"
        );
        assert!(!whole.truncated);

        // Two thirds along a rail one and a half words long leaves room for a
        // third of a word.
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

    /// **A rail with no length is not a rail**, and the node falls back to
    /// ordinary horizontal type rather than to nothing.
    ///
    /// This is what lets `io::schema` leave the field alone on the way in: a
    /// hand-written or corrupt file cannot produce an invisible layer here.
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
    //! The caret model behind in-canvas text editing. These exercise the parts
    //! a naive `String` buffer gets wrong: cluster boundaries, selection
    //! replacement, and vertical movement through a shaped layout.

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
        // "Bold, then type" has to work, and it is the *only* exception to the
        // inherit-from-the-left rule.
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

    /// **A `\r\n` pair is one line break, not two** — the Windows clipboard's line
    /// ending, and so the ending of most text that will ever be pasted into this app.
    ///
    /// parley maps `\r` and `\n` each to `Whitespace::Newline` on their own, and a newline
    /// cluster is what raises `BreakReason::Explicit`, so the pair raises two of them and
    /// every line of pasted text comes out double-spaced. Found by probe rather than by
    /// reading: `"a\r\nb"` measured **72 high, the same as `"a\nb\nc"`**, and that is the
    /// number this pins.
    ///
    /// **Heights, and the two-line answer as the fixture.** Flip `insert` back to a plain
    /// copy and the CRLF case alone fails, which is what says the normalization did not
    /// simply swallow every `\r` — the lone-CR line is the other half of that.
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

    /// **A cut hands back exactly what it removed, and a bare caret cuts nothing.**
    ///
    /// The `None` is the half worth a test. It is what lets a *Cut* row be dimmed
    /// rather than offered dead — and it is what stops a cut with no selection
    /// putting an empty string on the **system** clipboard, which is a keystroke
    /// that looks inert and silently destroys whatever was on it.
    ///
    /// Flip `cut` to hand back `Some(String::new())` for an empty range and only
    /// the second half fails, which is why both are asserted here rather than the
    /// happy path alone.
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

    /// **A cut must not eat a pending style**, which is the whole of why it is
    /// `replace` and not `insert("")`.
    ///
    /// Flip it to `insert("")` — the shorter spelling, and the one somebody would
    /// actually have written, since "replace the selection with nothing" is exactly
    /// what a cut removes — and the bold asked for before the selection is taken by
    /// the insert instead of the next character typed, so the last assertion reads
    /// 0 spans.
    ///
    /// **The fixture is the reachable route to the state**, and there is only one:
    /// `drag_to` is the sole mover that does not call `caret_moved`, so extending a
    /// selection by dragging is the only way to hold a selection and a pending
    /// style at the same time.
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

    /// **Why `Operation::SetText` carries the paragraph list.** A `ParaSpans` entry
    /// is a byte range, so an edit in one paragraph moves a *later* paragraph's
    /// first byte — and the first byte is what its style is resolved at. Drop the
    /// `edited` call in `TextEdit::insert` and the override is left describing
    /// somebody else's bytes.
    ///
    /// **The caret has to sit under the ink, and it did not.** Reported from use as
    /// the cursor staying where the text used to be after one paragraph was
    /// indented — parley's `Cursor::geometry` measures in a frame that omits the
    /// line's start edge, which is where a per-paragraph indent lives (§15 D163).
    /// Remove the correction in `caret_rect` and this reads 0.
    ///
    /// **A regression, not a missing feature**: the first-line indent has been in the
    /// panel since long before per-paragraph measure, and it went in through
    /// `set_text_indent`, which lands in `metrics.offset` — which parley's caret
    /// *does* read. So the second half of this pins the node-level control too.
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
            // And the second paragraph's caret is untouched by the first's indent.
            edit.move_down(false);
            edit.move_line_start(false);
            assert!(
                (edit.caret_rect(1.0).x0 - flush).abs() < 0.01,
                "{why}: paragraph two should be where it was"
            );
        }
    }

    /// **The selection highlight needs no correction, and that is worth pinning
    /// rather than trusting.** It is the other half of the caret's bug: the two
    /// rectangles are drawn side by side by `canvas.rs` from two different parley
    /// functions, and `Selection::geometry` opens with `metrics.offset +
    /// metrics.inline_min_coord` where `Cursor::geometry` reads only the first. So
    /// the fix belongs to one of them and applying it to both would push the
    /// highlight one indent too far right — a claim from reading parley's source,
    /// which is what this turns into a claim about behaviour.
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
        // And it lands where the caret does, which is the whole point of the pair.
        edit.move_line_start(false);
        assert!(
            (edit.caret_rect(1.0).x0 - indented[0].x0).abs() < 0.01,
            "the caret and the highlight have to agree about the line's start edge"
        );
    }

    /// **The second symptom of the same omission, and the worse one.** parley seeds
    /// its sticky column from `Cursor::geometry` and lands the caret with
    /// `Cursor::from_point`, which reads the start edge the other does not — so
    /// inside an indented paragraph every Down moved the caret one indent to the
    /// *left*, which past the first word means jammed against the line start.
    /// Nobody reported this one; it falls out of the same reading.
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
        // Six characters in on the first line, then straight down.
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

    /// **Vertical motion in a node whose paragraphs are spaced apart.** Two y spaces
    /// meet in the new caret code and they must not be mixed: `caret_line_index`
    /// compares `Cursor::geometry`'s y against `block_min_coord`, both of which are
    /// **layout** space, while `caret_rect` maps its rectangle through the `YMap`
    /// into local space. Paragraph spacing is the only thing that makes the two
    /// spaces differ, so it is the only thing that would show them being mixed, and
    /// nothing else in the suite exercised it.
    ///
    /// **Written to de-risk the line lookup and it found a different bug**, older
    /// than any of this: the map is keyed on line-*box* tops and was being handed
    /// *ink* coordinates, so both rectangles came out a whole gap out of place
    /// (§15 D166). The lookup was fine; the mapping was not.
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
        // And the caret's rectangle rides the spacing: each step down is a line box
        // plus the gap, in local space, where the layout-space step is a line box.
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

        // **The highlight is the second face of the same bug, and the worse-looking
        // one.** One rectangle takes *two* shifts: its top is an ink coordinate that
        // falls short of its own break, its bottom sits past the *next* line's break
        // and takes the shift belonging to the paragraph below. Measured before the
        // fix, the second paragraph's line highlighted local y 18 → 122 — 104pt of
        // ink over a 24pt line box, where the fix gives 58 → 82.
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

    /// The sticky column survives consecutive vertical moves — walking down through
    /// a short line and out the other side must not pull the caret in with it. That
    /// is what `Selection::h_pos` was doing for us before `move_line` took the job
    /// over, so it is the part of parley's behaviour worth a test of its own.
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
        // **Asserted on the byte offset, not on x.** The caret lands on a cluster
        // boundary, and the third line's letters are not the first's — twelve `c`s
        // measure 133.2 where twelve `a`s measure 131.25 — so an x comparison has to
        // carry a tolerance wide enough to hide the bug it is testing for. Twelve
        // characters in is twelve characters in.
        assert_eq!(
            edit.selected_range(),
            28..28,
            "the column the first line was in has to survive the short one, or the \
             caret comes back two characters in rather than twelve"
        );
    }

    /// **The panel's write path, end to end on the side of it that can be built.**
    /// The panel's own wiring is in the app crate, which this one cannot reach, so
    /// it rests on reading here — but everything below it can be
    /// driven: put the caret in the middle paragraph, apply the attribute the way
    /// `apply_para_attrs` does, and read the measure back off the shaped layout.
    ///
    /// What this pins that the pieces do not, tested apart: that
    /// `paragraph_range` and `style_paragraph` agree about the range, and that a
    /// re-shape after the write puts the indent where the caret was rather than
    /// where the selection started.
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
        // The caret lands at the end; walk it up into the middle paragraph.
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
        // And the caret is still where it was, which `reshape_keeping_selection` owes
        // every restyle (§15 D155's lesson in the other scope).
        assert_eq!(edit.selected_range(), 4..4);
    }

    /// **Both edit paths, and the assertion is the exact range.** `TextEdit` has
    /// two: `insert` handles a selection itself and `replace` is what the deletes
    /// call, and each owns its own re-basing — so a test that goes through one
    /// proves nothing about the other. The first version of this test went through
    /// `insert` alone *and* asserted only that the paragraph's first byte still
    /// landed inside the span, which passed with the re-basing removed: a
    /// one-character insert slides that byte further *into* a six-byte span. An
    /// exact range is the assertion that discriminates.
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

        // Select the first paragraph and delete it — through `replace`.
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

        // And type into it — through `insert`.
        edit.insert("ab");
        assert_eq!(edit.content(), "ab\nsecond");
        assert_eq!(span_over_paragraph_two(&edit), 3..9);
    }
}

#[cfg(test)]
mod rail_session_tests {
    //! A live edit session on type that is set along a path (§15 D407).
    //!
    //! **Its own module because the fixture is different in kind**, not because the
    //! assertions are: everything here needs a rail on the parts, and `edit_tests`'
    //! `editor` helper deliberately builds the ordinary case that every other test
    //! in that module wants.

    use super::*;
    use crate::typography::TextAlign;

    /// A rail from `(0, 0)` running right, `len` long — so a point's `x` is its
    /// distance along the curve and its `y` is its distance across, which makes
    /// every assertion below readable without any geometry.
    fn straight(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((len, 0.0));
        p
    }

    /// The centre of the first quad a caret draws as — the point a user would say
    /// the caret is at.
    fn caret_at(e: &TextEdit) -> Point {
        let q = e.caret_quads(1.0);
        let c = q.first().expect("a caret is drawn");
        (c.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / 4.0).to_point()
    }

    fn railed(content: &str, rail: BezPath, align: TextAlign) -> TextEdit {
        railed_at(content, rail, align, 0.0)
    }

    /// `railed` with an `on_path_offset` — the fraction of the rail the type
    /// starts at, which is the parameter the closed-rail arm of
    /// `PathWarp::shortest_way_round` exists for and which every case above
    /// leaves at zero.
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

    /// **The caret is drawn where the ink is** (§15 D407).
    ///
    /// ⚠️ **This is the reported bug**: "when editing a text on path, the cursor
    /// shows on the original position of the text, before it was set to follow the
    /// path — it keeps on editing the ghost of the old position". The session
    /// carried the rail, so its *box* bent, and every caret and highlight rectangle
    /// went on being computed against parley's flat layout. On this fixture the
    /// caret drew a whole baseline below the rail and, once centred, an alignment
    /// offset away along it as well.
    ///
    /// **Asserted against the glyphs rather than against a number**, because that
    /// is the claim — the caret belongs on the text, wherever the text turned out
    /// to be — and a hand-computed coordinate would only be re-deriving the bend.
    #[test]
    fn the_caret_sits_on_the_bent_ink() {
        // `TextEdit::new` leaves the caret at the end of the content, which is the
        // state this wants and the one a session opened by typing is in.
        let e = railed("Hamburgefonstiv", straight(600.0), TextAlign::Center);
        let layout = e.text_layout();
        let last = layout
            .runs
            .last()
            .and_then(|r| r.glyphs.last())
            .expect("the fixture shapes");
        let mid = caret_at(&e);

        // The caret is at the end of the text, so it belongs beside the last glyph
        // — within a letter of it along the rail, and within the line box across it.
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

    /// **Clicking the bent ink puts the caret in the character under the pointer**
    /// — the input half of the same bug, and the half that makes a session on a
    /// rail usable rather than merely correct-looking.
    ///
    /// ⚠️ **Round-tripped through the caret's own drawn position**, which is what
    /// makes this a test of the *pair* rather than of `nearest` alone: place the
    /// caret by byte, ask where it is drawn, click there, and the byte must come
    /// back. A version where both directions were wrong in the same way would still
    /// fail, because the byte offsets are not derived from the geometry.
    #[test]
    fn clicking_bent_ink_lands_on_the_character_under_it() {
        for at in [1_usize, 5, 9, 14] {
            // A fresh session per case: the caret starts at the end and walks back,
            // so each `at` is reached the same way and a failure names one byte.
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

    /// **A flat session is unchanged**, which is the control: `caret_quads` on
    /// ordinary type is the four corners of `caret_rect` and nothing else.
    ///
    /// Worth a test because the bent path is the interesting one and the flat path
    /// is the one every existing session takes — a warp built for a node with no
    /// rail would bend every caret in the app.
    #[test]
    fn a_flat_session_gets_exactly_its_own_rectangle() {
        // `TextEdit::new` leaves the caret at the end of the content, which is the
        // state this wants and the one a session opened by typing is in.
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

    /// **A click past the wrap point of a closed rail lands under the pointer,
    /// not at character 0** (`[S5.1-L6-04]`, §15 D496).
    ///
    /// ⚠️ **This is the one arm of `PathWarp::shortest_way_round` that does
    /// anything, and nothing in the workspace reached it.** Every case above uses
    /// `straight`, an **open** rail, where the whole function is `x`; the three
    /// existing rail-session tests and the placement tests in `mod tests` are all
    /// open rails or zero offsets. Reducing the function to `pub fn
    /// shortest_way_round(&self, x: f64) -> f64 { x }` — deleting §15 D409
    /// entirely — left 374 lib tests green while re-arming the exact symptom D407
    /// was written to fix.
    ///
    /// **What goes wrong without it.** Past the wrap, `PathWarp::nearest`
    /// legitimately answers a *negative* arc length — on this fixture around
    /// `−705` where the text-local `x` is `+49` — because `nearest` deliberately
    /// does not normalise (an offset being measured for a fresh node wants the
    /// plain distance, and normalising there made a half-way click read as
    /// `−0.5`). `Cursor::from_point` then clamps a negative `x` to the start of
    /// the string, so clicking the ink of the later letters put the caret at byte
    /// 0.
    ///
    /// **A non-zero `on_path_offset` is load-bearing in the fixture**: it is what
    /// moves the wrap point into the middle of the text. At offset 0 the string
    /// begins at the rail's own seam and every letter is on the near side of it.
    ///
    /// **Flip run**, `shortest_way_round` reduced to `x`: fails on `"D"`, glyph
    /// origin `(120.16, 3.40)`, caret `0` against `3` — five of the eight letters
    /// go to 0, which is the finding's own measurement reproduced. The predicted
    /// site was the first letter past the wrap and that is where it landed.
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
    //! **Optical margin alignment places a line by its ink** (§15 D830).
    //!
    //! ⚠️ **Names here are plain backticks, never `[links]`** — this is a
    //! `#[cfg(test)]` module, so `cargo doc` cannot see it and a link would be
    //! decoration no gate can validate (§15 D319).
    //!
    //! The three strings are chosen for their **left side bearings**, measured on
    //! the default face at 16px before any of this was built: `W` is 2.34% of the
    //! font size, `H` is 8.59% and `'` is 10.06%. A fixture of three `H` words
    //! would assert nothing, because they already agree.
    use super::*;
    use crate::typography::TextAlign;

    /// The union of every run's ink, which is what a reader sees as the text's
    /// edges — deliberately *not* the layout's advance extents, since the gap
    /// between the two is the whole subject.
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

    /// **Three auto-width labels starting with different letters line up.** This
    /// is `roadmap.md`'s complaint stated as an assertion: without the setting the
    /// three inks start at three different x, and the spread is what a reader sees
    /// as one label being indented.
    ///
    /// ⚠️ **Flipped** by dropping `x -= lsb` from the optical arm in `break_lines`
    /// — the plausible half-fix, since widening the measure alone still hangs the
    /// end: **red on the all-equal assertion**, the predicted site, with the three
    /// original bearings back as `[0.375, 1.375, 1.609]`.
    ///
    /// ⚠️ **That flip takes the other two tests with it, and in the direction
    /// nobody guesses**: with the measure widened and the line not moved, the end
    /// test fails at **200.375** — an *overshoot* past the measure, not a
    /// shortfall. Worth the sentence because the three assertions here read as
    /// three views of one number and are not: this flip overshoots, the `rsb` flip
    /// below undershoots, and only the pair of them pins the correction to
    /// *both* bearings rather than to some bearing.
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

    /// **The node's box does not move, and that is what makes the feature usable.**
    /// If switching this on shifted the box as well as the ink, two labels placed
    /// at the same x would be misaligned again by their origins and nothing would
    /// have been gained.
    ///
    /// 🚨 **This said *"a guard rather than a flip target: there is no plausible
    /// wrong version that moves the box"*, and the shipped version was one** (§15
    /// D855, `[X3-L2-01]`, `[X3-L6-01]`). The test asserted the start edge alone,
    /// and on an auto-width node the box *narrowed* by the first glyph's left
    /// bearing — 1.375 / 0.375 / 1.609 for these three — because a line started
    /// a bearing earlier pulls parley's width in with it. So two labels arranged
    /// by their right edges moved apart when the setting was switched on. The
    /// plausible wrong version was never "moves the box"; it was "tightens it on
    /// one side only", which is what shipped and which this could not see.
    ///
    /// ⚠️ **Flip-check, run**: `box_of` reading `layout.width()` again instead of
    /// the natural width fails at *"the box's width changed"*, on the first
    /// string, `Wamburg`, 72.25 to 71.875 — the finding's 0.375.
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

    /// **An end-aligned line hangs its last glyph out to the measure.** The
    /// advance end was already aligned without the setting — that is what makes
    /// this the *other* half of the feature rather than a restatement of the first
    /// test: the numbers that were equal are still equal, and the ones that were
    /// short now reach.
    ///
    /// ⚠️ **Flipped** by widening the measure by `lsb` alone instead of `lsb + rsb`:
    /// **red on the ink-reaches-the-edge assertion**, the predicted site, at
    /// 198.789 against 200 — which is the right bearing of the final `g`, and is
    /// exactly the gap the setting exists to close.
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

    /// 🚨 **A justified block spans its measure by ink at both ends**, which is the
    /// case the whole design turns on. Shifting a justified line cannot work — its
    /// two ends are pinned — so the correction is applied to the line's *measure*
    /// before parley justifies into it, and parley does the stretching.
    ///
    /// ⚠️ **Flipped** by shifting the line and leaving the measure alone
    /// (`x -= lsb` with no `measure += lsb + rsb`), which is the version that
    /// works for the first test and looks like it should work here: **red on the
    /// right-edge assertion**, at **199.023** against 200, with the left edge
    /// green — the whole block slid left and the right margin went ragged, which
    /// is the failure a start-edge-only implementation ships.
    ///
    /// ⚠️ **The predicted site was right and the predicted number was not**: this
    /// was written as 198.789, which is what the *end-aligned* test fails at under
    /// the `rsb` flip, and it is not what a justified line does. A justified line
    /// is stretched to its measure, so sliding it left leaves a gap of `lsb`
    /// rather than of `rsb` — a different bearing, from a different glyph, at the
    /// other end of the line. Corrected against the run rather than left standing.
    ///
    /// 🚨 **It asserted this at one width, 200, and the claim was false at 12 of
    /// 181** (§15 D855, `[X3-L6-02]`) — 200 being one of the widths where the
    /// second break pass happened to agree with the first. It sweeps every width
    /// from 120 to 300 now, which is what reaches the lines the second pass
    /// re-breaks. **Flip-check, run** against the version that shipped — one
    /// correcting pass, offsets read by line index from the uncorrected one: red
    /// at 123, the first width the finding listed.
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
            // **And no line leaves the box**, which is what the finding measured
            // failing: a line corrected by another line's bearings put its ink
            // past an edge. A *pinned* line (see `shape`) ends up to one bearing
            // short of the far edge — at 160 and 191 one line is — and that is
            // inside, which is the direction a line that cannot settle is allowed
            // to be wrong in.
            let lines = line_inks(&label(s, TextAlign::Justify, true, box_));
            for (i, (x0, x1)) in lines.iter().enumerate() {
                assert!(
                    *x0 > -0.01 && *x1 < w + 0.01,
                    "at {w}, line {i}'s ink {x0}..{x1} leaves its box: {lines:?}"
                );
            }
        }
    }

    /// Each line's ink, as `(x0, x1)`, keyed by baseline — a run belongs to the
    /// line whose baseline its first glyph sits on.
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

    /// **Every line of a wrapped, start-aligned block starts its ink on the
    /// edge, at every width** (§15 D855, `[X3-L1-01]`).
    ///
    /// 🚨 **At 160 the setting made the edge raggeder than leaving it off.** The
    /// second break pass moved words across line boundaries and then corrected
    /// each line by *another* line's first glyph: per-line ink `x0` read `0.000,
    /// 0.000, 0.422, −0.516` against `1.375, 1.203, 0.781, 1.203` off — a spread
    /// of 0.94 against 0.59, one line outside its own box. None of the tests that
    /// existed wrapped a start-aligned line, so none reached it.
    ///
    /// ⚠️ **Flip-checks, run.** One correcting pass, the shipped version: red at
    /// **123**, the sweep's first failing width, on line 2 at `1.016` — the
    /// finding's own 160 is further along. Settling without pinning: red at
    /// **160**, every line back at its uncorrected bearing (`1.375` …) — the
    /// cycle never settles, the cap runs out, and the fallback lays the node out
    /// uncorrected, which is what pinning exists to avoid and also what shows the
    /// fallback is the honest one.
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

    /// **The SVG writer gets this for free, and this test is what makes "for
    /// free" a fact rather than a hope.**
    ///
    /// `export_lines` and the canvas's `layout` both go through `shape`, and the
    /// correction is applied inside it — to parley's line geometry, before either
    /// consumer sees a line. So the writer's own computed `x` already carries it,
    /// which is what §15 D81 requires of every other alignment and indent: *"the
    /// writer no longer asks a viewer to re-align text it re-shaped itself."*
    ///
    /// **Asserted on `ExportLine::x` rather than on emitted markup**, because that
    /// is the seam — `ondin-export` turns this number into `<text x>`, and a test
    /// over the string would be testing the formatter.
    ///
    /// ⚠️ **Flipped** by gating the second pass off in `shape`, the plausible
    /// wrong version being a feature wired into the canvas path only: **red here**
    /// and green on the four tests above, which is the whole reason it exists —
    /// every one of those reads `layout`, so a canvas-only implementation passes
    /// all four and still exports unaligned text.
    ///
    /// 🚨 **This test was itself a doc-comment theft for about a minute**, and it
    /// is worth the sentence because of *when*. It was added by anchoring an
    /// `Edit` on the tail of the previous test's doc run, which put an item
    /// between that run and `a_justified_block_spans_its_measure_by_ink` — an
    /// instance of the class `CLAUDE.md` opens with, committed inside the same
    /// session that read that warning twice. Caught by habit 2, the neighbour
    /// grep, run as a routine rather than because anything felt wrong.
    /// *Knowing the trap is not what prevents the trap.*
    ///
    /// ⚠️ **No ordinal, deliberately.** This said *"the twenty-sixth instance"*,
    /// and the same edit sequence committed a **second** theft — `optical_offsets`
    /// merging its doc run with `break_lines`' — so one of the two is twenty-sixth
    /// and one twenty-seventh, and naming a position for this one alone is a count
    /// in prose that is already wrong. The tally lives in `CLAUDE.md` and is moved
    /// there.
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

    /// **`optical_offsets`' documented edges, each asserted** (§15 D855,
    /// `[X3-L6-04]`): a trailing space hangs nothing, a blank paragraph between
    /// two lines costs neither of them its correction, right-to-left text is
    /// corrected at its *right* edge, and empty content lays out.
    ///
    /// 🚨 **The RTL half is the one worth the test.** Its answer rests entirely on
    /// parley's `Run::visual_clusters` walking a right-to-left run in *visual*
    /// order, so the forward walk still meets the left-most glyph first. If that
    /// iteration order ever changes, `lsb` and `rsb` silently swap for every RTL
    /// document, and this is the only assertion in the workspace that would
    /// notice. All four were probed correct before this was written; the finding
    /// was that nothing held them.
    ///
    /// ⚠️ **Flip-check, run**: `optical_offsets` taking the last glyph whatever
    /// its ink — so a trailing space's whole advance becomes the right bearing —
    /// fails at *"a trailing space hangs nothing"*, 203.2 against 200.
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

    /// **Optical margins are the node's setting, and no paragraph attribute
    /// reaches them** (§15 D855, `[X3-L6-03]`, D163).
    ///
    /// The field's own doc decides the scope — *"paragraph scope and not
    /// spannable"* — and `ParaAttr` has no variant for it, so every paragraph's
    /// resolved style carries the node default. That makes the per-paragraph half
    /// of the gate in `Paragraphs::any_optical_margins`, and the per-paragraph
    /// read in `break_lines`, **inert**. §15 D163's rule is the reason to pin that
    /// rather than leave it: *"'inert' is indistinguishable from 'forgotten'
    /// without an assertion."*
    ///
    /// ⚠️ **The `match` is the tripwire**: a new `ParaAttrKind` fails to compile
    /// here until someone decides whether it reaches `optical_margins`, which is
    /// the moment the two inert clauses would stop being inert.
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
