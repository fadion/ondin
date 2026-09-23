//! The document's image table, and the reference a fill holds into it.
//!
//! **An image is a fill, not a node kind** (§5.5a): any shape, path or
//! boolean can carry one, so "image in a circle" works without masks existing.
//! What a `Fill` stores is an [`ImageRef`] — a key and nothing else — and the
//! bytes live once in a document-level table, so two layers showing the same
//! photograph store it once and a geometry edit produces a diff nowhere near it.
//!
//! **The stored bytes are the *encoded* original, never a decoded buffer.** A
//! 2000×1500 photo is 12 MB as RGBA and ~300 KB as the JPEG it arrived as, and
//! the file is written as text. Decoding happens in the app, once per image, into
//! a cache keyed by the same id.
//!
//! **The id is opaque here.** It is a content hash of the encoded bytes — which
//! is what makes deduplication free and makes an id stable across machines — but
//! core neither computes nor checks it: hashing would be a dependency core's
//! rules do not permit (§3), and the bytes arrive in the app already, where the
//! decode happens. Core stores keys and compares them.

use serde::{Deserialize, Serialize};

/// A key into the document's image table.
///
/// Ordered, because the table is written sorted by id for byte-stable output
/// (invariant 9).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageId(pub String);

impl std::fmt::Display for ImageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The encodings v1 accepts, named so that "why not TIFF" has an answer that is
/// a decision rather than an omission (§5.5a, §15 D191).
///
/// A GIF is its first frame; animation is out of v1 (§15 D191).
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

    /// The file extension for a *stored* picture's own format.
    ///
    /// ⚠️ **No caller, again.** It was written for *Export original…*, waited some
    /// months for it, got it in `OndinApp::export_image_action` (§15 D225), and
    /// lost it when that verb was removed on 2026-08-23 — the Export panel offers
    /// sizes and formats, which is what "export this picture" turns out to mean.
    /// So this is back to being a `pub` item in a library with nothing reaching it,
    /// where `dead_code` never fires and this comment is the only thing that says
    /// so. **Not to be confused with `ExportFormat::extension`**, which is the one
    /// with four live callers and is about a *rendered* file rather than a stored
    /// one.
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
/// **Embedded by default, linked as the per-image escape hatch** — Illustrator's
/// split, and for Illustrator's reason: a file that breaks when you send it to
/// someone is a worse failure than a large one. The two are convertible in both
/// directions (*Embed* and *Link*), which is why this is one enum on one entry
/// rather than two kinds of table.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageSource {
    /// The encoded file itself, written into the document as base64.
    ///
    /// **`Arc<[u8]>` rather than `Vec<u8>`, and it is a performance fix rather
    /// than a style** (§15 D301). `Document::apply` clones the whole document to
    /// get a working copy it can fail out of, so every committed edit — and every
    /// undo and every redo — deep-copied every photograph in the table. Measured
    /// before it was changed: **3.5 ms per commit at 20 MB of images, 10.2 ms at
    /// 60 MB**, in release, for a one-transform nudge. A held arrow key commits
    /// per repeat, so that is a stutter with no visible cause.
    ///
    /// **Safe because these bytes are immutable**, which is the property that
    /// matters rather than the table being append-only: nothing anywhere mutates
    /// an embedded blob in place — a *Replace…* writes a whole new entry under a
    /// new content hash — so no caller ever needs `Arc::make_mut` and the sharing
    /// can never be observed.
    Embedded(std::sync::Arc<[u8]>),
    /// A path on this machine, as written. **A string rather than a `PathBuf`**:
    /// a document is portable and a path is not, so what is stored is what the
    /// author's machine called it, and *Relink* is the answer when it no longer
    /// resolves. A link that has gone stale is never silently dropped from the
    /// document, and draws the visible placeholder [`missing_placeholder`]
    /// describes until something can read it (§15 D179). *Relink* is still the
    /// repair this variant wants; the one that exists is *Replace…*, which
    /// embeds a file rather than repointing the path.
    Linked(String),
}

/// One image the document knows about.
///
/// The intrinsic size is stored rather than derived, so placing a layer, laying
/// out its box and drawing the layers panel never wait on a decode — and so a
/// document with a broken link still knows how big the picture was.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageEntry {
    pub source: ImageSource,
    pub format: ImageFormat,
    /// Intrinsic pixel width.
    ///
    /// 🚨 **Render-originated data stored in the model, and it is *accepted*
    /// rather than tolerated** (§15 D788, `[A1-L3-11]`). Both this and
    /// [`Self::height`] are produced by `ondin-render`'s decoder and then stored in
    /// the `Document` and serialized — a derived value cached in the model, which
    /// is the pattern invariant 4 exists to prevent — and nothing re-decodes to
    /// check them, so a hand-edited or truncated file carries whatever size it
    /// claims.
    ///
    /// **Two reasons it is right anyway, and the second is the one that settles
    /// it.** Entries are content-hash-keyed and their bytes immutable, so it cannot
    /// go stale in ordinary use. And for [`ImageSource::Linked`] the bytes are **not
    /// in the document at all**, so the stored size is the only record there is —
    /// which is the point of a link, not a shortcoming of it. There is nothing to
    /// derive it from without the file, and the whole reason a link exists is that
    /// the file may be absent.
    ///
    /// ⚠️ **Recorded here because a reviewer read it as a violation and nearly
    /// filed it as one.** An accepted-by-necessity exception with no entry is
    /// indistinguishable from an oversight, which is the mechanism `CLAUDE.md`'s
    /// whole §15 rule is about.
    pub width: u32,
    /// Intrinsic pixel height. See [`Self::width`] — the two are one field for
    /// every purpose, including §15 D788's.
    pub height: u32,
}

impl ImageEntry {
    /// The encoded bytes, when they are in the document.
    ///
    /// `None` for a linked image, whose bytes are the app's to fetch — core does
    /// no file I/O.
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.source {
            ImageSource::Embedded(b) => Some(b),
            ImageSource::Linked(_) => None,
        }
    }

    /// ⚠️ **Twice without a caller, and each time for the same reason.** It sat
    /// `pub` here with none until `tools::original_refusal` gave it one (§15 D280)
    /// — and that mattered, because the two doors it replaced had each hand-rolled
    /// `bytes().is_some()`, which is equally false for a *linked* picture and for
    /// an id the table does not hold, so a document that had simply lost a picture
    /// was told it used linking. `original_refusal` went with the *Export
    /// original…* rows on 2026-08-23 and took this with it, and it had none again
    /// until `io::clip::parse` (§15 D852).
    ///
    /// **The distinction is the thing to keep, not this function.** Ask it
    /// explicitly rather than inferring "linked" from absent bytes; that inference
    /// is what cost behaviour last time, and `dead_code` will not warn here because
    /// this is a library.
    ///
    /// 🚨 **Linked sources are a decided non-goal for v1** (§15 D819). The three
    /// Asset rows that would author one — *Embed*/*Link*, *Relink…* and a
    /// document-wide *Embed all* — all wait on the same missing thing: a gesture
    /// that **authors** a linked source. §15 D191 is the out-of-v1 image list this
    /// joins.
    ///
    /// ⚠️ **This said *"`ImageSource::Linked` is constructed in one place … and
    /// nothing can make one"*, and the second half was false for a range** (§15
    /// D852, `[R2-L8-03]`) — the first staying literally true, one constructor,
    /// which is why nothing caught it. `io::schema`'s `into_entry` has **two**
    /// callers — the file loader and `io::clip::parse`, the clipboard crossing —
    /// and `build::missing_image_ops` carries whatever entries a paste brings into
    /// the document. So the doors onto a linked source are:
    ///
    /// - **A hand-written or foreign `.ondin`**, which the app opens as it always
    ///   has: the non-goal is authoring, not reading.
    /// - **The clipboard, which refuses one** — this function's only production
    ///   caller, and the non-goal's only enforcement. A link arriving there was an
    ///   attacker-chosen URL written verbatim into every SVG export.
    ///
    /// Every app path that *makes* a picture still funnels through
    /// `load_image_bytes`, which builds `Embedded` unconditionally.
    ///
    /// **This function stays**, and so does `tools::original_refusal`'s three-state
    /// reasoning in the comments where that function stood: the subject exists in
    /// the model, a foreign document can put one in front of the app, and the next
    /// person to build the authoring gesture should rebuild from those rather than
    /// re-derive them.
    pub fn is_linked(&self) -> bool {
        matches!(self.source, ImageSource::Linked(_))
    }

    /// How a document that is not ours refers to this picture: a `data:` URI for
    /// an embedded image, the stored path for a linked one.
    ///
    /// **Here rather than in the SVG writer, because base64 is core's already**
    /// — the save format writes the same bytes the same way (§5.5a), and one
    /// encoder is what keeps a document and its export saying the same thing
    /// about the same file. [`ImageFormat::mime`] was written for this caller
    /// before it existed.
    ///
    /// The *encoded original* goes out, never a re-encode: it is what the table
    /// stores and what the content hash names, so an exported picture is the file
    /// that was placed, byte for byte (§7.1).
    ///
    /// A linked image's path is emitted **as it was written**, relative or
    /// absolute, which is the only honest thing to do with a string this crate
    /// never resolved. It is not URL-escaped here; a writer that needs escaping
    /// for its own syntax does that itself, as the SVG one does.
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
/// **Four modes, and each of them is an affine plus a [`peniko::Extend`] (and,
/// for one of them, a clip) — no pixel work.** The picture is never resampled
/// into a new buffer to change how it sits; the same decoded blob is drawn with
/// a different brush transform, so switching mode costs nothing and scrubbing a
/// crop costs nothing.
///
/// **Fill and Fit are derived; Crop and Tile read a number stored beside this
/// one.** Fill and Fit answer "cover" and "contain" against whatever the frame
/// currently is, so resizing the shape re-frames the picture. Crop and Tile need
/// something the frame cannot re-derive — *this* rectangle of the source, *that*
/// repetition scale — and those live on [`ImageRef`] rather than in here. A mode
/// that stored a baked-in affine would come apart the first time the shape was
/// resized, which is why this is an enum of intents rather than a matrix.
///
/// **The payloads are deliberately not variant payloads.** They were, first, and
/// that made the mode strip a destructive control: clicking *Fit* to see the
/// whole picture and clicking back would hand you a fresh identity crop, because
/// the rectangle had nowhere to live while the mode was not Crop. A mode switch
/// must be a change of view, not a reset — so the parameters persist and only
/// their *meaning* switches on and off. It is the shape Figma's `scaleMode` /
/// `imageTransform` / `scalingFactor` has, for this reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageFit {
    /// Cover the frame, preserving aspect; whatever overflows is cut off.
    ///
    /// The default, and Figma's, because it is the mode that never shows the
    /// canvas through a hole the user did not ask for. A click-placed image is
    /// framed to its own aspect (§5.5a), so Fill and [`ImageFit::Fit`] are
    /// the same drawing there and only differ once the shape is resized.
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
    /// Whether this is the value a missing key means, for
    /// `skip_serializing_if`: an image placed before framing existed keeps its
    /// byte-for-byte line in the file (§5.11).
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Fill)
    }

    /// The four, in the order a mode strip offers them: the two that need no
    /// parameter first.
    pub const ALL: [ImageFit; 4] = [Self::Fill, Self::Fit, Self::Crop, Self::Tile];
}

/// A mirror and a quarter turn: how the picture sits inside its frame,
/// independent of the layer's own transform.
///
/// **Eight states, spelled canonically as mirror-then-turn.** The symmetries of
/// a rectangle are a group of exactly eight, and `flip_x`/`flip_y`/`turns` would
/// spell them sixteen ways — a horizontal flip *and* a vertical flip together
/// are a half turn — so two of those three fields would have to agree about
/// which spelling to write, and a file could carry a state the buttons can never
/// produce. Here [`Self::mirrored`] is applied first, in the source's own frame,
/// and [`Self::quarters`] clockwise turns after it; each of the eight has one
/// spelling and only one.
///
/// **The controls are actions, not toggles**, which is what this shape is for.
/// "Is it flipped horizontally?" has no answer once a quarter turn is involved:
/// the same drawing is a horizontal flip of one orientation and a vertical flip
/// of another. So [`OrientOp`] *composes* rather than sets, and the panel draws
/// plain buttons rather than toggles it would have to lie about. That holds even
/// though the popover offers only the two mirrors today ([`OrientOp::Turn`]): the
/// turn count is still live, because `FlipV` resolves to `quarters: 2`.
///
/// This is deliberately **not** [`crate::build::Orientation`], which decomposes a
/// layer's world matrix and carries a real angle. Nothing here is continuous:
/// the frame is fixed and the picture inside it turns in quarters, so there is
/// no angle to store and no decomposition to get wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageOrient {
    /// Quarter turns clockwise, applied **after** [`Self::mirrored`].
    ///
    /// Always written in `0..4` — [`ImageOrient::applying`] is the only thing
    /// that produces one — but read through [`ImageOrient::quarters`], which
    /// takes it modulo four so a hand-edited file naming five turns draws one
    /// rather than nothing. Note that the derived `PartialEq` compares the raw
    /// number, so a hand-written `4` is unequal to `0` while drawing the same;
    /// nothing in the app can reach that state.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub quarters: u8,
    /// Mirrored left-to-right in the source's own frame, applied **before** the
    /// turn.
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

    /// Whether this is the value a missing key means, for `skip_serializing_if`:
    /// an image placed before orientation existed keeps its byte-for-byte line in
    /// the file (§5.11).
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
    /// flipping an already-turned picture reverses the turn as well as the
    /// mirror. Spelling that out is the whole reason this type exists: written as
    /// two independent flags it is wrong for six of the eight states and right
    /// for the two anybody tests with.
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
    /// **Written out as exact matrices rather than composed from
    /// `Affine::rotate`.** `rotate(π/2)` returns a cosine of about `6.1e-17`, so
    /// a composed version leaves a shear of that size in every turned picture's
    /// brush transform and puts the turned box a hair off the corner it is
    /// supposed to land on. Invisible on screen; not invisible in the file, since
    /// a brush transform that differs in its last bits is a `SetFills` the
    /// equality guards do not catch. Pinned by
    /// `every_orientation_maps_the_source_box_exactly_onto_the_oriented_box`,
    /// which compares the box exactly and is the test that rejects the composed
    /// spelling.
    pub fn affine(self, width: f64, height: f64) -> kurbo::Affine {
        use kurbo::Affine;
        let (w, h) = (width, height);
        // Maps the source box onto itself, so the turn below still receives a box
        // at the origin whatever the mirror did.
        let mirror = if self.mirrored {
            Affine::new([-1.0, 0.0, 0.0, 1.0, w, 0.0])
        } else {
            Affine::IDENTITY
        };
        // Each turn is followed by the translation that brings the box back into
        // the positive quadrant, so the result is always `(0,0)..oriented_size`.
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
    /// **No control writes this today**, and that is a decision rather than an
    /// oversight, so it is said here where the next reader will look. The image
    /// popover had a rotate button beside the two mirrors and it was taken out:
    /// turning a picture inside a frame the box keeps is a real thing to want and
    /// a much rarer one than mirroring, and the Transform panel's own quarter
    /// turns already handle the common case — a photograph that came in sideways
    /// arrives in a shape cut to its own aspect, so turning the *layer* is the
    /// right answer there and turns the box with it.
    ///
    /// It stays because the arithmetic around it is what makes the other two
    /// correct, not because it is spare: [`ImageOrient::applying`] resolves
    /// `FlipV` to `quarters: 2`, so the turn count is live whether or not anything
    /// turns, and the odd quarters are one button away the moment an
    /// EXIF-orientation import or a rotate control wants them.
    Turn,
}

impl OrientOp {
    /// A normalized crop rectangle carried through this op.
    ///
    /// **[`ImageRef::crop`] is normalized to the picture *as oriented*, so a flip
    /// or a turn that left the rectangle alone would show a different part of the
    /// photograph** — you would turn a portrait and find yourself looking at its
    /// shoulder. Moving the rectangle with the picture is what makes the mirror
    /// buttons mirror what you are looking at rather than jump somewhere else in
    /// the source.
    ///
    /// [`whole_crop`] is a fixed point of all three, so a picture nobody has
    /// cropped keeps the sentinel — and keeps writing no rectangle at all.
    pub fn crop_carried(self, c: kurbo::Rect) -> kurbo::Rect {
        match self {
            Self::FlipH => kurbo::Rect::new(1.0 - c.x1, c.y0, 1.0 - c.x0, c.y1),
            Self::FlipV => kurbo::Rect::new(c.x0, 1.0 - c.y1, c.x1, 1.0 - c.y0),
            // A point at (x, y) of the unit square is at (1 − y, x) of the turned
            // one — the same map `ImageOrient::affine` makes, normalized.
            Self::Turn => kurbo::Rect::new(1.0 - c.y1, c.x0, 1.0 - c.y0, c.x1),
        }
    }
}

/// The rectangle an image that has **never been cropped** carries: the whole
/// source.
///
/// **A sentinel, not a picture anybody chose.** Mapped onto a frame of a
/// different aspect it stretches, which is nobody's idea of an uncropped
/// photograph — so everything that *enters* cropping replaces it with
/// [`ImageRef::cover_crop`], the rectangle `Fill` is already showing. That is
/// what makes choosing Crop change nothing on screen, and it is what *reset
/// crop* restores: putting this value back means "never cropped", and the next
/// entry seeds cover from it again.
///
/// It is also the `skip_serializing_if` value, so an image nobody has cropped
/// writes no rectangle at all (§5.11).
pub fn whole_crop() -> kurbo::Rect {
    kurbo::Rect::new(0.0, 0.0, 1.0, 1.0)
}

fn is_whole_crop(r: &kurbo::Rect) -> bool {
    *r == whole_crop()
}

/// Whether a width or a height is one this file's arithmetic can divide by
/// (§15 D452).
///
/// **Every degenerate-input guard in this module bounded the small end only** —
/// eight of them spelled `<= 0.0`, which `inf` passes, and the ninth and tenth
/// said the same thing in positive form — and that is the whole of
/// `[S9.1-L5-01]`. A `.ondin` whose image fill
/// carries `"crop": {"x0":-1e308, "y0":-1e308, "x1":1e308, "y1":1e308}` holds four
/// **finite** numbers — so it is not the non-finite family D421 and D451 cover —
/// but `crop.width()` overflows to `inf`, and `inf` is greater than zero. Three
/// measured consequences from that one document:
///
/// - `crop_clamped` divides `inf / inf` and returns an all-`NaN` rectangle, which
///   one crop-pan drag commits through `SetFills`; the file then saves `null` and
///   never opens again — D421's terminal failure by a different manufacturer.
/// - [`ImageRef::framing`] returns a **singular** `Affine` with a `NaN`
///   translation, which is exactly what `Framing::identity` exists to prevent and
///   what `degenerate_inputs_resolve_to_the_identity_framing` documents as
///   *"a backend question with no good answer"*.
/// - [`ImageRef::picture_box`] answers `Some(NaN…)` rather than `None`, and
///   `tools::crop_resized` then calls `f64::clamp(NaN, NaN)`, which **panics** —
///   on the UI thread, on a crop resize.
///
/// **Spelled once and asked at every span test in the module.** ⚠️ The finding
/// named **five** sites; a grep for the `<= 0.0` shape found **eight**; and a
/// ninth (`missing_placeholder`) plus a tenth in *positive* form
/// ([`ImageRef::crop_scale`]'s `span > 0.0`, which answers `Some(0.0)` for an
/// `inf` span where the honest answer is `None`) turned up only because the
/// eight were being changed and somebody read the neighbours. **Five, then
/// eight, then ten** — which is the argument for a named predicate over a
/// repeated comparison, and for not trusting a site count that came from one
/// grep shape. `ImageFit::Tile`'s arm already spelled this inline
/// (`is_finite() && > 0.0`) and is left as the one place that had it right.
fn usable_span(x: f64) -> bool {
    x > 0.0 && x.is_finite()
}

/// The crop after dragging the **picture** by `by`, in the frame's own units.
///
/// The picture follows the pointer, so the rectangle moves the other way: drag
/// right and you reveal what was off to the left. Both the source's intrinsic
/// size and the framing scale cancel out of that — the crop is normalized and
/// the frame is what it maps onto — so this needs neither.
///
/// Clamped by [`crop_clamped`], so a drag runs out at the edge of the picture
/// rather than pulling `Extend::Pad`'s smear into the frame.
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
/// **`k` scales the rectangle, so it is the reciprocal of the picture's zoom**: a
/// smaller crop shows less of the source and therefore a bigger picture. Named for
/// what it does to the number rather than for what the user sees, because the
/// caller is the one holding the scale the user typed.
///
/// **Uniform, and about the centre.** Scaling the normalized rectangle by one
/// factor scales it by that factor in source *pixels* too, so this can neither
/// introduce a stretch nor remove one the layer's own resize put there — which is
/// what lets [`ImageRef::crop_scale`] read a single axis and still round-trip. The
/// centre is the anchor because the window is not moving: what the eye is on is the
/// middle of the crop, and zooming about a corner would slide the subject out of it.
///
/// Clamped by [`crop_clamped`], so zooming out stops where the picture does rather
/// than pulling `Extend::Pad`'s smear into the frame — the same floor
/// [`ImageRef::cover_scale`] names. A factor that is not finite and positive gives
/// the rectangle back: a singular crop is worse than an ignored keystroke.
///
/// This is `crop_zoomed` under a new name and a fixed anchor. That function went
/// with the crop tool's corner-zoom in §15 D268, and the *gesture* has not come
/// back — what returned is the arithmetic, behind a typed number.
pub fn crop_scaled(crop: kurbo::Rect, k: f64) -> kurbo::Rect {
    if !k.is_finite() || k <= 0.0 {
        return crop;
    }
    // Smallest crop worth having: a hundredth of the source in the shorter
    // direction. Past that the picture is a few pixels stretched over the whole
    // frame, and there is nothing left to look at.
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
/// This is what makes the bounding box a crop rectangle (§15 D268). Under
/// [`ImageFit::Crop`] the crop maps onto the frame exactly, so the frame and the
/// crop are two views of one mapping — normalized source coordinate `n` sits at
/// local `frame.x0 + (n − crop.x0)/crop.width() · frame.width()`. Moving an edge
/// of the frame therefore has exactly one answer that does not move the picture:
/// re-read the *old* mapping at the *new* frame's corners.
///
/// A consequence worth naming, because it is the whole reason the gesture needs
/// no aspect handling of its own: the picture's scale in local units per source
/// pixel is unchanged on both axes, so a crop drag can neither stretch nor
/// squash. Only how much of the picture is showing changes.
///
/// **Not clamped, and deliberately.** A crop that left the source would be a
/// frame that had grown past the picture, and the caller is the one holding the
/// picture's rectangle to stop that with — [`ImageRef::picture_box`] is the
/// limit, and `tools::crop_resized` is what applies it. Clamping the rectangle
/// here instead would slide it out from under a frame this function cannot see,
/// which is the two-numbers-disagreeing bug the exactness above exists to rule
/// out.
///
/// A degenerate `from` has no mapping to re-read and gives `crop` back.
pub fn crop_reframed(crop: kurbo::Rect, from: kurbo::Rect, to: kurbo::Rect) -> kurbo::Rect {
    let (fw, fh) = (from.width(), from.height());
    if !usable_span(fw) || !usable_span(fh) {
        return crop;
    }
    let at_x = |x: f64| crop.x0 + (x - from.x0) * crop.width() / fw;
    let at_y = |y: f64| crop.y0 + (y - from.y0) * crop.height() / fh;
    kurbo::Rect::new(at_x(to.x0), at_y(to.y0), at_x(to.x1), at_y(to.y1))
}

/// `crop` brought back inside the source, keeping its size and aspect where it
/// can and its aspect where it cannot.
///
/// Two separate jobs, and the order matters. A rectangle **larger** than the
/// source in one direction is scaled down about its own centre — uniformly, so
/// the picture does not stretch — until it fits. Then whatever is left is
/// *slid* inside, which preserves the zoom the user just chose: clamping the
/// edges independently would silently change the shape of the crop and stretch
/// the picture as the drag ran into the edge.
pub fn crop_clamped(crop: kurbo::Rect) -> kurbo::Rect {
    let (w, h) = (crop.width(), crop.height());
    if !usable_span(w) || !usable_span(h) {
        return whole_crop();
    }
    // Shrink about the centre if it has outgrown the source in either direction.
    let over = (w.max(1.0) / 1.0).max(h.max(1.0) / 1.0);
    let (w, h, c) = if over > 1.0 {
        (w / over, h / over, crop.center())
    } else {
        (w, h, crop.center())
    };
    let mut r = kurbo::Rect::from_center_size(c, kurbo::Size::new(w, h));
    // ...then slide, rather than clamp each edge, so the size survives.
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
/// The three fields are the three things a backend needs and nothing else: what
/// the pixel grid maps to, what happens off the edge of it, and — for the one
/// mode that leaves part of the frame empty — what the ink has to be cut to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    /// Source-pixel space → the frame's own local space, which is the space the
    /// path is in. Both backends take this as a brush transform applied under
    /// the node's transform.
    pub transform: kurbo::Affine,
    /// What the backends sample beyond the source rectangle. Repeat for
    /// [`ImageFit::Tile`], pad for everything else.
    pub extend: peniko::Extend,
    /// The picture's own rectangle in local space, when it does not cover the
    /// frame and the ink has to be clipped to it.
    ///
    /// **Only [`ImageFit::Fit`] ever asks for one, and only when it actually
    /// letterboxes.** peniko has no transparent extend — `Extend` is pad,
    /// repeat or reflect — so the empty band beside a fitted picture would
    /// otherwise be a smear of its edge pixels, which reads as a rendering bug
    /// rather than as a fit. A clip costs a layer, so it is emitted only when
    /// the aspects genuinely differ.
    pub clip: Option<kurbo::Rect>,
}

impl Framing {
    /// The framing that changes nothing: one unit per source pixel at the
    /// origin, padded, unclipped. What a degenerate frame or a zero-sized source
    /// resolves to, since neither has a drawing to get right.
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
/// **33, and the number is part of the format rather than an implementation
/// detail**: it is the length of the `tableValues` list an SVG export writes, and
/// the pixel path evaluates the *same* list by the *same* linear interpolation
/// SVG defines for it — so the canvas and the export are one drawing by
/// construction rather than two that were tuned to look alike. 33 puts a sample
/// every 8 levels of an 8-bit channel, which is finer than the tone curves here
/// ever bend.
pub const TONE_STEPS: usize = 33;

/// The seven per-pixel adjustments an image fill carries, each `0.0` at rest and
/// running `-1.0..=1.0`.
///
/// **Per fill, not per image**, so two layers showing the same photograph can be
/// adjusted independently — which is the whole reason this lives on [`ImageRef`]
/// beside the framing rather than on [`ImageEntry`] beside the bytes. The bytes
/// are shared and deduplicated by content hash; what is done to them is not.
///
/// **Every field is a *symmetric* offset from a neutral zero**, not a multiplier
/// around one. That is what makes the default the identity, which is what makes
/// this an additive schema change (§5.11) — an image placed before adjustments
/// existed keeps its byte-for-byte line in the file — and it is also what makes
/// "reset" mean the same thing for all seven.
///
/// **These are not seven independent little features; they are one pipeline**
/// ([`ImageAdjust::pipeline`]), and the pipeline is the thing the three writers
/// share. Nothing outside this file should reimplement what a slider does.
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
    /// Magenta (`+`) to green (`-`) — the axis a white balance needs beside
    /// [`Self::temperature`], and the reason both exist rather than one.
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
    /// Nothing adjusted — the value a picture that has never been touched
    /// carries, and what *reset* puts back.
    pub const NEUTRAL: Self = Self {
        exposure: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        tint: 0.0,
        highlights: 0.0,
        shadows: 0.0,
    };

    /// Whether this is the value a missing key means, for `skip_serializing_if`
    /// on [`ImageRef::adjust`] (§5.11).
    ///
    /// **Compared exactly, and a non-finite value is not neutral.** A `NaN` in a
    /// hand-edited file has to reach [`Self::pipeline`], which refuses it there
    /// — treating it as neutral here would write a file that says one thing and
    /// draws another.
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
    /// **Two stages, and they are the two SVG filter primitives that can say
    /// this exactly**: a 4×5 colour matrix (`feColorMatrix type="matrix"`) and a
    /// per-channel transfer table (`feComponentTransfer type="table"`). That is
    /// not a coincidence dressed up as a design — it is the design. An adjusted
    /// picture has to be drawable by the GPU canvas, by the CPU export and by the
    /// SVG writer, and the SVG writer is the one of the three that cannot invent
    /// arithmetic: it can only name primitives a viewer already has. So the
    /// pipeline is built out of what SVG can name, and the two pixel backends
    /// evaluate the very same numbers.
    ///
    /// The stages are separately optional, so an exposure nudge does not carry a
    /// tone curve of 33 numbers it does not bend.
    ///
    /// [`None`] for the neutral value **and for any value outside `-1..=1`**,
    /// which includes the non-finite ones: a hand-written `NaN` would otherwise
    /// reach a buffer and paint a hole.
    ///
    /// ⚠️ **This asked only about finiteness until §15 D538, and the range was
    /// the part that mattered** — `[S9.1-L5-02]`. [`Adjustment::set`] clamps to
    /// `-1..=1` and its doc gives the reason verbatim: *"a range the model does
    /// not enforce is a range a hand-edited file ignores. The tone curve's
    /// monotonicity argument only holds inside this range."* This is the door a
    /// hand-edited file uses, and it was not asking. Two measured pictures, both
    /// from a `.ondin` that loads cleanly and survives a round trip:
    ///
    /// - `"exposure": 1000.0` is a finite `f32`, so it passed. [`Self::matrix`]
    ///   computes `2f32.powf(1000.0)` = `inf`, and `inf * 0.0` in the compose
    ///   makes **all twenty coefficients `NaN`** — every pixel of the photograph
    ///   renders `[0, 0, 0, 255]`, a solid black rectangle, on the canvas and in
    ///   the PNG. The threshold is exactly `|exposure| >= 128`, where `f32`
    ///   overflows at `2^128`; at 127 the matrix is still finite and still wrong.
    /// - `"shadows": 2.0` makes [`Self::curve`] **double back at 12 of its 32
    ///   steps**: source black comes out `[204,204,204]` and source mid-grey
    ///   `[179,179,179]`, so the dark end of the picture is inverted. The
    ///   threshold is the file's own algebra — the derivative of `v + 0.4s(1−v)²`
    ///   at `v = 0` is `1 − 0.8s`, so anything above `1.25` is non-monotonic.
    ///
    /// ⚠️ **And it is a backend divergence, which is what makes it worse than a
    /// wrong picture.** Both pixel backends quantise `NaN` to zero and agree on
    /// black; the SVG writer passes every coefficient through `fmt` and emits
    /// `<feColorMatrix values="NaN NaN …">`, which a viewer rejects outright. One
    /// number, black in two writers and invalid markup in the third.
    ///
    /// **Refused rather than clamped**, and the choice is not stylistic:
    /// [`Self::is_default`] already reports such a value as non-neutral, so
    /// clamping here would make the file *draw* something it does not *say* —
    /// the panel would read 1000 and the canvas would show the picture at 1.
    /// Refusing draws the unadjusted picture, which is the one honest answer
    /// available to a reader that has been handed a number the model forbids.
    ///
    /// ⚠️ **Flipped twice**, both against `an_impossible_value_is_refused_at_both_doors`.
    /// Reverting the predicate to bare `is_finite()` fails at the `exposure: 1000`
    /// arm — the predicted site. And the *plausible wrong fix*, clamping each
    /// value through [`Adjustment::set`] before building instead of refusing,
    /// **fails at the same assertion**, which is the point of writing it on
    /// `pipeline().is_none()` rather than on a pixel: a clamping version draws a
    /// perfectly good picture that the file does not describe, and only an
    /// assertion about the *refusal* can tell the two apart.
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
    /// **Composed in the order a photographer works in**: light first (exposure
    /// and the white balance, which are one diagonal between them), then
    /// colourfulness, then the tone spread. Each is a 4×5 matrix, so the whole of
    /// it collapses to one — which is the only reason five sliders cost one
    /// filter primitive.
    fn matrix(&self) -> Option<[f32; 20]> {
        let (e, c, s) = (self.exposure, self.contrast, self.saturation);
        let (t, n) = (self.temperature, self.tint);
        if e == 0.0 && c == 0.0 && s == 0.0 && t == 0.0 && n == 0.0 {
            return None;
        }
        // Exposure is stops, so it is a power of two; the two white-balance axes
        // are gentle channel gains on top of it. **±0.3 on the axis's own channel
        // and half that, the other way, on the pair beside it** — a full-tilt
        // temperature is a visible warm cast rather than a colour separation, and
        // tint's compensation on red and blue is what keeps the green axis from
        // reading as a brightness slider.
        let light = 2f32.powf(e);
        let gain = diagonal(
            light * (1.0 + 0.3 * t) * (1.0 + 0.15 * n),
            light * (1.0 - 0.3 * n),
            light * (1.0 - 0.3 * t) * (1.0 + 0.15 * n),
        );
        // **SVG's own luminance coefficients**, not Rec.601's, because
        // `feColorMatrix type="saturate"` is written with these — so the numbers
        // this emits are the numbers a reader of the markup would expect, and a
        // neutral-saturation export is recognisably the identity.
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
        // Contrast pivots on mid-grey, which is a scale plus the offset that
        // holds 0.5 still — the fifth column is exactly what that offset is for.
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
    /// (the derivative of `v + a(1−v)²` at `v = 0` is `1 − 2a`). A curve that
    /// doubled back would make a slider *invert* part of the picture at its
    /// extreme, which reads as a rendering bug rather than as a strong setting.
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
/// three writers can say.
///
/// **Here for the reason [`ImageRef::framing`] and [`missing_placeholder`] are
/// here.** Three copies of a per-pixel function are three pictures, and the two
/// that are pixels would agree with each other while the one that is markup
/// quietly did not — which is exactly how the frame background's missing
/// placeholder got lost (§15 D179).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjustPipeline {
    /// A 4×5 colour matrix over straight-alpha sRGB, row-major — the twenty
    /// numbers `feColorMatrix type="matrix"` takes, in its order.
    pub matrix: Option<[f32; 20]>,
    /// A transfer table applied to each of R, G and B **after** the matrix — the
    /// `tableValues` of an `feComponentTransfer type="table"`, sampled at
    /// [`TONE_STEPS`] points across `0..=1`.
    pub curve: Option<[f32; TONE_STEPS]>,
}

impl AdjustPipeline {
    /// One pixel through the pipeline: straight-alpha sRGB in `0..=1` in, the
    /// same out.
    ///
    /// **The order is the interesting part; the clamp is stated twice on
    /// purpose.** SVG runs the primitives in the order the `<filter>` lists them
    /// and cuts each one's result to `0..=1` before the next reads it. That bound
    /// is written here *and* again inside [`transfer`], which is belt and braces
    /// — remove either alone and nothing changes — but not decoration: remove
    /// **both** and an overshooting matrix reaches the table with an index the
    /// `.min` bounds and a fraction it does not, so the lerp extrapolates off the
    /// end of the curve. `the_matrix_runs_before_the_tone_curve` is the test that
    /// catches that, because it probes a pixel the exposure pushes past white.
    ///
    /// What the *order* buys is that such a pixel is a blown highlight the curve
    /// can recover: swapped round, the same two settings give a picture that is
    /// merely blown.
    ///
    /// Alpha is not touched by either stage, which is why it is not a parameter:
    /// the matrix's alpha row is the identity and no `feFuncA` is emitted.
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
/// `0..=1`, read by linear interpolation between the two straddling it.
///
/// Spelled out rather than approximated because it is the seam where the canvas
/// and the export have to agree — see [`TONE_STEPS`].
pub fn transfer(table: &[f32], v: f32) -> f32 {
    if table.is_empty() {
        return v;
    }
    if table.len() == 1 {
        return table[0];
    }
    let v = v.clamp(0.0, 1.0);
    let n = table.len() - 1;
    // The spec's own indexing: `k = floor(v·n)`, held at `n−1` so that `v = 1`
    // interpolates from the last pair rather than off the end of the list.
    let k = ((v * n as f32) as usize).min(n - 1);
    let t = v * n as f32 - k as f32;
    table[k] + t * (table[k + 1] - table[k])
}

/// One of the seven, named — so the panel draws them from this list rather than
/// from seven hand-written rows, and the order and the words live in one place.
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
    /// The seven, in the order the panel lists them: light, then colour, then the
    /// two that shape one end of the range each.
    pub const ALL: [Adjustment; 7] = [
        Self::Exposure,
        Self::Contrast,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
        Self::Highlights,
        Self::Shadows,
    ];

    /// The word the control carries. **A word rather than a glyph**, which is the
    /// rule the popover's mirror pair arrived at the hard way: a control that
    /// cannot be told from its neighbour by its drawing has to be told by its
    /// label, and no icon set has seven distinguishable tone-adjustment glyphs.
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
    /// **Clamped here rather than at the control**, because there are two writers
    /// — a slider and a *reset* — and a range the model does not enforce is a
    /// range a hand-edited file ignores. The tone curve's monotonicity argument
    /// only holds inside this range.
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
/// **A struct rather than a bare [`ImageId`]**, which is what let the framing
/// arrive as an additive schema change (§5.11) rather than a migration.
/// Everything about *sampling* is carried by peniko's own `ImageSampler` beside
/// this — extend in both axes, quality, and an alpha multiplier — so none of it
/// is restated here, and the one field of it the fit has an opinion about
/// (extend) is answered by [`ImageRef::framing`] at the render boundary rather
/// than written back into the model, so the two can never disagree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: ImageId,
    #[serde(default, skip_serializing_if = "ImageFit::is_default")]
    pub fit: ImageFit,
    /// The rectangle of the source [`ImageFit::Crop`] shows, **normalized** to
    /// the picture *as [`Self::orient`] leaves it*: `(0,0)..(1,1)` is the whole
    /// of it, and `(0,0)` is whichever source corner the flips and turns have
    /// put at the frame's top-left.
    ///
    /// Normalized to the **oriented** picture rather than to the raw source
    /// because that is the space the crop gesture drags in — its axes are the
    /// frame's — so `crop_panned` and `crop_reframed` need to know nothing about
    /// orientation at all. The price is that changing the orientation has to
    /// carry the rectangle with it, which [`OrientOp::crop_carried`] does and
    /// [`ImageRef::reoriented`] is the only way to reach.
    ///
    /// Normalized rather than in pixels so that *replace image* keeps the
    /// framing when the new picture has different dimensions, which §5.5a
    /// requires by name and which is the whole of what makes the popover's
    /// *Replace…* a repair rather than a fresh placement (§15 D179); in pixels it
    /// would either be silently wrong or would have to be rescaled on every
    /// replace.
    ///
    /// The rectangle maps onto the frame **exactly**, so aspect is the frame's
    /// business: cropping to a square and then stretching the shape stretches
    /// the picture. That is what having chosen a region means, and it keeps the
    /// crop gesture and the resize gesture from arguing about the same number.
    ///
    /// **Kept while the mode is not `Crop`**, which is the whole reason it lives
    /// here rather than inside the variant — see [`ImageFit`]. Coordinates
    /// outside `0..1` are not rejected: core stores what it is given, and they
    /// sample the edge pixel, since `Extend::Pad` is the only clamp peniko has.
    #[serde(default = "whole_crop", skip_serializing_if = "is_whole_crop")]
    pub crop: kurbo::Rect,
    /// What [`ImageFit::Tile`] repeats at, as a multiple of the source's
    /// intrinsic pixel size: `1.0` is one document unit per source pixel.
    ///
    /// Kept while the mode is not `Tile`, as [`Self::crop`] is. A value that is
    /// not finite and positive draws at `1.0` rather than being refused, because
    /// a singular brush transform is a backend question and this is a document.
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    pub tile_scale: f64,
    /// The flips and quarter turns applied to the picture **before** any of the
    /// four modes frames it.
    ///
    /// Read by all four, unlike [`Self::crop`] and [`Self::tile_scale`]: turning
    /// a picture turns it whether it is filling, fitting, cropped or tiled, and a
    /// mode that ignored it would be the one mode where the flip button does
    /// nothing.
    #[serde(default, skip_serializing_if = "ImageOrient::is_default")]
    pub orient: ImageOrient,
    /// The per-pixel adjustments applied to the picture before any of this is
    /// framed.
    ///
    /// **The one field here that is not geometry**, and the difference is what it
    /// costs: the other four are answered by a brush transform over the decoded
    /// buffer the whole document shares, where this one produces a *second
    /// buffer* — which is why `ondin_render::ImageStore` keys derived pixels by
    /// `(id, adjust)` and why [`ImageAdjust::pipeline`] is shaped the way it is.
    ///
    /// Read by all four framing modes, as [`Self::orient`] is: what is done to the
    /// picture's colours has nothing to do with how it sits in the frame.
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

    /// This reference flipped or turned inside its frame, **with the crop
    /// carried along**.
    ///
    /// One function rather than two writes at the call site, because the two have
    /// to happen together or the picture jumps to a different part of the source
    /// — see [`OrientOp::crop_carried`]. Making it the only way to reach the
    /// orientation is what stops that being got wrong at a call site, the way
    /// [`Self::framing`] is the only way to reach a brush transform.
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
    /// **What entering Crop seeds**, so that choosing it changes nothing on
    /// screen and the gesture starts from the picture the user is already looking
    /// at. Without it, Crop begins at the whole source stretched onto the frame —
    /// which is a different picture, arrived at by picking the mode whose entire
    /// job is not to change one.
    ///
    /// It is the widest crop the gesture can zoom out to — a rectangle any larger
    /// would have to leave the source, where `Extend::Pad` smears the edge row —
    /// and therefore what *reset crop* comes back to, which it does by restoring
    /// [`whole_crop`] and letting the next entry seed this again.
    ///
    /// Degenerate input gives [`whole_crop`] — there is no cover to compute, and
    /// nothing draws anyway.
    ///
    /// **A method rather than an associated function, because the answer depends
    /// on [`Self::orient`]**: cover is computed against the *turned* aspect, and
    /// the rectangle it returns is normalized to the turned picture, which is the
    /// space [`Self::crop`] is in. Taking the intrinsic size as an argument
    /// anyway, because the caller is the one holding the table entry.
    pub fn cover_crop(&self, frame: kurbo::Rect, width: u32, height: u32) -> kurbo::Rect {
        let (pw, ph) = (width as f64, height as f64);
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return whole_crop();
        }
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        // The scale cover uses, then the source rectangle that much of the frame
        // is worth — centred, as the cover framing centres what it overflows.
        // Orientation is a symmetry of the box about its own centre, so a centred
        // rectangle stays centred and nothing here has to un-turn anything.
        let s = (fw / iw).max(fh / ih);
        let (vw, vh) = (fw / s / iw, fh / s / ih);
        kurbo::Rect::new(
            (1.0 - vw) / 2.0,
            (1.0 - vh) / 2.0,
            (1.0 + vw) / 2.0,
            (1.0 + vh) / 2.0,
        )
    }

    /// How big the picture is on the page under [`ImageFit::Crop`], in local units
    /// per **oriented** source pixel — `1.0` being one unit per pixel, exactly as
    /// [`Self::tile_scale`] means it for the mode beside it.
    ///
    /// **The one thing the crop *gesture* cannot change**, which is what makes this
    /// a control rather than a duplicate of one: a handle drag moves an edge of the
    /// window with the picture nailed down, and a pan slides the picture under a
    /// window that is nailed down. Neither alters this number. So the window and
    /// the zoom are two separate questions with one answer each — where the old
    /// crop tool put both on the same drag, which is half of why "I chose Crop and
    /// it resized" was the report it was (§15 D268, D271).
    ///
    /// **Measured on x.** The two axes agree wherever the crop's aspect matches the
    /// frame's, which is every rectangle this app's own gestures produce — they
    /// seed from [`Self::cover_crop`] and scale uniformly from there. They can
    /// differ after the *layer* is resized non-uniformly outside image editing,
    /// which stretches the picture on purpose (the rectangle maps onto the frame
    /// exactly); there this reports the horizontal scale and the picture is taller
    /// or shorter than it says. Reading one axis is also what makes the field
    /// round-trip: [`crop_scaled`] multiplies the rectangle uniformly, so reading x
    /// and writing a uniform factor cannot drift.
    pub fn crop_scale(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<f64> {
        let (iw, _) = self
            .orient
            .oriented_size(f64::from(width), f64::from(height));
        let span = self.crop.width() * iw;
        // Positive-form, and it wants the same predicate for the same reason: an
        // `inf` span passes `> 0.0` and answers `Some(0.0)` where the honest
        // answer is `None`.
        (usable_span(span) && usable_span(frame.width())).then(|| frame.width() / span)
    }

    /// The scale `Fill` shows the picture at — and therefore the **floor** for
    /// [`Self::crop_scale`].
    ///
    /// Below it the crop rectangle would have to leave the source on one axis or
    /// both, where `Extend::Pad` smears the edge row into the frame. So a crop can
    /// only ever zoom *in* from cover, and that is a fact about the geometry rather
    /// than a limit anybody chose: the rectangle maps onto the frame exactly, so
    /// there is no letterboxing for a smaller picture to sit in. Figma's crop has
    /// the same floor for the same reason.
    ///
    /// Note that this is not the same as "100% and up": on a layer much smaller
    /// than its picture, cover is well under one unit per pixel and the field runs
    /// from there.
    pub fn cover_scale(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<f64> {
        let (pw, ph) = (f64::from(width), f64::from(height));
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return None;
        }
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        // The very expression `cover_crop` scales by, which is the point: the
        // rectangle it returns is this scale's, so a field clamped here and a
        // gesture seeded there cannot disagree about where the bottom is.
        Some((fw / iw).max(fh / ih))
    }

    /// How this reference sits in `frame`, given the source's intrinsic size.
    ///
    /// `frame` is the shape's bounding box in its own local space — the space
    /// the path is in, and therefore the space the brush transform is in
    /// (§6.2). The result is the same for both backends and for the SVG writer,
    /// which is why the arithmetic lives here rather than in either of them.
    ///
    /// A zero-sized source or an empty frame resolves to
    /// [`Framing::identity`]: neither has a drawing to get right, and a
    /// singular brush transform is worse than a wrong one.
    pub fn framing(&self, frame: kurbo::Rect, width: u32, height: u32) -> Framing {
        use kurbo::Affine;
        let (pw, ph) = (width as f64, height as f64);
        let (fw, fh) = (frame.width(), frame.height());
        if !usable_span(pw) || !usable_span(ph) || !usable_span(fw) || !usable_span(fh) {
            return Framing::identity();
        }
        // **Everything below frames the *oriented* picture.** The mirror and the
        // quarter turns happen in source space, before any of the four modes sees
        // it, so Fill covers with the turned aspect and the crop rectangle names a
        // region of what is actually on screen. Composed on the right at the end,
        // which leaves the four arms the arithmetic they always were.
        let orient = self.orient.affine(pw, ph);
        let (iw, ih) = self.orient.oriented_size(pw, ph);
        let base = match self.fit {
            ImageFit::Fill | ImageFit::Fit => {
                let (sx, sy) = (fw / iw, fh / ih);
                let cover = matches!(self.fit, ImageFit::Fill);
                let s = if cover { sx.max(sy) } else { sx.min(sy) };
                let (w, h) = (iw * s, ih * s);
                // Centred, which is the only anchor that does not need a second
                // control to explain it.
                let x = frame.x0 + (fw - w) / 2.0;
                let y = frame.y0 + (fh - h) / 2.0;
                Framing {
                    transform: Affine::translate((x, y)) * Affine::scale(s),
                    extend: peniko::Extend::Pad,
                    // Cover leaves nothing showing; contain does, but only where
                    // the aspects differ — a tolerance rather than an exact
                    // comparison, so a picture placed at its own aspect does not
                    // pay for a layer because of a rounding bit.
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
        // ⚠️ **A usable *span* is not a usable rectangle**, and the Crop arm is
        // where the two come apart: it also weighs the crop's **origin**, through
        // `Affine::translate((-r.x0 * iw, -r.y0 * ih))`, and nothing above tests
        // that. `{"x0": -1e308, "x1": -1e308 + 1e300}` spans `1e300` — finite,
        // positive, and `usable_span` says yes to `1e300 * iw` — while
        // `1e308 * iw` overflows for any picture wider than one pixel. Measured on
        // a 4000×3000 source: the composed transform comes back
        // `[2.5e-302, 0, 0, 3.33e-302, NaN, NaN]`, so [`Self::picture_box`] answers
        // `Some(NaN…)` and `tools::crop_resized` panics in `f64::clamp` — which is
        // `[S9.1-L5-01]`'s third measured consequence reached by a crop D452's own
        // guards call usable (§15 D452, amended).
        //
        // **Tested on the result rather than on the operands** because the operands
        // are not the only road to it: `fw / sw` and `-r.x0 * iw` can each be
        // finite and their product infinite, and a fourth road would need a fourth
        // guard. This one is closed by construction — it asks the question
        // `Framing::identity` exists to answer, of the value that is actually
        // returned.
        if framed.transform.as_coeffs().iter().all(|c| c.is_finite()) {
            framed
        } else {
            Framing::identity()
        }
    }

    /// Where the **whole** picture lands in the frame's own local space — the
    /// rectangle the frame is a window onto.
    ///
    /// **Read off [`Self::framing`] rather than computed beside it**, which is the
    /// only thing keeping it honest: the transform this maps the source rectangle
    /// through is the very one the two backends and the SVG writer paint with, so
    /// the outline drawn round a picture cannot land anywhere but where the
    /// picture is. A second copy of the four modes' arithmetic would be a fifth
    /// opinion about the same drawing.
    ///
    /// Defined in all four modes, and it says something different in each: under
    /// `Fill` it is the rectangle that overflows the frame, under `Fit` the one
    /// that falls short of it (`Framing::clip`'s rectangle, when there is one),
    /// under `Crop` the source the crop is a region of, and under `Tile` the first
    /// repeat. **Only `Crop` draws it** (§15 D268) — under the other three the
    /// frame is not a window the user is moving, so an outline round the overflow
    /// would be marking something no gesture acts on.
    ///
    /// `None` for a degenerate frame or a zero-sized source, where
    /// [`Framing::identity`] is standing in and there is no picture to bound.
    pub fn picture_box(&self, frame: kurbo::Rect, width: u32, height: u32) -> Option<kurbo::Rect> {
        if width == 0 || height == 0 || !usable_span(frame.width()) || !usable_span(frame.height())
        {
            return None;
        }
        let f = self.framing(frame, width, height);
        let source = kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        // A bounding box rather than the mapped rectangle, because the transform
        // carries `orient`: a quarter turn maps the source box to itself only up
        // to a swap of its corners, and `Rect` has no way to hold the swapped one.
        // The box is the same either way, which is all this is asked for.
        Some(f.transform.transform_rect_bbox(source))
    }
}

/// The drawing that stands in for a picture that cannot be drawn (§15 D179).
///
/// **Here rather than in either writer, for the reason [`ImageRef::framing`] is
/// here**: the answer has to be the same on the GPU canvas, in the CPU export and
/// in the SVG writer, and three copies of a drawing are three drawings. The
/// caller supplies the shape and does the painting; this supplies what to paint.
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
/// **Grey and a cross, which is the one idiom everybody already reads.** No glyph
/// and no words: a placeholder that needed a font could not be drawn by a walk
/// that has none, and the export path has to draw the same thing as the canvas.
/// Neutral rather than an alarm colour, because the usual cause is a file that
/// moved rather than a mistake in the document.
///
/// **The cross is the *box's* diagonals and the rim is the caller's own path**,
/// so an image fill in a circle keeps the circle: the caller clips the cross to
/// the shape. A rectangle drawn over a round shape would read as a layer of its
/// own rather than as that shape having lost its picture.
///
/// The width is a fortieth of the shorter side, held between 0.5 and 4 units. A
/// fixed width is a hairline on a photograph's frame and a slab on a 20-unit
/// icon; this is ink, so it scales with the zoom either way.
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
        // Light, not a mid grey: a placeholder should read as an *absence*, and a
        // solid slab reads as artwork somebody chose. The cross is what carries it
        // against a white page, which is why the ground can afford to be quiet.
        ground: peniko::Color::from_rgb8(0xE4, 0xE4, 0xE6),
        ink: peniko::Color::from_rgb8(0xA0, 0xA0, 0xA8),
        width: (bounds.width().min(bounds.height()) / 40.0).clamp(0.5, 4.0),
        cross,
    })
}

/// The model's brush: peniko's, over an image *reference* instead of pixels.
///
/// **Not a brush enum of our own, which is what the image plan expected to
/// need** (§15 D176, which is the whole record of that reversal).
/// Its reasoning was that "an image reference cannot live in `peniko::Brush`,
/// which holds pixels" — true of the shape peniko had when that was written, and
/// not of 0.6: `Brush<I, G>` and `ImageBrush<D>` are both generic over their
/// storage, and peniko's own documentation gives the reason as *"different
/// renderers can use different types here, such as a pre-registered id"*. So the
/// model substitutes [`ImageRef`] for the pixels and keeps everything else.
///
/// Three consequences, all of them the point:
///
/// - **The save format does not move.** `Solid` and `Gradient` are the same
///   variants of the same enum carrying the same types, so every document
///   written before images existed loads byte-for-byte unchanged, with no schema
///   bump and no migration. A brush enum of our own would have re-spelled two
///   variants that were already right in order to add a third.
///   ⚠️ **The `Gradient` half of that stopped being literally true on 2026-09-03
///   and the property it was claiming still holds** (§15 D412): the payload is
///   [`GradientBrush`] now rather than `peniko::Gradient`, and it stays
///   wire-compatible by `#[serde(flatten)]` rather than by being the same type.
///   *The second generic parameter is what made that a one-line change* — the
///   same door `ImageRef` came through, used a second time and for a reason
///   nobody had in mind when it was chosen.
/// - **`alpha` and `extend` are not restated.** `ImageBrush` already carries an
///   `ImageSampler`, which is the plan's own trap ("do not add parallel fields
///   for them") answered by construction rather than by remembering (§15 D176).
/// - **The render boundary is the only thing that changes**, which is where the
///   seam was already waiting: `ondin_render::color::brush_to_backend` turns this
///   into the backends' `peniko::Brush` by resolving the reference through the
///   decode cache.
pub type Brush = peniko::Brush<peniko::ImageBrush<ImageRef>, GradientBrush>;

/// The image half of [`Brush`], for the call sites that build one.
pub type ImageBrush = peniko::ImageBrush<ImageRef>;

/// The gradient half of [`Brush`]: peniko's gradient, plus the affine that maps
/// the space it was authored in onto the shape's own.
///
/// **Why the model needs a transform peniko does not have.** A
/// `peniko::RadialGradientPosition` is two *circles*, so nothing in it can say
/// "an ellipse" — and an elliptical radial gradient is the ordinary case in real
/// files, since `gradientUnits="objectBoundingBox"` on any non-square shape
/// produces one. ⚠️ **The reading that this is a peniko limitation is wrong and
/// was carried in `roadmap.md` for days** (§15 D412): an ellipse *is* a circle
/// under a non-uniform transform, and **both backends already had the
/// mechanism** — vello's `Scene::fill` takes a `brush_transform` and
/// `vello_cpu`'s `RenderContext` has `set_paint_transform`, both of which the
/// image path had been using for framing all along while the gradient path
/// explicitly reset them. What was missing was a place in the *model* to put the
/// affine, which is this field.
///
/// **On the brush rather than on [`crate::node::Fill`]**, which is what makes it
/// one change instead of two: a `Stroke` carries a `Brush` as well, and a
/// gradient-stroked shape wants the same treatment for the same reason. A field
/// on `Fill` would have needed a twin on `Stroke` and a rule about which of the
/// two a shared helper meant.
///
/// ⚠️ **Wire-compatible by `#[serde(flatten)]`, which is load-bearing rather than
/// tidy.** A gradient written before this field existed is a JSON object of
/// peniko's own keys; flattening puts `transform` beside them instead of nesting
/// the gradient one level down, so those files load unchanged and a new file
/// gains a key only when the transform is not the identity. Nesting would have
/// been a schema break for every gradient ever saved, to add a field almost none
/// of them use. `a_gradient_written_before_the_transform_existed_still_reads` is
/// what says so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientBrush {
    /// The ramp, its kind and its geometry, in the space `transform` maps from.
    #[serde(flatten)]
    pub gradient: peniko::Gradient,
    /// Gradient space → the shape's local space. The identity for everything the
    /// app itself authors, because the panel builds a gradient's geometry from
    /// the shape's own box; non-identity comes from an import
    /// (`svg_in`'s `gradientTransform` and `objectBoundingBox` units) and rides
    /// along untouched while stops are edited.
    ///
    /// ⚠️ **A *similarity* is not stored here**, and that is why this field is
    /// rarer than it looks: a rotation or a uniform scale can be baked into the
    /// two circles exactly, and `svg_in` still does that. Only the part peniko
    /// genuinely cannot express — a squash — has to survive as a transform.
    #[serde(default, skip_serializing_if = "is_identity")]
    pub transform: kurbo::Affine,
    /// The whole ramp's opacity, `0.0..=1.0`, multiplied over every stop's own
    /// alpha at paint time (§15 D767).
    ///
    /// 🚨 **A field rather than a rewrite of the stops, because the rewrite was
    /// lossy and the loss was silent.** `panels::paint::with_alpha` used to scale
    /// every stop by `alpha / peak`; taking a ramp to 0% made `peak` zero, and the
    /// next call could only fall back to writing one flat alpha across the whole
    /// ramp. `[1.0, 0.0]` — the shape **every** gradient this app authors starts
    /// as, a colour fading to transparent — came back `[1.0, 1.0]`, flat and
    /// opaque, with the fade gone and no undo step to blame. Scaling is not
    /// invertible through zero, and the maintainer's ruling that *"0 alpha means
    /// fully invisible"* rules out the floors that would have kept it away from
    /// zero.
    ///
    /// **This is the shape the image brush has had all along**, which is the
    /// argument for it: `peniko::ImageSampler::alpha` is a multiplier beside the
    /// pixels, so `with_alpha` on a picture assigns a number and touches nothing —
    /// already lossless, already invertible. The gradient arm was the only one of
    /// the three that destroyed its subject, and the asymmetry *was* the defect.
    ///
    /// ⚠️ **Wire-compatible by the same mechanism `transform` uses**: `default` plus
    /// `skip_serializing_if`, so every gradient ever saved loads unchanged and
    /// re-saves byte-identical (invariant 9). No `CURRENT_SCHEMA_VERSION` bump and
    /// no migration.
    ///
    /// ⚠️ **The repair is forward-only and the record should not imply otherwise.**
    /// A ramp already flattened to `[1.0, 1.0]` in a saved document stays flat: the
    /// ratio information is gone from the file and nothing can invent it.
    ///
    /// ⚠️ **Clamped by its reader, not trusted from the file.** A `serde(default)`
    /// float the loader does not validate is precisely §15 D713's shape — any six
    /// characters in a `.ondin` become the field — so `build::brush_is_finite`
    /// carries its range and `paint::alpha_of` clamps.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
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
/// **This is SVG's rule, verbatim** — *"each gradient offset value is required to
/// be equal to or greater than the previous"*, and a smaller one is corrected up
/// — and it is the rule because a `.ondin` gradient's ramp is most often an SVG
/// gradient's ramp. It is **not a sort**, and the difference is the whole point:
/// `[0, 0.8, 0.2, 1]` clamps to `[0, 0.8, 0.8, 1]` (the third stop collapses onto
/// the second) and sorts to `[0, 0.2, 0.8, 1]` (the third stop moves), which are
/// two different pictures.
///
/// ⚠️ **A descending ramp is not a wrong picture, it is *no* picture.**
/// `[S11.1-L1-02]`, measured in release through `VelloCpuRenderer`: a 100×20 rect
/// with a horizontal linear gradient came back **the first stop's colour in all
/// 100 columns** — the ramp collapses to a flat fill — and a descent of one part
/// in ten million does it. Equal offsets, which is exactly what this produces,
/// are fine.
///
/// ⚠️ **A sort is still the right rule where the *user* is the author**, which is
/// why `panels::paint::with_stops` keeps its `sort_by` and this does not replace
/// it. Dragging a stop handle past its neighbour means *"put it there"*; a file
/// that says `0.2` after `0.8` is a file to be read, not an intention to be
/// guessed at. Before this existed the two rules and the raw list gave one
/// document **three** renderings depending on the route taken to it — import,
/// render, or touch a colour and render again.
pub fn make_stops_monotonic(stops: &mut [peniko::ColorStop]) {
    // Starting at 0 rather than at the first stop's own offset, because a ramp is
    // defined on `0..=1` and both authoring doors already say so:
    // `svg_in::stop_of` clamps to `0.0..1.0` and `panels::paint::with_stops`
    // clamps each stop it writes. A negative offset therefore reaches this only
    // from a hand-edited file, where "at the start" is the honest reading.
    let mut hi = 0.0f32;
    for s in stops {
        s.offset = monotonic_offset(&mut hi, s.offset);
    }
}

/// [`make_stops_monotonic`]'s rule as one step, for a caller whose stops are not
/// [`peniko::ColorStop`]s (§15 D564).
///
/// **It exists because there is a fourth door and it was about to hold a fourth
/// copy of the rule.** D455 fixed the importer, the renderer and the SVG writer,
/// which share this function; the app's
/// gradient *chips* — `ui::paint_ramp`, which the paint row, the picker's preview
/// chip, its stop bar and the typography swatch all funnel through — read
/// `Fill::brush`'s stop list verbatim and painted a descending ramp **mirrored**,
/// while the canvas beside them painted SVG's clamped reading. Spelling the
/// running maximum a second time there would have been G8's own shape, so it is
/// spelled here instead.
///
/// `hi` is the running maximum and is updated; the return is the offset to use.
/// `NaN` is `max`ed away rather than propagated — `f32::max` answers the non-NaN
/// operand, so a NaN offset takes the running maximum and the ramp stays
/// readable. It should never arrive (`build::brush_is_finite` refuses one at the
/// op boundary, §15 D451) and both callers are drawing, which is not the place to
/// find out.
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
            // Opaque: the stops carry whatever alpha the author gave them, and
            // this multiplier starts out saying nothing (§15 D767).
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

    /// **A brush that was written before images existed still reads.**
    ///
    /// The whole argument for reusing peniko's generic enum rather than
    /// declaring one of our own is that the two solid/gradient variants keep
    /// their serde shape, so no saved document has to be migrated. That is a
    /// claim about *bytes*, so it is asserted against bytes: the JSON here was
    /// produced by the old `peniko::Brush` (pixels in the image slot) and is
    /// parsed by the new one (a reference in it).
    #[test]
    fn a_solid_brush_written_before_images_existed_still_parses() {
        let json = r#"{"Solid":{"components":[0.2,0.4,0.6,1.0]}}"#;
        let brush: Brush = serde_json::from_str(json).expect("the old shape still parses");
        let peniko::Brush::Solid(c) = brush else {
            panic!("expected a solid, got {brush:?}");
        };
        assert_eq!(c.components, [0.2, 0.4, 0.6, 1.0]);
        // And back out to the same bytes, which is what invariant 9 turns on.
        let out = serde_json::to_string(&Brush::Solid(c)).expect("serializes");
        assert_eq!(out, json);
    }

    /// **A gradient written before the transform existed still reads, and one
    /// with the identity still writes the same bytes** (§15 D412).
    ///
    /// This is the claim `#[serde(flatten)]` is there to make, and it is a claim
    /// about *bytes*, so it is asserted against bytes. Nesting the gradient under
    /// a key instead — the obvious spelling — would have been a schema break for
    /// every gradient ever saved, in order to add a field almost none of them
    /// use.
    ///
    /// ⚠️ **The second half is the one that would rot quietly.** A `transform`
    /// key appearing on every gradient would still *load* everywhere, so nothing
    /// would fail; what it would cost is invariant 9 — every document containing
    /// a gradient would rewrite on the next save, and the git diff this format
    /// exists to keep readable would be noise. `skip_serializing_if` is what
    /// stops that, and only this assertion says so.
    ///
    /// ⚠️ **The old bytes are produced by the old *type*, not typed out here.**
    /// The payload before this change was a bare `peniko::Gradient`, so
    /// serializing one is exactly what a pre-2026-09-03 file contains — and it
    /// cannot drift when peniko adds a field, where a hand-written literal would
    /// silently start testing a shape nothing ever wrote. (The first draft of
    /// this test was such a literal and was wrong on its first run: it guessed
    /// `"interpolation_alpha_space":"Separate"`, a variant that does not exist.)
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
        // **And the same for `opacity`, which arrived the same way** (§15 D767).
        // Asserted rather than left implicit: the byte-for-byte check below would
        // catch a missing `skip_serializing_if`, but *this* is the assertion that
        // names what an absent key means, and a default of `0.0` would make every
        // pre-D767 gradient load invisible.
        assert_eq!(
            g.opacity, 1.0,
            "an absent opacity is fully opaque, not fully transparent"
        );
        // And back out to the same bytes — no `transform` key, because it is the
        // identity, and no `opacity` key, because it is 1. This is the half that
        // keeps invariant 9, and it is why D767 needed no schema bump and no
        // migration: every gradient ever saved re-saves byte-identical.
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

    /// **An image brush is a key, and the sampler is peniko's.**
    ///
    /// Asserting on the JSON rather than on a round trip, because what this is
    /// about is that no *pixels* can reach the file: a table of encoded bytes
    /// written once is the whole point of the reference, and a brush that
    /// serialized a decoded buffer would defeat it invisibly.
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

    /// **A chosen fit is written; the default one is not.**
    ///
    /// Both halves matter and they are different claims. The default staying
    /// out is what made framing an additive change (§5.11) — every image placed
    /// before it existed keeps its exact line in the file, which is what the
    /// test above asserts against literal JSON. The chosen one going *in* is the
    /// half a `skip_serializing_if` gets wrong by being pointed at the wrong
    /// predicate, and it fails silently: the document would load with the crop
    /// thrown away and look like the crop gesture had not committed.
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

    /// The source rectangle's two opposite corners, in the frame's local space —
    /// which is the whole of what a framing transform says.
    fn corners(f: &Framing, w: u32, h: u32) -> (kurbo::Point, kurbo::Point) {
        (
            f.transform * kurbo::Point::ZERO,
            f.transform * kurbo::Point::new(w as f64, h as f64),
        )
    }

    /// All eight symmetries of the rectangle, so a test that only wants to know
    /// "does this hold whatever the picture has been turned to" says so once.
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

    /// **Entering Crop must not change the picture**, which is what
    /// `ImageRef::cover_crop` is for — and the assertion is that the two
    /// framings are *equal*, not that they look similar.
    ///
    /// Crop's rectangle maps onto the frame exactly, so the seed has to undo the
    /// cover framing precisely: get it slightly wrong and choosing Crop nudges
    /// the picture, which is the one thing the mode whose job is not to change
    /// one must not do. Asserted across three aspect relationships — a source
    /// wider than its frame, taller than it, and matching it — because the
    /// centring term vanishes on the matching case and a version that forgot it
    /// passes there.
    ///
    /// **Run again under every one of the eight orientations**, which is where
    /// the seed is easiest to get wrong: the turned cases need cover computed
    /// against the *turned* aspect, and a `cover_crop` that ignored the
    /// orientation is right for the four unturned ones and wrong for the four
    /// that swap the axes.
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
                // Compared through the corners rather than the affine's
                // coefficients, so a transform composed a different way but
                // landing in the same place still counts as equal.
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

    /// **The same guarantee is unreachable for `Fit`, and this is the proof
    /// rather than the excuse** — §15 D561.
    ///
    /// `canvas::crop_and_limit` seeds every non-`Crop` fill from
    /// `ImageRef::cover_crop`, and its doc used to promise the picture *"starts
    /// where the eye already is"* for all four fits. The test above shows that
    /// exactly for `Fill`. For `Fit` it is false — entering the crop on a
    /// letterboxed picture scales it up and throws part of it away — and it
    /// cannot be made true: a `Crop` framing maps its rectangle onto the **whole**
    /// frame, so reproducing a letterbox would need a rectangle *larger than the
    /// source*, and `crop_clamped` shrinks any such rectangle back about its own
    /// centre before it can be stored.
    ///
    /// So the assertion is the sharper statement, and it is what makes cover the
    /// right answer rather than a convenient one: **`crop_clamped` of the exact
    /// rectangle *is* `cover_crop`**, to the last bit, at every orientation.
    ///
    /// The exact rectangle is read off `ImageRef::picture_box` — where the whole
    /// picture actually lands — rather than re-derived from the aspect ratios,
    /// because a re-derivation would be this file's own arithmetic checked against
    /// itself. ⚠️ **The `Fill` row is the control that makes the derivation
    /// credible**: there the exact rectangle needs no clamping at all and equals
    /// `cover_crop` on its own, so the equality below cannot be an artefact of
    /// clamping everything onto the same rectangle.
    ///
    /// **Flip-checked, and it bit at the control rather than where it was aimed**:
    /// `cover_crop`'s `.max` turned into `.min` — cover made contain, which is the
    /// implementation somebody would actually have written — fails the *Fill* row
    /// first, at `(-0.5, 0) – (1.5, 1)` against an exact `(0, 0.25) – (1, 0.75)`.
    /// The clamped assertion never runs. So the control is not decoration here; it
    /// is the assertion with the teeth.
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
                // The rectangle of the picture the frame is a window onto,
                // normalised the way a crop is: the frame expressed against the
                // whole picture's own box.
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

    /// The seed is also the widest the crop can honestly go: any larger and it
    /// leaves the source, where `Extend::Pad` smears the edge row across the
    /// frame. One axis always reaches the full `0..1` — the other is the one
    /// cover overflows.
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
        // Degenerate input has no cover to compute and must not divide by zero.
        assert_eq!(
            ImageRef::new(ImageId("x".into())).cover_crop(
                kurbo::Rect::new(0.0, 0.0, 10.0, 10.0),
                0,
                5
            ),
            whole_crop()
        );
    }

    /// **The picture follows the pointer, and the rectangle goes the other way.**
    ///
    /// The sign is the whole of this function and it is the thing that is wrong
    /// half the time by construction — a crop that ran away from the drag would
    /// be reported as "it moves backwards" and nothing else about the gesture
    /// would look different. Asserted by where a *source point lands*, not by
    /// which way the numbers went: drag right by a quarter of the frame and the
    /// pixel that was at the frame's left edge must now be a quarter of the way
    /// across it.
    #[test]
    fn panning_moves_the_picture_with_the_pointer() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let start = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        let r = ImageRef {
            fit: ImageFit::Crop,
            crop: start,
            ..ImageRef::new(ImageId("x".into()))
        };
        // The source pixel showing at the frame's left edge before the drag.
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

    /// **A drag stops at the edge of the picture, and stops by *sliding*.**
    ///
    /// Clamping the edges independently is the obvious spelling and it silently
    /// changes the crop's size — so a pan that ran into the edge would zoom the
    /// picture as well as stop it, which reads as the image "snapping bigger" at
    /// the end of a drag. The size surviving is the assertion.
    #[test]
    fn panning_into_the_edge_slides_rather_than_resizing_the_crop() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let start = kurbo::Rect::new(0.1, 0.1, 0.6, 0.6);
        // Far more than enough to run out of picture on the left.
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
        // The other axis was not dragged and must not have moved.
        assert!((r.y0 - start.y0).abs() < 1e-9);
    }

    /// **Reframing does not move the picture**, which is the whole of what makes
    /// the bounding box a crop rectangle rather than a resize.
    ///
    /// Asserted where it is felt rather than on the numbers: the picture's scale
    /// in *local units per source pixel* is what the eye reads, so the test
    /// recomputes it on both axes either side of the change. A version that only
    /// checked the crop's width would pass against an implementation that slid
    /// the rectangle as well as shrinking it, which is the plausible wrong answer
    /// here — dragging the right edge in and having the picture jump left.
    #[test]
    fn reframing_keeps_the_picture_where_it_was_on_both_axes() {
        let (w, h) = (400u32, 300u32);
        let from = kurbo::Rect::new(0.0, 0.0, 200.0, 100.0);
        let start = kurbo::Rect::new(0.2, 0.1, 0.9, 0.6);
        // The right and bottom edges pulled in, the top-left corner held.
        let to = kurbo::Rect::new(0.0, 0.0, 150.0, 60.0);
        let next = crop_reframed(start, from, to);

        // Local units per source pixel, before and after. Both axes, because a
        // rule that held on one of them would stretch the picture on the other.
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
        // And the held corner still reads the same part of the source, which is
        // the half that catches a rectangle that slid.
        assert!(
            (next.x0 - start.x0).abs() < 1e-9 && (next.y0 - start.y0).abs() < 1e-9,
            "the anchored corner moved in the source: {next:?} against {start:?}"
        );

        // Dragging the *near* edges instead moves the origin and holds the far
        // one, which is the case an implementation written only for the far edges
        // gets wrong in silence.
        let near = crop_reframed(start, from, kurbo::Rect::new(50.0, 25.0, 200.0, 100.0));
        assert!(
            (near.x1 - start.x1).abs() < 1e-9 && (near.y1 - start.y1).abs() < 1e-9,
            "the far corner moved in the source: {near:?} against {start:?}"
        );
        assert!(near.x0 > start.x0 && near.y0 > start.y0, "{near:?}");

        // A frame with no extent has no mapping to re-read.
        let flat = kurbo::Rect::new(0.0, 0.0, 0.0, 100.0);
        assert_eq!(crop_reframed(start, flat, to), start);
    }

    /// **The scale field round-trips, and its floor is cover.**
    ///
    /// Two claims the control rests on and neither is obvious. *Round-trips*: read
    /// the scale, ask for a factor of it, read it back and get what was asked —
    /// which is only true because `crop_scale` measures one axis and `crop_scaled`
    /// multiplies uniformly. A version that recomputed each axis from the frame
    /// would also pass a symmetric fixture and would silently un-stretch a
    /// deliberately stretched picture, so the fixture here is **oblong on both
    /// counts** — a 2:1 source in a 1:2 frame.
    ///
    /// *The floor is cover*: zooming out stops where the crop reaches the source,
    /// and that is exactly the rectangle `Fill` shows. Asserted by asking for a
    /// tenth of the cover scale and getting cover back, which is the case that
    /// distinguishes a real clamp from an unbounded field whose number walks away
    /// from the picture.
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

        // Zoom in to 2.5× cover and read it back.
        let want = cover * 2.5;
        r.crop = crop_scaled(r.crop, at_cover / want);
        let got = r.crop_scale(frame, w, h).expect("a scale");
        assert!(
            (got - want).abs() < 1e-9,
            "asked for {want} and read back {got}"
        );
        // Uniform: the crop's proportions are unchanged, so the picture is not
        // stretched by having been zoomed.
        let seed = ImageRef::new(ImageId("x".into())).cover_crop(frame, w, h);
        assert!(
            (r.crop.width() / r.crop.height() - seed.width() / seed.height()).abs() < 1e-9,
            "zooming changed the crop's proportions: {:?}",
            r.crop
        );

        // And back out past the bottom: it stops at cover rather than running on.
        r.crop = crop_scaled(r.crop, 10.0);
        let floor = r.crop_scale(frame, w, h).expect("a scale");
        assert!(
            (floor - cover).abs() < 1e-9,
            "zooming out ran past cover to {floor}, where cover is {cover} — the \
             crop left the source and `Extend::Pad` is smearing the edge row"
        );

        // A nonsense factor is refused rather than producing a singular crop.
        let before = r.crop;
        assert_eq!(crop_scaled(before, 0.0), before);
        assert_eq!(crop_scaled(before, f64::NAN), before);
    }

    /// **The picture's rectangle is where the picture is**, in every mode — the
    /// outline the crop gesture draws and clamps against.
    ///
    /// Checked against what each mode *means* rather than against a recomputed
    /// transform, since recomputing it here is the copy `picture_box` exists to
    /// avoid: Fill overflows the frame, Fit falls short of it, and a Crop of half
    /// the source is twice the frame.
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
        // Fit's box is the rectangle it clips to, which is the same statement
        // made by the code that paints it.
        assert_eq!(r.framing(frame, w, h).clip, Some(fit));

        // Half the source in each direction maps onto the frame, so the whole of
        // it is twice the frame — and centred on it, since this crop is.
        r.fit = ImageFit::Crop;
        r.crop = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        let cropped = r.picture_box(frame, w, h).expect("a picture");
        assert!(
            (cropped.width() - frame.width() * 2.0).abs() < 1e-9
                && (cropped.height() - frame.height() * 2.0).abs() < 1e-9,
            "{cropped:?} against {frame:?}"
        );
        assert!((cropped.center() - frame.center()).hypot() < 1e-9);

        // Nothing to bound where nothing draws.
        assert_eq!(r.picture_box(frame, 0, h), None);
        assert_eq!(r.picture_box(kurbo::Rect::ZERO, w, h), None);
    }

    /// **A parameter is read by the mode that needs it and ignored by the three
    /// that do not.**
    ///
    /// This is what carrying `crop` and `tile_scale` beside the mode rather than
    /// inside its variant costs: with variant payloads a mode physically cannot
    /// see another's number, and here it can. So the isolation the enum used to
    /// give for free is asserted instead — a Fill that quietly applied the crop
    /// would look like the crop gesture leaking into every other mode, and it is
    /// the exact bug this shape makes possible.
    ///
    /// The trade it buys is in `paint::with_fit`'s test: a mode strip that must
    /// not throw a crop away when you click *Fit* to look at the whole picture.
    #[test]
    fn a_mode_ignores_the_parameters_it_does_not_use() {
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let plain = ImageRef::new(ImageId("x".into()));
        // The same reference carrying a crop and a scale that are *not* the
        // defaults — the state an image has after being cropped and set back.
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
        // ...and each really is using its own, rather than both defaulting.
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

    /// **A parameter the mode is not using stays out of the file.**
    ///
    /// Three `skip_serializing_if`s rather than one, so an image that has been
    /// cropped and then set back to Fill writes its rectangle (it is still the
    /// user's) while one that has never been cropped writes nothing at all — the
    /// byte-for-byte line §5.11 wants. Asserting the *absence* is the half that
    /// fails silently: a default that reached the file would put a `crop` and a
    /// `tile_scale` on every image brush ever written.
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

    /// **Cover and contain, on a frame that is not at the origin.**
    ///
    /// The offset frame is the point: `(frame.width() − w) / 2` centres a
    /// picture in a frame's *size* and puts it in the wrong place in every frame
    /// that has been moved, which is every frame in a real document and none in
    /// the obvious test. A 200×100 frame and a square source also separate the
    /// two modes — cover scales by 2 and overflows top and bottom, contain
    /// scales by 1 and leaves a band either side.
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

    /// **A fit that does not letterbox pays for no clip**, which is the case a
    /// click-placed image is always in: the shape was made at the picture's own
    /// aspect, so contain and cover are the same drawing and a layer per fill
    /// per frame would be bought for nothing.
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

    /// **A crop maps its rectangle onto the frame and nothing else onto it.**
    ///
    /// The rectangle is normalized to the source, so the assertion is that the
    /// *pixels* it names — the middle half of a 100×50 picture — land on the
    /// frame's corners exactly. Reading the crop as pixels rather than as a
    /// fraction passes any test whose source happens to be 1×1.
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
        // Source pixels 25..75 × 0..50 — a 50×50 region — onto a 100×100 frame.
        assert_eq!(f.transform * kurbo::Point::new(25.0, 0.0), frame.origin());
        assert_eq!(
            f.transform * kurbo::Point::new(75.0, 50.0),
            kurbo::Point::new(110.0, 110.0)
        );
        assert!(f.clip.is_none(), "a crop covers its frame by construction");

        // The identity crop is the whole picture stretched onto the frame, which
        // is what *reset crop* restores.
        let whole = fitted(ImageFit::Crop, frame, 100, 50);
        assert_eq!(
            corners(&whole, 100, 50),
            (frame.origin(), kurbo::Point::new(110.0, 110.0))
        );
    }

    /// **Tile is anchored at the frame's corner, at its own scale, repeating.**
    ///
    /// Anchored rather than centred: a repeat has no centre, and a tiling that
    /// shifted when the shape was resized from the far edge would look like the
    /// pattern was sliding.
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

    /// **Nothing degenerate produces a singular brush transform.**
    ///
    /// Each of these reaches a division that would otherwise be by zero, and
    /// what comes out of a singular affine is a backend question with no good
    /// answer — vello draws nothing, `vello_cpu` has been known to draw a smear.
    /// A frame with no area draws nothing anyway, so the identity is a safe
    /// answer rather than a fudged one.
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
        // A scale of zero would be singular; it is drawn at 1:1 instead.
        let zero = parameterized(ImageFit::Tile, whole_crop(), 0.0, frame, 100, 100);
        assert_eq!(
            corners(&zero, 100, 100),
            (kurbo::Point::ZERO, kurbo::Point::new(100.0, 100.0))
        );
        assert_eq!(zero.extend, peniko::Extend::Repeat);

        // ⚠️ **The overflow arm, added 2026-09-07 (§15 D452).** These four
        // coordinates are *finite*, so the non-finite family (D421, D451) never
        // sees them; it is `x1 - x0` that overflows. `inf` passes `> 0.0`, and
        // bounding the small end is all these guards used to do, so `framing` returned
        // `Affine([0, 0, 0, 0, NaN, NaN])` — singular, with a NaN translation,
        // the exact thing `Framing::identity` exists to give an answer instead of.
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

    /// **A crop whose span overflows never reaches the document.**
    ///
    /// `[S9.1-L5-01]`. The four coordinates are finite — `-1e308` and `1e308` are
    /// ordinary JSON numbers a hand-edited file can carry — and it is `x1 - x0`
    /// that overflows to `inf`. Every guard in this module bounded the small end
    /// only — eight of them spelled `<= 0.0` — which `inf` passes, so
    /// `crop_clamped` went on to divide `inf / inf` and returned
    /// an all-`NaN` rectangle. **One crop-pan drag commits that through
    /// `SetFills`**, the file saves `"x0": null`, and it never opens again — D421's
    /// terminal failure reached without a single non-finite number in the input.
    ///
    /// ⚠️ **Two guards, and the test says which is which.** `usable_span` stops
    /// the NaN being *manufactured* here; `build::brush_is_finite` (D451) stops one
    /// reaching the document if anything else ever makes one. The second was
    /// blind to this until today — its `Brush::Image` arm read `sampler.alpha`
    /// alone, and the crop hangs off the `ImageRef` one level further in, which is
    /// what a check written by reading `peniko::Brush` misses.
    ///
    /// ⚠️ **Flipped** by restoring `w <= 0.0 || h <= 0.0` in `crop_clamped`: fails
    /// on the first assertion with all four coordinates `NaN`. The framing
    /// assertions in `degenerate_inputs_resolve_to_the_identity_framing` stay
    /// **green** under that flip, which is what says the two guards are separate
    /// and neither covers the other.
    #[test]
    fn a_crop_whose_span_overflows_never_reaches_the_document() {
        let huge = kurbo::Rect::new(-1e308, -1e308, 1e308, 1e308);
        assert_eq!(
            crop_clamped(huge),
            whole_crop(),
            "an unusable crop resolves to the whole picture, as a zero-sized one does"
        );

        // And `crop_panned` cannot emit one either — it is the door the drag
        // actually goes through.
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let panned = crop_panned(huge, kurbo::Vec2::new(5.0, 0.0), frame);
        assert!(
            [panned.x0, panned.y0, panned.x1, panned.y1]
                .iter()
                .all(|f| f.is_finite()),
            "a crop-pan of an unusable crop is finite: {panned:?}"
        );

        // The control: an ordinary crop is untouched by any of this.
        let ordinary = kurbo::Rect::new(0.25, 0.25, 0.75, 0.75);
        assert_eq!(
            crop_clamped(ordinary),
            ordinary,
            "a real crop is left alone"
        );
    }

    /// **The turn has period four and each mirror is its own inverse**, so
    /// pressing a button until you are back where you started really does get you
    /// back where you started.
    ///
    /// What this does **not** prove is that the matrices are exact, and it is
    /// worth saying so: `quarters` is recomputed from the count rather than
    /// accumulated, so the fourth turn lands on the `_ => Affine::IDENTITY` arm
    /// whatever the other three arms are made of — an `Affine::rotate` spelling
    /// passes this test untouched. `every_orientation_maps_the_source_box_exactly_onto_the_oriented_box`
    /// is the one that rejects it. (Checked by putting that spelling in: three
    /// tests fail and this is not one of them.)
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
        // The same for the two mirrors, which are their own inverses.
        for op in [OrientOp::FlipH, OrientOp::FlipV] {
            let there_and_back = start.reoriented(op).reoriented(op);
            assert_eq!(there_and_back.orient, start.orient, "{op:?} twice");
            assert_eq!(
                there_and_back.orient.affine(320.0, 200.0).as_coeffs(),
                kurbo::Affine::IDENTITY.as_coeffs()
            );
        }
    }

    /// **A button means the same thing whatever the picture has already been
    /// turned to**, which is the claim `ImageOrient::applying` makes and the one
    /// two independent flags get wrong.
    ///
    /// Asserted against the matrix rather than against the fields: pressing *flip
    /// horizontal* must compose a horizontal mirror **in the frame's axes** onto
    /// whatever transform the picture already had. Written as `mirrored =
    /// !mirrored` and nothing else — the spelling anyone reaches for first — this
    /// passes for the two unturned orientations and fails for the six others,
    /// because a mirror conjugates a rotation into its inverse. Every one of the
    /// eight is checked for exactly that reason.
    #[test]
    fn a_flip_or_a_turn_composes_in_the_frames_axes_from_every_orientation() {
        use kurbo::Affine;
        // The source is oblong so a wrong turn cannot hide behind a square.
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

    /// **An orientation is a symmetry of the source box**, so however the picture
    /// is turned its four corners still land on the four corners of the oriented
    /// box — no gap, no overhang, and the aspect swapped exactly when the turn is
    /// an odd one.
    ///
    /// This is what the framing arms rely on when they take
    /// `ImageOrient::oriented_size` and then never think about orientation
    /// again. A translation left off one of the three turn matrices puts the box
    /// in the wrong quadrant, which reads on screen as the picture vanishing.
    ///
    /// **Compared exactly, and that is the second thing it is for.** Built out of
    /// `Affine::rotate(FRAC_PI_2)` the box lands within `1e-14` of the right place
    /// and this fails — which is the point, because a brush transform that
    /// differs in its last bits is a `SetFills` no equality guard catches, so a
    /// turn and three more would write the file every time.
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
            // ...and it is a rigid motion, so the area cannot have changed.
            assert_eq!(got.width() * got.height(), w * h);
        }
    }

    /// **A turned picture is framed by its turned aspect.**
    ///
    /// The case that separates "the orientation is composed onto the transform"
    /// from "the orientation is composed onto the transform *and* the mode knows
    /// about it": a 200×100 source exactly fills a 200×100 frame, and turned a
    /// quarter it is a 100×200 picture that cover has to scale by 2. A version
    /// that composed the turn but kept the original aspect scales by 1 and leaves
    /// half the frame empty — which draws, and looks like a rendering bug rather
    /// than like arithmetic.
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
        // The oriented picture is 100 wide by 200 tall; covering a 200×100 frame
        // scales it by 2, to 200×400, hanging 150 above and below.
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

        // Contain goes the other way: scale 0.5, a 50×100 picture centred, and a
        // letterbox that now has to be cut.
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

    /// **Turning or flipping a cropped picture turns what you are looking at**,
    /// rather than moving the frame to a different part of the photograph.
    ///
    /// Asserted on the *source pixels reaching the frame*, which is the thing the
    /// user would report: map the frame's corners back through the framing and
    /// compare the box they name in the source before and after. The obvious
    /// wrong version — write the orientation and leave `crop` alone — passes
    /// every test above and fails this one, because a crop is normalized to the
    /// oriented picture and the orientation is what says which pixels that is.
    ///
    /// A crop that is neither centred nor square, so a version that carried the
    /// rectangle the wrong way round is caught too.
    #[test]
    fn reorienting_a_cropped_picture_keeps_the_same_pixels_in_the_frame() {
        let frame = kurbo::Rect::new(10.0, 20.0, 210.0, 120.0);
        let (w, h) = (400u32, 200u32);
        // The pixels a framing puts inside the frame, as a box in source space.
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
        // The fixture has to be in the state the test is about, or it is about
        // nothing: an off-centre, non-square crop of an oblong source. Compared
        // loosely because `shown` goes back through a matrix inverse, which is
        // where the last bit of 50.0 goes; the claim is about which pixels, not
        // about arithmetic exactness.
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
        // And the whole-source sentinel is a fixed point of all three, so a
        // picture nobody has cropped still writes no rectangle after a flip.
        for op in [OrientOp::FlipH, OrientOp::FlipV, OrientOp::Turn] {
            assert_eq!(
                ImageRef::new(ImageId("x".into())).reoriented(op).crop,
                whole_crop(),
                "{op:?} disturbed the never-cropped sentinel"
            );
        }
    }

    /// **An orientation nobody set stays out of the file, and one somebody set
    /// reaches it.**
    ///
    /// The absence is the half that fails silently — a default reaching the file
    /// would put an `orient` on every image brush ever written and break the
    /// byte-for-byte line §5.11 wants. The presence is the half a
    /// `skip_serializing_if` pointed at the wrong predicate gets wrong: the
    /// document would load with the picture the right way up and look like the
    /// flip button had never been pressed.
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
        // Mirrored with no turn: the turn count is itself at its default and
        // stays out, so what lands is the one fact that changed.
        assert!(
            json.contains(r#""orient":{"mirrored":true}"#),
            "a flip must reach the file and take nothing else with it, got {json}"
        );
        assert_eq!(
            serde_json::from_str::<Brush>(&json).expect("round trips"),
            brush
        );

        // A turn on its own is the mirror of that case.
        let turned = ImageRef::new(ImageId("abc".into())).reoriented(OrientOp::Turn);
        let json = serde_json::to_string(&turned).expect("serializes");
        assert!(
            json.contains(r#""orient":{"quarters":1}"#),
            "a turn must reach the file, got {json}"
        );
    }

    /// **A crop whose *span* is usable and whose *origin* overflows.**
    ///
    /// `[S9.1-L5-01]`'s residue, found by `arch-scribe` reading D452's brief
    /// against the code and measured here. D452 asked `usable_span` of
    /// `r.width() * iw` at every span test in the module, which is the whole of
    /// what the finding named — but `ImageRef::framing`'s Crop arm also weighs
    /// the crop's **origin**, and a rectangle can have a finite, positive,
    /// perfectly usable span sitting at an origin that overflows the moment it is
    /// multiplied by the source width.
    ///
    /// `1e300` of span at `x0 = -1e308` is that rectangle. All four coordinates
    /// are finite, so `build::image_ref_is_finite` (D451) passes it too — **both
    /// of D452's two guards say yes** — and the composed transform came back
    /// `[2.5e-302, 0, 0, 3.33e-302, NaN, NaN]`, which is the singular-with-a-NaN
    /// -translation shape `Framing::identity` exists to prevent. `picture_box`
    /// reads off `framing`, so it answered `Some(NaN…)`, and that is the input
    /// `tools::crop_resized` hands to `f64::clamp` — the panic the finding
    /// measured, on a crop resize, on the UI thread.
    ///
    /// ⚠️ **The obvious fixture does not reach it.** `{-1e308 … -9.99e307}` — the
    /// rectangle the residue was first written down as — spans `1e305`, and
    /// `1e305 * 4000` overflows too, so `usable_span(sw)` catches it and `framing`
    /// already answered the identity. The case only exists where the span survives
    /// the multiply and the origin does not, which is why the fixture is written
    /// as an offset from `-1e308` rather than as two round numbers.
    ///
    /// ⚠️ **Flipped** by removing the finiteness test on the composed transform:
    /// fails on the first assertion, printing the NaN coefficients. The
    /// `usable_span` assertion below stays **green** under that flip, which is
    /// what says D452's guard does not cover this and a fourth road would need a
    /// fourth guard — the reason the test is on the *result*.
    #[test]
    fn a_crop_whose_origin_overflows_resolves_to_the_identity_framing() {
        let mut r = ImageRef::new(ImageId("a".into()));
        r.fit = ImageFit::Crop;
        r.crop = kurbo::Rect::new(-1e308, -1e308, -1e308 + 1e300, -1e308 + 1e300);
        let frame = kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);

        // The fixture is in the state the test is about, or it is about nothing:
        // a span D452's guard calls usable, at an origin that does not survive the
        // multiply.
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

    /// One channel of a pixel put through the pipeline, as a byte, so a failure
    /// message reads in the units the picture is in.
    fn through(p: &AdjustPipeline, rgb: [u8; 3]) -> [u8; 3] {
        p.apply(rgb.map(|c| c as f32 / 255.0))
            .map(|c| (c * 255.0 + 0.5) as u8)
    }

    /// **The neutral value describes nothing and writes nothing**, which is the
    /// whole of what makes adjustments an additive schema change (§5.11).
    ///
    /// Both halves are asserted, because they fail separately: a `pipeline` that
    /// answered `Some` for the identity would put every image in the document
    /// through a second buffer for no visual difference, and a `skip_serializing_if`
    /// pointed at the wrong predicate would rewrite every existing file's image
    /// lines.
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

        // And the other half: a moved slider does reach the file. A
        // `skip_serializing_if` on the wrong predicate loses this silently, and
        // the document would load looking like the scrub had not committed.
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

    /// **Each of the seven does what its word says**, asserted on pixels rather
    /// than on the matrix, because the matrix is the thing that could be composed
    /// in the wrong order and still look plausible.
    ///
    /// Mid-grey is the probe for the three that are about light, and a saturated
    /// colour for the two that are about colour — a grey put through a saturation
    /// change is grey whichever way the coefficients went, so testing saturation
    /// on grey tests nothing.
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

        // Contrast pushes what is *above* mid-grey up and what is below it down,
        // so it is tested at two levels — one probe at mid-grey would pass
        // against a version that did nothing at all.
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

    /// **Highlights and shadows each leave the other end of the range alone**,
    /// which is the only thing that distinguishes them from a second exposure
    /// slider.
    ///
    /// The discriminating probe is the *far* end: a version that lifted the
    /// whole curve passes every test aimed at the end the slider is named for,
    /// and fails only here. Black and white are exact fixed points of both — the
    /// weights are `(1−v)²` and `v²` — so those are asserted to the byte.
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
        // **Each slider's weight is exactly zero at the far end**, so that end is
        // a fixed point *to the byte* — and it is the opposite end from the one
        // the slider is named for: shadows cannot touch white, highlights cannot
        // touch black. Asserting the pair the wrong way round is how this test
        // was first written and it failed, which is worth leaving as the reason
        // the pairing is spelled out.
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

    /// **The tone curve never doubles back**, at either extreme of either
    /// slider.
    ///
    /// A curve that did would make one end of the picture *invert* at a strong
    /// setting — dark pixels coming out lighter than the ones beside them — which
    /// reads as a rendering fault rather than as a strong setting. The ±0.4
    /// strength is the largest that keeps it monotonic, and the check that this
    /// test is not vacuous is to raise it: at 0.55 the shadow arm fails here.
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

    /// **Contrast pivots on mid-grey**, which is what the matrix's fifth column
    /// is for.
    ///
    /// The plausible wrong version is a bare diagonal scale with no offset — it
    /// spreads the tones exactly as this one does at the top of the range and
    /// darkens the whole picture, so "contrast made the light parts lighter"
    /// passes against it and only the fixed point catches it.
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

    /// **A table is read the way SVG reads one**, because that agreement is the
    /// whole reason the tone curve is a table at all: the canvas evaluates the
    /// same list a viewer does, so the export is the same picture rather than a
    /// near one (see `TONE_STEPS`).
    ///
    /// The two cases worth naming are the spec's own corners: `v = 1` must
    /// interpolate from the *last* pair rather than index off the end, and a
    /// value between two samples must land between them rather than at the
    /// nearest.
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
        // Degenerate lists answer rather than panic — a hand-edited file can
        // carry one.
        assert_eq!(transfer(&[], 0.3), 0.3);
        assert_eq!(transfer(&[0.2], 0.9), 0.2);
    }

    /// **The matrix runs first and the tone curve reads its clamped result** —
    /// which is a claim about the *order* of the two stages, and the order is not
    /// a detail: it is what "set the exposure, then pull the highlights back"
    /// means, and it is what the emitted `<filter>` says by listing its
    /// primitives in that sequence.
    ///
    /// The discriminating probe is a pixel the matrix pushes **past white**. Run
    /// the other way round the same two settings answer 1.0 — a blown-out
    /// highlight — where run this way they answer 0.6, a recovered one. Both are
    /// plausible pictures, which is exactly why the number is asserted rather
    /// than the direction: 0.9 doubled is 1.8, cut to 1.0, and `v + 0.4·(−1)·v²`
    /// at 1.0 is 0.6 exactly.
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

    /// **Nothing that is not a number can reach a buffer.**
    ///
    /// Two doors, and they are different: `Adjustment::set` is the one the
    /// panel and *reset* write through and it clamps, while
    /// `ImageAdjust::pipeline` is what a *hand-edited file* reaches, where
    /// there was no `set` to clamp anything. A `NaN` past either would paint a
    /// hole in a photograph.
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

        // ⚠️ **The two arms below are the ones this test was named for and did
        // not have** (§15 D538, `[S9.1-L5-02]`). Both are *finite*, so the
        // finiteness guard this predicate used to be passed them, and both are
        // reachable by hand-editing a `.ondin` that then loads cleanly and
        // survives a round trip. `Adjustment::set` has clamped to `-1..=1` all
        // along and its doc says why — *"a range the model does not enforce is a
        // range a hand-edited file ignores"* — but `set` is the door the panel
        // uses and a file does not.
        //
        // **Asserted on `pipeline()` and on `is_default()` together.** A version
        // that *clamped* instead of refusing would pass a `pipeline().is_some()`
        // assertion and draw a picture the file does not describe; what says the
        // value is refused rather than quietly repaired is that it is still
        // reported non-neutral, so the panel shows 1000 and nothing pretends
        // otherwise.
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

        // **Controls, at both ends of the range**, because a predicate written
        // `(-1.0..1.0)` or `(0.0..=1.0)` would pass every assertion above and
        // silently refuse half the sliders the panel can produce.
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

    /// **The seven are one list**, so a slider cannot exist that the reset, the
    /// key or the snapshot does not know about.
    #[test]
    fn the_list_of_adjustments_covers_the_whole_struct() {
        let mut a = ImageAdjust::NEUTRAL;
        for (i, k) in Adjustment::ALL.iter().enumerate() {
            k.set(&mut a, (i + 1) as f32 / 10.0);
        }
        assert!(!a.is_default());
        // Every field is now non-zero, so a struct field missing from `ALL`
        // fails here rather than silently never being reset.
        for k in Adjustment::ALL {
            assert!(k.of(&a) != 0.0, "{} is not in ALL", k.label());
        }
        a.reset();
        assert_eq!(a, ImageAdjust::NEUTRAL, "reset puts all seven back");
        assert!(a.is_default());
    }
}
