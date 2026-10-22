//! The effect stack (§5.3a).
//!
//! A layer carries an ordered list of effects like its fills and strokes; the
//! order is the compositing order. [`Effect`] mirrors [`crate::Fill`]: a payload
//! and a `visible` flag.
//!
//! **Four effects**: drop shadow, inner shadow, layer blur and colour filters
//! are the ones that agree across the canvas (vello on wgpu), the raster export
//! (`vello_cpu`) and the SVG writer. Two are deliberately absent:
//!
//! - **Background blur** only means something when the export contains the
//!   backdrop, and SVG cannot express it (`BackgroundImage` was never
//!   implemented by any renderer).
//! - **Noise** would need one generator written three times and agreeing to the
//!   pixel; it belongs in the fill list as a brush question.
//!
//! **Blend modes are absent** too, including the per-effect one the mockup
//! draws: they are a deferred non-goal with a keyboard block reserved
//! (`shortcuts.md` §2).

use crate::item::Keyed;
use kurbo::{Insets, Vec2};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// The fraction of a blur's stated radius that is its gaussian standard
/// deviation.
///
/// The model stores the radius the designer typed, not the deviation the writers
/// want, and this is the one place the two are related. CSS `blur(12px)` means a
/// deviation of 6; SVG's `feGaussianBlur` and both vello backends take the
/// deviation directly.
pub const BLUR_DEVIATION: f64 = 0.5;

/// How many standard deviations of a gaussian are worth rasterizing.
///
/// Three (~99.7% of the kernel), matching `vello_common`'s `bounds_expansion` so
/// that the reach this module reports and the CPU backend's padding cannot
/// disagree.
pub const BLUR_CUTOFF: f64 = 3.0;

/// The largest blur radius an effect may carry — a shadow's or a layer blur's.
///
/// The CPU backend allocates padding proportional to [`reach`], so an absurd
/// radius is an enormous buffer. `Document::apply` refuses past it and the SVG
/// importer clamps to it.
pub const MAX_BLUR: f64 = 1_000.0;

/// Whether no blur in `effect` is past [`MAX_BLUR`].
pub fn blur_within_cap(effect: &Effect) -> bool {
    let ok = |r: f64| r <= MAX_BLUR;
    match &effect.kind {
        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => ok(s.blur),
        EffectKind::LayerBlur { radius } => ok(*radius),
        EffectKind::Filters(_) => true,
    }
}

/// The radius a freshly added blur — a shadow's or a [`EffectKind::LayerBlur`]'s
/// — starts on.
///
/// One number for both, because it is one quantity in the same units
/// ([`BLUR_DEVIATION`]); it is also why [`EffectKind::retyped`] can carry the
/// value across. Twelve rather than zero, for [`Shadow::default`]'s reason.
pub const DEFAULT_BLUR_RADIUS: f64 = 12.0;

/// The deviation, and then the distance, a blur of `radius` actually reaches.
///
/// `reach` is what bounds and culling care about; `deviation` is what the three
/// writers take.
pub fn deviation(radius: f64) -> f64 {
    radius.max(0.0) * BLUR_DEVIATION
}

/// See [`deviation`].
pub fn reach(radius: f64) -> f64 {
    deviation(radius) * BLUR_CUTOFF
}

/// The largest gaussian deviation an effect layer's buffer is worth rasterizing
/// at, in *buffer* pixels.
///
/// A separable blur costs `O(w · h · σ)`, and σ in device pixels is the authored
/// deviation times the zoom, so cost would grow without bound as someone zooms
/// in. See [`buffer_downscale`] for why bounding it costs no fidelity. Twenty-four
/// is a balance, not a threshold: the kernel is `2⌈3σ⌉+1` = 145 taps.
pub const BLUR_SIGMA_BUDGET: f64 = 24.0;

/// The widest kernel half-width any blur in this project builds, in pixels
/// (§15 D454).
///
/// A net, not a budget: [`BLUR_SIGMA_BUDGET`] yields a radius of 72, and this
/// stops a document (`<feGaussianBlur stdDeviation="1e30">` imports cleanly) from
/// asking `render::effects::kernel` for an enormous or capacity-overflowing
/// `Vec`. 512 is 7× the budget's radius, so it cannot bite on a reasonable blur.
///
/// Reaching it always means something upstream failed: a layer blur arrives at
/// σ ≤ 24 through [`buffer_downscale`], a shadow through [`shadow_downscale`].
/// A kernel at this cap is a wrong picture, but the cheapest one.
pub const MAX_KERNEL_RADIUS: usize = 512;

/// The largest block a shadow's silhouette may be resampled into, in device
/// pixels (§15 D454).
///
/// Bounds a different cost from [`MAX_KERNEL_RADIUS`], so neither makes the other
/// unnecessary: coarsening is `Θ(k)` in the block factor whatever the kernel does
/// (`render::effects::coarsen` walks `k × k` samples per coarse cell), so an
/// uncapped `k` is a hang. 4096 is already one coarse cell for almost anything
/// either backend allocates, and keeps `k as i32` (the shader's `Params.radius`)
/// from wrapping negative, which drew no shadow at all.
pub const MAX_SHADOW_BLOCK: u32 = 4096;

/// The largest deviation applied to the layer's output **as a whole**, in
/// authored units — the visible [`EffectKind::LayerBlur`]s and nothing else.
///
/// A shadow's blur does not count: a layer blur softens everything the layer
/// produces, but a drop shadow blurs a *silhouette* composited behind artwork
/// that stays sharp, so counting it would license a downscale that visibly
/// softens the drawing.
pub fn layer_blur_deviation(effects: &[Keyed<Effect>]) -> f64 {
    effects
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match e.kind {
            EffectKind::LayerBlur { radius } if radius > 0.0 => Some(deviation(radius)),
            _ => None,
        })
        .fold(0.0, f64::max)
}

/// How many device pixels one pixel of an effect layer's buffer should cover, so
/// that the blur that layer asks for stays inside [`BLUR_SIGMA_BUDGET`].
///
/// A gaussian of deviation σ carries nothing finer than σ, so a layer blurred by
/// 222 device pixels is fully described by a buffer eight times coarser, whose
/// blur is eight times narrower: the same picture for a sixty-fourth of the
/// convolution. Past the budget the cost stops growing with the zoom.
///
/// Only [`layer_blur_deviation`] may pay for this: the licence to lose resolution
/// is that the output already lost it. A stack that leaves any artwork sharp
/// gets `1.0`.
///
/// `1.0` — never less — when the layer is not blurred enough to have earned it.
/// Nothing between 1 and 2: a shallow downscale resamples without the blur being
/// wide enough to hide it and saves little, so an ordinary zoom is left exactly
/// alone.
pub fn buffer_downscale(effects: &[Keyed<Effect>], scale: (f64, f64)) -> f64 {
    let sigma = layer_blur_deviation(effects) * scale.0.abs().max(scale.1.abs());
    let k = sigma / BLUR_SIGMA_BUDGET;
    if k < 2.0 { 1.0 } else { k }
}

/// How many device pixels one pixel of a **shadow's own silhouette** should
/// cover, so that the shadow's blur stays inside [`BLUR_SIGMA_BUDGET`] too.
///
/// This is [`buffer_downscale`]'s argument applied where that function may not
/// go: the shadow's silhouette is an alpha field on its way to becoming a
/// gaussian of deviation σ, so *that output* carries nothing finer than σ. The
/// resampling happens inside the shadow pass, over the mask, and the sharp
/// artwork is never touched.
///
/// A whole number, because the mask is box-averaged into `k`×`k` blocks, blurred
/// at σ/k and read back bilinearly; `ceil` so σ/k lands at or under the budget.
/// The same deadband as [`buffer_downscale`]; `1` means "blur it as authored".
///
/// Capped at [`MAX_SHADOW_BLOCK`], and a non-finite σ gets the cap rather than
/// `1` (§15 D454): `k.ceil() as u32` saturates, and for σ = `inf` no downscale is
/// certainly wrong. At the cap σ/k is whatever it is; [`MAX_KERNEL_RADIUS`] stops
/// that number hurting, and the two together bound the pass.
pub fn shadow_downscale(sigma_device: f64) -> u32 {
    let k = sigma_device / BLUR_SIGMA_BUDGET;
    // NaN first: `<` cannot classify it, and it means "no answer" rather than
    // "very blurred". `inf.ceil().min(cap)` is the cap, so infinity needs no arm
    // of its own, and `-inf` shares the deadband's `1`.
    if k.is_nan() || k < 2.0 {
        1
    } else {
        k.ceil().min(f64::from(MAX_SHADOW_BLOCK)) as u32
    }
}

/// One entry in a layer's effect stack.
///
/// A `visible` flag beside the payload rather than removal-as-hiding, as
/// [`crate::Fill`] does: a shadow is half a dozen tuned numbers, and "let me see
/// it without the shadow" must not be destructive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    pub visible: bool,
    /// The number variables its numbers are bound to (§15 D1074), in
    /// [`EffectVars`]' order, in the item so an effects style, a reorder and a
    /// copy carry them. A bound number's literal is 0 and never drawn: readers
    /// take the list through `Document::resolved_effects`.
    #[serde(default, skip_serializing_if = "no_effect_vars")]
    pub vars: EffectVars,
}

/// An effect's number bindings: a shadow's X, Y, blur and spread, in that order
/// (§15 D1074). A layer blur's radius takes the blur slot and its other three are
/// always `None`; a filter's numbers bind nothing.
pub type EffectVars = [Option<crate::variable::VarId>; 4];

/// [`EffectVars`]' places.
pub const EFFECT_X: usize = 0;
pub const EFFECT_Y: usize = 1;
pub const EFFECT_BLUR: usize = 2;
pub const EFFECT_SPREAD: usize = 3;

fn no_effect_vars(v: &EffectVars) -> bool {
    v.iter().all(Option::is_none)
}

impl Effect {
    /// A new effect of `kind`, visible.
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            visible: true,
            vars: Default::default(),
        }
    }

    /// The effect's bindable numbers, in [`EffectVars`]' order — `None` where
    /// its kind has no such number (a layer blur's X, Y and spread; all four of
    /// a filter's). What a binding stands in for and a detach writes.
    pub fn numbers(&self) -> [Option<f64>; 4] {
        match &self.kind {
            EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => [
                Some(s.offset.x),
                Some(s.offset.y),
                Some(s.blur),
                Some(s.spread),
            ],
            EffectKind::LayerBlur { radius } => [None, None, Some(*radius), None],
            EffectKind::Filters(_) => [None; 4],
        }
    }

    /// Write number `slot` ([`EffectVars`]' places), where the kind has it.
    ///
    /// Every effect field's valve in the inspector writes through here, so "a
    /// drop and an inner shadow are authored identically" is stated once. The
    /// silent no-op is deliberate: a row can be retyped between the frame a field
    /// was drawn and the frame its drag ends, and a write must not invent a slot.
    pub fn set_number(&mut self, slot: usize, x: f64) {
        match &mut self.kind {
            EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => match slot {
                EFFECT_X => s.offset.x = x,
                EFFECT_Y => s.offset.y = x,
                EFFECT_BLUR => s.blur = x,
                EFFECT_SPREAD => s.spread = x,
                _ => {}
            },
            EffectKind::LayerBlur { radius } if slot == EFFECT_BLUR => *radius = x,
            _ => {}
        }
    }

    /// A bound number as it is read: a blur within `0..=`[`MAX_BLUR`], the range
    /// the effects card and `Document::apply` hold a typed one to, so a negative
    /// variable draws no blur and an absurd one no enormous buffer; an offset or a
    /// spread as it is, which the card lets go negative. `NaN` reads as the floor.
    pub fn number_read(slot: usize, x: f64) -> f64 {
        if slot == EFFECT_BLUR {
            // `max` first: it answers 0 for `NaN`, where `clamp` would keep it.
            x.max(0.0).clamp(0.0, MAX_BLUR)
        } else {
            x
        }
    }

    /// How far this effect's ink can land outside the geometry it applies to,
    /// or zero insets for one that cannot escape it.
    ///
    /// Positive means "outward", the opposite of [`Insets`]' usual reading:
    /// `rect + insets` is the answer. Invisible effects reach nowhere, so a hidden
    /// shadow does not keep widening the cached world bounds.
    pub fn escape(&self) -> Insets {
        if !self.visible {
            return Insets::ZERO;
        }
        self.kind.escape()
    }
}

/// What an effect *is*.
///
/// The two shadows share [`Shadow`]: they differ in where the ink lands, not in
/// what is authored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectKind {
    DropShadow(Shadow),
    InnerShadow(Shadow),
    /// Blurs the layer's own drawing, children included.
    LayerBlur {
        /// In the same units as a shadow's blur — see [`BLUR_DEVIATION`].
        radius: f64,
    },
    Filters(Filters),
}

impl EffectKind {
    /// See [`Effect::escape`]; this is the same answer without the visibility
    /// gate, and is the one to call when the flag has already been read.
    pub fn escape(&self) -> Insets {
        match self {
            // Grown by `spread`, blurred, then moved by `offset`: the reach is the
            // sum on the side the offset points at and the difference on the
            // other, so it cannot be one number.
            Self::DropShadow(s) => {
                let r = reach(s.blur) + s.spread.max(0.0);
                Insets {
                    x0: r - s.offset.x,
                    y0: r - s.offset.y,
                    x1: r + s.offset.x,
                    y1: r + s.offset.y,
                }
            }
            // Drawn inside the shape's own alpha, so none of it lands outside.
            Self::InnerShadow(_) => Insets::ZERO,
            Self::LayerBlur { radius } => Insets::uniform(reach(*radius)),
            // Per-pixel, so every pixel it touches was already there.
            Self::Filters(_) => Insets::ZERO,
        }
    }

    /// The label the inspector row and the layers panel both show.
    pub fn label(&self) -> &'static str {
        match self {
            Self::DropShadow(_) => "Drop shadow",
            Self::InnerShadow(_) => "Inner shadow",
            Self::LayerBlur { .. } => "Layer blur",
            Self::Filters(_) => "Filters",
        }
    }

    /// One of each kind, at its default, in the order the panel offers them.
    ///
    /// The order and the words are the model's, as with
    /// [`crate::Adjustment::ALL`], so a panel cannot offer three of the four or
    /// name one differently from the snapshot. A function rather than a `const`
    /// because every variant carries a payload.
    pub fn all_defaults() -> [Self; 4] {
        [
            Self::DropShadow(Shadow::default()),
            Self::InnerShadow(Shadow::default()),
            Self::LayerBlur {
                radius: DEFAULT_BLUR_RADIUS,
            },
            Self::Filters(Filters::default()),
        ]
    }

    /// Whether these are the same *kind*, whatever the two are holding.
    ///
    /// What the kind selector asks to decide which cell is lit; `==` cannot
    /// answer it, since two drop shadows at different offsets are not equal.
    pub fn same_kind(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// This effect's numbers carried into `to`'s kind — what the panel writes
    /// when a row is retyped.
    ///
    /// Carries across what the two kinds share and defaults the rest, so a retype
    /// does not throw tuned numbers away: the two shadows share every field, a
    /// shadow and a blur share the radius, and [`Filters`] shares nothing.
    ///
    /// Only the kind of `to` is read, so a caller can pass one of
    /// [`Self::all_defaults`] as the name of a kind.
    pub fn retyped(&self, to: &Self) -> Self {
        // Read once so every arm agrees about what "the blur" is.
        let blur = self.blur_radius();
        match to {
            Self::DropShadow(_) | Self::InnerShadow(_) => {
                let shadow = match self {
                    Self::DropShadow(s) | Self::InnerShadow(s) => s.clone(),
                    _ => Shadow {
                        blur,
                        ..Shadow::default()
                    },
                };
                match to {
                    Self::DropShadow(_) => Self::DropShadow(shadow),
                    _ => Self::InnerShadow(shadow),
                }
            }
            Self::LayerBlur { .. } => Self::LayerBlur { radius: blur },
            Self::Filters(_) => match self {
                Self::Filters(f) => Self::Filters(*f),
                _ => Self::Filters(Filters::default()),
            },
        }
    }

    /// The blur radius this kind carries, or [`DEFAULT_BLUR_RADIUS`] for the one
    /// kind that has none.
    ///
    /// Private: outside [`Self::retyped`], "the blur of a `Filters`" has no
    /// answer.
    fn blur_radius(&self) -> f64 {
        match self {
            Self::DropShadow(s) | Self::InnerShadow(s) => s.blur,
            Self::LayerBlur { radius } => *radius,
            Self::Filters(_) => DEFAULT_BLUR_RADIUS,
        }
    }
}

/// A drop or inner shadow's five authored numbers.
///
/// The opacity is the colour's alpha and not a field of its own, as for fills
/// (`paint::alpha_of`/`with_alpha`); a second spelling of alpha would disagree
/// with itself. The panel's `Opacity` field edits this alpha.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    /// Where the shadow sits relative to the layer, in the layer's own space.
    pub offset: Vec2,
    /// The blur *radius* as typed, not the gaussian deviation — see
    /// [`BLUR_DEVIATION`].
    pub blur: f64,
    /// How far the silhouette grows (or shrinks, when negative) before it is
    /// blurred. On the inner shadow too: it is the same dilation of the same
    /// silhouette.
    pub spread: f64,
    /// Straight (non-premultiplied) sRGB, alpha carrying the opacity
    /// (invariant 7).
    pub color: Color,
}

impl Default for Shadow {
    /// The shadow a freshly added row starts on: down, soft and black.
    ///
    /// Not zero offset and blur, which would be a row that visibly does nothing
    /// and reads as broken rather than untuned.
    fn default() -> Self {
        Self {
            offset: Vec2::new(0.0, 4.0),
            blur: DEFAULT_BLUR_RADIUS,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, 102),
        }
    }
}

/// The per-pixel colour adjustments, as one row of four.
///
/// Factors with 1.0 neutral, and degrees for the hue — the CSS and SVG
/// convention, so `feColorMatrix`/`feComponentTransfer` and the shader take the
/// numbers as they are. It deliberately differs from [`crate::ImageAdjust`]'s
/// `-1.0..=1.0` about zero (§5.5a): that is a photographic control surface, this
/// is a filter graph, and the writer that cannot invent arithmetic chooses the
/// units (§15 D186).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    /// 1.0 = unchanged. 0.0 is black.
    pub brightness: f64,
    /// 1.0 = unchanged. 0.0 is flat mid-grey.
    pub contrast: f64,
    /// 1.0 = unchanged. 0.0 is greyscale.
    pub saturation: f64,
    /// Degrees, 0.0 = unchanged. Wraps.
    pub hue: f64,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            hue: 0.0,
        }
    }
}

impl Filters {
    /// Whether every channel is on its neutral value, i.e. this row is present
    /// and doing nothing.
    ///
    /// A neutral `Filters` should emit no filter primitive at all rather than an
    /// identity matrix nothing can see but everything must composite.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// How many of the four are off their neutral value.
    pub fn changed(&self) -> usize {
        let d = Self::default();
        usize::from(self.brightness != d.brightness)
            + usize::from(self.contrast != d.contrast)
            + usize::from(self.saturation != d.saturation)
            + usize::from(self.hue != d.hue)
    }
}

/// One of [`Filters`]' four, named — so the panel draws them from this list
/// rather than from four hand-written rows.
///
/// [`crate::Adjustment`]'s twin, shaped the same way: the order and the words
/// live in one place. The units differ (§15 D333): that one is `-1..=1` about
/// zero, this one goes straight into `feColorMatrix`, so this list carries
/// [`Self::neutral`] and [`Self::degrees`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterChannel {
    Brightness,
    Contrast,
    Saturation,
    Hue,
}

impl FilterChannel {
    /// The four, in the order the popover lists them: the two that move light,
    /// then the two that move colour.
    pub const ALL: [FilterChannel; 4] = [
        Self::Brightness,
        Self::Contrast,
        Self::Saturation,
        Self::Hue,
    ];

    /// The word the field carries. Words rather than glyphs, for the reason
    /// [`crate::Adjustment::label`] gives: no icon set has four tone glyphs
    /// distinguishable at 15pt.
    pub fn label(self) -> &'static str {
        match self {
            Self::Brightness => "Brightness",
            Self::Contrast => "Contrast",
            Self::Saturation => "Saturation",
            Self::Hue => "Hue",
        }
    }

    /// What each end of the field does, for its tooltip.
    ///
    /// Written as three points — bottom, neutral, top — because the field draws a
    /// bar growing from its middle (§15 D343) and the tooltip says where that
    /// middle sits and how far the ends are.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Brightness => "0% is black, 100% leaves it alone, 200% is twice as bright",
            Self::Contrast => "0% is flat mid-grey, 100% leaves it alone, 200% is twice the spread",
            Self::Saturation => "0% is greyscale, 100% leaves it alone, 200% is twice as colourful",
            Self::Hue => "Turn every colour round the wheel",
        }
    }

    /// The value at which this channel changes nothing — `1.0` for the three
    /// factors, `0.0` for the angle.
    ///
    /// The panel dims a field at rest against this rather than a literal, which
    /// keeps it and [`Filters::changed`]'s count from disagreeing.
    pub fn neutral(self) -> f64 {
        match self {
            Self::Hue => 0.0,
            _ => 1.0,
        }
    }

    /// Whether this channel is an angle in **degrees** rather than a factor.
    ///
    /// Asked here rather than by the panel matching on `Hue`, so a fifth channel
    /// in either unit is a line in this file and nothing in the popover.
    pub fn degrees(self) -> bool {
        matches!(self, Self::Hue)
    }

    pub fn of(self, f: &Filters) -> f64 {
        match self {
            Self::Brightness => f.brightness,
            Self::Contrast => f.contrast,
            Self::Saturation => f.saturation,
            Self::Hue => f.hue,
        }
    }

    /// Write one of the four.
    ///
    /// Non-finite is refused here rather than at the control, as
    /// [`crate::Adjustment::set`] does: a typed `1/0` evaluates to `inf`, and a
    /// `NaN` factor would write a filter no reader can render. There is no
    /// other clamp: a factor has no meaningful upper bound and the hue wraps.
    pub fn set(self, f: &mut Filters, v: f64) {
        let v = if v.is_finite() { v } else { self.neutral() };
        match self {
            Self::Brightness => f.brightness = v,
            Self::Contrast => f.contrast = v,
            Self::Saturation => f.saturation = v,
            Self::Hue => f.hue = v,
        }
    }
}

/// Grow a **world** box by a **local** escape.
///
/// An effect is authored in the layer's own space (a shadow at `(0, 4)` follows
/// the layer when it rotates) while bounds are cached in world space, so the
/// padding is carried across.
///
/// The escape is split into an offset and a radius rather than transformed as
/// four corners: a blur's reach is isotropic, and as a local square its rotated
/// AABB is up to √2 too big. Split, the offset is a vector the linear part maps
/// and the radius becomes the exact half-extents of the mapped ellipse.
///
/// Never shrinks: each edge takes the wider of where the effect reached and where
/// the layer already was, which is what the `min`/`max` against zero are for.
///
/// A non-finite `escape` is refused upstream: `build::effect_is_finite` rejects
/// it at the operation, so production callers (all pass
/// `stack_escape(node.effects())`) hand over a finite one. The guard below is only
/// the floor; the unbounded rect it answers becomes the absence §5.9 prefers in
/// `geometry::measurable` (§15 D495).
///
/// The preview path (`RenderOverrides`, uncommitted) does not come through an
/// operation; the inspector's ranges, [`MAX_BLUR`] and `MAX_OFFSET`, bound it.
/// Those bounds, `Document::apply` and `svg_in::deviation_of` are not redundant
/// with this function.
pub fn escaped(bounds: kurbo::Rect, escape: Insets, world: kurbo::Affine) -> kurbo::Rect {
    if escape == Insets::ZERO {
        return bounds;
    }
    // An escape nobody can measure covers everything and must not read as covering
    // nothing: below, an infinite inset gives `inf − inf` = NaN and `f64::min`
    // returns the non-NaN operand, leaving the box unchanged. This is the floor
    // under a radius near `f64::MAX`, where the answer really does overflow.
    if !(escape.x0.is_finite()
        && escape.y0.is_finite()
        && escape.x1.is_finite()
        && escape.y1.is_finite())
    {
        return kurbo::Rect::new(
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::INFINITY,
        );
    }
    // x0 = radius − offset and x1 = radius + offset (`EffectKind::escape`), so
    // this reads the pair back apart.
    let off = Vec2::new((escape.x1 - escape.x0) * 0.5, (escape.y1 - escape.y0) * 0.5);
    let rad = Vec2::new((escape.x0 + escape.x1) * 0.5, (escape.y0 + escape.y1) * 0.5);
    let [a, b, c, d, _, _] = world.as_coeffs();
    // The linear part only: the box is already where the translation put it.
    let t = Vec2::new(a * off.x + c * off.y, b * off.x + d * off.y);
    // Half-extents of the mapped radius as an **ellipse**, not a box: the box
    // reading `|a|·rx + |c|·ry` returns `r·√2` at 45°, but a blur's reach is a
    // disc, whose extent is the row's euclidean norm (exactly `r` under rotation).
    //
    // `hypot`, not `(x² + y²).sqrt()` (§15 D676): the square overflows near
    // `1.3e154` where the product stays finite, which gave an infinite ink box
    // that culling, raster sizing and the SVG writer all read.
    let h = Vec2::new((a * rad.x).hypot(c * rad.y), (b * rad.x).hypot(d * rad.y));
    kurbo::Rect::new(
        bounds.x0 + (t.x - h.x).min(0.0),
        bounds.y0 + (t.y - h.y).min(0.0),
        bounds.x1 + (t.x + h.x).max(0.0),
        bounds.y1 + (t.y + h.y).max(0.0),
    )
}

/// The device box an effect layer's buffer must cover: the layer's ink box,
/// clipped to the target it will be composited into — but the target **grown by
/// the escape first**.
///
/// Clipping to the bare target would cut the silhouette the shadow is read from,
/// so a layer scrolled half off the top would cast a shadow from the visible
/// sliver alone, and one scrolled fully off none at all (§15 D342).
///
/// The two sides swap: ink landing at the target's right edge is cast from a
/// silhouette `offset` to the *left* of it, a distance packed into `escape.x0`,
/// so the right side grows by `x0` and the left side by `x1`.
///
/// Grown, never shrunk: a large offset makes one inset negative, and pulling the
/// target's edge inwards would cut the layer itself, which is drawn in this
/// buffer too.
pub fn buffer_box(
    device_ink: kurbo::Rect,
    target: kurbo::Rect,
    escape: Insets,
    scale: (f64, f64),
) -> kurbo::Rect {
    grow_to_escape(device_ink, target, escape, scale, 1.0)
}

/// [`buffer_box`] with the escape scaled by `t`, which is what
/// [`buffer_box_within`] bisects on.
fn grow_to_escape(
    device_ink: kurbo::Rect,
    target: kurbo::Rect,
    escape: Insets,
    scale: (f64, f64),
    t: f64,
) -> kurbo::Rect {
    let grown = kurbo::Rect::new(
        target.x0 - escape.x1.max(0.0) * scale.0 * t,
        target.y0 - escape.y1.max(0.0) * scale.1 * t,
        target.x1 + escape.x0.max(0.0) * scale.0 * t,
        target.y1 + escape.y0.max(0.0) * scale.1 * t,
    );
    device_ink.intersect(grown)
}

/// [`buffer_box`], with the escape given up as far as necessary to keep the
/// buffer inside `budget` device pixels.
///
/// The escape is in device pixels, so it grows with the zoom without bound: at
/// 32× an 18-unit shadow blur takes a viewport-sized buffer to almost three times
/// the page's area, past vello's fixed intermediate buffers, where the shader
/// hangs and the GPU resets (§15 D402).
///
/// What is given up is the far tail of a shadow cast from off-screen, which then
/// fades short. It bites at every zoom, not only the extreme one, since a layer
/// covering the viewport is already at the budget; the loss is the faint end of a
/// gaussian (worst channel error 1/255 against an unclamped CPU render).
///
/// Both backends call this: backends that chose different buffer boxes would blur
/// the same drawing differently.
pub fn buffer_box_within(
    device_ink: kurbo::Rect,
    target: kurbo::Rect,
    escape: Insets,
    scale: (f64, f64),
    budget: f64,
) -> kurbo::Rect {
    let full = grow_to_escape(device_ink, target, escape, scale, 1.0);
    if full.area() <= budget {
        return full;
    }
    // Monotonic in `t`, so twenty halvings land within a pixel of the largest
    // escape that fits. Bisection rather than a solve: the area is piecewise
    // quadratic in `t` (the box intersects the layer's ink).
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if grow_to_escape(device_ink, target, escape, scale, mid).area() <= budget {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    grow_to_escape(device_ink, target, escape, scale, lo)
}

/// How far a whole stack reaches outside the geometry it is applied to.
///
/// Per stage of `ondin_render::effects::run`, and a union within each. Stage 1 is
/// one isotropic reach from the convolved deviation of every visible `LayerBlur`
/// (`σ = √(σ₁² + σ₂²)`, since the blurs apply in sequence; `Filters` add
/// nothing). Stage 2 is each shadow's own escape grown by stage 1 on every side,
/// since stage 1's output is its silhouette. Two shadows reaching 10px each reach
/// 10, not 20.
///
/// Too small is the one error these bounds may not make: the box is also the
/// buffer (`grow_to_escape` intersects `device_ink` with it), so an under-covering
/// stage cuts the shadow's tail off with a hard edge.
pub fn stack_escape(effects: &[Keyed<Effect>]) -> Insets {
    // Stage 1: the visible layer blurs convolve, composed in deviations and turned
    // back into a reach once (§15 D473).
    //
    // Scaled by the largest deviation before squaring (§15 D685): the plain
    // `deviation(r).powi(2)` overflowed near `r ≈ 2.7e154`, a radius
    // `build::effect_is_finite` accepts, and the infinite escape then made
    // [`escaped`] return the box unchanged. Dividing through by the largest term,
    // summing squares `≤ 1` and multiplying back is exact where the plain form
    // was, and finite where it was not.
    let sigmas: Vec<f64> = effects
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match e.kind {
            EffectKind::LayerBlur { radius } => Some(deviation(radius)),
            _ => None,
        })
        .collect();
    let largest = sigmas.iter().copied().fold(0.0_f64, f64::max);
    let stage1 = if largest > 0.0 {
        let normalized: f64 = sigmas.iter().map(|s| (s / largest).powi(2)).sum();
        largest * normalized.sqrt() * BLUR_CUTOFF
    } else {
        0.0
    };

    effects.iter().fold(Insets::uniform(stage1), |acc, e| {
        let i = e.escape();
        // A shadow is cast from stage 1's output, so its reach starts from a
        // silhouette already `stage1` wider. Nothing else composes: a layer
        // blur's escape is the seed, and `Filters` and `InnerShadow` escape nothing.
        let grown = if matches!(e.kind, EffectKind::DropShadow(_)) && e.visible {
            Insets {
                x0: i.x0 + stage1,
                y0: i.y0 + stage1,
                x1: i.x1 + stage1,
                y1: i.y1 + stage1,
            }
        } else {
            i
        };
        Insets {
            x0: acc.x0.max(grown.x0),
            y0: acc.y0.max(grown.y0),
            x1: acc.x1.max(grown.x1),
            y1: acc.y1.max(grown.y1),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bound blur reads within `0..=MAX_BLUR` and `NaN` reads as 0; an offset
    /// reads as it is. Flip: `number_read` without its cap → the cap fails.
    #[test]
    fn a_bound_blur_reads_within_its_range() {
        assert_eq!(Effect::number_read(EFFECT_BLUR, 1e30), MAX_BLUR, "the cap");
        assert_eq!(Effect::number_read(EFFECT_BLUR, -4.0), 0.0);
        assert_eq!(Effect::number_read(EFFECT_BLUR, f64::NAN), 0.0);
        assert_eq!(Effect::number_read(EFFECT_X, -1e30), -1e30);
    }

    /// A shadow pushed to `+y` reaches further down and less far up. Flipping
    /// either sign in `escape` fails this.
    #[test]
    fn a_shadows_reach_follows_the_side_its_offset_points_at() {
        let e = Effect::new(EffectKind::DropShadow(Shadow {
            offset: Vec2::new(0.0, 10.0),
            blur: 0.0,
            spread: 0.0,
            color: Color::BLACK,
        }));
        let i = e.escape();
        assert_eq!(i.y1, 10.0, "reaches 10 down");
        assert_eq!(i.y0, -10.0, "and 10 *inside* the top edge");
        assert_eq!((i.x0, i.x1), (0.0, 0.0), "nothing sideways");
    }

    /// A radius of 12 is deviation 6 and reach 18; a dropped halving gives 36 and
    /// a doubled one 9.
    #[test]
    fn a_blurs_reach_is_three_deviations_and_a_deviation_is_half_the_radius() {
        assert_eq!(deviation(12.0), 6.0);
        assert_eq!(reach(12.0), 18.0);
        let i = Effect::new(EffectKind::LayerBlur { radius: 12.0 }).escape();
        assert_eq!(i, Insets::uniform(18.0));
    }

    /// Two layer blurs convolve at `σ = √(σ₁² + σ₂²)` (§6.4, §15 D473), neither
    /// summing nor taking the larger. The fixture makes the three readings three
    /// different numbers — 28 summed, 18 unioned, 20.59 composed.
    #[test]
    fn two_layer_blurs_convolve_rather_than_summing_or_taking_the_larger() {
        let stack = crate::keyed_by_position([
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::LayerBlur { radius: 20.0 / 3.0 }),
        ]);
        assert_eq!(reach(20.0 / 3.0), 10.0, "the fixture is the size it says");
        let want = (6.0f64.powi(2) + (10.0f64 / 3.0).powi(2)).sqrt() * BLUR_CUTOFF;
        let got = stack_escape(&stack);
        assert!(
            (got.x0 - want).abs() < 1e-9 && got == Insets::uniform(got.x0),
            "√(σ₁² + σ₂²)·3 is {want:.4}, got {got:?}"
        );
        assert!(
            got.x0 > 18.0 && got.x0 < 28.0,
            "and it must sit strictly between the union (18) and the sum (28), \
             or this fixture cannot tell the three rules apart: {}",
            got.x0
        );
    }

    /// A drop shadow is cast from the blurred layer, so its reach starts one
    /// stage 1 further out: `[LayerBlur 12, DropShadow offset (0, 40) blur 4]`
    /// reaches `18 + 6 + 40 = 64` down, not `max(18, 6 + 40) = 46`.
    ///
    /// flip: seeding the fold with `Insets::ZERO` and dropping the `grown` arm in
    /// `stack_escape` → fails at the `y1` assertion (46 against 64); the
    /// `InnerShadow` control stays green.
    #[test]
    fn a_drop_shadow_over_a_layer_blur_reaches_past_both() {
        let stack = crate::keyed_by_position([
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 40.0),
                blur: 4.0,
                spread: 0.0,
                color: Color::BLACK,
            })),
        ]);
        let i = stack_escape(&stack);
        assert!(
            (i.y1 - 64.0).abs() < 1e-9,
            "18 of layer blur, then 6 of the shadow's own blur, then the 40 it is \
             offset by: {}",
            i.y1
        );
        assert!(
            (i.x1 - 24.0).abs() < 1e-9,
            "sideways the shadow reaches 6 from stage 1's edge, so 18 + 6: {}",
            i.x1
        );

        // Control: an inner shadow escapes nothing, so stage 1 is the whole answer;
        // a fold that grew every effect by stage 1 would fail here.
        let inner = crate::keyed_by_position([
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::InnerShadow(Shadow {
                offset: Vec2::new(0.0, 40.0),
                blur: 4.0,
                spread: 0.0,
                color: Color::BLACK,
            })),
        ]);
        assert_eq!(
            stack_escape(&inner),
            Insets::uniform(18.0),
            "an inner shadow is drawn inside the alpha and escapes nothing"
        );
    }

    /// A hidden effect stops widening the bounds.
    #[test]
    fn a_hidden_effect_reaches_nowhere() {
        let mut e = Effect::new(EffectKind::LayerBlur { radius: 40.0 });
        assert_ne!(
            e.escape(),
            Insets::ZERO,
            "the fixture reaches while visible"
        );
        e.visible = false;
        assert_eq!(e.escape(), Insets::ZERO);
    }

    /// An inner shadow escapes nothing, however far its blur and offset reach; an
    /// arm shared with `DropShadow` would answer a 60px escape.
    #[test]
    fn an_inner_shadow_never_escapes_the_shape() {
        let i = Effect::new(EffectKind::InnerShadow(Shadow {
            offset: Vec2::new(20.0, 20.0),
            blur: 40.0,
            spread: 10.0,
            color: Color::BLACK,
        }))
        .escape();
        assert_eq!(i, Insets::ZERO);
    }

    /// A 10px blur on a layer rotated 45° reaches 10px each way, not 14.14; the
    /// four-corner version of `escaped` answers 14.142….
    #[test]
    fn a_rotated_blur_reaches_its_radius_and_not_its_diagonal() {
        let e = Effect::new(EffectKind::LayerBlur { radius: 20.0 / 3.0 });
        assert_eq!(reach(20.0 / 3.0), 10.0, "the fixture reaches what it says");
        let turned = kurbo::Affine::rotate(std::f64::consts::FRAC_PI_4);
        let out = escaped(kurbo::Rect::new(0.0, 0.0, 100.0, 100.0), e.escape(), turned);
        assert!(
            (out.x0 - -10.0).abs() < 1e-9 && (out.x1 - 110.0).abs() < 1e-9,
            "reached {out:?}, wanted the radius on each side"
        );
    }

    /// A shadow thrown down and right must not pull the top-left edges inward;
    /// adding the signed excursion to every edge walks the origin to (6, 6).
    #[test]
    fn an_offset_shadow_grows_the_box_without_shrinking_it() {
        let e = Effect::new(EffectKind::DropShadow(Shadow {
            offset: Vec2::new(6.0, 6.0),
            blur: 0.0,
            spread: 0.0,
            color: Color::BLACK,
        }));
        let out = escaped(
            kurbo::Rect::new(0.0, 0.0, 50.0, 50.0),
            e.escape(),
            kurbo::Affine::IDENTITY,
        );
        assert_eq!(out, kurbo::Rect::new(0.0, 0.0, 56.0, 56.0));
    }

    /// `changed()` and `is_neutral()` agree; a `changed()` that counted fields
    /// rather than comparing them answers 4 for the default.
    #[test]
    fn a_neutral_filter_row_counts_nothing_changed() {
        let mut f = Filters::default();
        assert!(f.is_neutral());
        assert_eq!(f.changed(), 0);
        f.hue = 30.0;
        assert!(!f.is_neutral());
        assert_eq!(f.changed(), 1);
    }

    /// Retyping a row carries across what the two kinds share. Every arm asserts a
    /// value the default does not have, since a fresh default passes any check of
    /// the kind alone.
    #[test]
    fn retyping_keeps_what_the_two_kinds_share() {
        let tuned = Shadow {
            offset: Vec2::new(7.0, -3.0),
            blur: 21.0,
            spread: 5.0,
            color: Color::from_rgba8(10, 20, 30, 40),
        };
        let [drop, inner, blur, filters] = EffectKind::all_defaults();

        // Two shadows share everything they have, so nothing is lost either way.
        let EffectKind::InnerShadow(s) = EffectKind::DropShadow(tuned.clone()).retyped(&inner)
        else {
            panic!("retyping to an inner shadow gave something else");
        };
        assert_eq!(s, tuned, "the whole shadow crosses between the two kinds");

        // A shadow and a blur share the radius and nothing else.
        let EffectKind::LayerBlur { radius } = EffectKind::DropShadow(tuned.clone()).retyped(&blur)
        else {
            panic!("retyping to a layer blur gave something else");
        };
        assert_eq!(radius, 21.0, "the blur radius crosses, not the default 12");
        let EffectKind::DropShadow(back) = EffectKind::LayerBlur { radius: 21.0 }.retyped(&drop)
        else {
            panic!("retyping back to a drop shadow gave something else");
        };
        assert_eq!(back.blur, 21.0, "and it crosses back");
        assert_eq!(
            back.offset,
            Shadow::default().offset,
            "what a blur does not carry comes back at its default"
        );

        // `Filters` shares nothing with any of them, in either direction.
        let EffectKind::Filters(f) = EffectKind::DropShadow(tuned.clone()).retyped(&filters) else {
            panic!("retyping to filters gave something else");
        };
        assert!(f.is_neutral(), "a shadow has no channel to hand a filter");
        let bright = Filters {
            brightness: 1.5,
            ..Filters::default()
        };
        let EffectKind::Filters(kept) = EffectKind::Filters(bright).retyped(&filters) else {
            panic!("retyping filters to filters gave something else");
        };
        assert_eq!(kept.brightness, 1.5, "and filters keep their own");
    }

    /// `same_kind` answers about the variant, not the value; `==` fails the second
    /// assertion.
    #[test]
    fn same_kind_ignores_what_the_kinds_hold() {
        let a = EffectKind::DropShadow(Shadow::default());
        let b = EffectKind::DropShadow(Shadow {
            blur: 99.0,
            ..Shadow::default()
        });
        assert_ne!(a, b, "the fixture is two *different* drop shadows");
        assert!(a.same_kind(&b));
        assert!(!a.same_kind(&EffectKind::InnerShadow(Shadow::default())));
    }

    /// A fresh row of every kind does something visible, and `all_defaults` and
    /// `Shadow::default` agree about the blur.
    #[test]
    fn a_freshly_added_row_of_every_kind_is_visible_work() {
        for kind in EffectKind::all_defaults() {
            match &kind {
                EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
                    assert!(s.blur > 0.0 && s.color.components[3] > 0.0, "{kind:?}");
                }
                EffectKind::LayerBlur { radius } => {
                    assert_eq!(*radius, Shadow::default().blur, "{kind:?}");
                }
                // Neutral on purpose: off-neutral channels would change the
                // artwork the instant the row was added.
                EffectKind::Filters(f) => assert!(f.is_neutral(), "{kind:?}"),
            }
        }
    }

    /// `FilterChannel::ALL` and `Filters::changed` count the same four. Compared
    /// to the literal 4, not `ALL.len()`, which shrinks with the list.
    #[test]
    fn every_channel_the_summary_counts_has_a_control() {
        let mut off = Filters::default();
        for c in FilterChannel::ALL {
            c.set(&mut off, c.neutral() + 0.5);
        }
        assert_eq!(
            off.changed(),
            4,
            "`changed()` and `FilterChannel::ALL` disagree about how many there are"
        );
        // The list's neutral is `Default`'s neutral.
        let mut back = off;
        for c in FilterChannel::ALL {
            c.set(&mut back, c.neutral());
        }
        assert!(back.is_neutral());
        assert_eq!(back, Filters::default());
    }

    /// A channel refuses a non-finite value (`expr::eval` hands `1/0` over) at the
    /// model.
    #[test]
    fn a_channel_refuses_a_value_that_is_not_a_number() {
        let mut f = Filters::default();
        FilterChannel::Brightness.set(&mut f, 2.0);
        FilterChannel::Brightness.set(&mut f, f64::INFINITY);
        assert_eq!(
            f.brightness, 1.0,
            "an infinity puts the channel back at rest"
        );
        FilterChannel::Hue.set(&mut f, f64::NAN);
        assert_eq!(f.hue, 0.0);
        assert!(f.is_neutral());
    }

    /// The buffer shrinks by the factor the deviation overshoots by, so σ in
    /// buffer pixels lands on the budget at any zoom; unbounded, a deep zoom
    /// outlasts Windows' two-second GPU reset (§15 D395).
    #[test]
    fn a_blurs_kernel_stops_growing_once_the_buffer_starts_shrinking() {
        let fx = crate::keyed_by_position([Effect::new(EffectKind::LayerBlur { radius: 55.5 })]);
        // 1x: deviation 27.75, under the budget's deadband, so nothing changes.
        assert_eq!(buffer_downscale(&fx, (1.0, 1.0)), 1.0);
        for zoom in [4.0, 8.0, 16.0, 64.0, 1000.0] {
            let k = buffer_downscale(&fx, (zoom, zoom));
            let sigma = deviation(55.5) * zoom / k;
            assert!(
                (sigma - BLUR_SIGMA_BUDGET).abs() < 1e-9,
                "at {zoom}x the buffer sigma is {sigma}, not the budget"
            );
        }
    }

    /// The deadband keeps an ordinary zoom pixel-identical: below 2x the blur is
    /// not wide enough to hide the resampling.
    #[test]
    fn a_shallow_downscale_is_declined_rather_than_taken() {
        let fx = crate::keyed_by_position([Effect::new(EffectKind::LayerBlur { radius: 55.5 })]);
        // Just under 2x, and just over.
        assert_eq!(buffer_downscale(&fx, (1.7, 1.7)), 1.0);
        assert!(buffer_downscale(&fx, (1.8, 1.8)) > 2.0);
    }

    /// Only a layer blur may buy a downscale: a shadow composites behind artwork
    /// that stays sharp, so counting its deviation would visibly soften the
    /// drawing.
    #[test]
    fn a_shadows_blur_does_not_buy_a_downscale() {
        let huge = Shadow {
            blur: 400.0,
            ..Default::default()
        };
        for kind in [
            EffectKind::DropShadow(huge.clone()),
            EffectKind::InnerShadow(huge.clone()),
        ] {
            let fx = crate::keyed_by_position([Effect::new(kind)]);
            assert_eq!(
                buffer_downscale(&fx, (8.0, 8.0)),
                1.0,
                "the layer under this shadow is still sharp"
            );
            assert_eq!(layer_blur_deviation(&fx), 0.0);
        }
        // And a stack holding both is bounded by the *layer* blur, not the shadow's.
        let fx = crate::keyed_by_position([
            Effect::new(EffectKind::LayerBlur { radius: 55.5 }),
            Effect::new(EffectKind::DropShadow(huge)),
        ]);
        assert_eq!(layer_blur_deviation(&fx), deviation(55.5));
    }

    /// A hidden effect buys nothing, as with `Effect::escape`: an undrawn row
    /// cannot license losing resolution.
    #[test]
    fn a_hidden_blur_does_not_buy_a_downscale() {
        let fx = crate::keyed_by_position([Effect {
            kind: EffectKind::LayerBlur { radius: 55.5 },
            visible: false,
            vars: Default::default(),
        }]);
        assert_eq!(buffer_downscale(&fx, (8.0, 8.0)), 1.0);
    }

    /// Whenever the buffer fits, `buffer_box_within` is `buffer_box` exactly.
    /// `budget` is a page count; reading it as the layer's own area would clamp
    /// here.
    #[test]
    fn an_escape_that_fits_inside_the_page_is_left_alone() {
        let page = kurbo::Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let ink = kurbo::Rect::new(100.0, 100.0, 500.0, 400.0);
        let escape = Insets::uniform(60.0);
        assert_eq!(
            buffer_box_within(ink, page, escape, (1.0, 1.0), 1920.0 * 1080.0),
            buffer_box(ink, page, escape, (1.0, 1.0)),
        );
    }

    /// A layer that covers the page gives up its escape, never the page itself
    /// (§15 D402). The area assertion is what the budget is for; the cover
    /// assertion is the safety rule, since `t = 0` is the target itself.
    ///
    /// flip: `buffer_box_within` returning the bisection's `hi` rather than `lo` →
    /// fails at the area assertion.
    #[test]
    fn a_layer_covering_the_page_gives_up_its_escape_rather_than_the_gpu() {
        let page = kurbo::Rect::new(0.0, 0.0, 3520.0, 1980.0);
        let budget = 3520.0 * 1980.0;
        // The layer's ink is far bigger than the view, as at any deep zoom.
        let ink = kurbo::Rect::new(-40000.0, -40000.0, 40000.0, 40000.0);
        let got = buffer_box_within(ink, page, Insets::uniform(27.0), (32.0, 32.0), budget);
        assert!(
            got.area() <= budget,
            "buffer is {}x{} = {} against a budget of {budget}",
            got.width(),
            got.height(),
            got.area()
        );
        assert!(
            got.contains_rect(page.intersect(ink)),
            "the visible part of the layer must still be in the buffer, got {got:?}"
        );
    }

    /// The clamp keeps as much escape as fits: with room for twice the target's
    /// area the buffer comes back close to the budget.
    ///
    /// flip: `buffer_box_within` returning `t = 0` on any overflow → passes the
    /// test above and fails at the `0.99 * budget` assertion.
    #[test]
    fn the_clamp_spends_the_whole_budget_it_is_given() {
        let page = kurbo::Rect::new(0.0, 0.0, 1000.0, 1000.0);
        let ink = kurbo::Rect::new(-9000.0, -9000.0, 9000.0, 9000.0);
        let budget = 2_000_000.0;
        let got = buffer_box_within(ink, page, Insets::uniform(500.0), (1.0, 1.0), budget);
        assert!(got.area() <= budget, "{} over budget", got.area());
        assert!(
            got.area() > 0.99 * budget,
            "only {} of a {budget} budget was spent",
            got.area()
        );
    }
}
