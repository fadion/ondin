//! The effect stack (§5.3a).
//!
//! A layer carries an ordered list of effects the way it carries fills and
//! strokes, and for the same reason: the order is the compositing order, and a
//! designer who wants two drop shadows at different distances is not asking for
//! anything unusual. [`Effect`] mirrors [`crate::Fill`] exactly — a payload and
//! a `visible` flag — so the panel that draws a fill row and the panel that
//! draws an effect row are the same shape of code.
//!
//! **Four effects, and the four are not an arbitrary subset.** Drop shadow,
//! inner shadow, layer blur and colour filters are the ones that can be made to
//! agree across all three of the canvas (vello on wgpu), the raster export
//! (`vello_cpu`) and the SVG writer. The two that were designed alongside them
//! and are *not* here — background blur and noise — each fail that test in their
//! own way, and the reasoning is recorded rather than left to be rediscovered:
//!
//! - **Background blur** blurs the backdrop, so it only means anything when the
//!   thing being exported contains the backdrop. Export the frame and it is
//!   already baked in; export the blurred layer alone and there is nothing
//!   behind it to sample. SVG cannot express it at all: the `BackgroundImage`
//!   filter input was specified and never implemented by any renderer, which is
//!   why Figma's own SVG output substitutes a transparent flood for it.
//! - **Noise** would need the same generator written three times — a WGSL
//!   shader, the CPU rasterizer and SVG's `feTurbulence` — and agreeing to the
//!   pixel, or the file stops matching the screen. It is also probably not an
//!   effect: Figma moved noise into the *fill* list, where it inherits the
//!   brush machinery and the SVG writer's existing embedded-raster fallback
//!   makes it exact for free. Filed as a brush question, not a deferred effect.
//!
//! **Blend modes are deliberately absent**, including the per-effect one the
//! mockup draws inside the shadow popovers. Blend modes are their own deferred
//! non-goal with a keyboard block reserved behind them (`shortcuts.md` §2 holds
//! `Shift+3`…`Shift+9`), and reversing that deserves its own decision rather
//! than a ride-along on this one. Both backends' `push_layer` already takes a
//! blend mode and SVG has `feBlend`, so the cost when it comes will be small.

use kurbo::{Insets, Vec2};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// The fraction of a blur's stated radius that is its gaussian standard
/// deviation.
///
/// **The model stores the radius the designer typed, not the deviation the
/// writers want**, and this is the one place the two are related. CSS spells a
/// blur as `blur(12px)` and means a deviation of 6; SVG's `feGaussianBlur` and
/// both vello backends take the deviation directly. Storing the deviation would
/// make every panel field divide on read and multiply on write, and would put a
/// number in the save file that no part of the UI ever shows.
pub const BLUR_DEVIATION: f64 = 0.5;

/// How many standard deviations of a gaussian are worth rasterizing.
///
/// Three, which captures ~99.7% of the kernel and is the same cutoff
/// `vello_common`'s own `bounds_expansion` uses — matched deliberately, so that
/// the reach this module reports and the padding the CPU backend allocates
/// cannot disagree.
pub const BLUR_CUTOFF: f64 = 3.0;

/// The radius a freshly added blur — a shadow's or a [`EffectKind::LayerBlur`]'s
/// — starts on.
///
/// **One number for both, because it is one quantity**: the two are the same
/// radius in the same units ([`BLUR_DEVIATION`]), so a shadow whose blur says 12
/// and a layer blur that says something else would be two defaults for one idea.
/// It is also why [`EffectKind::retyped`] can carry the value across.
///
/// Twelve rather than zero, for [`Shadow::default`]'s reason: a row that has been
/// added and visibly does nothing is the state a designer reads as "it is broken"
/// rather than "you have not tuned it yet".
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
/// deviation times the zoom — so the cost of a fixed drawing grows without bound
/// as someone zooms in. This is the number that stops it; see
/// [`buffer_downscale`] for why bounding it costs no fidelity.
///
/// **Twenty-four is a balance, not a threshold.** The kernel is `2⌈3σ⌉+1` taps,
/// so this is 145 of them, and the downscale it implies is the *only* thing that
/// grows with the zoom instead of the work.
pub const BLUR_SIGMA_BUDGET: f64 = 24.0;

/// The widest kernel half-width any blur in this project builds, in pixels
/// (§15 D454).
///
/// ⚠️ **A net, not a budget.** [`BLUR_SIGMA_BUDGET`] is the number the design
/// aims at, and it yields a radius of 72; this is the number that stops a
/// document from asking for one nobody has. `<feGaussianBlur stdDeviation="1e30">`
/// imports with both of `Import`'s report lists **empty**, saves and reloads
/// cleanly, and asks `render::effects::kernel` for a `Vec` of 6e30 taps — a **24
/// GB** allocation at σ = 1e9, and above σ ≈ 6.1e18 `(σ * 3).ceil() as usize`
/// saturates `usize::MAX` and `Vec::with_capacity` is a capacity-overflow panic
/// (`[S10.2-L1-03]`).
///
/// **512 is 7.1× the radius the budget produces**, so it cannot bite on a blur
/// the design considers reasonable, and it is a bound on a *cost that is already
/// linear in the radius*: measured in release on a 200×220 surface, a full-res
/// `blur` is 9.3 ms at r = 72 and 239 ms at r = 3000, so the cap holds the
/// degenerate case to a small multiple of the ordinary worst case rather than to
/// a hang or a panic. 1025 taps is 4 KB.
///
/// ⚠️ **Reaching it always means something upstream failed.** Both blurs are
/// supposed to arrive here at σ ≤ 24 — a layer blur through
/// [`buffer_downscale`]'s smaller buffer, a shadow through
/// [`shadow_downscale`]'s coarse grid. A kernel at this cap is a wrong picture
/// rather than a right one; it is the *cheapest* wrong picture, which is the
/// whole of what it is for.
pub const MAX_KERNEL_RADIUS: usize = 512;

/// The largest block a shadow's silhouette may be resampled into, in device
/// pixels (§15 D454).
///
/// ⚠️ **[`MAX_KERNEL_RADIUS`] does not make this unnecessary and this does not
/// make that unnecessary** — they bound two different costs, which is what
/// `[S10.2-L1-03]` established by refuting a fix that had only this one.
/// Coarsening is `Θ(k)` in the block factor independently of the surface
/// (`render::effects::coarsen` walks `k × k` samples per coarse cell and the
/// compiler drops the runs that fall off the buffer), so an uncapped `k` is a
/// hang whatever the kernel does: measured in release on a 200×220 surface,
/// 184 µs at k = 4096, 2.76 ms at 65 536, 44.3 ms at 1 048 576, and **≈180 s** at
/// the `u32::MAX` [`shadow_downscale`]'s cast used to saturate to — which is why
/// two 100-second kills saw no return.
///
/// **4096 is already one coarse cell for almost anything either backend
/// allocates.** The GPU's layer buffers are clamped to the device's texture
/// limit, and the widest raster `ondin export --png` will produce is
/// `export::png::MAX_RASTER_SIDE` = 65 408, which this divides into a 16-cell
/// grid. It also keeps `k as i32` — the shader's `Params.radius` — well inside
/// range, where above `i32::MAX` the cast wrapped negative, the coarsen loops did
/// not run, and the GPU drew **no shadow at all**.
pub const MAX_SHADOW_BLOCK: u32 = 4096;

/// The largest deviation applied to the layer's output **as a whole**, in
/// authored units — the visible [`EffectKind::LayerBlur`]s and nothing else.
///
/// ⚠️ **A shadow's blur does not count, and that is the whole subtlety.** A blur
/// on the layer softens everything the layer produces, so the output carries no
/// detail finer than it. A drop shadow blurs a *silhouette* and composites it
/// behind artwork that stays sharp — so a stack of nothing but shadows has a
/// crisp layer in it, and treating its shadow's deviation as the layer's would
/// license a downscale that visibly softens the drawing.
pub fn layer_blur_deviation(effects: &[Effect]) -> f64 {
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
/// **Rasterizing a blurred layer at full device resolution is wasted work, and
/// the waste is unbounded.** A gaussian of deviation σ carries nothing finer than
/// σ, so a layer whose output is blurred by 222 device pixels is fully described
/// by a buffer eight times coarser — and the blur that buffer then needs is eight
/// times narrower, which is the same picture for a *sixty-fourth* of the
/// convolution. Past the budget the cost of a blur stops growing with the zoom
/// and starts shrinking, because the kernel is pinned and the buffer keeps
/// shrinking.
///
/// ⚠️ **Only [`layer_blur_deviation`] may pay for this**, for the reason that
/// function documents: the licence to lose resolution is that the output has
/// already lost it. A stack that leaves any of its artwork sharp gets `1.0` and
/// its blur costs whatever it costs.
///
/// `1.0` — never less — when the layer is not blurred enough to have earned it.
///
/// ⚠️ **Nothing between 1 and 2, and the deadband is measured rather than
/// cautious.** Rendering the car drawing at three zooms with and without this
/// function came back *byte-identical* at 4× and 8× — where the factors are 4.6
/// and 9.3 — and differed at 1×, where the largest factor is 1.157. That is the
/// shape to expect: a shallow downscale resamples without the blur being wide
/// enough to hide it, and it saves a quarter of a convolution that was never the
/// problem. The whole benefit is in the large factors, so the small ones are
/// declined and the ordinary zoom a designer works at is left exactly alone.
pub fn buffer_downscale(effects: &[Effect], scale: (f64, f64)) -> f64 {
    let sigma = layer_blur_deviation(effects) * scale.0.abs().max(scale.1.abs());
    let k = sigma / BLUR_SIGMA_BUDGET;
    if k < 2.0 { 1.0 } else { k }
}

/// How many device pixels one pixel of a **shadow's own silhouette** should
/// cover, so that the shadow's blur stays inside [`BLUR_SIGMA_BUDGET`] too.
///
/// ⚠️ **This is [`buffer_downscale`]'s argument applied where that function is
/// forbidden to go, and the difference is what makes it sound.** A drop shadow
/// may not coarsen the *layer's buffer*, because the artwork composited over the
/// shadow stays sharp — [`layer_blur_deviation`] exists to say so. But the
/// silhouette the shadow is blurred from is not the artwork: it is an alpha field
/// on its way to becoming a gaussian of deviation σ, and *that output* carries
/// nothing finer than σ by the same reasoning. So the resampling happens inside
/// the shadow pass, over the mask, and nothing that was ever sharp is touched.
///
/// **A whole number, not a ratio**, because a mask is resampled on a grid: the
/// silhouette is box-averaged into `k`×`k` blocks, blurred at σ/k, and read back
/// bilinearly. `ceil` rather than `round`, so σ/k lands *at or under* the budget
/// rather than near it.
///
/// **The same deadband as [`buffer_downscale`], for the same measured reason.** A
/// shallow resample costs fidelity without saving the convolution that was the
/// problem; the whole benefit is in the large factors, and an ordinary zoom is
/// left alone entirely. `1` means "blur it as authored".
///
/// Without this, σ for a shadow is the authored deviation times the zoom with
/// nothing bounding it: 55.5 units at 256× is a **1,557 ms** frame against
/// Windows' two-second reset (§15 D403).
/// ⚠️ **Capped at [`MAX_SHADOW_BLOCK`], and a non-finite σ gets the cap rather
/// than `1`** (§15 D454). `[S10.1-L1-03]`: `k.ceil() as u32` *saturates*, so
/// above σ ≈ 1.03e11 the block factor pinned at `u32::MAX` while σ kept growing
/// and σ/k went 24.0 → 46.6 → 2.33e20 — the one contract this function has,
/// broken by the cast that was meant to deliver it. The degenerate arm was wrong
/// the other way: `if k.is_finite() && k >= 2.0 { … } else { 1 }` answered *"blur
/// it as authored"* for σ = `inf`, which is the single input where no downscale
/// at all is certainly wrong.
///
/// **The cap does not restore the contract and is not meant to.** At the cap σ/k
/// is whatever it is, and [`MAX_KERNEL_RADIUS`] is what stops *that* number
/// hurting anybody; the two together are what bound the pass, and neither does it
/// alone (`[S10.2-L1-03]`).
pub fn shadow_downscale(sigma_device: f64) -> u32 {
    let k = sigma_device / BLUR_SIGMA_BUDGET;
    // NaN first, because it is the one input `<` cannot classify and because it
    // is not "very blurred" but "no answer" — it reaches here only from a
    // document the finiteness guards should already have refused. Everything
    // else falls through one expression: `inf.ceil()` is `inf` and `inf.min(cap)`
    // is the cap, so the infinite case needs no arm of its own, and `-inf` and a
    // shallow factor share the deadband's `1`.
    if k.is_nan() || k < 2.0 {
        1
    } else {
        k.ceil().min(f64::from(MAX_SHADOW_BLOCK)) as u32
    }
}

/// One entry in a layer's effect stack.
///
/// **A `visible` flag beside the payload rather than removal-as-hiding**, which
/// is the same decision [`crate::Fill`] made and for a stronger reason here: a
/// shadow is half a dozen tuned numbers, and the row's `×` throwing them away
/// makes "let me see it without the shadow" a destructive act. The mockup drew
/// only the `×`; the eye is the addition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    pub visible: bool,
}

impl Effect {
    /// A new effect of `kind`, visible.
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            visible: true,
        }
    }

    /// How far this effect's ink can land outside the geometry it applies to,
    /// or zero insets for one that cannot escape it.
    ///
    /// Positive in every direction means "outward", which is the opposite of
    /// [`Insets`]'s usual reading — it is the padding a bounds needs, so
    /// `rect + insets` is the answer rather than `rect - insets`.
    ///
    /// **Invisible effects reach nowhere**, so a hidden shadow does not keep
    /// widening the cached world bounds. That is the bug §15 D56 records against
    /// the second stroke, in the one place it would recur.
    pub fn escape(&self) -> Insets {
        if !self.visible {
            return Insets::ZERO;
        }
        self.kind.escape()
    }
}

/// What an effect *is*.
///
/// The two shadows share [`Shadow`] rather than each carrying their own copy of
/// the same five numbers: they differ in where the ink lands, not in what is
/// authored. Which one it is decides that, and nothing else does.
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
            // The blurred silhouette is the shape grown by `spread`, blurred,
            // then moved by `offset` — so the reach is the sum on the side the
            // offset points at and the difference on the side it leaves, which
            // is why this cannot be one number.
            Self::DropShadow(s) => {
                let r = reach(s.blur) + s.spread.max(0.0);
                Insets {
                    x0: r - s.offset.x,
                    y0: r - s.offset.y,
                    x1: r + s.offset.x,
                    y1: r + s.offset.y,
                }
            }
            // Drawn *inside* the shape's own alpha, by definition. However far
            // the blur reaches, none of it lands outside.
            Self::InnerShadow(_) => Insets::ZERO,
            Self::LayerBlur { radius } => Insets::uniform(reach(*radius)),
            // Per-pixel, so every pixel it touches is one that was already there.
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
    /// **The order and the words are the model's**, which is the rule
    /// [`crate::Adjustment::ALL`] already sets for the image card's seven rows: a
    /// panel spelling its own list can offer three of the four, or call one of
    /// them something the snapshot calls something else. It is a function rather
    /// than a `const` because every variant carries a payload, so "one of each"
    /// is four constructed values rather than four names.
    ///
    /// The order is the mockup's — the two shadows, the blur, the filters — minus
    /// the two that were declined (see this module's header).
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
    /// What the panel's kind selector asks to decide which cell is lit. `==`
    /// cannot answer it: two drop shadows at different offsets are the same kind
    /// and are not equal.
    pub fn same_kind(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// This effect's numbers carried into `to`'s kind — what the panel writes
    /// when a row is retyped.
    ///
    /// **Carry across what the two kinds share and default the rest**, rather
    /// than starting the new kind from scratch. The two shadows share every field
    /// they have, so switching between them keeps the offset, the spread and the
    /// colour that were tuned; a shadow and a blur share the blur radius and
    /// nothing else; [`Filters`] shares nothing with any of them. Resetting
    /// instead would throw half a dozen tuned numbers away to answer a question
    /// about *where* the ink lands — which is the argument [`Effect::visible`]
    /// already won against the mockup's `×`, one step quieter.
    ///
    /// Only the *kind* of `to` is read; whatever it is holding is discarded, so a
    /// caller can pass one of [`Self::all_defaults`] as the name of a kind.
    pub fn retyped(&self, to: &Self) -> Self {
        // Read once so every arm below agrees about what "the blur" is, including
        // the arms that have to invent one.
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
    /// Private, and the only reader is [`Self::retyped`]: outside that, "the blur
    /// of a `Filters`" is not a question with an answer, and offering it as an
    /// accessor would invite one.
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
/// **The opacity is the colour's alpha and is not a field of its own.** That is
/// the rule fills already follow — `paint::alpha_of`/`with_alpha` read and write
/// one value, and the row shows a swatch and a percentage over it — and a second
/// spelling of alpha is the kind of thing that ends up disagreeing with itself
/// the first time a gradient or an eyedropper writes one of the two. The
/// mockup's `Opacity` field is that percentage; it edits this alpha.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    /// Where the shadow sits relative to the layer, in the layer's own space.
    pub offset: Vec2,
    /// The blur *radius* as typed, not the gaussian deviation — see
    /// [`BLUR_DEVIATION`].
    pub blur: f64,
    /// How far the silhouette grows (or shrinks, when negative) before it is
    /// blurred.
    ///
    /// On the inner shadow too, which the mockup left off the popover: it is the
    /// same dilation of the same silhouette, and Figma offers it on both.
    pub spread: f64,
    /// Straight (non-premultiplied) sRGB, alpha carrying the opacity
    /// (invariant 7).
    pub color: Color,
}

impl Default for Shadow {
    /// The shadow a freshly added row starts on: down and soft, at the alpha the
    /// mockup shows, in black.
    ///
    /// Not `Vec2::ZERO` with a zero blur, which would be a row that has been
    /// added and visibly does nothing — the state a designer reads as "it is
    /// broken" rather than "you have not tuned it yet".
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
/// **Factors with 1.0 neutral, and degrees for the hue** — the convention CSS
/// and SVG both use, so `feColorMatrix`/`feComponentTransfer` and the shader
/// take these numbers essentially as they are. It deliberately does *not* match
/// [`crate::ImageAdjust`]'s `-1.0..=1.0` with zero neutral (§5.5a), and the
/// difference is worth stating rather than quietly carrying: that one is a
/// photographic control surface authored against a picture, this one is a
/// filter graph, and D186's lesson was that the writer which cannot invent
/// arithmetic is the thing that should choose the units.
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
    /// The row's summary shows a *count* of the channels that are off-neutral
    /// (the mockup's "3"), so the panel needs this question and the writers need
    /// it too: a neutral `Filters` should emit no filter primitive at all rather
    /// than an identity matrix nothing can see but everything must composite.
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
/// [`crate::Adjustment`]'s twin for the other set of colour controls, and
/// deliberately shaped the same way: the order and the words live in one place,
/// so a panel cannot list three of the four or call one of them something the
/// snapshot calls something else. The **units** differ, which is the whole of
/// §15 D333's note about the two conventions — that one is a photographic
/// surface at `-1..=1` about zero, this one goes straight into `feColorMatrix` —
/// so this list carries [`Self::neutral`] and [`Self::degrees`] where that one
/// could take a single range for granted.
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

    /// The word the field carries. **Words rather than glyphs**, for the reason
    /// [`crate::Adjustment::label`] gives: no icon set has four tone glyphs anyone
    /// can tell apart at 15pt, and a control whose drawing does not distinguish it
    /// has to be labelled.
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
    /// **Written as three points — bottom, neutral, top — rather than "above 100%
    /// is more"**, because the field draws a bar growing from its middle (§15
    /// D343) and the tooltip is what says where that middle sits and how far the
    /// two ends are. The panel's range is symmetric about the neutral, so these
    /// three numbers are the whole of the control.
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
    /// The panel's field is dimmed at rest against this rather than against a
    /// literal, which is what keeps "this row is doing nothing" and
    /// [`Filters::changed`]'s count from disagreeing.
    pub fn neutral(self) -> f64 {
        match self {
            Self::Hue => 0.0,
            _ => 1.0,
        }
    }

    /// Whether this channel is an angle in **degrees** rather than a factor.
    ///
    /// The one thing the four do not share, and it is asked here rather than by
    /// the panel matching on `Hue` itself: a fifth channel in either unit is then
    /// a line in this file and nothing in the popover.
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
    /// **Non-finite is refused here rather than at the control**, as
    /// [`crate::Adjustment::set`] does it and for the same reason: a typed
    /// expression can evaluate to `inf` (`1/0` is a thing the field's parser will
    /// hand over), and a `NaN` factor reaching `feColorMatrix` writes a filter no
    /// reader can render. There is no clamp beyond that — a factor has no upper
    /// bound that means anything, and the hue wraps.
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
/// The two spaces are the whole reason this is a function. An effect is authored
/// in the layer's own space — a shadow at `(0, 4)` follows the layer when it is
/// rotated, exactly as Figma's does — while bounds are cached in world space, so
/// the padding has to be carried across.
///
/// **The escape is split into an offset and a radius rather than transformed as
/// four corners**, and the difference is not academic. A blur's reach is
/// isotropic: as a local *square* its rotated AABB is up to √2 bigger than it
/// should be, so a rotated blurred layer would cache — and, through
/// `ondin_export::plan`, *export* — a box 41% too wide. Split, the offset is a
/// vector the linear part maps directly and the radius becomes the exact
/// half-extents of the mapped ellipse.
///
/// **Never shrinks.** A shadow thrown entirely to one side leaves the geometry's
/// own box needing coverage on the other, so each edge takes the wider of "where
/// the effect reached" and "where the layer already was" — which is what the
/// `min`/`max` against zero are for.
///
/// 🚨 **A non-finite `escape` comes back as `bounds` unchanged, and that is not
/// a guard — it is `f64::min` swallowing the operand** (`[S10.1-L1-05]`,
/// §15 D641). With `x0 = x1 = inf` the split below gives `off = inf − inf`,
/// which is `NaN`; the mapped offset is `NaN`, and `NaN.min(0.0)` resolves to
/// the **non-`NaN`** operand, so every edge contributes zero. A layer whose blur
/// reached everywhere would report an ink box identical to its geometry, and the
/// viewport cull, the export plan and both backends' buffer boxes would all
/// believe it reaches nowhere.
///
/// ⚠️ **The fix is upstream and deliberately not here**, which is worth stating
/// because the obvious repair is wrong twice over. `build::effect_is_finite`
/// refuses such a value at the operation, so every production caller — all of
/// them pass `stack_escape(node.effects())` — now hands this function a finite
/// escape. Answering an *infinite box* instead, which reads as the conservative
/// choice, is precisely the bug §15 D495 removed: an infinite box unions all the
/// way up, so `world_bounds(root)` stays infinite for the life of the session,
/// and `plan::raster_size`/`plan::raster_opts` read it. §5.9's answer to a bound
/// that is not a number is **absence**, not a large box — and this function's
/// signature has no room for absence, which is the honest reason the check is
/// one level out rather than a shape anybody preferred.
///
/// `reach(f64::NAN)` is `0.0` by the same `max`-against-zero convention, so a
/// `NaN` radius was always a benign no-op while an infinite one was a bounds
/// lie: two non-finite inputs, two different silent answers, from one idiom.
///
/// ⚠️ **The *preview* path does not come through an operation and is bounded by
/// something else.** An effect being scrubbed reaches the renderer through
/// `RenderOverrides` before anything is committed, so the operation guard is not
/// what covers it. `stack_escape` reads five fields, and **two** constants cover
/// them: `inspector.rs`'s `MAX_BLUR` (1000) on the two blur scrubs, and
/// `MAX_OFFSET` (10,000) on a shadow's offset and spread. `Filters` escape
/// nothing at all, so the four filter factors are not on this path. On the
/// import side `svg_in::deviation_of` filters `is_finite`. **Not one of those
/// bounds is this function** — worth knowing before somebody removes one as
/// redundant.
///
/// ⚠️ **Corrected after being written as *"three bounds"* over two fields**
/// (§15 D641). `arch-scribe` read the claim against `inspector.rs` and found the
/// offset and spread scrubs, which the sentence had left out; the ranged fields
/// are five, not two.
pub fn escaped(bounds: kurbo::Rect, escape: Insets, world: kurbo::Affine) -> kurbo::Rect {
    if escape == Insets::ZERO {
        return bounds;
    }
    // ⚠️ **An escape nobody can measure covers everything, and it must not read as
    // covering nothing** (§15 D685). Below this line the insets are taken apart as
    // `x1 - x0` and `x0 + x1`, so an infinite one gives `inf − inf` = NaN, and
    // `f64::min` returns the operand that is *not* NaN — the box comes back
    // **unchanged**, which is the one answer these bounds may never give. The
    // honest rect is the unbounded one, and `recompute_bounds` passes the ink
    // through [`crate::geometry::measurable`], which turns it into the absence §15
    // D495 prefers. `stack_escape` no longer produces one from an ordinary radius;
    // this is the floor under a radius near `f64::MAX`, where the *answer* really
    // does overflow rather than the route to it.
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
    // x0 = radius − offset and x1 = radius + offset, so this reads the pair back
    // apart. See `EffectKind::escape`, which is where they were put together.
    let off = Vec2::new((escape.x1 - escape.x0) * 0.5, (escape.y1 - escape.y0) * 0.5);
    let rad = Vec2::new((escape.x0 + escape.x1) * 0.5, (escape.y0 + escape.y1) * 0.5);
    let [a, b, c, d, _, _] = world.as_coeffs();
    // The linear part only: a translation moves the box, and the box is already
    // where the translation put it.
    let t = Vec2::new(a * off.x + c * off.y, b * off.x + d * off.y);
    // Half-extents of the mapped radius, treated as an **ellipse** with those
    // semi-axes rather than as a box with those half-widths.
    //
    // ⚠️ The box reading is `|a|·rx + |c|·ry`, which is the absolute row sum and
    // looks like the obvious answer. It is the four-corner answer wearing
    // different arithmetic: at 45° it returns `r·√2`, because the corners of a
    // square really are that far out. A blur's reach is a *disc*, so what maps is
    // an ellipse and its extent is the row's euclidean norm — which comes back to
    // exactly `r` under any rotation, as it must.
    //
    // 🚨 **`hypot`, not `(x² + y²).sqrt()`** (§15 D676). The two are the same
    // number and not the same computation: the **square** overflows at about
    // `1.3e154`, where the product itself stays finite all the way to `1.8e308`.
    // So a legal blur radius, a `scale(1e155)` that `affine_is_finite` accepts and
    // a world box that `geometry::measurable` accepts gave `h = inf` and an ink box
    // of `(−inf, −inf, inf, inf)` — the box `scene::paint_node` culls whole subtrees
    // on, `plan::raster_size` sizes a raster from, and the SVG writer measures. It
    // is not `world_bounds`, so §15 D495 and D667 never look at it.
    //
    // ⚠️ **Not a guard, and that is the point.** There was nothing wrong with the
    // answer, only with the route to it, so the repair is the overflow-safe spelling
    // of the same formula rather than a refusal — `hypot` also loses less precision
    // near the bottom of the range, which nothing here depends on.
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
/// ⚠️ **Clipping to the bare target is the bug this function exists to prevent**,
/// and it is not obvious because the result looks plausible. An effect layer's
/// buffer is both the thing that gets drawn *and* the thing the silhouette is read
/// from, so cutting it at the target's edge cuts the silhouette — and a layer
/// scrolled half off the top then casts a shadow computed from the visible sliver
/// alone. Scrolled fully off, it casts none at all, while its shadow should still
/// be reaching into view. Found on 2026-08-25 by asking where the ink actually
/// landed rather than by comparing the two backends, which had the same clamp and
/// therefore agreed perfectly on the wrong answer (§15 D342).
///
/// **The two sides swap, which is the part to read twice.** Ink landing at the
/// target's right edge is cast from silhouette `offset` to the *left* of it and
/// `radius` beyond that — a distance this module packs into `escape.x0`, the
/// *left* inset. So the right side grows by `x0` and the left side by `x1`.
///
/// **Grown, never shrunk.** A large offset makes one inset negative, and the
/// arithmetic above would then pull the target's own edge inwards — correct for a
/// shadow, wrong for the layer, which is drawn in this buffer too and has to
/// survive wherever it is visible.
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
/// ⚠️ **The escape is measured in device pixels, so it grows with the zoom
/// without any bound at all** — and this is the half of the extreme-zoom crash
/// that survived §15 D395 (which bounded a blur's *kernel*, not its buffer). An
/// 18-unit shadow blur reaches 27 units; at 32× that is 864 device pixels on every
/// side, which takes a 3520×1980 viewport's buffer to 5248×3708 — **19.5 MP for a
/// 7 MP page**. Past vello's fixed-size intermediate buffers the fine shader hangs,
/// the watchdog resets the GPU, and the app dies in whatever the next frame touches
/// (§15 D402).
///
/// **What is given up is the far tail of a shadow cast from off-screen.** The
/// escape's whole job is to keep silhouette that is outside the target but can
/// still cast into it (D342); shrinking it drops the outermost part of that
/// silhouette, so a shadow reaching in from beyond the edge fades short.
///
/// ⚠️ **It bites at every zoom, not only the extreme one, and the first draft of
/// this comment claimed otherwise.** A layer that covers the viewport is already at
/// the budget before any escape is added, so *any* escape is clipped — measured on
/// the sweep fixture at 4×, 16× and 64×, and the clamp was active at all three.
/// What makes it acceptable is the size of the loss rather than its rarity:
/// against an unclamped CPU render, **worst channel error 1/255 on 0.28% of bytes**
/// at every zoom, and the same 1/255 with the shadow blur raised to 55.5 units.
/// The tail being given up is the faint end of a gaussian. A drawing where a wide
/// shadow falls from off-screen onto *empty* canvas is the case that would show
/// more, and it has not been measured.
///
/// **Both backends call this**, for the reason D342 and D395 both give from
/// different directions: two backends that chose different buffer *boxes* would
/// blur the same drawing differently and would agree about nothing.
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
    // Monotonic in `t`, so twenty halvings put it inside a pixel of the largest
    // escape that fits. Bisection rather than a solve: the box is an intersection
    // with the layer's own ink, so the area is piecewise quadratic in `t` and the
    // closed form is three cases where this is one.
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
/// The **union**, not the sum: two effects both reaching 10px out do not reach
/// 20px out, they each reach 10. Summing is the obvious wrong answer and it
/// inflates every cached bound on a layer with two shadows.
///
/// ⚠️ **A union of the right quantities, and it was a union of the wrong ones**
/// (§15 D473). `[S10.1-L2-02]`: the sentence above is true of two *shadows* and
/// **false of anything §6.4 composes**, and §6.4 composes two things.
/// `render::effects::run` applies `Filters` and `LayerBlur` **sequentially** to
/// the layer, keeps that as `graphic`, and casts every shadow from *it* — so a
/// `DropShadow`'s silhouette is the **already blurred** layer, and two
/// `LayerBlur`s convolve at `σ = √(σ₁² + σ₂²)` rather than at `max(σ₁, σ₂)`.
///
/// Measured in release through the CPU backend, a 40×40 rect carrying
/// `[LayerBlur { radius: 12 }, DropShadow { offset: (0, 40), blur: 4 }]` — an
/// everyday Figma stack. The shadow's alpha down the centre column ran
/// `… 87, 71, 54, 38, 0, 0, 0` — it **stopped dead at 38/255 with a hard edge**,
/// exactly at `ink_bounds.y1`, losing the whole faint end of the gaussian. The
/// arithmetic predicted the shortfall to the unit: claimed `max(18, reach(4) + 40)
/// = 46`, true `18 + 6 + 40 = 64`, measured cut 18 units short at the claimed
/// number. Two `LayerBlur { radius: 12 }` cut at 5/255 where the single blur of
/// the same composed deviation fades to 0 with 7.5 units to spare.
///
/// ⚠️ **The two single-effect controls were always clean**, which is what says
/// this is the composition and not the bound: the same shadow with no layer blur
/// falls to `… 10, 3, 1, 0` and reaches zero exactly at its own `ink_bounds.y1`,
/// and the same layer blur with no shadow ends at 0 too.
///
/// ⚠️ **The under-covered box is also the buffer**, which is why it cuts rather
/// than merely mis-culls: `grow_to_escape` ends `device_ink.intersect(grown)`, so
/// the silhouette is read from a buffer that has already lost the tail — §15
/// D342's mechanism one level up. And §5.9's rule is that **too small is the one
/// error these bounds may not make.**
///
/// **So: per stage, and still a union.** Stage 1 is one isotropic reach from the
/// convolved deviation of every visible `LayerBlur` (`Filters` add nothing).
/// Stage 2 is each shadow's own escape **grown by stage 1 on every side**, since
/// stage 1's output is its silhouette. The answer is the union of those — the
/// same rule the old comment states, applied to the quantities §6.4 actually
/// produces. §15 **D334**'s *"union, never a sum"* is right about two shadows and
/// is what this narrows.
pub fn stack_escape(effects: &[Effect]) -> Insets {
    // Stage 1: the visible layer blurs convolve. `deviation` is what `reach`
    // multiplies by `BLUR_CUTOFF`, so the composition is done in deviations and
    // turned back into a reach once — squaring a reach and rooting it would be
    // the same number only because the cutoff is a constant, and would stop
    // being true the day it is not.
    //
    // 🚨 **Scaled by the largest deviation before squaring** (§15 D685). The plain
    // `deviation(r).powi(2)` this replaces overflowed at `r ≈ 2.7e154` — a radius
    // `build::effect_is_finite` accepts at both its doors, because the *square* is
    // what goes past `1.8e308` and the value itself does not. `sigma2` came out
    // infinite, `stage1` with it, and [`escaped`] then received
    // `Insets::uniform(inf)`, whose `x1 - x0` is **`inf − inf` = NaN** — §15 D641's
    // swallow, under which `f64::min` returns the non-NaN operand and the box comes
    // back **unchanged**. An unbounded blur reporting that it reaches nowhere.
    //
    // ⚠️ **Not the same repair as D676's**, though it is the same class: that one
    // was a pair and `f64::hypot` is the overflow-safe spelling of a pair. This is
    // a fold, so the safe spelling is the classical one — divide through by the
    // largest term, sum squares that are all `≤ 1`, and multiply back. Exact for
    // every input the old form got right, and finite for the ones it did not.
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
        // A shadow is cast from stage 1's output, so its own reach starts
        // from a silhouette already `stage1` wider on every side. Nothing
        // else composes: a layer blur's own escape is stage 1 and is already
        // the seed, and `Filters` and `InnerShadow` escape nothing.
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

    /// The offset is the reason a shadow's reach cannot be one number, and the
    /// arithmetic is easy to write with the signs the wrong way round — which
    /// draws a bound that clips the shadow on the side it actually fell.
    ///
    /// A shadow pushed to `+y` (down, in this space) reaches further *down* and
    /// less far *up*. Flipping either sign in `escape` fails this.
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

    /// ⚠️ The blur field is a radius and the writers want a deviation. Asserting
    /// the reach of a *round* number is what catches the `/2` being dropped or
    /// applied twice: 12 → deviation 6 → reach 18. A missing halving gives 36, a
    /// doubled one gives 9, and both are wrong in a way no picture makes obvious.
    #[test]
    fn a_blurs_reach_is_three_deviations_and_a_deviation_is_half_the_radius() {
        assert_eq!(deviation(12.0), 6.0);
        assert_eq!(reach(12.0), 18.0);
        let i = Effect::new(EffectKind::LayerBlur { radius: 12.0 }).escape();
        assert_eq!(i, Insets::uniform(18.0));
    }

    /// **Two effects reaching outward do not add up — and two layer blurs do not
    /// take the larger either** (§15 D473).
    ///
    /// Written against the summing version, which answers 28 here. ⚠️ **It then
    /// asserted 18, the *union*, until 2026-09-07, and that was the defect
    /// `[S10.1-L2-02]` found**: §6.4 applies `LayerBlur`s **sequentially**, so two
    /// of them convolve at `σ = √(σ₁² + σ₂²)` rather than at `max(σ₁, σ₂)`. This
    /// fixture is chosen so the three readings are three different numbers —
    /// **28 summed, 18 unioned, 20.59 composed** — because a fixture where two of
    /// them coincide cannot say which rule is in force.
    ///
    /// ⚠️ **The under-covered box is also the buffer**, so this was not a
    /// mis-culled bound but a **cut picture**: measured in release, two
    /// `LayerBlur { radius: 12 }` stopped dead at alpha 5/255 where the single
    /// blur of the same composed deviation fades to 0 with 7.5 units to spare.
    #[test]
    fn two_layer_blurs_convolve_rather_than_summing_or_taking_the_larger() {
        let stack = vec![
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::LayerBlur { radius: 20.0 / 3.0 }),
        ];
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

    /// **A drop shadow is cast from the *blurred* layer, so its reach starts one
    /// stage 1 further out** (§15 D473).
    ///
    /// `[S10.1-L2-02]`'s headline case, and the arithmetic that predicted the
    /// measured shortfall to the unit. A 40×40 rect carrying
    /// `[LayerBlur { radius: 12 }, DropShadow { offset: (0, 40), blur: 4 }]` — an
    /// everyday Figma stack. Claimed `y1` escape `max(18, reach(4) + 40) = 46`;
    /// true `18 + 6 + 40 = 64`. Rendered, the shadow's alpha down the centre
    /// column ran `… 87, 71, 54, 38, 0, 0, 0` — **a hard edge at 38/255**, exactly
    /// at the claimed number, with roughly 18 device units of gaussian missing.
    ///
    /// ⚠️ **Both single-effect controls were always clean**, which is what says
    /// this is the composition rather than the bound: the same shadow with no
    /// layer blur falls to `… 10, 3, 1, 0` and reaches zero at its own
    /// `ink_bounds.y1`, and the same layer blur alone ends at 0.
    ///
    /// ⚠️ **Flipped** by seeding the fold with `Insets::ZERO` and dropping the
    /// `grown` arm — the old body: fails on the `y1` assertion at 46 against 64,
    /// which is the finding's own pair of numbers. The `InnerShadow` control below
    /// stays green under that flip, because an inner shadow escapes nothing
    /// whatever stage 1 does.
    #[test]
    fn a_drop_shadow_over_a_layer_blur_reaches_past_both() {
        let stack = vec![
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::DropShadow(Shadow {
                offset: Vec2::new(0.0, 40.0),
                blur: 4.0,
                spread: 0.0,
                color: Color::BLACK,
            })),
        ];
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

        // **The control.** An inner shadow escapes nothing, so stage 1 is the
        // whole answer however loud it is — a fold that grew every effect by
        // stage 1 rather than the shadows alone would fail here.
        let inner = vec![
            Effect::new(EffectKind::LayerBlur { radius: 12.0 }),
            Effect::new(EffectKind::InnerShadow(Shadow {
                offset: Vec2::new(0.0, 40.0),
                blur: 4.0,
                spread: 0.0,
                color: Color::BLACK,
            })),
        ];
        assert_eq!(
            stack_escape(&inner),
            Insets::uniform(18.0),
            "an inner shadow is drawn inside the alpha and escapes nothing"
        );
    }

    /// A hidden effect must stop widening the bounds, or a layer whose shadow is
    /// switched off still culls and exports as though it were on.
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

    /// An inner shadow is clipped to the alpha it is drawn inside, however far
    /// its own blur and offset would otherwise have carried it. Written against
    /// the version that shares `DropShadow`'s arm, which is the natural way to
    /// write this and answers a 60px escape.
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

    /// ⚠️ **The reason `escaped` splits the escape instead of mapping corners.**
    /// A 10px blur on a layer rotated 45° reaches 10px in every direction, not
    /// 14.14 — the local square's rotated AABB. Written against the four-corner
    /// version, which answers 14.142… here and would export a rotated blurred
    /// layer 41% too wide.
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

    /// A shadow thrown down and right must not pull the top-left edges inward:
    /// the layer itself is still there. Written against the version that adds the
    /// signed excursion to every edge, which walks the box's origin to (6, 6).
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

    /// The row summary is a count of off-neutral channels, so the count and the
    /// "is this doing anything" question must not disagree. A `changed()` that
    /// counted fields rather than comparing them answers 4 for the default.
    #[test]
    fn a_neutral_filter_row_counts_nothing_changed() {
        let mut f = Filters::default();
        assert!(f.is_neutral());
        assert_eq!(f.changed(), 0);
        f.hue = 30.0;
        assert!(!f.is_neutral());
        assert_eq!(f.changed(), 1);
    }

    /// Retyping a row carries across what the two kinds share. Written against
    /// the *plausible* wrong version rather than against nothing: the
    /// implementation anybody writes first is `EffectKind::from(new_kind)` — a
    /// fresh default — and that version passes any assertion that only checks the
    /// kind came out right, which is why every arm here asserts a **value** the
    /// default does not have.
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

    /// The kind selector lights a cell by `EffectKind::same_kind`, so it has to
    /// answer about the *variant* and not about the value. `==` is the thing this
    /// exists instead of, and the second assertion is the one it fails.
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

    /// A fresh row of any kind has to *do* something, or it reads as broken. The
    /// blur is the one that can be zero without looking like a mistake in the
    /// model, so it is the one worth pinning: `all_defaults` and `Shadow::default`
    /// must agree about it, which is what `DEFAULT_BLUR_RADIUS` is for.
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
                // The one exception, and it is not one: a fresh `Filters` row is
                // neutral on purpose. Four channels started off their neutral
                // value would be a row that changed the artwork the instant it was
                // added, which is the *opposite* failure — and unlike a zero
                // shadow it has a summary that says so (`changed()` reads 0).
                EffectKind::Filters(f) => assert!(f.is_neutral(), "{kind:?}"),
            }
        }
    }

    /// The popover draws one field per `FilterChannel::ALL` and the row summary
    /// is `Filters::changed`'s count, so the two have to be counting the same
    /// four. A channel dropped from the list leaves a value the summary counts and
    /// no control can reach; a fifth field on `Filters` leaves one nothing counts.
    ///
    /// **Against the literal four, not against `ALL.len()`** — which is the version
    /// that reads better and tests nothing at all, since removing a channel shrinks
    /// both sides of the comparison together.
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
        // The other direction: what the list calls neutral is what `Default` calls
        // neutral, or a field would sit dimmed while the summary counted it.
        let mut back = off;
        for c in FilterChannel::ALL {
            c.set(&mut back, c.neutral());
        }
        assert!(back.is_neutral());
        assert_eq!(back, Filters::default());
    }

    /// A typed expression can evaluate to infinity — `expr::eval` hands `1/0`
    /// straight over — and a non-finite factor reaches `feColorMatrix` and the
    /// shader as a value neither can render. The channel refuses it at the model.
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

    /// ⚠️ **A blur's cost grows without bound as someone zooms in, and this is the
    /// number that stops it.** A separable gaussian is `O(w · h · σ)` and σ in
    /// device pixels is the authored deviation times the zoom — so pasting a
    /// drawing whose largest blur is 55 units and zooming to 800% asked for a
    /// 1,335-tap kernel over a viewport-sized buffer, about 32 **billion** texel
    /// fetches in one frame. That is not slow, it is fatal: Windows resets a GPU
    /// that has not finished in two seconds, every resource on the device becomes
    /// invalid at once, and the app dies in whichever wgpu call happens to touch
    /// one first (§15 D395).
    ///
    /// The buffer shrinks by exactly the factor the deviation overshoots by, so σ
    /// in *buffer* pixels lands on the budget however far in the view is zoomed.
    #[test]
    fn a_blurs_kernel_stops_growing_once_the_buffer_starts_shrinking() {
        let fx = vec![Effect::new(EffectKind::LayerBlur { radius: 55.5 })];
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

    /// **The deadband, which is what keeps an ordinary zoom pixel-identical.**
    /// Below 2x the downscale saves a quarter of a convolution that was never the
    /// problem and resamples a picture the blur is not yet wide enough to hide the
    /// resampling of — measured, not guessed: the car drawing came back
    /// byte-identical at 4x and 8x and differed at 1x, where the factor was 1.157.
    #[test]
    fn a_shallow_downscale_is_declined_rather_than_taken() {
        let fx = vec![Effect::new(EffectKind::LayerBlur { radius: 55.5 })];
        // Just under 2x, and just over.
        assert_eq!(buffer_downscale(&fx, (1.7, 1.7)), 1.0);
        assert!(buffer_downscale(&fx, (1.8, 1.8)) > 2.0);
    }

    /// ⚠️ **Only a layer blur may buy a downscale, and a shadow may not.** A blur
    /// on the layer softens everything the layer produces, so the output has no
    /// detail left for a coarser buffer to lose. A drop shadow blurs a *silhouette*
    /// and composites it behind artwork that stays **sharp** — so reading its
    /// deviation as the layer's would license a downscale that visibly softens the
    /// drawing, which is the one way this optimisation could be wrong rather than
    /// merely ineffective.
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
            let fx = vec![Effect::new(kind)];
            assert_eq!(
                buffer_downscale(&fx, (8.0, 8.0)),
                1.0,
                "the layer under this shadow is still sharp"
            );
            assert_eq!(layer_blur_deviation(&fx), 0.0);
        }
        // And a stack holding both is bounded by the *layer* blur, not the shadow's.
        let fx = vec![
            Effect::new(EffectKind::LayerBlur { radius: 55.5 }),
            Effect::new(EffectKind::DropShadow(huge)),
        ];
        assert_eq!(layer_blur_deviation(&fx), deviation(55.5));
    }

    /// **A hidden effect buys nothing**, which is the same rule `Effect::escape`
    /// follows and for the same reason: a row with its eye closed is not drawn, so
    /// it cannot be the licence for losing resolution.
    #[test]
    fn a_hidden_blur_does_not_buy_a_downscale() {
        let fx = vec![Effect {
            kind: EffectKind::LayerBlur { radius: 55.5 },
            visible: false,
        }];
        assert_eq!(buffer_downscale(&fx, (8.0, 8.0)), 1.0);
    }

    /// The escape survives untouched whenever the buffer fits, which is every
    /// ordinary frame — a layer smaller than the page has room for its shadow's
    /// reach and `buffer_box_within` must be `buffer_box` exactly there.
    ///
    /// The flip is the interesting half: `budget` is a *page* count, and reading it
    /// as the layer's own area instead would clamp here, where nothing is wrong.
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

    /// **A layer that covers the page gives up its escape, never the page itself.**
    ///
    /// This is the extreme-zoom crash's second half (§15 D402): the escape is in
    /// device pixels, so an 18-unit shadow blur at 32× asks for 864 pixels on every
    /// side, and a viewport-sized layer's buffer goes to 19.5 MP for a 7 MP page —
    /// past vello's fixed buffers, into a hung shader and a reset GPU.
    ///
    /// Two assertions and they fail to different things. The **area** is what the
    /// budget is for; the **cover** is the safety rule — everything visible is still
    /// in the buffer, because `t` only ever scales the escape and `t = 0` is the
    /// target itself. ⚠️ Flipped by returning the bisection's `hi` rather than its
    /// `lo`, the area assertion is the one that fails, by a hair: the predicted
    /// failure is the *cover* only if the clamp is written to shrink the box, which
    /// is the mistake this test exists to make loud.
    #[test]
    fn a_layer_covering_the_page_gives_up_its_escape_rather_than_the_gpu() {
        let page = kurbo::Rect::new(0.0, 0.0, 3520.0, 1980.0);
        let budget = 3520.0 * 1980.0;
        // The layer's own ink is far bigger than the view, as it is at any zoom
        // deep enough to matter.
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

    /// The clamp keeps **as much escape as fits**, rather than dropping it whole:
    /// with room for twice the target's area the buffer comes back close to the
    /// budget, not equal to the target.
    ///
    /// Flip: returning `t = 0` on any overflow passes the test above and fails this
    /// one, which is exactly the difference between a shadow that fades a little
    /// early and one that stops at the edge of the screen.
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
