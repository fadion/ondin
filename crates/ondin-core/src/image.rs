//! The document's image table, and the reference a fill holds into it.
//!
//! **An image is a fill, not a node kind** (§5.5a). A `Fill` stores an
//! [`ImageRef`] — a key — and the bytes live once in a document-level table.
//!
//! **The stored bytes are the *encoded* original, never a decoded buffer**; the
//! app decodes once per image into a cache keyed by the same id.
//!
//! **The id is opaque here**: a content hash core neither computes nor checks
//! (hashing is a dependency core's rules do not permit, §3).

use serde::{Deserialize, Serialize};

/// A key into the document's image table. Ordered, because the table is written
/// sorted by id (invariant 9).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageId(pub String);

impl std::fmt::Display for ImageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The encodings v1 accepts (§5.5a, §15 D191). A GIF is its first frame;
/// animation is out of v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl ImageFormat {
    /// The MIME type, for an SVG data URI and for the OS clipboard.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }

    /// The file extension for a *stored* picture's format. Has no caller; not
    /// `ExportFormat::extension`, which is about a *rendered* file.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }
}

/// Where an image's bytes come from.
///
/// **Embedded by default, linked as the per-image escape hatch**: a file that
/// breaks when you send it is worse than a large one.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageSource {
    /// The encoded file itself, written into the document as base64.
    ///
    /// **`Arc<[u8]>` rather than `Vec<u8>`** (§15 D301): `Document::apply` clones
    /// the whole document, which would deep-copy every photograph per commit. Safe
    /// because the bytes are immutable (*Replace…* writes a new entry).
    Embedded(std::sync::Arc<[u8]>),
    /// A path on this machine, as written. **A string rather than a `PathBuf`**:
    /// a document is portable and a path is not. A stale link draws the
    /// [`missing_placeholder`] (§15 D179).
    Linked(String),
}

/// One image the document knows about. The intrinsic size is stored so layout
/// never waits on a decode and a broken link still knows its size.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageEntry {
    pub source: ImageSource,
    pub format: ImageFormat,
    /// Intrinsic pixel width.
    ///
    /// **Render-originated data stored in the model, accepted deliberately**
    /// (§15 D788): nothing re-checks it, but bytes are immutable and a
    /// [`ImageSource::Linked`] entry has no bytes, so the stored size is the only
    /// record.
    pub width: u32,
    /// Intrinsic pixel height. See [`Self::width`].
    pub height: u32,
}

impl ImageEntry {
    /// The encoded bytes; `None` for a linked image (core does no file I/O).
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.source {
            ImageSource::Embedded(b) => Some(b),
            ImageSource::Linked(_) => None,
        }
    }

    /// Whether the source is a link; ask rather than infer it from absent bytes.
    ///
    /// Authoring linked sources is a non-goal for v1 (§15 D819). A foreign
    /// `.ondin` can still carry one, and the clipboard refuses one (the only
    /// production caller), since a pasted link would be an attacker-chosen URL in
    /// every SVG export.
    pub fn is_linked(&self) -> bool {
        matches!(self.source, ImageSource::Linked(_))
    }

    /// How a foreign document refers to this picture: a `data:` URI of the
    /// *encoded original* (never a re-encode, §7.1) or the stored path, unescaped
    /// (the SVG writer escapes). Here because base64 is core's already (§5.5a).
    pub fn href(&self) -> String {
        use base64::Engine as _;
        match &self.source {
            ImageSource::Embedded(bytes) => format!(
                "data:{};base64,{}",
                self.format.mime(),
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ),
            ImageSource::Linked(path) => path.clone(),
        }
    }
}

/// How an image sits inside the frame its fill paints.
///
/// **Four modes, each an affine plus a [`peniko::Extend`] (and, for one, a
/// clip) — no pixel work**, so switching mode or scrubbing a crop is free.
///
/// Fill and Fit are derived from the frame; Crop and Tile read a number stored on
/// [`ImageRef`]. **The payloads are deliberately not variant payloads**: a mode
/// switch must not reset them, so clicking *Fit* and back keeps the crop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageFit {
    /// Cover the frame, preserving aspect; whatever overflows is cut off.
    ///
    /// The default. A click-placed image is framed to its own aspect (§5.5a), so
    /// Fill and [`ImageFit::Fit`] draw the same until the shape is resized.
    #[default]
    Fill,
    /// The whole picture inside the frame, preserving aspect; the frame shows
    /// through wherever the picture does not reach.
    Fit,
    /// Show [`ImageRef::crop`] and nothing else, mapped onto the frame exactly.
    Crop,
    /// Repeat the picture at [`ImageRef::tile_scale`], from the frame's corner.
    Tile,
}

impl ImageFit {
    /// Whether this is the value a missing key means (§5.11).
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Fill)
    }

    /// The four, in the order a mode strip offers them.
    pub const ALL: [ImageFit; 4] = [Self::Fill, Self::Fit, Self::Crop, Self::Tile];
}

/// A mirror and a quarter turn: how the picture sits inside its frame,
/// independent of the layer's own transform.
///
/// **Eight states, spelled canonically as mirror-then-turn**: `flip_x`/`flip_y`/
/// `turns` would spell the eight symmetries sixteen ways. [`Self::mirrored`] is
/// applied first, then [`Self::quarters`] clockwise turns.
///
/// **The controls are actions, not toggles**: "flipped horizontally?" has no
/// answer once a turn is involved, so [`OrientOp`] *composes*. Not
/// [`crate::build::Orientation`], which carries a real angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageOrient {
    /// Quarter turns clockwise, applied **after** [`Self::mirrored`].
    ///
    /// Written in `0..4`, read through [`ImageOrient::quarters`] (modulo four) so
    /// a hand-edited file still draws; the derived `PartialEq` sees the raw number.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub quarters: u8,
    /// Mirrored left-to-right in the source's frame, **before** the turn.
    #[serde(default, skip_serializing_if = "is_not")]
    pub mirrored: bool,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

fn is_not(b: &bool) -> bool {
    !*b
}

impl ImageOrient {
    /// The picture the right way up and the right way round.
    pub const IDENTITY: Self = Self {
        quarters: 0,
        mirrored: false,
    };

    /// Whether this is the value a missing key means (§5.11).
    pub fn is_default(&self) -> bool {
        self.quarters.is_multiple_of(4) && !self.mirrored
    }

    /// The turn count brought into `0..4`.
    pub fn quarters(self) -> u8 {
        self.quarters % 4
    }

    /// Whether the picture's width and height trade places — an odd number of
    /// quarter turns.
    pub fn swaps_axes(self) -> bool {
        self.quarters() % 2 == 1
    }

    /// `(width, height)` as the frame sees them once the turn is applied.
    pub fn oriented_size(self, width: f64, height: f64) -> (f64, f64) {
        if self.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// This orientation with `op` composed **on top of it**, in the frame's own
    /// axes — which is what the button the user just pressed means.
    ///
    /// **A mirror conjugates a rotation into its inverse** (`M∘Rᵏ = R⁻ᵏ∘M`), so
    /// flipping an already-turned picture reverses the turn as well as the mirror.
    pub fn applying(self, op: OrientOp) -> Self {
        let k = self.quarters();
        match op {
            OrientOp::FlipH => Self {
                quarters: (4 - k) % 4,
                mirrored: !self.mirrored,
            },
            // A vertical mirror is a horizontal one with a half turn on it.
            OrientOp::FlipV => Self {
                quarters: (6 - k) % 4,
                mirrored: !self.mirrored,
            },
            OrientOp::Turn => Self {
                quarters: (k + 1) % 4,
                mirrored: self.mirrored,
            },
        }
    }

    /// Source-pixel space → oriented-pixel space: the box `(0,0)..(width,height)`
    /// onto `(0,0)..`[`oriented_size`](Self::oriented_size).
    ///
    /// **Exact matrices, not `Affine::rotate`**: `rotate(π/2)` has a cosine of
    /// about `6.1e-17`, leaving a shear that differs in the file's last bits.
    /// Pinned by `every_orientation_maps_the_source_box_exactly_onto_the_oriented_box`.
    pub fn affine(self, width: f64, height: f64) -> kurbo::Affine {
        use kurbo::Affine;
        let (w, h) = (width, height);
        // Maps the source box onto itself, so the turn still gets a box at the origin.
        let mirror = if self.mirrored {
            Affine::new([-1.0, 0.0, 0.0, 1.0, w, 0.0])
        } else {
            Affine::IDENTITY
        };
        // Each turn carries the translation back into the positive quadrant.
        let turn = match self.quarters() {
            1 => Affine::new([0.0, 1.0, -1.0, 0.0, h, 0.0]),
            2 => Affine::new([-1.0, 0.0, 0.0, -1.0, w, h]),
            3 => Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, w]),
            _ => Affine::IDENTITY,
        };
        turn * mirror
    }
}

/// One press of the popover's mirror buttons: a symmetry composed onto whatever
/// orientation the picture is already in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientOp {
    /// Mirror left-to-right, in the frame's axes.
    FlipH,
    /// Mirror top-to-bottom, in the frame's axes.
    FlipV,
    /// A quarter turn clockwise.
    ///
    /// **No control writes this today, by decision**: the Transform panel's own
    /// quarter turns cover the common case. It stays because
    /// [`ImageOrient::applying`] resolves `FlipV` to `quarters: 2`.
    Turn,
}

impl OrientOp {
    /// A normalized crop rectangle carried through this op, since
    /// [`ImageRef::crop`] is normalized to the picture *as oriented*.
    /// [`whole_crop`] is a fixed point of all three.
    pub fn crop_carried(self, c: kurbo::Rect) -> kurbo::Rect {
        match self {
            Self::FlipH => kurbo::Rect::new(1.0 - c.x1, c.y0, 1.0 - c.x0, c.y1),
            Self::FlipV => kurbo::Rect::new(c.x0, 1.0 - c.y1, c.x1, 1.0 - c.y0),
            // (x, y) maps to (1 − y, x): `ImageOrient::affine`, normalized.
            Self::Turn => kurbo::Rect::new(1.0 - c.y1, c.x0, 1.0 - c.y0, c.x1),
        }
    }
}

/// The rectangle of an image **never cropped**: the whole source.
///
/// **A sentinel, not a chosen crop**: it stretches on a frame of another aspect,
/// so entering cropping replaces it with [`ImageRef::cover_crop`]; *reset crop*
/// puts it back. It is also the `skip_serializing_if` value (§5.11).
pub fn whole_crop() -> kurbo::Rect {
    kurbo::Rect::new(0.0, 0.0, 1.0, 1.0)
}

fn is_whole_crop(r: &kurbo::Rect) -> bool {
    *r == whole_crop()
}

/// Whether a width or a height is one this file's arithmetic can divide by
/// (§15 D452): positive **and finite**. A bare `> 0.0` passes `inf`, and a crop of
/// `±1e308` is four finite numbers whose `width()` overflows, yielding an
/// all-`NaN` rectangle, a singular `Affine`, and a `Some(NaN…)` `picture_box`
/// that panics `tools::crop_resized`. Asked at every span test in the module.
fn usable_span(x: f64) -> bool {
    x > 0.0 && x.is_finite()
}

/// The crop after dragging the **picture** by `by`, in the frame's own units.
///
/// The picture follows the pointer, so the rectangle moves the other way. Clamped
/// by [`crop_clamped`], so a drag stops at the picture's edge rather than pulling
/// `Extend::Pad`'s smear into the frame.
pub fn crop_panned(crop: kurbo::Rect, by: kurbo::Vec2, frame: kurbo::Rect) -> kurbo::Rect {
    let (fw, fh) = (frame.width(), frame.height());
    if !usable_span(fw) || !usable_span(fh) {
        return crop;
    }
    let d = kurbo::Vec2::new(-by.x * crop.width() / fw, -by.y * crop.height() / fh);
    crop_clamped(crop + d)
}

/// The crop scaled by `k` about its own centre — the *zoom* control, where
/// [`crop_reframed`] is the *window* one (§15 D271).
///
/// **`k` scales the rectangle, so it is the reciprocal of the picture's zoom.**
/// Uniform, so it adds no stretch and [`ImageRef::crop_scale`] can read one axis
/// and round-trip; about the centre so the subject stays put. Clamped by
/// [`crop_clamped`] (the floor [`ImageRef::cover_scale`] names); a `k` that is not
/// finite and positive gives the rectangle back.
pub fn crop_scaled(crop: kurbo::Rect, k: f64) -> kurbo::Rect {
    if !k.is_finite() || k <= 0.0 {
        return crop;
    }
    // Smallest crop worth having: a hundredth of the source in the shorter direction.
    const MIN: f64 = 0.01;
    let k = k.max(MIN / crop.width().min(crop.height()).max(f64::MIN_POSITIVE));
    crop_clamped(kurbo::Rect::from_center_size(
        crop.center(),
        kurbo::Size::new(crop.width() * k, crop.height() * k),
    ))
}

/// The crop after the **frame** changed from `from` to `to`, with the picture
/// left exactly where it was on screen.
///
/// This is what makes the bounding box a crop rectangle (§15 D268): the *old*
/// mapping of crop onto frame, read at the *new* frame's corners. The picture's
/// scale per source pixel is unchanged on both axes.
///
/// **Not clamped, deliberately**: the caller holds the limit
/// ([`ImageRef::picture_box`], applied by `tools::crop_resized`), and clamping
/// here would slide the rectangle out from under a frame this function cannot see.
/// A degenerate `from` gives `crop` back.
pub fn crop_reframed(crop: kurbo::Rect, from: kurbo::Rect, to: kurbo::Rect) -> kurbo::Rect {
    let (fw, fh) = (from.width(), from.height());
    if !usable_span(fw) || !usable_span(fh) {
        return crop;
    }
    let at_x = |x: f64| crop.x0 + (x - from.x0) * crop.width() / fw;
    let at_y = |y: f64| crop.y0 + (y - from.y0) * crop.height() / fh;
    kurbo::Rect::new(at_x(to.x0), at_y(to.y0), at_x(to.x1), at_y(to.y1))
}

/// `crop` brought back inside the source, keeping its size where it can and its
/// aspect always. The order matters: a rectangle **larger** than the source is
/// scaled down about its centre first, then *slid* inside (clamping edges
/// independently would stretch the picture).
pub fn crop_clamped(crop: kurbo::Rect) -> kurbo::Rect {
    let (w, h) = (crop.width(), crop.height());
    if !usable_span(w) || !usable_span(h) {
        return whole_crop();
    }
    // Shrink about the centre if it has outgrown the source.
    let over = (w.max(1.0) / 1.0).max(h.max(1.0) / 1.0);
    let (w, h, c) = if over > 1.0 {
        (w / over, h / over, crop.center())
    } else {
        (w, h, crop.center())
    };
    let mut r = kurbo::Rect::from_center_size(c, kurbo::Size::new(w, h));
    // Slide rather than clamp each edge, so the size survives.
    let dx = if r.x0 < 0.0 {
        -r.x0
    } else if r.x1 > 1.0 {
        1.0 - r.x1
    } else {
        0.0
    };
    let dy = if r.y0 < 0.0 {
        -r.y0
    } else if r.y1 > 1.0 {
        1.0 - r.y1
    } else {
        0.0
    };
    r = r + kurbo::Vec2::new(dx, dy);
    r
}

/// One document unit per source pixel — [`ImageRef::tile_scale`]'s default.
fn unit_scale() -> f64 {
    1.0
}

fn is_unit_scale(s: &f64) -> bool {
    *s == 1.0
}

/// An image fill's framing, resolved against the frame it paints into and the
/// source's intrinsic size. See [`ImageRef::framing`].
///
/// The three things a backend needs: the pixel grid's mapping, what happens off
/// its edge, and what the ink is cut to when part of the frame is empty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    /// Source-pixel space → the frame's local space, applied by both backends as a
    /// brush transform under the node's transform.
    pub transform: kurbo::Affine,
    /// What the backends sample beyond the source: Repeat for
    /// [`ImageFit::Tile`], pad otherwise.
    pub extend: peniko::Extend,
    /// The picture's rectangle in local space when the ink has to be clipped to it.
    ///
    /// **Only [`ImageFit::Fit`] asks for one, and only when it letterboxes**:
    /// peniko has no transparent extend, so the band would be a smear of edge
    /// pixels. A clip costs a layer.
    pub clip: Option<kurbo::Rect>,
}

impl Framing {
    /// The framing that changes nothing, which a degenerate frame or a
    /// zero-sized source resolves to.
    pub fn identity() -> Self {
        Self {
            transform: kurbo::Affine::IDENTITY,
            extend: peniko::Extend::Pad,
            clip: None,
        }
    }
}

/// How many samples a tone curve is written as.
///
/// **33, and the number is part of the format**: the length of the `tableValues`
/// list an SVG export writes, which the pixel path evaluates identically.
pub const TONE_STEPS: usize = 33;

/// The seven per-pixel adjustments an image fill carries, each `0.0` at rest and
/// running `-1.0..=1.0`.
///
/// **Per fill, not per image** (on [`ImageRef`], not [`ImageEntry`]). **Every field
/// is a symmetric offset from zero**, so the default is the identity (§5.11).
/// They are one pipeline ([`ImageAdjust::pipeline`]) shared by the three writers;
/// nothing else should reimplement what a slider does.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ImageAdjust {
    /// Stops of exposure: the picture is multiplied by `2^exposure`, so `-1` is
    /// half the light and `+1` is twice it.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub exposure: f32,
    /// Tone spread about mid-grey.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub contrast: f32,
    /// Colourfulness. `-1` is greyscale; `+1` is twice as saturated.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub saturation: f32,
    /// Warm (`+`, more red and less blue) to cool (`-`).
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub temperature: f32,
    /// Magenta (`+`) to green (`-`) — the second axis of a white balance.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub tint: f32,
    /// Lifts (`+`) or recovers (`-`) the bright end, leaving the dark end alone.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub highlights: f32,
    /// Lifts (`+`) or deepens (`-`) the dark end, leaving the bright end alone.
    #[serde(default, skip_serializing_if = "is_neutral")]
    pub shadows: f32,
}

fn is_neutral(v: &f32) -> bool {
    *v == 0.0
}

impl ImageAdjust {
    /// Nothing adjusted; what *reset* puts back.
    pub const NEUTRAL: Self = Self {
        exposure: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        tint: 0.0,
        highlights: 0.0,
        shadows: 0.0,
    };

    /// Whether this is the value a missing key means (§5.11). **Compared exactly,
    /// and a non-finite value is not neutral**: a `NaN` must reach
    /// [`Self::pipeline`], which refuses it.
    pub fn is_default(&self) -> bool {
        Adjustment::ALL.iter().all(|k| k.of(self) == 0.0)
    }

    /// Every slider back to zero.
    pub fn reset(&mut self) {
        *self = Self::NEUTRAL;
    }

    /// The per-pixel function these seven numbers describe, or [`None`] when
    /// they describe the identity.
    ///
    /// **Two stages, the two SVG filter primitives that can say this exactly**: a
    /// 4×5 colour matrix and a per-channel transfer table. The SVG writer cannot
    /// invent arithmetic, so the pixel backends evaluate the same numbers.
    ///
    /// [`None`] for the neutral value **and for any value outside `-1..=1`**,
    /// including non-finite ones (§15 D538): `"exposure": 1000.0` makes every
    /// matrix coefficient `NaN`, and `"shadows": 2.0` makes [`Self::curve`]
    /// non-monotonic. **Refused rather than clamped**, since clamping would make the
    /// file *draw* what it does not *say*; `an_impossible_value_is_refused_at_both_doors`
    /// asserts on `pipeline().is_none()` to tell the two apart.
    pub fn pipeline(&self) -> Option<AdjustPipeline> {
        if Adjustment::ALL.iter().any(|k| {
            let v = k.of(self);
            !v.is_finite() || !(-1.0..=1.0).contains(&v)
        }) {
            return None;
        }
        let matrix = self.matrix();
        let curve = self.curve();
        (matrix.is_some() || curve.is_some()).then_some(AdjustPipeline { matrix, curve })
    }

    /// The colour matrix for the five adjustments that are linear in the
    /// channels, or [`None`] when all five are neutral.
    ///
    /// Composed light first (exposure and white balance), then colourfulness, then
    /// the tone spread, collapsing to one matrix and one filter primitive.
    fn matrix(&self) -> Option<[f32; 20]> {
        let (e, c, s) = (self.exposure, self.contrast, self.saturation);
        let (t, n) = (self.temperature, self.tint);
        if e == 0.0 && c == 0.0 && s == 0.0 && t == 0.0 && n == 0.0 {
            return None;
        }
        // Exposure is stops, so a power of two; the white-balance axes are gentle
        // channel gains on top: ±0.3 on the axis's own channel and half that, the
        // other way, on the pair beside it, so tint does not read as brightness.
        let light = 2f32.powf(e);
        let gain = diagonal(
            light * (1.0 + 0.3 * t) * (1.0 + 0.15 * n),
            light * (1.0 - 0.3 * n),
            light * (1.0 - 0.3 * t) * (1.0 + 0.15 * n),
        );
        // SVG's own luminance coefficients, not Rec.601's: the ones
        // `feColorMatrix type="saturate"` is written with.
        let k = 1.0 + s;
        let (lr, lg, lb) = (0.213, 0.715, 0.072);
        let sat = [
            lr + k * (1.0 - lr),
            lg - k * lg,
            lb - k * lb,
            0.0,
            0.0,
            lr - k * lr,
            lg + k * (1.0 - lg),
            lb - k * lb,
            0.0,
            0.0,
            lr - k * lr,
            lg - k * lg,
            lb + k * (1.0 - lb),
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
        ];
        // Contrast pivots on mid-grey: a scale plus the offset that holds 0.5 still.
        let kc = 1.0 + c;
        let mut contrast = diagonal(kc, kc, kc);
        for row in 0..3 {
            contrast[row * 5 + 4] = 0.5 * (1.0 - kc);
        }
        Some(compose(&contrast, &compose(&sat, &gain)))
    }

    /// The tone curve for highlights and shadows, sampled at [`TONE_STEPS`]
    /// points, or [`None`] when both are neutral.
    ///
    /// **The two ends are weighted so that each slider leaves the other end
    /// alone** — `(1−v)²` for the shadows and `v²` for the highlights — and the
    /// ±0.4 strength is the largest that keeps the curve monotonic at both ends
    /// (the derivative of `v + a(1−v)²` at `v = 0` is `1 − 2a`).
    fn curve(&self) -> Option<[f32; TONE_STEPS]> {
        let (h, s) = (self.highlights, self.shadows);
        if h == 0.0 && s == 0.0 {
            return None;
        }
        let mut table = [0.0f32; TONE_STEPS];
        for (i, out) in table.iter_mut().enumerate() {
            let v = i as f32 / (TONE_STEPS - 1) as f32;
            let lifted = v + 0.4 * s * (1.0 - v) * (1.0 - v) + 0.4 * h * v * v;
            *out = lifted.clamp(0.0, 1.0);
        }
        Some(table)
    }
}

/// A 4×5 colour matrix that scales each channel and touches nothing else.
fn diagonal(r: f32, g: f32, b: f32) -> [f32; 20] {
    let mut m = [0.0f32; 20];
    for (i, k) in [r, g, b, 1.0].into_iter().enumerate() {
        m[i * 5 + i] = k;
    }
    m
}

/// `a` applied to the result of `b` — one 4×5 colour matrix doing the work of
/// two, with the fifth column carried through as the offset it is.
fn compose(a: &[f32; 20], b: &[f32; 20]) -> [f32; 20] {
    let mut out = [0.0f32; 20];
    for row in 0..4 {
        for col in 0..4 {
            out[row * 5 + col] = (0..4).map(|k| a[row * 5 + k] * b[k * 5 + col]).sum();
        }
        out[row * 5 + 4] =
            (0..4).map(|k| a[row * 5 + k] * b[k * 5 + 4]).sum::<f32>() + a[row * 5 + 4];
    }
    out
}

/// What [`ImageAdjust`] resolves to: the per-pixel function, in the one shape all
/// three writers can say (§15 D179).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjustPipeline {
    /// A 4×5 colour matrix over straight-alpha sRGB, row-major, as
    /// `feColorMatrix type="matrix"` takes it.
    pub matrix: Option<[f32; 20]>,
    /// A [`TONE_STEPS`]-point transfer table applied to R, G and B **after** the
    /// matrix (`feComponentTransfer type="table"`).
    pub curve: Option<[f32; TONE_STEPS]>,
}

impl AdjustPipeline {
    /// One pixel through the pipeline: straight-alpha sRGB in `0..=1`, in and out.
    ///
    /// **The order matters, and the clamp is stated twice on purpose.** SVG cuts
    /// each primitive's result to `0..=1`; the bound is written here *and* inside
    /// [`transfer`]. Removing both lets an overshooting matrix make the lerp
    /// extrapolate off the curve (`the_matrix_runs_before_the_tone_curve`).
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut v = rgb;
        if let Some(m) = &self.matrix {
            let [r, g, b] = v;
            v = [0, 1, 2].map(|row| {
                (m[row * 5] * r + m[row * 5 + 1] * g + m[row * 5 + 2] * b + m[row * 5 + 4])
                    .clamp(0.0, 1.0)
            });
        }
        if let Some(c) = &self.curve {
            v = v.map(|x| transfer(c, x));
        }
        v
    }
}

/// SVG's `feComponentTransfer type="table"`, to the letter: `n` values spanning
/// `0..=1`, linearly interpolated. The seam where canvas and export must agree —
/// see [`TONE_STEPS`].
pub fn transfer(table: &[f32], v: f32) -> f32 {
    if table.is_empty() {
        return v;
    }
    if table.len() == 1 {
        return table[0];
    }
    let v = v.clamp(0.0, 1.0);
    let n = table.len() - 1;
    // The spec's indexing: `k = floor(v·n)`, held at `n−1` so `v = 1` uses the last pair.
    let k = ((v * n as f32) as usize).min(n - 1);
    let t = v * n as f32 - k as f32;
    table[k] + t * (table[k + 1] - table[k])
}

/// One of the seven, named, so the panel draws them from one list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    Exposure,
    Contrast,
    Saturation,
    Temperature,
    Tint,
    Highlights,
    Shadows,
}

impl Adjustment {
    /// The seven, in the order the panel lists them.
    pub const ALL: [Adjustment; 7] = [
        Self::Exposure,
        Self::Contrast,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
        Self::Highlights,
        Self::Shadows,
    ];

    /// The word the control carries (no icon set has seven distinguishable glyphs).
    pub fn label(self) -> &'static str {
        match self {
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Saturation => "Saturation",
            Self::Temperature => "Temperature",
            Self::Tint => "Tint",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
        }
    }

    /// What each end of the slider does, for the control's tooltip.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Exposure => "Stops of light: −100 halves it, +100 doubles it",
            Self::Contrast => "Spread the tones about mid-grey, or flatten them",
            Self::Saturation => "−100 is greyscale; +100 is twice as colourful",
            Self::Temperature => "Warmer to the right, cooler to the left",
            Self::Tint => "Magenta to the right, green to the left",
            Self::Highlights => "Lift or recover the bright end, leaving the dark end",
            Self::Shadows => "Lift or deepen the dark end, leaving the bright end",
        }
    }

    pub fn of(self, a: &ImageAdjust) -> f32 {
        match self {
            Self::Exposure => a.exposure,
            Self::Contrast => a.contrast,
            Self::Saturation => a.saturation,
            Self::Temperature => a.temperature,
            Self::Tint => a.tint,
            Self::Highlights => a.highlights,
            Self::Shadows => a.shadows,
        }
    }

    /// Write one of the seven, held inside `-1..=1`.
    ///
    /// **Clamped here rather than at the control**: a range the model does not
    /// enforce is a range a hand-edited file ignores, and the tone curve's
    /// monotonicity argument only holds inside it.
    pub fn set(self, a: &mut ImageAdjust, v: f32) {
        let v = if v.is_finite() {
            v.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        match self {
            Self::Exposure => a.exposure = v,
            Self::Contrast => a.contrast = v,
            Self::Saturation => a.saturation = v,
            Self::Temperature => a.temperature = v,
            Self::Tint => a.tint = v,
            Self::Highlights => a.highlights = v,
            Self::Shadows => a.shadows = v,
        }
    }
}

/// What a fill holds: which image, and how it sits in the frame.
///
/// **A struct rather than a bare [`ImageId`]**, so framing is an additive schema
/// change (§5.11). Sampling is peniko's own `ImageSampler`; the one field the fit
/// has an opinion about (extend) is answered by [`ImageRef::framing`] at the
/// render boundary rather than written into the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: ImageId,
    #[serde(default, skip_serializing_if = "ImageFit::is_default")]
    pub fit: ImageFit,
    /// The rectangle of the source [`ImageFit::Crop`] shows, **normalized** to
    /// the picture *as [`Self::orient`] leaves it* (the space the crop gesture
    /// drags in), so `crop_panned` and `crop_reframed` know nothing about
    /// orientation; [`ImageRef::reoriented`] is the only way to change it, carrying
    /// the rectangle along. Normalized rather than in pixels so *replace image*
    /// keeps the framing (§5.5a, §15 D179).
    ///
    /// Maps onto the frame **exactly**, so stretching the shape stretches the
    /// picture. **Kept while the mode is not `Crop`** — see [`ImageFit`]. Coordinates
    /// outside `0..1` are not rejected; they sample the edge pixel.
    #[serde(default = "whole_crop", skip_serializing_if = "is_whole_crop")]
    pub crop: kurbo::Rect,
    /// What [`ImageFit::Tile`] repeats at: `1.0` is one document unit per source
    /// pixel. Kept while the mode is not `Tile`; a value that is not finite and
    /// positive draws at `1.0`.
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    pub tile_scale: f64,
    /// The flips and quarter turns applied **before** any mode frames the
    /// picture; read by all four, unlike [`Self::crop`] and [`Self::tile_scale`].
    #[serde(default, skip_serializing_if = "ImageOrient::is_default")]
    pub orient: ImageOrient,
    /// The per-pixel adjustments, applied before framing. **The one field that is
    /// not geometry**: it produces a *second buffer*, so `ondin_render::ImageStore`
    /// keys derived pixels by `(id, adjust)`.
    #[serde(default, skip_serializing_if = "ImageAdjust::is_default")]
    pub adjust: ImageAdjust,
}

impl ImageRef {
    pub fn new(id: ImageId) -> Self {
        Self {
            id,
            fit: ImageFit::default(),
            crop: whole_crop(),
            tile_scale: unit_scale(),
            orient: ImageOrient::IDENTITY,
            adjust: ImageAdjust::NEUTRAL,
        }
    }

    /// This reference flipped or turned inside its frame, **with the crop carried
    /// along** (the two must happen together or the picture jumps).
    pub fn reoriented(&self, op: OrientOp) -> Self {
        Self {
            orient: self.orient.applying(op),
            crop: op.crop_carried(self.crop),
            ..self.clone()
        }
    }

    /// The picture's intrinsic pixel size as the frame sees it — *original
    /// size*'s answer, which is the turned one for a turned picture.
    pub fn oriented_size(&self, width: u32, height: u32) -> (f64, f64) {
        self.orient.oriented_size(width as f64, height as f64)
    }

    /// The crop rectangle that shows exactly what [`ImageFit::Fill`] shows.
    ///
    /// **What entering Crop seeds**, so choosing it changes nothing on screen: the
    /// widest crop the gesture can zoom out to (any larger leaves the source).
    /// Degenerate input gives [`whole_crop`]. Cover is computed against the
    /// *turned* aspect ([`Self::orient`]).
    pub fn cover_crop(&self, frame: kurbo::Rect, width: u32, height: u32) -> kurbo::Rect {
        let (pw, ph) = (width as f64, height as f64);
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return whole_crop();
        }
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        // The cover scale, then the centred source rectangle that much frame is worth.
        // Orientation is a symmetry about the centre, so nothing needs un-turning.
        let s = (fw / iw).max(fh / ih);
        let (vw, vh) = (fw / s / iw, fh / s / ih);
        kurbo::Rect::new(
            (1.0 - vw) / 2.0,
            (1.0 - vh) / 2.0,
            (1.0 + vw) / 2.0,
            (1.0 + vh) / 2.0,
        )
    }

    /// The picture's size under [`ImageFit::Crop`], in local units per **oriented**
    /// source pixel (as [`Self::tile_scale`]).
    ///
    /// **The one thing the crop *gesture* cannot change** (§15 D268, D271): window
    /// and zoom are separate questions. **Measured on x**: the axes agree for every
    /// rectangle the app's gestures produce, differ after a non-uniform layer
    /// resize, and one axis round-trips with [`crop_scaled`]'s uniform factor.
    pub fn crop_scale(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<f64> {
        let (iw, _) = self
            .orient
            .oriented_size(f64::from(width), f64::from(height));
        let span = self.crop.width() * iw;
        // An `inf` span passes `> 0.0` and would answer `Some(0.0)` instead of `None`.
        (usable_span(span) && usable_span(frame.width())).then(|| frame.width() / span)
    }

    /// The scale `Fill` shows the picture at: the **floor** for
    /// [`Self::crop_scale`], below which the crop would leave the source. Not
    /// "100% and up": on a small layer cover is well under one unit per pixel.
    pub fn cover_scale(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<f64> {
        let (pw, ph) = (f64::from(width), f64::from(height));
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return None;
        }
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        // The very expression `cover_crop` scales by, so the two agree on the floor.
        Some((fw / iw).max(fh / ih))
    }

    /// How this reference sits in `frame`, given the source's intrinsic size.
    ///
    /// `frame` is the shape's bounding box in its local space (§6.2). Shared by
    /// both backends and the SVG writer. A zero-sized source or an empty frame
    /// resolves to [`Framing::identity`].
    pub fn framing(&self, frame: kurbo::Rect, width: u32, height: u32) -> Framing {
        use kurbo::Affine;
        let (pw, ph) = (width as f64, height as f64);
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return Framing::identity();
        }
        // Everything below frames the *oriented* picture; the orientation affine is
        // composed on the right at the end.
        let orient = self.orient.affine(pw, ph);
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        let base = match self.fit {
            ImageFit::Fill | ImageFit::Fit => {
                let (sx, sy) = (fw / iw, fh / ih);
                let cover = matches!(self.fit, ImageFit::Fill);
                let s = if cover { sx.max(sy) } else { sx.min(sy) };
                let (w, h) = (iw * s, ih * s);
                let x = frame.x0 + (fw - w) / 2.0;
                let y = frame.y0 + (fh - h) / 2.0;
                Framing {
                    transform: Affine::translate((x, y)) * Affine::scale(s),
                    extend: peniko::Extend::Pad,
                    // Contain clips only where the aspects differ, with a tolerance so
                    // a rounding bit does not pay for a layer.
                    clip: (!cover && (w < fw - 1e-9 || h < fh - 1e-9))
                        .then(|| kurbo::Rect::new(x, y, x + w, y + h)),
                }
            }
            ImageFit::Crop => {
                let r = self.crop;
                let (sw, sh) = (r.width() * iw, r.height() * ih);
                if !usable_span(sw) || !usable_span(sh) {
                    return Framing::identity();
                }
                Framing {
                    transform: Affine::translate((frame.x0, frame.y0))
                        * Affine::scale_non_uniform(fw / sw, fh / sh)
                        * Affine::translate((-r.x0 * iw, -r.y0 * ih)),
                    extend: peniko::Extend::Pad,
                    clip: None,
                }
            }
            ImageFit::Tile => {
                let s = if self.tile_scale.is_finite() && self.tile_scale > 0.0 {
                    self.tile_scale
                } else {
                    1.0
                };
                Framing {
                    transform: Affine::translate((frame.x0, frame.y0)) * Affine::scale(s),
                    extend: peniko::Extend::Repeat,
                    clip: None,
                }
            }
        };
        let framed = Framing {
            transform: base.transform * orient,
            ..base
        };
        // **A usable span is not a usable rectangle.** The Crop arm also weighs the
        // crop's origin (`-r.x0 * iw`), which `usable_span` never tests: a crop at
        // `x0 = -1e308` with a `1e300` span has a finite span but an overflowing
        // translation, and `picture_box` would answer `Some(NaN…)` (§15 D452). So the
        // *result* is tested rather than the operands, which closes every road to it.
        if framed.transform.as_coeffs().iter().all(|c| c.is_finite()) {
            framed
        } else {
            Framing::identity()
        }
    }

    /// Where the **whole** picture lands in the frame's local space. **Read off
    /// [`Self::framing`]**, so an outline cannot land anywhere but where the
    /// picture is painted. Defined in all four modes (under `Tile`, the first
    /// repeat); **only `Crop` draws it** (§15 D268). `None` for a degenerate frame
    /// or a zero-sized source.
    pub fn picture_box(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<kurbo::Rect> {
        if width == 0 || height == 0 || !usable_span(frame.width()) || !usable_span(frame.height())
        {
            return None;
        }
        let f = self.framing(frame, width, height);
        let source = kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        // A bounding box: the transform carries `orient`, and a quarter turn swaps
        // the corners, which `Rect` cannot hold.
        Some(f.transform.transform_rect_bbox(source))
    }
}

/// The drawing that stands in for a picture that cannot be drawn (§15 D179).
///
/// Here for the reason [`ImageRef::framing`] is: canvas, CPU export and SVG writer
/// must draw the same thing. The caller supplies the shape and does the painting.
pub struct Missing {
    /// The ground the shape is filled with.
    pub ground: peniko::Color,
    /// The rim and the cross.
    pub ink: peniko::Color,
    /// Line width for both, in the shape's own units.
    pub width: f64,
    /// The two diagonals of the shape's box, as one path.
    pub cross: kurbo::BezPath,
}

/// Grey and a cross over `bounds`, or [`None`] for a box with no area.
///
/// **No glyph and no words**, since a walk with no font must draw it. The cross is
/// the *box's* diagonals and the rim is the caller's own path, so an image fill in
/// a circle keeps the circle (the caller clips the cross). The width is a
/// fortieth of the shorter side, held between 0.5 and 4 units.
pub fn missing_placeholder(bounds: kurbo::Rect) -> Option<Missing> {
    if !usable_span(bounds.width()) || !usable_span(bounds.height()) {
        return None;
    }
    let mut cross = kurbo::BezPath::new();
    cross.move_to((bounds.x0, bounds.y0));
    cross.line_to((bounds.x1, bounds.y1));
    cross.move_to((bounds.x1, bounds.y0));
    cross.line_to((bounds.x0, bounds.y1));
    Some(Missing {
        // Light, so it reads as an *absence* rather than artwork somebody chose.
        ground: peniko::Color::from_rgb8(0xE4, 0xE4, 0xE6),
        ink: peniko::Color::from_rgb8(0xA0, 0xA0, 0xA8),
        width: (bounds.width().min(bounds.height()) / 40.0).clamp(0.5, 4.0),
        cross,
    })
}

/// The model's brush: peniko's, over an image *reference* instead of pixels.
///
/// **Not a brush enum of our own** (§15 D176): `Brush<I, G>` and `ImageBrush<D>`
/// are generic over their storage, so the model substitutes [`ImageRef`] for the
/// pixels and keeps everything else.
///
/// - **The save format does not move.** `Solid` and `Gradient` are the same
///   variants carrying wire-compatible types, so documents written before images
///   existed load unchanged. The gradient payload is [`GradientBrush`], which
///   stays compatible by `#[serde(flatten)]` (§15 D412).
/// - **`alpha` and `extend` are not restated**: `ImageBrush` already carries an
///   `ImageSampler`.
/// - **Only the render boundary changes**:
///   `ondin_render::color::brush_to_backend` resolves the reference through the
///   decode cache.
pub type Brush = peniko::Brush<peniko::ImageBrush<ImageRef>, GradientBrush>;

/// The image half of [`Brush`], for the call sites that build one.
pub type ImageBrush = peniko::ImageBrush<ImageRef>;

/// The gradient half of [`Brush`]: peniko's gradient, plus the affine that maps
/// the space it was authored in onto the shape's own.
///
/// **Why the model needs a transform peniko does not have**: a
/// `peniko::RadialGradientPosition` is two *circles*, but an elliptical gradient
/// is the ordinary case in real files, and an ellipse is a circle under a
/// non-uniform transform that both backends already take (§15 D412). It sits on
/// the brush rather than on [`crate::node::Fill`] since a `Stroke` carries a
/// `Brush` too.
///
/// **Wire-compatible by `#[serde(flatten)]`, which is load-bearing**: saved
/// gradients load unchanged
/// (`a_gradient_written_before_the_transform_existed_still_reads`). **Read through
/// [`GradientBrushWire`]**, which makes the stops monotonic
/// ([`make_stops_monotonic`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "GradientBrushWire")]
pub struct GradientBrush {
    /// The ramp, its kind and its geometry, in the space `transform` maps from.
    #[serde(flatten)]
    pub gradient: peniko::Gradient,
    /// Gradient space → the shape's local space. The identity for everything the
    /// app authors; non-identity comes from an import (`svg_in`'s
    /// `gradientTransform` and `objectBoundingBox` units) and rides along while
    /// stops are edited.
    ///
    /// A *similarity* is not stored here: a rotation or uniform scale is baked
    /// into the two circles, and only a squash survives as a transform.
    #[serde(default, skip_serializing_if = "is_identity")]
    pub transform: kurbo::Affine,
    /// The whole ramp's opacity, `0.0..=1.0`, multiplied over every stop's own
    /// alpha at paint time (§15 D767).
    ///
    /// **A field rather than a rewrite of the stops**: scaling every stop's alpha
    /// is not invertible through zero, so taking a `[1.0, 0.0]` fade to 0% and back
    /// came back flat and opaque. This matches the image brush, whose
    /// `peniko::ImageSampler::alpha` is a multiplier beside the pixels.
    ///
    /// Wire-compatible like `transform`: `default` plus `skip_serializing_if`, so
    /// saved gradients load unchanged and re-save byte-identical (invariant 9).
    ///
    /// **Clamped by its reader, not trusted from the file** (§15 D713):
    /// `build::brush_is_finite` carries its range and `paint::alpha_of` clamps.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
}

/// A [`GradientBrush`] as read, before its stops are made monotonic. The same
/// keys and defaults as the brush.
#[derive(Deserialize)]
struct GradientBrushWire {
    #[serde(flatten)]
    gradient: peniko::Gradient,
    #[serde(default)]
    transform: kurbo::Affine,
    #[serde(default = "one")]
    opacity: f32,
}

impl From<GradientBrushWire> for GradientBrush {
    fn from(w: GradientBrushWire) -> Self {
        let mut gradient = w.gradient;
        make_stops_monotonic(&mut gradient.stops);
        Self {
            gradient,
            transform: w.transform,
            opacity: w.opacity,
        }
    }
}

/// Whether `t` is the identity, for `skip_serializing_if`.
fn is_identity(t: &kurbo::Affine) -> bool {
    *t == kurbo::Affine::IDENTITY
}

/// [`GradientBrush::opacity`]'s default — fully opaque, i.e. "the stops decide".
fn one() -> f32 {
    1.0
}

/// Whether `a` is fully opaque, for `skip_serializing_if`.
fn is_one(a: &f32) -> bool {
    *a == 1.0
}

/// Clamp a gradient's stop offsets up to the running maximum, in place
/// (§15 D455).
///
/// **This is SVG's rule**: each offset must be at least the previous, and a
/// smaller one is corrected up. It is **not a sort**: `[0, 0.8, 0.2, 1]` clamps to
/// `[0, 0.8, 0.8, 1]` and sorts to `[0, 0.2, 0.8, 1]`, two different pictures.
///
/// A descending ramp collapses to the first stop's colour in the vello CPU
/// renderer; equal offsets, which this produces, are fine.
///
/// A sort is still right where the *user* is the author, so
/// `panels::paint::with_stops` keeps its `sort_by` (dragging a handle past its
/// neighbour means "put it there"); it only ever sees an already-ordered ramp.
pub fn make_stops_monotonic(stops: &mut [peniko::ColorStop]) {
    // Start at 0, not the first stop's offset: a ramp is defined on `0..=1`, so a
    // negative offset can only come from a hand-edited file.
    let mut hi = 0.0f32;
    for s in stops {
        s.offset = monotonic_offset(&mut hi, s.offset);
    }
}

/// [`make_stops_monotonic`]'s rule as one step, for a caller whose stops are not
/// [`peniko::ColorStop`]s (§15 D564).
///
/// Shared with the app's gradient chips (`ui::paint_ramp`), which would otherwise
/// hold a second copy of the rule and paint a descending ramp mirrored.
///
/// `hi` is the running maximum and is updated; the return is the offset to use.
/// `NaN` takes the running maximum (`f32::max` answers the non-NaN operand) so the
/// ramp stays readable; it should never arrive (`build::brush_is_finite`, §15
/// D451) and both callers are drawing.
pub fn monotonic_offset(hi: &mut f32, offset: f32) -> f32 {
    *hi = hi.max(offset);
    *hi
}

impl From<peniko::Gradient> for GradientBrush {
    /// A gradient authored in the shape's own space, which is every gradient the
    /// app itself makes.
    fn from(gradient: peniko::Gradient) -> Self {
        Self {
            gradient,
            transform: kurbo::Affine::IDENTITY,
            // Opaque: the stops carry their own alpha.
            opacity: 1.0,
        }
    }
}

/// A brush showing `id` with the default sampling.
pub fn image_brush(id: ImageId) -> Brush {
    Brush::Image(ImageBrush {
        image: ImageRef::new(id),
        sampler: peniko::ImageSampler::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid brush written by the old `peniko::Brush` still parses and
    /// re-serializes to the same bytes.
    #[test]
    fn a_solid_brush_written_before_images_existed_still_parses() {
        let json = r#"{"Solid":{"components":[0.2,0.4,0.6,1.0]}}"#;
        let brush: Brush = serde_json::from_str(json).expect("the old shape still parses");
        let peniko::Brush::Solid(c) = brush else {
            panic!("expected a solid, got {brush:?}");
        };
        assert_eq!(c.components, [0.2, 0.4, 0.6, 1.0]);
        let out = serde_json::to_string(&Brush::Solid(c)).expect("serializes");
        assert_eq!(out, json);
    }

    /// A gradient written before the transform existed still reads, and one with
    /// the identity (and opacity 1) re-saves the same bytes (§15 D412). The old
    /// bytes come from serializing a bare `peniko::Gradient`, so they cannot drift
    /// when peniko adds a field. Only the round-trip assertion catches a
    /// `skip_serializing_if` that stopped skipping, which loads everywhere.
    #[test]
    fn a_gradient_written_before_the_transform_existed_still_reads() {
        let old = serde_json::to_string(&peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0)))
            .expect("the pre-change payload serializes");
        let json = format!(r#"{{"Gradient":{old}}}"#);
        let brush: Brush = serde_json::from_str(&json).expect("the old shape still parses");
        let peniko::Brush::Gradient(g) = &brush else {
            panic!("expected a gradient, got {brush:?}");
        };
        assert_eq!(
            g.transform,
            kurbo::Affine::IDENTITY,
            "an absent transform is the identity"
        );
        assert!(matches!(g.gradient.kind, peniko::GradientKind::Linear(_)));
        // A default of `0.0` would make every older gradient load invisible.
        assert_eq!(
            g.opacity, 1.0,
            "an absent opacity is fully opaque, not fully transparent"
        );
        // No `transform` or `opacity` key goes out: invariant 9.
        assert_eq!(serde_json::to_string(&brush).expect("serializes"), json);

        // A squash *is* written, and comes back as itself.
        let squashed = Brush::Gradient(GradientBrush {
            gradient: peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0)),
            transform: kurbo::Affine::scale_non_uniform(1.0, 6.0),
            opacity: 1.0,
        });
        let out = serde_json::to_string(&squashed).expect("serializes");
        assert!(
            out.contains(r#""transform":[1.0,0.0,0.0,6.0,0.0,0.0]"#),
            "the affine writes as its six coefficients, beside the gradient's own \
             keys rather than nested under them: {out}"
        );
        assert_eq!(
            serde_json::from_str::<Brush>(&out).expect("round trips"),
            squashed
        );
    }

    /// A descending ramp is read clamped, SVG's reading: `[0, 0.8, 0.2, 1]`
    /// arrives as `[0, 0.8, 0.8, 1]`, not sorted. Flip: `GradientBrushWire`'s
    /// `From` without `make_stops_monotonic` → the offsets assertion fails.
    #[test]
    fn a_descending_ramp_is_read_clamped() {
        let stops = [0.0, 0.8, 0.2, 1.0].map(|o| peniko::ColorStop {
            offset: o,
            color: peniko::color::DynamicColor::from_alpha_color(peniko::Color::BLACK),
        });
        let written = Brush::Gradient(GradientBrush {
            gradient: peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops(stops),
            transform: kurbo::Affine::IDENTITY,
            opacity: 1.0,
        });
        let json = serde_json::to_string(&written).expect("serializes");
        let peniko::Brush::Gradient(g) = serde_json::from_str::<Brush>(&json).expect("reads")
        else {
            panic!("a gradient");
        };
        let offsets: Vec<f32> = g.gradient.stops.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, [0.0, 0.8, 0.8, 1.0]);
    }

    /// An image brush serializes as a key plus peniko's sampler; asserted on the
    /// JSON so no decoded pixels can reach the file.
    #[test]
    fn an_image_brush_serializes_as_a_key_and_a_sampler() {
        let brush = image_brush(ImageId("abc123".into()));
        let json = serde_json::to_string(&brush).expect("serializes");
        assert_eq!(
            json,
            r#"{"Image":{"image":{"id":"abc123"},"sampler":{"x_extend":"Pad","y_extend":"Pad","quality":"Medium","alpha":1.0}}}"#
        );
        let back: Brush = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, brush);
    }

    /// A chosen fit and crop are written (a `skip_serializing_if` on the wrong
    /// predicate would silently drop them); the default stays out (§5.11).
    #[test]
    fn a_chosen_fit_is_written_and_the_default_one_is_not() {
        let mut brush = image_brush(ImageId("abc123".into()));
        let peniko::Brush::Image(img) = &mut brush else {
            unreachable!("image_brush builds an image brush")
        };
        img.image.fit = ImageFit::Crop;
        img.image.crop = kurbo::Rect::new(0.25, 0.0, 0.75, 1.0);
        let json = serde_json::to_string(&brush).expect("serializes");
        assert!(
            json.contains(r#""fit":"Crop""#)
                && json.contains(r#""crop":{"x0":0.25,"y0":0.0,"x1":0.75,"y1":1.0}"#),
            "a crop must reach the file, got {json}"
        );
        let back: Brush = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, brush);
    }

    /// The source rectangle's two opposite corners in the frame's local space.
    fn corners(f: &Framing, w: u32, h: u32) -> (kurbo::Point, kurbo::Point) {
        (
            f.transform * kurbo::Point::ZERO,
            f.transform * kurbo::Point::new(w as f64, h as f64),
        )
    }

    /// All eight symmetries of the rectangle.
    fn every_orientation() -> impl Iterator<Item = ImageOrient> {
        (0..4).flat_map(|quarters| {
            [false, true]
                .into_iter()
                .map(move |mirrored| ImageOrient { quarters, mirrored })
        })
    }

    fn fitted(fit: ImageFit, frame: kurbo::Rect, w: u32, h: u32) -> Framing {
        ImageRef {
            fit,
            ..ImageRef::new(ImageId("x".into()))
        }
        .framing(frame, w, h)
    }

    /// `fitted` for the two modes that read a parameter beside the mode.
    fn parameterized(
        fit: ImageFit,
        crop: kurbo::Rect,
        tile_scale: f64,
        frame: kurbo::Rect,
        w: u32,
        h: u32,
    ) -> Framing {
        ImageRef {
            fit,
            crop,
            tile_scale,
            ..ImageRef::new(ImageId("x".into()))
        }
        .framing(frame, w, h)
    }

    /// Entering Crop at `cover_crop` draws exactly what Fill drew, across three
    /// aspect relationships (the centring term vanishes on the matching one) and
    /// all eight orientations (a `cover_crop` that ignored orientation is wrong
    /// for the four that swap the axes).
    #[test]
    fn entering_crop_at_its_cover_rectangle_draws_exactly_what_fill_drew() {
        let id = ImageId("x".into());
        for (frame, w, h, what) in [
            (
                kurbo::Rect::new(10.0, 20.0, 210.0, 120.0),
                100,
                100,
                "square in a wide frame",
            ),
            (
                kurbo::Rect::new(0.0, 0.0, 100.0, 300.0),
                400,
                200,
                "wide source in a tall frame",
            ),
            (
                kurbo::Rect::new(5.0, 5.0, 205.0, 105.0),
                200,
                100,
                "source at the frame's own aspect",
            ),
        ] {
            for orient in every_orientation() {
                let filled = ImageRef {
                    fit: ImageFit::Fill,
                    orient,
                    ..ImageRef::new(id.clone())
                };
                let cropped = ImageRef {
                    fit: ImageFit::Crop,
                    crop: filled.cover_crop(frame, w, h),
                    ..filled.clone()
                };
                let (a, b) = (filled.framing(frame, w, h), cropped.framing(frame, w, h));
                // Compared through the corners, not the coefficients.
                let corner = |f: &Framing, p: kurbo::Point| f.transform * p;
                for p in [kurbo::Point::ZERO, kurbo::Point::new(w as f64, h as f64)] {
                    let (pa, pb) = (corner(&a, p), corner(&b, p));
                    assert!(
                        (pa - pb).hypot() < 1e-9,
                        "{what} at {orient:?}: cover put {p:?} at {pa:?} \
                         and the seeded crop at {pb:?}"
                    );
                }
            }
        }
    }

    /// Entering Crop on a letterboxed `Fit` picture cannot reproduce it (§15
    /// D561): the exact rectangle leaves the source, and `crop_clamped` of it *is*
    /// `cover_crop`, at every orientation. The exact rectangle is read off
    /// `ImageRef::picture_box` rather than re-derived. The `Fill` row is the
    /// control (its exact rectangle equals cover unclamped).
    ///
    /// flip: `cover_crop`'s `.max` → `.min` → fails at the Fill row's derivation
    /// assertion, before the clamped one runs.
    #[test]
    fn a_fit_pictures_exact_crop_leaves_the_source_and_clamps_to_the_cover_one() {
        let id = ImageId("x".into());
        for (frame, w, h, letterboxes, what) in [
            (
                kurbo::Rect::new(10.0, 20.0, 210.0, 120.0),
                100,
                100,
                true,
                "square in a wide frame",
            ),
            (
                kurbo::Rect::new(0.0, 0.0, 100.0, 300.0),
                400,
                200,
                true,
                "wide source in a tall frame",
            ),
            (
                kurbo::Rect::new(5.0, 5.0, 205.0, 105.0),
                200,
                100,
                false,
                "source at the frame's own aspect",
            ),
        ] {
            for orient in every_orientation() {
                // The frame expressed against the whole picture's box, as a crop is.
                let exact_for = |fit: ImageFit| {
                    let r = ImageRef {
                        fit,
                        orient,
                        ..ImageRef::new(id.clone())
                    };
                    let pb = r.picture_box(frame, w, h).expect("a picture box");
                    kurbo::Rect::new(
                        (frame.x0 - pb.x0) / pb.width(),
                        (frame.y0 - pb.y0) / pb.height(),
                        (frame.x1 - pb.x0) / pb.width(),
                        (frame.y1 - pb.y0) / pb.height(),
                    )
                };
                let cover = ImageRef {
                    orient,
                    ..ImageRef::new(id.clone())
                }
                .cover_crop(frame, w, h);

                // The control: cover's own exact rectangle needs no clamping.
                let filled = exact_for(ImageFit::Fill);
                assert!(
                    (filled.x0 - cover.x0).abs() < 1e-9
                        && (filled.y0 - cover.y0).abs() < 1e-9
                        && (filled.x1 - cover.x1).abs() < 1e-9
                        && (filled.y1 - cover.y1).abs() < 1e-9,
                    "{what} at {orient:?}: the derivation itself is wrong — \
                     Fill's exact rectangle {filled:?} is not the cover {cover:?}"
                );

                let fitted = exact_for(ImageFit::Fit);
                if letterboxes {
                    assert!(
                        fitted.x0 < -1e-9
                            || fitted.y0 < -1e-9
                            || fitted.x1 > 1.0 + 1e-9
                            || fitted.y1 > 1.0 + 1e-9,
                        "{what} at {orient:?}: a letterboxed picture's exact crop \
                         {fitted:?} is inside the source, so cover was avoidable"
                    );
                }
                let clamped = crop_clamped(fitted);
                assert!(
                    (clamped.x0 - cover.x0).abs() < 1e-9
                        && (clamped.y0 - cover.y0).abs() < 1e-9
                        && (clamped.x1 - cover.x1).abs() < 1e-9
                        && (clamped.y1 - cover.y1).abs() < 1e-9,
                    "{what} at {orient:?}: clamping Fit's exact crop gave \
                     {clamped:?}, not the cover {cover:?}"
                );
            }
        }
    }

    /// The cover crop lies inside the source and spans one axis fully (it is the
    /// widest crop); degenerate input gives `whole_crop`.
    #[test]
    fn the_cover_crop_lies_inside_the_source_and_touches_one_pair_of_edges() {
        for (frame, w, h) in [
            (kurbo::Rect::new(0.0, 0.0, 200.0, 100.0), 100, 100),
            (kurbo::Rect::new(0.0, 0.0, 100.0, 300.0), 400, 200),
            (kurbo::Rect::new(0.0, 0.0, 200.0, 100.0), 200, 100),
        ] {
            for orient in every_orientation() {
                let r = ImageRef {
                    orient,
                    ..ImageRef::new(ImageId("x".into()))
                };
                let c = r.cover_crop(frame, w, h);
                assert!(
                    c.x0 >= -1e-9 && c.y0 >= -1e-9 && c.x1 <= 1.0 + 1e-9 && c.y1 <= 1.0 + 1e-9,
                    "{c:?} left the source at {orient:?}"
                );
                let spans_x = (c.width() - 1.0).abs() < 1e-9;
                let spans_y = (c.height() - 1.0).abs() < 1e-9;
                assert!(
                    spans_x || spans_y,
                    "{c:?} touches neither pair of edges at {orient:?}, \
                     so it is not the widest crop"
                );
            }
        }
        assert_eq!(
            ImageRef::new(ImageId("x".into())).cover_crop(
                kurbo::Rect::new(0.0, 0.0, 10.0, 10.0),
                0,
                5
            ),
            whole_crop()
        );
    }

    /// Panning moves the picture with the pointer, asserted by where a source
    /// point lands: dragging right by a quarter of the frame carries the pixel at
    /// the left edge a quarter of the way across.
    #[test]
    fn panning_moves_the_picture_with_the_pointer() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let start = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        let r = ImageRef {
            fit: ImageFit::Crop,
            crop: start,
            ..ImageRef::new(ImageId("x".into()))
        };
        let before = r.framing(frame, 100, 100);
        let at_left = kurbo::Point::new(start.x0 * 100.0, start.center().y * 100.0);
        assert!(
            (before.transform * at_left).x.abs() < 1e-9,
            "it starts at the left edge"
        );

        let dragged = ImageRef {
            crop: crop_panned(start, kurbo::Vec2::new(25.0, 0.0), frame),
            ..r.clone()
        };
        let after = dragged.framing(frame, 100, 100);
        assert!(
            ((after.transform * at_left).x - 25.0).abs() < 1e-9,
            "dragging right by 25 must carry that pixel 25 to the right, it went to {}",
            (after.transform * at_left).x
        );
    }

    /// A pan into the edge slides the crop rather than resizing it (independent
    /// edge clamping would zoom the picture at the end of a drag).
    #[test]
    fn panning_into_the_edge_slides_rather_than_resizing_the_crop() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let start = kurbo::Rect::new(0.1, 0.1, 0.6, 0.6);
        let r = crop_panned(start, kurbo::Vec2::new(1000.0, 0.0), frame);
        assert!(
            (r.width() - start.width()).abs() < 1e-9,
            "the crop kept its width"
        );
        assert!((r.height() - start.height()).abs() < 1e-9, "and its height");
        assert!(
            r.x0 >= -1e-9,
            "and stopped at the source's edge, at {}",
            r.x0
        );
        assert!(
            (r.x0).abs() < 1e-9,
            "flush against it rather than short of it"
        );
        assert!((r.y0 - start.y0).abs() < 1e-9);
    }

    /// Reframing keeps the picture's scale (local units per source pixel) on both
    /// axes and holds the anchored corner, for far and near edges alike; a frame
    /// with no extent returns the crop. A width-only check would pass a version
    /// that slid the rectangle.
    #[test]
    fn reframing_keeps_the_picture_where_it_was_on_both_axes() {
        let (w, h) = (400u32, 300u32);
        let from = kurbo::Rect::new(0.0, 0.0, 200.0, 100.0);
        let start = kurbo::Rect::new(0.2, 0.1, 0.9, 0.6);
        let to = kurbo::Rect::new(0.0, 0.0, 150.0, 60.0);
        let next = crop_reframed(start, from, to);

        let scale = |crop: kurbo::Rect, frame: kurbo::Rect| {
            (
                frame.width() / (crop.width() * f64::from(w)),
                frame.height() / (crop.height() * f64::from(h)),
            )
        };
        let (sx0, sy0) = scale(start, from);
        let (sx1, sy1) = scale(next, to);
        assert!(
            (sx1 - sx0).abs() < 1e-9 && (sy1 - sy0).abs() < 1e-9,
            "the picture changed size: {sx0}×{sy0} became {sx1}×{sy1}"
        );
        assert!(
            (next.x0 - start.x0).abs() < 1e-9 && (next.y0 - start.y0).abs() < 1e-9,
            "the anchored corner moved in the source: {next:?} against {start:?}"
        );

        let near = crop_reframed(start, from, kurbo::Rect::new(50.0, 25.0, 200.0, 100.0));
        assert!(
            (near.x1 - start.x1).abs() < 1e-9 && (near.y1 - start.y1).abs() < 1e-9,
            "the far corner moved in the source: {near:?} against {start:?}"
        );
        assert!(near.x0 > start.x0 && near.y0 > start.y0, "{near:?}");

        let flat = kurbo::Rect::new(0.0, 0.0, 0.0, 100.0);
        assert_eq!(crop_reframed(start, flat, to), start);
    }

    /// The crop scale round-trips through `crop_scaled`, and its floor is cover
    /// (asking for a tenth of cover gives cover back). The fixture is oblong on
    /// both counts (a 2:1 source in a 1:2 frame), since a symmetric one would pass
    /// a version that recomputed each axis.
    #[test]
    fn the_crop_scale_round_trips_and_bottoms_out_at_cover() {
        let (w, h) = (400u32, 200u32);
        let frame = kurbo::Rect::new(10.0, 20.0, 110.0, 220.0);
        let mut r = ImageRef::new(ImageId("x".into()));
        r.fit = ImageFit::Crop;
        r.crop = r.cover_crop(frame, w, h);

        let cover = r.cover_scale(frame, w, h).expect("a cover scale");
        let at_cover = r.crop_scale(frame, w, h).expect("a scale");
        assert!(
            (at_cover - cover).abs() < 1e-9,
            "the cover rectangle must read as the cover scale: {at_cover} against {cover}"
        );

        let want = cover * 2.5;
        r.crop = crop_scaled(r.crop, at_cover / want);
        let got = r.crop_scale(frame, w, h).expect("a scale");
        assert!(
            (got - want).abs() < 1e-9,
            "asked for {want} and read back {got}"
        );
        let seed = ImageRef::new(ImageId("x".into())).cover_crop(frame, w, h);
        assert!(
            (r.crop.width() / r.crop.height() - seed.width() / seed.height()).abs() < 1e-9,
            "zooming changed the crop's proportions: {:?}",
            r.crop
        );

        r.crop = crop_scaled(r.crop, 10.0);
        let floor = r.crop_scale(frame, w, h).expect("a scale");
        assert!(
            (floor - cover).abs() < 1e-9,
            "zooming out ran past cover to {floor}, where cover is {cover} — the \
             crop left the source and `Extend::Pad` is smearing the edge row"
        );

        let before = r.crop;
        assert_eq!(crop_scaled(before, 0.0), before);
        assert_eq!(crop_scaled(before, f64::NAN), before);
    }

    /// `picture_box` says where each mode put the picture, checked against what
    /// the mode means (Fill overflows, Fit falls short, a half Crop is twice the
    /// frame) rather than a recomputed transform.
    #[test]
    fn the_pictures_box_says_where_each_mode_put_it() {
        let (w, h) = (400u32, 200u32); // 2:1
        let frame = kurbo::Rect::new(10.0, 20.0, 110.0, 220.0); // 1:2, offset
        let mut r = ImageRef::new(ImageId("x".into()));

        let fill = r.picture_box(frame, w, h).expect("a picture");
        assert!(
            fill.width() >= frame.width() - 1e-9 && fill.height() >= frame.height() - 1e-9,
            "Fill must cover the frame: {fill:?}"
        );
        assert!(
            fill.width() > frame.width() + 1.0,
            "and overflow it: {fill:?}"
        );

        r.fit = ImageFit::Fit;
        let fit = r.picture_box(frame, w, h).expect("a picture");
        assert!(
            fit.width() <= frame.width() + 1e-9 && fit.height() <= frame.height() + 1e-9,
            "Fit must stay inside the frame: {fit:?}"
        );
        assert_eq!(r.framing(frame, w, h).clip, Some(fit));

        // Half the source in each direction: twice the frame, centred on it.
        r.fit = ImageFit::Crop;
        r.crop = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        let cropped = r.picture_box(frame, w, h).expect("a picture");
        assert!(
            (cropped.width() - frame.width() * 2.0).abs() < 1e-9
                && (cropped.height() - frame.height() * 2.0).abs() < 1e-9,
            "{cropped:?} against {frame:?}"
        );
        assert!((cropped.center() - frame.center()).hypot() < 1e-9);

        assert_eq!(r.picture_box(frame, 0, h), None);
        assert_eq!(r.picture_box(kurbo::Rect::ZERO, w, h), None);
    }

    /// A parameter is read only by the mode that needs it. With `crop` and
    /// `tile_scale` beside the mode rather than in its variant, a mode could see
    /// another's number, so the isolation is asserted instead.
    #[test]
    fn a_mode_ignores_the_parameters_it_does_not_use() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let plain = ImageRef::new(ImageId("x".into()));
        let loaded = ImageRef {
            crop: kurbo::Rect::new(0.25, 0.0, 0.75, 1.0),
            tile_scale: 3.0,
            ..plain.clone()
        };

        for mode in [ImageFit::Fill, ImageFit::Fit] {
            let bare = ImageRef {
                fit: mode,
                ..plain.clone()
            };
            let carrying = ImageRef {
                fit: mode,
                ..loaded.clone()
            };
            assert_eq!(
                bare.framing(frame, 100, 50),
                carrying.framing(frame, 100, 50),
                "{mode:?} must draw the same whether or not a crop is stored"
            );
        }

        // Crop reads the rectangle and not the scale; Tile the reverse.
        let cropped = ImageRef {
            fit: ImageFit::Crop,
            ..loaded.clone()
        };
        assert_eq!(
            cropped.framing(frame, 100, 50),
            ImageRef {
                fit: ImageFit::Crop,
                tile_scale: 99.0,
                ..loaded.clone()
            }
            .framing(frame, 100, 50),
            "a crop must not read the tile scale"
        );
        let tiled = ImageRef {
            fit: ImageFit::Tile,
            ..loaded.clone()
        };
        assert_eq!(
            tiled.framing(frame, 100, 50),
            ImageRef {
                fit: ImageFit::Tile,
                crop: kurbo::Rect::new(0.1, 0.1, 0.2, 0.2),
                ..loaded.clone()
            }
            .framing(frame, 100, 50),
            "a tile must not read the crop"
        );
        // Each really uses its own, rather than both defaulting.
        assert_eq!(
            tiled.framing(frame, 100, 50).transform * kurbo::Point::new(100.0, 50.0),
            kurbo::Point::new(300.0, 150.0),
            "the tile scale in force is the stored 3.0"
        );
        assert_eq!(
            cropped.framing(frame, 100, 50).transform * kurbo::Point::new(25.0, 0.0),
            kurbo::Point::ZERO,
            "the crop in force is the stored quarter-to-three-quarters"
        );
    }

    /// Only changed parameters are written: a never-cropped image writes no
    /// framing (§5.11), a crop the user set is written whatever the mode.
    #[test]
    fn only_the_parameters_that_were_changed_are_written() {
        let plain = image_brush(ImageId("abc".into()));
        let json = serde_json::to_string(&plain).expect("serializes");
        assert!(
            !json.contains("crop") && !json.contains("tile_scale") && !json.contains("fit"),
            "an untouched image brush must write no framing at all, got {json}"
        );

        let mut brush = image_brush(ImageId("abc".into()));
        let peniko::Brush::Image(img) = &mut brush else {
            unreachable!()
        };
        img.image.crop = kurbo::Rect::new(0.1, 0.2, 0.8, 0.9);
        img.image.fit = ImageFit::Fill; // the default: still no `fit` key
        let json = serde_json::to_string(&brush).expect("serializes");
        assert!(
            json.contains(r#""crop":{"x0":0.1,"y0":0.2,"x1":0.8,"y1":0.9}"#),
            "a crop the user set is written whatever the mode is, got {json}"
        );
        assert!(
            !json.contains("\"fit\""),
            "and the default mode still writes nothing, got {json}"
        );
        assert_eq!(
            serde_json::from_str::<Brush>(&json).expect("round trips"),
            brush
        );
    }

    /// Cover and contain on a frame that is not at the origin (centring by the
    /// frame's size alone is wrong in every moved frame). A 200×100 frame and a
    /// square source separate the modes: cover scales by 2, contain by 1.
    #[test]
    fn fill_covers_the_frame_and_fit_is_contained_by_it() {
        let frame = kurbo::Rect::new(10.0, 20.0, 210.0, 120.0);

        let fill = fitted(ImageFit::Fill, frame, 100, 100);
        assert_eq!(
            corners(&fill, 100, 100),
            (
                kurbo::Point::new(10.0, -30.0),
                kurbo::Point::new(210.0, 170.0)
            ),
            "cover scales by 2 and hangs 50 above and below"
        );
        assert!(fill.clip.is_none(), "nothing shows through a cover");

        let fit = fitted(ImageFit::Fit, frame, 100, 100);
        assert_eq!(
            corners(&fit, 100, 100),
            (
                kurbo::Point::new(60.0, 20.0),
                kurbo::Point::new(160.0, 120.0)
            ),
            "contain scales by 1 and centres, leaving 50 either side"
        );
        assert_eq!(
            fit.clip,
            Some(kurbo::Rect::new(60.0, 20.0, 160.0, 120.0)),
            "the letterbox must be cut, since Extend::Pad would smear into it"
        );
        assert_eq!(fill.extend, peniko::Extend::Pad);
        assert_eq!(fit.extend, peniko::Extend::Pad);
    }

    /// A fit that does not letterbox emits no clip (the click-placed case).
    #[test]
    fn fit_clips_only_where_it_letterboxes() {
        let frame = kurbo::Rect::new(0.0, 0.0, 300.0, 150.0);
        let square = fitted(ImageFit::Fit, frame, 100, 100);
        assert!(square.clip.is_some(), "a square in a 2:1 frame letterboxes");
        let matched = fitted(ImageFit::Fit, frame, 200, 100);
        assert!(
            matched.clip.is_none(),
            "the same aspect fills the frame exactly and needs no clip"
        );
        assert_eq!(
            corners(&matched, 200, 100),
            (kurbo::Point::ZERO, kurbo::Point::new(300.0, 150.0))
        );
    }

    /// A crop maps the *pixels* its normalized rectangle names (the middle half of
    /// a 100×50 picture) onto the frame's corners; a 1×1 source would hide a
    /// fraction-for-pixels mistake.
    #[test]
    fn a_crop_maps_its_rectangle_onto_the_frame() {
        let frame = kurbo::Rect::new(10.0, 10.0, 110.0, 110.0);
        let f = parameterized(
            ImageFit::Crop,
            kurbo::Rect::new(0.25, 0.0, 0.75, 1.0),
            1.0,
            frame,
            100,
            50,
        );
        assert_eq!(f.transform * kurbo::Point::new(25.0, 0.0), frame.origin());
        assert_eq!(
            f.transform * kurbo::Point::new(75.0, 50.0),
            kurbo::Point::new(110.0, 110.0)
        );
        assert!(f.clip.is_none(), "a crop covers its frame by construction");

        let whole = fitted(ImageFit::Crop, frame, 100, 50);
        assert_eq!(
            corners(&whole, 100, 50),
            (frame.origin(), kurbo::Point::new(110.0, 110.0))
        );
    }

    /// Tile is anchored at the frame's corner (a repeat has no centre), at its own
    /// scale, repeating.
    #[test]
    fn tile_repeats_at_its_scale_from_the_frames_corner() {
        let frame = kurbo::Rect::new(10.0, 20.0, 210.0, 120.0);
        let f = parameterized(ImageFit::Tile, whole_crop(), 0.5, frame, 100, 100);
        assert_eq!(
            corners(&f, 100, 100),
            (kurbo::Point::new(10.0, 20.0), kurbo::Point::new(60.0, 70.0)),
            "one tile is 50 units square in the frame's top-left corner"
        );
        assert_eq!(f.extend, peniko::Extend::Repeat, "the tiles must repeat");
        assert!(f.clip.is_none());
    }

    /// Degenerate inputs, including spans that overflow to `inf` (§15 D452),
    /// resolve to the identity framing rather than a singular brush transform.
    #[test]
    fn degenerate_inputs_resolve_to_the_identity_framing() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(fitted(ImageFit::Fill, frame, 0, 100), Framing::identity());
        assert_eq!(fitted(ImageFit::Fill, frame, 100, 0), Framing::identity());
        assert_eq!(
            fitted(ImageFit::Fit, kurbo::Rect::new(5.0, 5.0, 5.0, 90.0), 10, 10),
            Framing::identity(),
            "a frame with no width has no framing to get right"
        );
        assert_eq!(
            parameterized(
                ImageFit::Crop,
                kurbo::Rect::new(0.5, 0.0, 0.5, 1.0),
                1.0,
                frame,
                100,
                100
            ),
            Framing::identity(),
            "a crop of no width names no pixels"
        );
        let zero = parameterized(ImageFit::Tile, whole_crop(), 0.0, frame, 100, 100);
        assert_eq!(
            corners(&zero, 100, 100),
            (kurbo::Point::ZERO, kurbo::Point::new(100.0, 100.0))
        );
        assert_eq!(zero.extend, peniko::Extend::Repeat);

        // Finite coordinates whose span `x1 - x0` overflows to `inf`.
        let huge = kurbo::Rect::new(-1e308, -1e308, 1e308, 1e308);
        assert!(
            huge.width().is_infinite(),
            "the fixture overflows as intended"
        );
        assert_eq!(
            parameterized(ImageFit::Crop, huge, 1.0, frame, 100, 100),
            Framing::identity(),
            "a crop whose span overflows has no framing to get right"
        );
        assert_eq!(
            fitted(
                ImageFit::Fill,
                kurbo::Rect::new(-1e308, 0.0, 1e308, 100.0),
                100,
                100
            ),
            Framing::identity(),
            "and neither has a frame whose span overflows"
        );
    }

    /// A crop whose span overflows never reaches the document: `crop_clamped`
    /// and `crop_panned` stay finite (`usable_span` stops the NaN being made;
    /// `build::brush_is_finite`, D451, stops one reaching the document).
    ///
    /// flip: `w <= 0.0 || h <= 0.0` in `crop_clamped` → fails at the first
    /// assertion, while `degenerate_inputs_resolve_to_the_identity_framing` stays
    /// green, so the two guards are separate.
    #[test]
    fn a_crop_whose_span_overflows_never_reaches_the_document() {
        let huge = kurbo::Rect::new(-1e308, -1e308, 1e308, 1e308);
        assert_eq!(
            crop_clamped(huge),
            whole_crop(),
            "an unusable crop resolves to the whole picture, as a zero-sized one does"
        );

        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let panned = crop_panned(huge, kurbo::Vec2::new(5.0, 0.0), frame);
        assert!(
            [panned.x0, panned.y0, panned.x1, panned.y1]
                .iter()
                .all(|f| f.is_finite()),
            "a crop-pan of an unusable crop is finite: {panned:?}"
        );

        let ordinary = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        assert_eq!(
            crop_clamped(ordinary),
            ordinary,
            "a real crop is left alone"
        );
    }

    /// The turn has period four and each mirror is its own inverse. It does not
    /// prove the matrices exact: the fourth turn lands on the `_ =>
    /// Affine::IDENTITY` arm, so an `Affine::rotate` spelling passes;
    /// `every_orientation_maps_the_source_box_exactly_onto_the_oriented_box`
    /// rejects it.
    #[test]
    fn four_quarter_turns_come_back_to_exactly_where_they_started() {
        let start = ImageRef::new(ImageId("x".into()));
        let mut r = start.clone();
        for _ in 0..4 {
            r = r.reoriented(OrientOp::Turn);
        }
        assert_eq!(r.orient, start.orient, "four turns is not the identity");
        assert_eq!(
            r.orient.affine(320.0, 200.0).as_coeffs(),
            kurbo::Affine::IDENTITY.as_coeffs(),
            "and the matrix has to be the identity to the bit"
        );
        for op in [OrientOp::FlipH, OrientOp::FlipV] {
            let there_and_back = start.reoriented(op).reoriented(op);
            assert_eq!(there_and_back.orient, start.orient, "{op:?} twice");
            assert_eq!(
                there_and_back.orient.affine(320.0, 200.0).as_coeffs(),
                kurbo::Affine::IDENTITY.as_coeffs()
            );
        }
    }

    /// Each op composes onto the existing transform in the frame's axes, from all
    /// eight orientations, asserted against the matrix. `mirrored = !mirrored`
    /// alone passes for the two unturned orientations and fails the other six.
    #[test]
    fn a_flip_or_a_turn_composes_in_the_frames_axes_from_every_orientation() {
        use kurbo::Affine;
        // Oblong, so a wrong turn cannot hide behind a square.
        let (w, h) = (320.0, 200.0);
        for o in every_orientation() {
            let (ow, oh) = o.oriented_size(w, h);
            for (op, in_frame, what) in [
                (
                    OrientOp::FlipH,
                    Affine::new([-1.0, 0.0, 0.0, 1.0, ow, 0.0]),
                    "mirror about the frame's vertical axis",
                ),
                (
                    OrientOp::FlipV,
                    Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, oh]),
                    "mirror about the frame's horizontal axis",
                ),
                (
                    OrientOp::Turn,
                    Affine::new([0.0, 1.0, -1.0, 0.0, oh, 0.0]),
                    "a quarter turn clockwise",
                ),
            ] {
                let want = in_frame * o.affine(w, h);
                let got = o.applying(op).affine(w, h);
                assert_eq!(
                    got.as_coeffs(),
                    want.as_coeffs(),
                    "{op:?} on {o:?} must be {what}: wanted {want:?}, got {got:?}"
                );
            }
        }
    }

    /// Every orientation maps the source box onto `(0,0)..oriented_size` with no
    /// gap or overhang (a translation missing from a turn matrix puts it in the
    /// wrong quadrant). **Compared exactly**: with `Affine::rotate(FRAC_PI_2)` the
    /// box lands within `1e-14` and this fails.
    #[test]
    fn every_orientation_maps_the_source_box_exactly_onto_the_oriented_box() {
        let (w, h) = (320.0, 200.0);
        for o in every_orientation() {
            let (ow, oh) = o.oriented_size(w, h);
            let a = o.affine(w, h);
            let mut got = kurbo::Rect::from_points(a * kurbo::Point::ZERO, a * kurbo::Point::ZERO);
            for p in [
                kurbo::Point::new(w, 0.0),
                kurbo::Point::new(0.0, h),
                kurbo::Point::new(w, h),
            ] {
                got = got.union_pt(a * p);
            }
            assert_eq!(
                (got.x0, got.y0, got.x1, got.y1),
                (0.0, 0.0, ow, oh),
                "{o:?} put the source box at {got:?} instead of (0,0)..({ow},{oh})"
            );
            assert_eq!(got.width() * got.height(), w * h);
        }
    }

    /// A turned picture is framed by its turned aspect: a 200×100 source turned a
    /// quarter is 100×200, which cover scales by 2. A version that composed the
    /// turn but kept the original aspect would scale by 1 and leave half the frame
    /// empty.
    #[test]
    fn a_turned_picture_covers_and_contains_by_its_turned_aspect() {
        let frame = kurbo::Rect::new(0.0, 0.0, 200.0, 100.0);
        let turned = ImageRef {
            fit: ImageFit::Fill,
            orient: ImageOrient {
                quarters: 1,
                mirrored: false,
            },
            ..ImageRef::new(ImageId("x".into()))
        };
        let f = turned.framing(frame, 200, 100);
        // 100×200 scaled by 2: 200×400, hanging 150 above and below.
        let (a, b) = corners(&f, 200, 100);
        let (lo, hi) = (
            kurbo::Point::new(a.x.min(b.x), a.y.min(b.y)),
            kurbo::Point::new(a.x.max(b.x), a.y.max(b.y)),
        );
        assert_eq!(
            (lo, hi),
            (
                kurbo::Point::new(0.0, -150.0),
                kurbo::Point::new(200.0, 250.0)
            ),
            "cover of a turned 200×100 in a 200×100 frame"
        );
        assert!(f.clip.is_none(), "a cover leaves nothing showing");

        // Contain: scale 0.5, a 50×100 picture centred.
        let fitted = ImageRef {
            fit: ImageFit::Fit,
            ..turned.clone()
        }
        .framing(frame, 200, 100);
        assert_eq!(
            fitted.clip,
            Some(kurbo::Rect::new(75.0, 0.0, 125.0, 100.0)),
            "a turned picture letterboxes on the other axis"
        );
    }

    /// Reorienting a cropped picture keeps the same source pixels in the frame,
    /// asserted by mapping the frame's corners back through the framing. Writing
    /// the orientation and leaving `crop` alone fails here only. The crop is
    /// neither centred nor square, so a rectangle carried the wrong way round is
    /// caught too.
    #[test]
    fn reorienting_a_cropped_picture_keeps_the_same_pixels_in_the_frame() {
        let frame = kurbo::Rect::new(10.0, 20.0, 210.0, 120.0);
        let (w, h) = (400u32, 200u32);
        let shown = |r: &ImageRef| -> kurbo::Rect {
            let inv = r.framing(frame, w, h).transform.inverse();
            let mut b = kurbo::Rect::from_points(inv * frame.origin(), inv * frame.origin());
            for p in [
                kurbo::Point::new(frame.x1, frame.y0),
                kurbo::Point::new(frame.x0, frame.y1),
                kurbo::Point::new(frame.x1, frame.y1),
            ] {
                b = b.union_pt(inv * p);
            }
            b
        };
        let start = ImageRef {
            fit: ImageFit::Crop,
            crop: kurbo::Rect::new(0.1, 0.25, 0.55, 0.6),
            ..ImageRef::new(ImageId("x".into()))
        };
        // Compared loosely: `shown` goes through a matrix inverse.
        let before = shown(&start);
        let d = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            d(before.x0, 40.0) && d(before.y0, 50.0) && d(before.x1, 220.0) && d(before.y1, 120.0),
            "the fixture is showing {before:?}, not the (40,50)..(220,120) it thinks it is"
        );

        let mut r = start.clone();
        for op in [
            OrientOp::Turn,
            OrientOp::FlipH,
            OrientOp::Turn,
            OrientOp::FlipV,
            OrientOp::Turn,
        ] {
            r = r.reoriented(op);
            let after = shown(&r);
            assert!(
                d(after.x0, before.x0)
                    && d(after.y0, before.y0)
                    && d(after.x1, before.x1)
                    && d(after.y1, before.y1),
                "after {op:?} the frame shows {after:?} of the source instead of {before:?}"
            );
        }
        // The whole-source sentinel is a fixed point of all three ops.
        for op in [OrientOp::FlipH, OrientOp::FlipV, OrientOp::Turn] {
            assert_eq!(
                ImageRef::new(ImageId("x".into())).reoriented(op).crop,
                whole_crop(),
                "{op:?} disturbed the never-cropped sentinel"
            );
        }
    }

    /// An orientation nobody set stays out of the file (§5.11), and one somebody
    /// set reaches it alone.
    #[test]
    fn only_an_orientation_that_was_set_is_written() {
        let plain = image_brush(ImageId("abc".into()));
        let json = serde_json::to_string(&plain).expect("serializes");
        assert!(
            !json.contains("orient"),
            "an untouched image brush must write no orientation, got {json}"
        );

        let mut brush = image_brush(ImageId("abc".into()));
        let peniko::Brush::Image(img) = &mut brush else {
            unreachable!()
        };
        img.image = img.image.reoriented(OrientOp::FlipH);
        let json = serde_json::to_string(&brush).expect("serializes");
        assert!(
            json.contains(r#""orient":{"mirrored":true}"#),
            "a flip must reach the file and take nothing else with it, got {json}"
        );
        assert_eq!(
            serde_json::from_str::<Brush>(&json).expect("round trips"),
            brush
        );

        let turned = ImageRef::new(ImageId("abc".into())).reoriented(OrientOp::Turn);
        let json = serde_json::to_string(&turned).expect("serializes");
        assert!(
            json.contains(r#""orient":{"quarters":1}"#),
            "a turn must reach the file, got {json}"
        );
    }

    /// A crop whose span is usable but whose origin overflows (`1e300` of span at
    /// `x0 = -1e308`, source 4000 wide) resolves to the identity framing, and
    /// `picture_box` answers the source box rather than NaNs. The fixture is an
    /// offset from `-1e308` because two round numbers overflow the span too.
    ///
    /// flip: removing the finiteness test on the composed transform in
    /// `ImageRef::framing` → fails at the first assertion; the `usable_span`
    /// assertions stay green.
    #[test]
    fn a_crop_whose_origin_overflows_resolves_to_the_identity_framing() {
        let mut r = ImageRef::new(ImageId("a".into()));
        r.fit = ImageFit::Crop;
        r.crop = kurbo::Rect::new(-1e308, -1e308, -1e308 + 1e300, -1e308 + 1e300);
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);

        assert!(
            usable_span(r.crop.width() * 4000.0),
            "the span must pass D452's guard, or this is D452's own case again"
        );
        assert!(
            !(r.crop.x0 * 4000.0).is_finite(),
            "and the origin must not, or there is nothing to overflow"
        );

        assert_eq!(
            r.framing(frame, 4000, 3000),
            Framing::identity(),
            "a crop whose origin overflows has no framing to get right"
        );
        assert_eq!(
            r.picture_box(frame, 4000, 3000),
            Some(kurbo::Rect::new(0.0, 0.0, 4000.0, 3000.0)),
            "and picture_box reads off framing, so it answers the source box \
             rather than the four NaNs tools::crop_resized clamps against"
        );
    }
}

#[cfg(test)]
mod adjust_tests {
    use super::*;

    fn adjusted(set: impl FnOnce(&mut ImageAdjust)) -> AdjustPipeline {
        let mut a = ImageAdjust::NEUTRAL;
        set(&mut a);
        a.pipeline().expect("a moved slider describes a pipeline")
    }

    /// A pixel put through the pipeline, as bytes.
    fn through(p: &AdjustPipeline, rgb: [u8; 3]) -> [u8; 3] {
        p.apply(rgb.map(|c| c as f32 / 255.0))
            .map(|c| (c * 255.0 + 0.5) as u8)
    }

    /// The neutral value is no pipeline and writes no bytes (§5.11), and a moved
    /// slider does reach the file. The halves fail separately: `Some` for the
    /// identity costs every image a second buffer.
    #[test]
    fn a_neutral_adjustment_is_no_pipeline_and_no_bytes() {
        assert!(ImageAdjust::NEUTRAL.pipeline().is_none());
        assert!(ImageAdjust::default().is_default());

        let brush = image_brush(ImageId("abc123".into()));
        let json = serde_json::to_string(&brush).expect("serializes");
        assert!(
            !json.contains("adjust"),
            "an untouched picture must write no adjustment at all, got {json}"
        );

        let mut moved = image_brush(ImageId("abc123".into()));
        let peniko::Brush::Image(img) = &mut moved else {
            unreachable!("image_brush builds an image brush")
        };
        img.image.adjust.exposure = 0.25;
        let json = serde_json::to_string(&moved).expect("serializes");
        assert!(
            json.contains(r#""adjust":{"exposure":0.25}"#),
            "only the moved slider is written, got {json}"
        );
        let back: Brush = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, moved);
    }

    /// Each of the seven does what its word says, asserted on pixels. Mid-grey
    /// probes the light sliders; saturation needs a saturated colour, since a grey
    /// stays grey whichever way the coefficients went.
    #[test]
    fn every_slider_moves_the_picture_the_way_its_word_says() {
        let grey = [128u8, 128, 128];
        let sky = [60u8, 120, 200];

        let up = through(&adjusted(|a| a.exposure = 0.5), grey);
        let down = through(&adjusted(|a| a.exposure = -0.5), grey);
        assert!(
            up[0] > 160 && down[0] < 100,
            "exposure is stops of light: {up:?} up, {down:?} down"
        );

        // Two levels: a probe at mid-grey would pass a version that did nothing.
        let hi = through(&adjusted(|a| a.contrast = 0.5), [180, 180, 180]);
        let lo = through(&adjusted(|a| a.contrast = 0.5), [80, 80, 80]);
        assert!(
            hi[0] > 190 && lo[0] < 70,
            "contrast spreads about mid-grey: {hi:?} and {lo:?}"
        );

        let flat = through(&adjusted(|a| a.saturation = -1.0), sky);
        assert!(
            flat[0] == flat[1] && flat[1] == flat[2],
            "full desaturation is grey, got {flat:?}"
        );
        let loud = through(&adjusted(|a| a.saturation = 1.0), sky);
        let spread = |c: [u8; 3]| c.iter().max().unwrap() - c.iter().min().unwrap();
        assert!(
            spread(loud) > spread(sky),
            "saturation widens the channels: {loud:?} against {sky:?}"
        );

        let warm = through(&adjusted(|a| a.temperature = 1.0), grey);
        let cool = through(&adjusted(|a| a.temperature = -1.0), grey);
        assert!(
            warm[0] > warm[2] && cool[2] > cool[0],
            "temperature trades red against blue: {warm:?} warm, {cool:?} cool"
        );

        let magenta = through(&adjusted(|a| a.tint = 1.0), grey);
        let green = through(&adjusted(|a| a.tint = -1.0), grey);
        assert!(
            magenta[1] < magenta[0] && green[1] > green[0],
            "tint trades green against magenta: {magenta:?} and {green:?}"
        );
    }

    /// Highlights and shadows each leave the other end of the range alone; the
    /// far end is the probe that fails a version lifting the whole curve. Black and
    /// white are fixed points to the byte (weights `(1−v)²` and `v²`).
    #[test]
    fn each_end_of_the_tone_curve_moves_alone() {
        for (name, set) in [
            (
                "shadows",
                (|a: &mut ImageAdjust| a.shadows = 1.0) as fn(&mut ImageAdjust),
            ),
            ("highlights", |a: &mut ImageAdjust| a.highlights = 1.0),
        ] {
            let p = adjusted(set);
            let dark = through(&p, [40, 40, 40]);
            let light = through(&p, [215, 215, 215]);
            let (near, far) = match name {
                "shadows" => (dark[0], light[0]),
                _ => (light[0], dark[0]),
            };
            let (started_near, started_far) = match name {
                "shadows" => (40u8, 215u8),
                _ => (215u8, 40u8),
            };
            assert!(
                near > started_near + 20,
                "{name} must lift the end it names: {near} from {started_near}"
            );
            assert!(
                far.abs_diff(started_far) < 12,
                "{name} must leave the other end alone: {far} from {started_far}"
            );
        }
        // Shadows cannot touch white, highlights cannot touch black.
        for (name, set, fixed, at) in [
            (
                "shadows",
                (|a: &mut ImageAdjust| a.shadows = -1.0) as fn(&mut ImageAdjust),
                255u8,
                "white",
            ),
            (
                "highlights",
                |a: &mut ImageAdjust| a.highlights = -1.0,
                0,
                "black",
            ),
        ] {
            let p = adjusted(set);
            assert_eq!(
                through(&p, [fixed; 3]),
                [fixed; 3],
                "{name} must leave {at} exactly where it is"
            );
        }
    }

    /// The tone curve never doubles back at either extreme of either slider.
    ///
    /// flip: raising the ±0.4 strength to 0.55 in `ImageAdjust::curve` → the
    /// shadow arm fails the monotonic assertion.
    #[test]
    fn the_tone_curve_is_monotonic_at_every_extreme() {
        for (h, s) in [
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (1.0, -1.0),
        ] {
            let mut a = ImageAdjust::NEUTRAL;
            a.highlights = h;
            a.shadows = s;
            let table = a.curve().expect("a moved end has a curve");
            for pair in table.windows(2) {
                assert!(
                    pair[1] >= pair[0] - 1e-6,
                    "the curve fell back at ({h}, {s}): {:?} then {:?}",
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    /// Contrast pivots on mid-grey. A bare diagonal scale with no offset spreads
    /// the tones the same at the top of the range and darkens the whole picture;
    /// only the fixed point catches it.
    #[test]
    fn contrast_holds_mid_grey_still() {
        for c in [-0.75, -0.25, 0.25, 0.9] {
            let p = adjusted(|a| a.contrast = c);
            let mid = p.apply([0.5, 0.5, 0.5]);
            assert!(
                (mid[0] - 0.5).abs() < 1e-5,
                "contrast {c} moved mid-grey to {}",
                mid[0]
            );
        }
    }

    /// A table is read the way SVG reads one (see `TONE_STEPS`): `v = 1` uses the
    /// last pair rather than indexing off the end, and a value between two samples
    /// interpolates rather than snaps.
    #[test]
    fn a_transfer_table_is_read_by_svgs_own_rule() {
        let table = [0.0f32, 0.5, 1.0];
        assert!((transfer(&table, 0.0) - 0.0).abs() < 1e-6);
        assert!(
            (transfer(&table, 1.0) - 1.0).abs() < 1e-6,
            "v = 1 reads the last value"
        );
        assert!(
            (transfer(&table, 0.25) - 0.25).abs() < 1e-6,
            "a value between two samples interpolates, it does not snap"
        );
        assert!((transfer(&table, 0.75) - 0.75).abs() < 1e-6);
        assert_eq!(transfer(&[], 0.3), 0.3);
        assert_eq!(transfer(&[0.2], 0.9), 0.2);
    }

    /// The matrix runs first and the tone curve reads its clamped result. The
    /// probe is a pixel the matrix pushes past white: 0.9 doubled is 1.8, cut to
    /// 1.0, and `v + 0.4·(−1)·v²` at 1.0 is 0.6 (the other order gives a blown
    /// 1.0). Both are plausible pictures, so the number is asserted.
    #[test]
    fn the_matrix_runs_before_the_tone_curve() {
        let mut a = ImageAdjust::NEUTRAL;
        a.exposure = 1.0;
        a.highlights = -1.0;
        let p = a.pipeline().expect("two moved sliders");
        let hot = p.apply([0.9, 0.9, 0.9]);
        assert!(
            (hot[0] - 0.6).abs() < 1e-5,
            "the stages ran in the wrong order, or the overshoot reached the \
             curve unclamped: {} rather than 0.6",
            hot[0]
        );
    }

    /// An impossible value is refused at both doors: `Adjustment::set` (the
    /// panel's, which clamps) and `ImageAdjust::pipeline` (a hand-edited file's)
    /// — §15 D538. A clamping `pipeline` would pass `is_some()` and draw a picture
    /// the file does not describe, so `is_default()` is asserted false too.
    #[test]
    fn an_impossible_value_is_refused_at_both_doors() {
        let mut a = ImageAdjust::NEUTRAL;
        Adjustment::Exposure.set(&mut a, 12.0);
        assert_eq!(a.exposure, 1.0, "written values are held inside the range");
        Adjustment::Exposure.set(&mut a, f32::NAN);
        assert_eq!(
            a.exposure, 0.0,
            "and a non-number resets rather than sticks"
        );

        let hand_edited = ImageAdjust {
            contrast: f32::NAN,
            ..ImageAdjust::NEUTRAL
        };
        assert!(
            hand_edited.pipeline().is_none(),
            "a file carrying a NaN must draw the picture, not a hole"
        );
        assert!(
            !hand_edited.is_default(),
            "and it is not the neutral value, so it is not silently written away"
        );

        // Finite but out of range, so a bare finiteness guard passes both.
        for (label, a) in [
            (
                "exposure 1000: 2f32.powf(1000) is inf, and inf * 0.0 makes all \
                 twenty matrix coefficients NaN — every pixel renders black",
                ImageAdjust {
                    exposure: 1000.0,
                    ..ImageAdjust::NEUTRAL
                },
            ),
            (
                "shadows 2.0: the tone curve doubles back at 12 of its 32 steps, \
                 so source black comes out lighter than source mid-grey",
                ImageAdjust {
                    shadows: 2.0,
                    ..ImageAdjust::NEUTRAL
                },
            ),
        ] {
            assert!(a.pipeline().is_none(), "{label}");
            assert!(
                !a.is_default(),
                "{label} — and refusing must not make it look neutral"
            );
        }

        // Controls at both ends: `(-1.0..1.0)` or `(0.0..=1.0)` would pass the above.
        for v in [-1.0f32, 1.0, 0.5, -0.5] {
            let a = ImageAdjust {
                exposure: v,
                ..ImageAdjust::NEUTRAL
            };
            assert!(
                a.pipeline().is_some(),
                "control: {v} is inside the range `set` clamps to and must draw"
            );
        }
    }

    /// `Adjustment::ALL` covers every field of `ImageAdjust`.
    #[test]
    fn the_list_of_adjustments_covers_the_whole_struct() {
        let mut a = ImageAdjust::NEUTRAL;
        for (i, k) in Adjustment::ALL.iter().enumerate() {
            k.set(&mut a, (i + 1) as f32 / 10.0);
        }
        assert!(!a.is_default());
        for k in Adjustment::ALL {
            assert!(k.of(&a) != 0.0, "{} is not in ALL", k.label());
        }
        a.reset();
        assert_eq!(a, ImageAdjust::NEUTRAL, "reset puts all seven back");
        assert!(a.is_default());
    }
}
