//! The text attribute model (§5.4): what a character, a paragraph and a whole
//! text node can carry, and how a sub-range overrides it.
//!
//! **Three scopes, three types.** A character attribute may vary over a range
//! and so lives in [`TextStyle`] (the node's default) plus [`CharSpans`] (the
//! overrides). A paragraph attribute and a block attribute may not vary within a
//! node today, so each is a plain field of [`ParagraphStyle`] / [`BlockStyle`].
//! Scope is therefore a type-level fact: nothing can write a per-range
//! alignment, because there is no span type that carries one. See §15 for why
//! paragraph attributes are node-level rather than spanned — the short version is
//! that parley lays a whole node out as one `Layout`, so a second `Alignment`
//! would be a value the model could hold and the renderer had to ignore.
//!
//! **One span carries one attribute.** Two adjacent runs differing only in
//! weight share every other attribute rather than duplicating a whole style
//! struct, which is also the shape parley resolves styles in — so the layout
//! call in `text.rs` is a translation rather than a reconciliation.
//!
//! ## The boundary rule
//!
//! **Typing at the edge of a span inherits from the character to the left**,
//! unless the caret was explicitly restyled while empty (`TextEdit::pending`).
//! Every rich-text bug in every editor lives in this question, so it is stated
//! once here rather than rediscovered per attribute; [`CharSpans::edited`] is
//! the single implementation, and `spans_grow_leftwards_when_typing_at_a_seam`
//! is the guard.

use peniko::Color;
use serde::{Deserialize, Serialize};
use std::ops::Range;

// ---------------------------------------------------------------------------
// Length
// ---------------------------------------------------------------------------

/// A length that is either absolute or a fraction of the font size.
///
/// One type for line height, letter spacing, word spacing, paragraph spacing,
/// indent, baseline shift and decoration thickness/offset — which is what makes
/// [`Scaling::Photographic`](crate::node::TextSizing) legible as one rule
/// ("multiply the `Px`, leave the `Em`") instead of a per-field list the scale
/// tool has to remember.
///
/// **`Em` is a fraction, not a CSS percentage.** The UI shows it as one (×100,
/// which is the unit designers read) but the stored number is em, because CSS's
/// own percentages do not mean this: `letter-spacing` rejects them outright, and
/// for `text-indent` and margins they resolve against the containing block's
/// width — a different basis, and never what is meant here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Length {
    Px(f64),
    /// A fraction of the font size: `0.02` is the 2% the field shows.
    Em(f64),
}

/// Decimal places a stored [`Length`] keeps.
///
/// **Canonicalized on write, so span coalescing can use exact equality.**
/// Without it a drag producing `0.020000000000000004` never merges with a typed
/// `0.02`: the span list fragments, the file grows, and every diff churns. Four
/// places is a ten-thousandth of an em — finer than any font's unit grid — and
/// a hundredth of a percent in the display unit.
const LENGTH_PLACES: i32 = 4;

impl Default for Length {
    fn default() -> Self {
        Length::Px(0.0)
    }
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);

    /// This length rounded to [`LENGTH_PLACES`] — the only form that is stored.
    ///
    /// Every constructor path goes through here: the setters below, the scale
    /// tool, and IO on load, so a hand-edited file cannot smuggle a value that
    /// defeats coalescing.
    ///
    /// ⚠️ **"IO on load" was aspirational until 2026-09-06 and is now true, but by
    /// a road worth naming.** Nothing in `io/` calls this function — `TextStyle`
    /// and `Spans<A>` are plain `#[derive(Deserialize)]`, so serde builds them
    /// field by field. What reaches it is [`Spans::clamped`], the loader's one
    /// pass over every span, through [`canonical_attr`] → `set_on` → the `set` arm
    /// that calls this. **The defaults themselves are still not canonicalized on
    /// load**; `canonical_attr`'s doc says why and how far that reaches.
    pub fn canonical(self) -> Self {
        let round = |v: f64| {
            if !v.is_finite() {
                return 0.0;
            }
            let r = round_to_places(v);
            // `-0.0 == 0.0` but they are different bytes on disk and different
            // text in a field.
            if r == 0.0 { 0.0 } else { r }
        };
        match self {
            Length::Px(v) => Length::Px(round(v)),
            Length::Em(v) => Length::Em(round(v)),
        }
    }

    /// The length in world units, given the font size it is relative to.
    pub fn resolve(self, font_size: f64) -> f64 {
        match self {
            Length::Px(v) => v,
            Length::Em(v) => v * font_size,
        }
    }

    /// Takes `&self` so serde's `skip_serializing_if` can name it directly —
    /// which is the only reason a `Copy` type has a by-reference predicate.
    pub fn is_zero(&self) -> bool {
        match self {
            Length::Px(v) | Length::Em(v) => *v == 0.0,
        }
    }

    /// Whether this is the zero the absent key reads back as — `Px(0.0)`, and
    /// **not** `Em(0.0)`.
    ///
    /// The `skip_serializing_if` predicate for every optional [`Length`] in this
    /// module (§15 D537). [`Self::is_zero`] was that predicate and answers `true`
    /// for both variants, so a field holding `Em(0.0)` was **absent from the
    /// file** and came back as `Px(0.0)`, because [`Length::default`] is `Px`.
    /// `[S6.1-L1-03]`: set the letter-spacing field to `%`, leave it at 0, save,
    /// reopen — the suffix reads `px`. Measured: `Em(0) reload = Px(0.0)
    /// unit=Px`.
    ///
    /// **Zero is the one value at which the unit is unobservable in the
    /// drawing and still observable in the panel**, which is what makes it the
    /// only value this can go wrong at: `resolve` gives `0.0` for both, so
    /// nothing rendered ever disagrees, and the suffix toggle
    /// ([`Self::in_unit`]) is a pure model call that maps `Px(0.0)` straight to
    /// `Em(0.0)`. The user's choice of unit is real data and survives now.
    ///
    /// **Costs four bytes and only where the unit is `Em`.** The common case —
    /// a field never touched, sitting at the `Px(0.0)` default — is elided
    /// exactly as before, so no existing file grows.
    ///
    /// ⚠️ **No schema bump.** This only ever *adds* a key an older reader
    /// already understands: the field is `#[serde(default)]` and `Em` is not a
    /// new variant, so a file written here loads in an older build as `Em(0.0)`
    /// and a file written there loads here as `Px(0.0)` — which is what it
    /// always meant. The change is invisible to `io::MIGRATIONS`.
    ///
    /// ⚠️ **This closes the *save* half of `[S6.1-L1-03]` and deliberately not
    /// the other half.** A zero-valued `Em` span is still kept by `normalize`
    /// (which compares structurally) and so still reported *Mixed* through
    /// `shared_in` for text whose letter spacing is zero everywhere. The two
    /// halves pull opposite ways — preserving the unit **requires** keeping the
    /// span, so making `Em(0.0)` and `Px(0.0)` compare equal would fix the
    /// readout by throwing away the very thing this predicate exists to keep.
    /// The readout is a comparison on the *resolved* value and belongs at the
    /// reader, and **that is where it now is** (§15 D799):
    /// `panels::typography`'s `agreed_zero`, reached from `TypeSubject::shared`
    /// and `para_shared` only after `shared_in` has answered `None`, so nothing
    /// here or in `normalize` had to change. ⚠️ **It stops at zero deliberately**
    /// — zero is the one amount at which the unit says nothing about the ink,
    /// which is why it was the only amount that read wrong; widening it to
    /// lengths that merely resolve alike *at the current size* is a further
    /// decision and is argued at `agreed_zero`.
    pub fn is_default_zero(&self) -> bool {
        matches!(self, Length::Px(v) if *v == 0.0)
    }

    /// The same length in the other unit, given the font size — what the `px`/`%`
    /// suffix toggle produces, so flipping the unit does not move the text.
    pub fn in_unit(self, unit: LengthUnit, font_size: f64) -> Self {
        match (self, unit) {
            (Length::Px(_), LengthUnit::Px) | (Length::Em(_), LengthUnit::Em) => self,
            (Length::Px(v), LengthUnit::Em) => {
                Length::Em(if font_size > 0.0 { v / font_size } else { 0.0 }).canonical()
            }
            (Length::Em(v), LengthUnit::Px) => Length::Px(v * font_size).canonical(),
        }
    }

    pub fn unit(self) -> LengthUnit {
        match self {
            Length::Px(_) => LengthUnit::Px,
            Length::Em(_) => LengthUnit::Em,
        }
    }

    /// The bare number, in whichever unit this is.
    pub fn amount(self) -> f64 {
        match self {
            Length::Px(v) | Length::Em(v) => v,
        }
    }

    /// Multiplied by `factor` — **the `Px` variant only**.
    ///
    /// The photographic scale tool's rule for every length in one place: an
    /// absolute length is part of the drawing and scales with it, an em-relative
    /// one already scales because the font size did (§5.6).
    pub fn scaled(self, factor: f64) -> Self {
        match self {
            Length::Px(v) => Length::Px(v * factor).canonical(),
            Length::Em(_) => self,
        }
    }
}

/// Which unit a [`Length`] field is showing — the state of its `px`/`%` suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthUnit {
    Px,
    Em,
}

impl LengthUnit {
    /// The suffix the field paints. `%` rather than `em`, because the stored em
    /// is displayed ×100 (see [`Length`]).
    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Px => "px",
            LengthUnit::Em => "%",
        }
    }

    /// The other one — what clicking the suffix asks for.
    pub fn other(self) -> Self {
        match self {
            LengthUnit::Px => LengthUnit::Em,
            LengthUnit::Em => LengthUnit::Px,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenType tags and settings
// ---------------------------------------------------------------------------

/// A four-byte OpenType tag — `wght`, `liga`, `tnum`.
///
/// A newtype rather than a `String` so a malformed tag cannot reach the shaper,
/// and so the sort that keeps saved files byte-stable is a plain `Ord`. Stored
/// on disk as the four characters, which is what makes a `.ondin` file readable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag([u8; 4]);

impl Tag {
    /// `None` unless `s` is exactly four printable ASCII bytes, which is what
    /// the OpenType spec allows.
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 4 || !b.iter().all(|c| (0x20..=0x7e).contains(c)) {
            return None;
        }
        Some(Tag([b[0], b[1], b[2], b[3]]))
    }

    pub const fn new(bytes: [u8; 4]) -> Self {
        Tag(bytes)
    }

    pub fn as_str(&self) -> &str {
        // Constructed only from printable ASCII, so this cannot fail.
        std::str::from_utf8(&self.0).unwrap_or("????")
    }

    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tag({})", self.as_str())
    }
}

impl Serialize for Tag {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Tag::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("malformed tag {s:?}")))
    }
}

/// One variable-font axis set to a value, in the axis's own user units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisSetting {
    pub tag: Tag,
    pub value: f64,
}

/// The `opsz` axis — the one axis with a mode rather than just a value.
pub const OPSZ: Tag = Tag::new(*b"opsz");
/// The `wght` axis, which the weight field drives rather than the axis list.
pub const WGHT: Tag = Tag::new(*b"wght");
/// The `ital` axis, which the italic toggle drives.
pub const ITAL: Tag = Tag::new(*b"ital");
/// The `slnt` axis, the other way a family expresses a slope.
pub const SLNT: Tag = Tag::new(*b"slnt");

/// Axes the panel does not list, because a dedicated control already drives
/// them and two controls for one axis is two chances to disagree.
///
/// `opsz` is *not* here: it has its own Auto/Manual mode and its own row, which
/// is the row this list would otherwise hide.
pub const AXES_DRIVEN_ELSEWHERE: [Tag; 3] = [WGHT, ITAL, SLNT];

/// Quantum an axis value is rounded to.
///
/// Tenths: fine enough for `wght` (a 1–1000 axis) and for `opsz` in points,
/// coarse enough that dragging a slider cannot fill the file with sixteen
/// digits. Canonicalized for the same reason [`Length`] is.
const AXIS_QUANTUM: f64 = 0.1;

impl AxisSetting {
    pub fn new(tag: Tag, value: f64) -> Self {
        AxisSetting {
            tag,
            value: quantize(value, AXIS_QUANTUM),
        }
    }
}

fn quantize(v: f64, q: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = (v / q).round() * q;
    // The division reintroduces the noise the rounding removed (0.1 is not
    // representable), so settle the result to the same decimal count.
    round_to_places(r)
}

/// Round to [`LENGTH_PLACES`] decimals **without overflowing** (§15 D424).
///
/// ⚠️ **The obvious spelling, `(v * 10^places).round() / 10^places`, turns a
/// finite number into an infinite one**, and the guard above it does not help:
/// `is_finite` is asked of the *input*. `10^4` is the multiplier here, so any
/// `|v| > f64::MAX / 1e4 ≈ 1.7977e304` overflows in the multiply and
/// `inf.round() / 1e4` is `inf`. Measured: `canonical(Px(1e304))` is finite,
/// `canonical(Px(1e305))` is `inf`.
///
/// **That is a document that never opens again**, because `Px(inf)` is written
/// by `serde_json` as `null` and refused on the way back in. And it is reachable
/// through the documented door — `Spans::set` routes every value through the
/// canonicalizer — so this is the sanitizer *manufacturing* the bad value out of
/// a good one, which is the one position in this class where the operation-level
/// guard would be the wrong answer: it would report a refused letter-spacing
/// edit for a number the user is entitled to type.
///
/// A magnitude that large has no fractional part left to round — the gap between
/// representable neighbours is astronomically larger than `1e-4` — so returning
/// it unchanged is not an approximation but the exact answer.
fn round_to_places(v: f64) -> f64 {
    let scale = 10_f64.powi(LENGTH_PLACES);
    if v.abs() >= f64::MAX / scale {
        return v;
    }
    (v * scale).round() / scale
}

/// One OpenType feature switched on or off (or set to an alternate index).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureSetting {
    pub tag: Tag,
    /// `0` off, `1` on; higher values select an alternate where the feature
    /// offers them (`salt`, `cvXX`).
    pub value: u16,
}

/// `Some(n)` for `ss01`–`ss20`, the stylistic sets.
///
/// **The number, not the prefix.** `"ss".starts_with` is the obvious test and it
/// is wrong twice: it accepts `ssty` (math script-style alternates, a shaping
/// mechanic) and it says nothing about which set, which is the whole of the
/// generic name a face that leaves `FeatureParams` NULL gets. Anything that
/// wants "is this a stylistic set" wants this.
///
/// The range is closed at 20 because the registry closes it there — a `ss21` is
/// an unregistered private tag, and treating it as a set would invent a meaning
/// the spec does not give it.
pub fn stylistic_set(tag: Tag) -> Option<u8> {
    numbered_tag(tag, b"ss", 1..=20)
}

/// `Some(n)` for `cv01`–`cv99`, the character variants.
///
/// The other font-defined range, and the other half of [`stylistic_set`]'s
/// reasoning. `cvXX` differs in what the font may say about it: a
/// `CharacterVariantParams` table carries a label, a tooltip, the characters the
/// feature varies and a count of *named values*, where a stylistic set carries a
/// label alone.
pub fn character_variant(tag: Tag) -> Option<u8> {
    numbered_tag(tag, b"cv", 1..=99)
}

/// `ssNN` / `cvNN` → `NN`, when it is in `range`.
fn numbered_tag(tag: Tag, prefix: &[u8; 2], range: std::ops::RangeInclusive<u8>) -> Option<u8> {
    let b = tag.as_str().as_bytes();
    if &b[..2] != prefix {
        return None;
    }
    // Two ASCII digits and nothing else, so `ssty` is not set 0 and a private
    // `ss+1` is not set 1 — `u8::from_str` accepts a leading sign, which a tag
    // may legally contain.
    if !b[2..].iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n = std::str::from_utf8(&b[2..]).ok()?.parse::<u8>().ok()?;
    range.contains(&n).then_some(n)
}

// ---------------------------------------------------------------------------
// Character attributes
// ---------------------------------------------------------------------------

/// How a run's characters are cased on the way to the shaper.
///
/// **A display transform, never a rewrite of `content`.** The stored text is
/// what the user typed, so switching back to [`TextCase::Original`] recovers it
/// exactly and a search or an export reads the real string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextCase {
    #[default]
    Original,
    Upper,
    Lower,
    /// First letter of each word up, the rest down.
    Title,
}

impl TextCase {
    /// **Display order, and it is not the declaration order.** The panel draws
    /// this as one four-cell track, and the three transforms read as a scale
    /// there — nothing done, then a capital per word, then all capitals, then
    /// none — which is the order a designer scans them in. `Title` is declared
    /// last because it was added last; where it *sits* is a UI question.
    pub const ALL: [TextCase; 4] = [
        TextCase::Original,
        TextCase::Title,
        TextCase::Upper,
        TextCase::Lower,
    ];

    /// **The label is a sample of what the cell does**, not a name for it: three
    /// of the four are one word each in every language and the picture is faster
    /// to read than any of them. `Aa`/`AA`/`aa` rather than `Ag`/`AG`/`ag` — the
    /// doubled letter is what the rest of the industry draws, and consistency
    /// with what a designer already recognises beats the descender.
    pub fn label(self) -> &'static str {
        match self {
            TextCase::Original => "As typed",
            TextCase::Title => "Aa",
            TextCase::Upper => "AA",
            TextCase::Lower => "aa",
        }
    }
}

/// How the ink of an underline or strikethrough is broken up.
///
/// Ours, not parley's — parley draws no decoration at all, it only reports where
/// one goes. Solid and the two dashed forms reuse the stroke dash machinery
/// (§6.3); wavy is a sine sampled along the run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    Wavy,
}

impl LineStyle {
    pub const ALL: [LineStyle; 4] = [
        LineStyle::Solid,
        LineStyle::Dashed,
        LineStyle::Dotted,
        LineStyle::Wavy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LineStyle::Solid => "Solid",
            LineStyle::Dashed => "Dashed",
            LineStyle::Dotted => "Dotted",
            LineStyle::Wavy => "Wavy",
        }
    }
}

/// An underline or a strikethrough.
///
/// **`None` means "the font decides"** for thickness, offset and colour — the
/// same "absence is a state" pattern as [`Pivot`](crate::node::Pivot), and what
/// keeps the value correct across a font swap: a stored 1.2px underline on a
/// 12px face is wrong the moment the family changes, where the font's own `post`
/// entry is right on both.
///
/// **[`Self::skip_ink`] is the one field whose default is not the zero value**, so
/// [`Default`] is written out below rather than derived: skip-ink is on unless it
/// is switched off, and a derived `false` would have silently changed what every
/// `..Decoration::default()` in the workspace means.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decoration {
    /// `None` = the font's own thickness (`post.underlineThickness`,
    /// `OS/2.yStrikeoutSize`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness: Option<Length>,
    /// Distance from the baseline, `None` = the font's own.
    ///
    /// **Signed, and positive is *up*** — against the direction every other y in
    /// the model runs, because this one is parley's, inherited from
    /// `post.underlinePosition`. `text::decoration_ink` computes
    /// `top = baseline - offset` and the panel's tooltip says upwards; this
    /// comment used to say *down*, and was the odd one out of the three.
    ///
    /// The practical consequence, and the reason it is worth a paragraph: pushing
    /// an underline further **away** from the glyphs means typing a *negative*
    /// number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Length>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub style: LineStyle,
    /// `None` = the text's own colour, so recolouring the text takes the
    /// decoration with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Whether the band breaks around the glyph ink it crosses — CSS's
    /// `text-decoration-skip-ink` (§5.4, `text::skip_ink`, §15 D356/D357).
    ///
    /// **`true` is the default and the CSS initial value**, which is why this is
    /// the field that costs `Decoration` its derived [`Default`]: `false` is the
    /// state somebody asked for, and it is stored rather than the other way round
    /// so that every file written before the toggle existed keeps the drawing it
    /// was saved with.
    ///
    /// **Read for underlines only**, because css-text-decor-4 skips underlines and
    /// overlines and never a `line-through` — a strikethrough is *meant* to cross
    /// the letters. A strikethrough therefore carries this field and nothing looks
    /// at it, and the panel does not offer the toggle on one: a control whose state
    /// changed nothing would be the more expensive kind of honesty.
    #[serde(default = "skips_ink", skip_serializing_if = "is_skipping_ink")]
    pub skip_ink: bool,
}

impl Default for Decoration {
    fn default() -> Self {
        Self {
            thickness: None,
            offset: None,
            style: LineStyle::default(),
            color: None,
            skip_ink: skips_ink(),
        }
    }
}

/// Skip-ink's default: on, as CSS's initial value is. See [`Decoration::skip_ink`].
fn skips_ink() -> bool {
    true
}

fn is_skipping_ink(v: &bool) -> bool {
    *v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// One character attribute, as a span carries it.
///
/// The values are the same ones [`TextStyle`] holds as defaults; the enum is
/// what lets a span say "weight, over these bytes" without carrying a whole
/// style struct. Kept in step with `TextStyle` by [`TextStyle::get`] and
/// [`TextStyle::set`], which are exhaustive matches — add a variant and the
/// compiler names both places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CharAttr {
    Family(String),
    Size(f64),
    Weight(u16),
    Italic(bool),
    Variations(Vec<AxisSetting>),
    Features(Vec<FeatureSetting>),
    Locale(Option<String>),
    /// `None` is Auto — the font's own line height, CSS `normal`.
    LineHeight(Option<Length>),
    LetterSpacing(Length),
    WordSpacing(Length),
    Underline(Option<Decoration>),
    Strikethrough(Option<Decoration>),
    Case(TextCase),
    BaselineShift(Length),
    /// The ink the glyphs are drawn in. `None` means the node's own fill stack,
    /// which is what an unstyled run has always drawn in — see [`TextStyle::color`].
    Color(Option<Color>),
}

/// Which attribute a [`CharAttr`] is, without its value.
///
/// The identity a span is keyed by: spans of one kind never overlap, and the
/// stable order of these variants is the `(attribute, start)` sort that keeps a
/// saved file byte-stable (§5.11, invariant 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CharAttrKind {
    Family,
    Size,
    Weight,
    Italic,
    Variations,
    Features,
    Locale,
    LineHeight,
    LetterSpacing,
    WordSpacing,
    Underline,
    Strikethrough,
    Case,
    BaselineShift,
    /// **Appended, and that is load-bearing rather than tidiness.** This order *is*
    /// the `(attribute, start)` sort a saved file is written in, so a variant that
    /// **moves** reorders the spans of every document already on disk — every one of
    /// them churns a diff on its next save, saying nothing.
    ///
    /// ⚠️ **The hazard is a variant that moves, not a variant that arrives** (§15
    /// D598, correcting the reason §15 D154 gave and this comment repeated). A *new*
    /// kind inserted in the middle leaves the fourteen already here in the same
    /// relative order, so a document that does not use it sorts and serializes
    /// byte-for-byte as before; what churns every file is swapping two of the
    /// fourteen, which is a one-line edit with no other symptom. Appending is still
    /// the rule, because it is the only edit that *cannot* move one — but
    /// [`CharAttrKind::ALL`] is what enforces it, and it enforces the real rule
    /// rather than this one's proxy.
    Color,
}

impl CharAttrKind {
    /// Every kind, in the declaration order that *is* the on-disk span sort.
    ///
    /// **Written out so that a reorder is a diff.** The order of these fifteen is
    /// invariant 9's byte-stability guarantee (§5.11), and until §15 D598 nothing
    /// could see it change: `colour_sorts_after_every_other_attribute` compared
    /// `Color` against a hand-written list of the other fourteen, and swapping any
    /// two of those fourteen leaves all fourteen comparisons true. The measured flip
    /// — `Family` and `Size` exchanged, one line — left the whole workspace green.
    ///
    /// ⚠️ **Completeness is not compiler-checked and deliberately so.** A sixteenth
    /// variant that is not added here makes no test red, and that is the correct
    /// answer rather than a hole: a kind appended after `Color` moves none of these
    /// fifteen, so no document on disk changes and there is nothing to catch. What
    /// this list is for is the edit that *does* churn every file, and for that it is
    /// exact — see `char_attribute_kinds_keep_the_order_a_saved_file_is_written_in`.
    ///
    /// 🚨 **Adding a sixteenth kind here *will* go red, and this is the note that
    /// says what to do about it.** That test also asserts `ALL.last() == Color`,
    /// which restates D154's own rule — so appending, the one edit D154 blesses,
    /// fails it. **Amend the assertion; do not move `Color` to keep it.** Moving a
    /// variant that is already on disk is the exact thing this list exists to
    /// prevent, and the ascending assertion above it is the load-bearing half
    /// (§15 D598). Written here rather than only in the test, because the person
    /// who trips it will be standing at the enum.
    pub const ALL: [CharAttrKind; 15] = [
        CharAttrKind::Family,
        CharAttrKind::Size,
        CharAttrKind::Weight,
        CharAttrKind::Italic,
        CharAttrKind::Variations,
        CharAttrKind::Features,
        CharAttrKind::Locale,
        CharAttrKind::LineHeight,
        CharAttrKind::LetterSpacing,
        CharAttrKind::WordSpacing,
        CharAttrKind::Underline,
        CharAttrKind::Strikethrough,
        CharAttrKind::Case,
        CharAttrKind::BaselineShift,
        CharAttrKind::Color,
    ];
}

impl CharAttr {
    pub fn kind(&self) -> CharAttrKind {
        match self {
            CharAttr::Family(_) => CharAttrKind::Family,
            CharAttr::Size(_) => CharAttrKind::Size,
            CharAttr::Weight(_) => CharAttrKind::Weight,
            CharAttr::Italic(_) => CharAttrKind::Italic,
            CharAttr::Variations(_) => CharAttrKind::Variations,
            CharAttr::Features(_) => CharAttrKind::Features,
            CharAttr::Locale(_) => CharAttrKind::Locale,
            CharAttr::LineHeight(_) => CharAttrKind::LineHeight,
            CharAttr::LetterSpacing(_) => CharAttrKind::LetterSpacing,
            CharAttr::WordSpacing(_) => CharAttrKind::WordSpacing,
            CharAttr::Underline(_) => CharAttrKind::Underline,
            CharAttr::Strikethrough(_) => CharAttrKind::Strikethrough,
            CharAttr::Case(_) => CharAttrKind::Case,
            CharAttr::BaselineShift(_) => CharAttrKind::BaselineShift,
            CharAttr::Color(_) => CharAttrKind::Color,
        }
    }
}

// ---------------------------------------------------------------------------
// TextStyle — the node's character defaults
// ---------------------------------------------------------------------------

/// A text node's character attributes: the value every byte carries unless a
/// [`CharSpans`] entry says otherwise.
///
/// Every field is `#[serde(default)]` with `skip_serializing_if` wherever the
/// default has an honest "absent" reading, so growing this struck changed no
/// existing document's bytes past the one migration that had to happen (§5.11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub font_family: String,
    pub font_size: f64,
    pub weight: u16,
    pub italic: bool,
    /// Explicit variable-axis settings, sorted by tag.
    ///
    /// **`opsz`'s absence is its Auto mode**: with no entry, `text.rs` tracks the
    /// font size the way CSS `font-optical-sizing: auto` does; with one, the
    /// number is the user's. Neither parley nor fontique does this on its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<AxisSetting>,
    /// OpenType feature settings, sorted by tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<FeatureSetting>,
    /// BCP-47 language tag, driving language-specific shaping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// `None` is Auto: the font's own line height (`MetricsRelative(1.0)`, CSS
    /// `normal`). `Em` is a multiple of the font size, `Px` an absolute height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<Length>,
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub letter_spacing: Length,
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub word_spacing: Length,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<Decoration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<Decoration>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub case: TextCase,
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub baseline_shift: Length,
    /// The ink the glyphs are drawn in, or `None` for **the node's own fill stack**.
    ///
    /// **The one character attribute whose absence is not merely a default**, and
    /// the reason it is `Option` rather than a `Color`: a text node's colour is
    /// already expressed, as `Paint.fills` — a *list*, which a character attribute
    /// cannot be without a coalescing story for `Vec<Fill>` and its visibility
    /// flags. So `None` keeps the multi-fill feature working for every run that has
    /// not been given a colour of its own, and needs no migration: every existing
    /// document deserializes to `None` and draws exactly as it did.
    ///
    /// A run *with* a colour draws once, in that colour. The asymmetry is
    /// deliberate — a node can have two stacked translucent fills and a run cannot
    /// — and it is the same trade [`Decoration::color`] already makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// Smallest font size the fields accept. Zero is not a size, and a negative one
/// is not a drawing.
pub const MIN_FONT_SIZE: f64 = 1.0;
/// Largest font size the fields accept — a cap on the control, not on the model
/// (the same bargain as `MAX_STROKE_WIDTH`).
pub const MAX_FONT_SIZE: f64 = 1000.0;

impl Default for TextStyle {
    /// Inter at 16 — parley's own default size, and the family core bundles, so
    /// a default style shapes on every machine and offline.
    fn default() -> Self {
        TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            weight: 400,
            italic: false,
            variations: Vec::new(),
            features: Vec::new(),
            locale: None,
            line_height: None,
            letter_spacing: Length::ZERO,
            word_spacing: Length::ZERO,
            underline: None,
            strikethrough: None,
            case: TextCase::Original,
            baseline_shift: Length::ZERO,
            color: None,
        }
    }
}

impl TextStyle {
    /// This style's value for one attribute, as a span would carry it.
    pub fn get(&self, kind: CharAttrKind) -> CharAttr {
        match kind {
            CharAttrKind::Family => CharAttr::Family(self.font_family.clone()),
            CharAttrKind::Size => CharAttr::Size(self.font_size),
            CharAttrKind::Weight => CharAttr::Weight(self.weight),
            CharAttrKind::Italic => CharAttr::Italic(self.italic),
            CharAttrKind::Variations => CharAttr::Variations(self.variations.clone()),
            CharAttrKind::Features => CharAttr::Features(self.features.clone()),
            CharAttrKind::Locale => CharAttr::Locale(self.locale.clone()),
            CharAttrKind::LineHeight => CharAttr::LineHeight(self.line_height),
            CharAttrKind::LetterSpacing => CharAttr::LetterSpacing(self.letter_spacing),
            CharAttrKind::WordSpacing => CharAttr::WordSpacing(self.word_spacing),
            CharAttrKind::Underline => CharAttr::Underline(self.underline),
            CharAttrKind::Strikethrough => CharAttr::Strikethrough(self.strikethrough),
            CharAttrKind::Case => CharAttr::Case(self.case),
            CharAttrKind::BaselineShift => CharAttr::BaselineShift(self.baseline_shift),
            CharAttrKind::Color => CharAttr::Color(self.color),
        }
    }

    /// Overwrite one attribute, canonicalizing whatever needs it.
    ///
    /// **The only way a value reaches a style**, so the rounding that lets spans
    /// coalesce by exact equality ([`Length::canonical`]) and the tag sort that
    /// keeps files byte-stable cannot be skipped by a caller.
    pub fn set(&mut self, attr: CharAttr) {
        match attr {
            CharAttr::Family(v) => self.font_family = v,
            CharAttr::Size(v) => self.font_size = v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE),
            CharAttr::Weight(v) => self.weight = v.clamp(1, 1000),
            CharAttr::Italic(v) => self.italic = v,
            CharAttr::Variations(mut v) => {
                v.sort_by_key(|a| a.tag);
                v.dedup_by_key(|a| a.tag);
                self.variations = v
                    .into_iter()
                    .map(|a| AxisSetting::new(a.tag, a.value))
                    .collect();
            }
            CharAttr::Features(mut v) => {
                v.sort_by_key(|f| f.tag);
                v.dedup_by_key(|f| f.tag);
                self.features = v;
            }
            CharAttr::Locale(v) => self.locale = v.filter(|s| !s.is_empty()),
            CharAttr::LineHeight(v) => self.line_height = v.map(Length::canonical),
            CharAttr::LetterSpacing(v) => self.letter_spacing = v.canonical(),
            CharAttr::WordSpacing(v) => self.word_spacing = v.canonical(),
            CharAttr::Underline(v) => self.underline = v.map(canonical_decoration),
            CharAttr::Strikethrough(v) => self.strikethrough = v.map(canonical_decoration),
            CharAttr::Case(v) => self.case = v,
            CharAttr::BaselineShift(v) => self.baseline_shift = v.canonical(),
            CharAttr::Color(v) => self.color = v.map(canonical_color),
        }
    }

    /// The same style with one attribute replaced.
    pub fn with(&self, attr: CharAttr) -> Self {
        let mut next = self.clone();
        next.set(attr);
        next
    }

    /// The photographic scale tool's pass over the **node default** scope —
    /// [`ParagraphStyle::scaled`]'s twin, and the one that was missing (§15 D664,
    /// `[S6.1-L3-07]`).
    ///
    /// 🚨 **Core owned this rule for the span half and published nothing for the
    /// node half, so `ondin-app` re-wrote it.** `CharAttr::scaled_by` already
    /// encodes all six decisions — `Size` clamped to
    /// [`MIN_FONT_SIZE`]`..=`[`MAX_FONT_SIZE`], the four lengths through
    /// [`Length::scaled`], both decorations through [`scale_decoration`] — and
    /// `tools::scaled_text_scopes` re-stated every one of them in a `TextStyle`
    /// literal, above a **byte-identical private twin** of `scale_decoration`.
    /// The two agreed; nothing made them agree tomorrow, and the asymmetry was
    /// the tell: `ParagraphStyle::scaled` existed for the paragraph scope and its
    /// caller sat one line below the copy.
    ///
    /// 🚨 **Written as a field literal and *not* as a loop through
    /// [`Self::set`], which is the obvious spelling and is wrong here.** `set`
    /// canonicalizes — [`Length::canonical`] rounds, `canonical_decoration`
    /// floors a thickness at zero — and [`Spans::scaled`] does **not**: it maps
    /// `scaled_by` over the span attrs and hands them to `from_parts` raw. A
    /// canonicalized default compared against an un-canonicalized span is a
    /// different comparison from the one `normalize` makes today, so a span that
    /// equalled the default before the scale could stop equalling it after and be
    /// **stored** where it used to vanish. The two scopes have to be scaled the
    /// same way or the normalization between them stops meaning what it says.
    ///
    /// What keeps this and [`CharAttr::scaled_by`] in step is therefore a test
    /// rather than the type system — `typography::tests::
    /// a_scaled_style_agrees_with_the_span_rule_attribute_by_attribute` — and it
    /// is the reason [`scale_decoration`] is called from both rather than copied.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        TextStyle {
            font_size: (self.font_size * factor).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE),
            line_height: self.line_height.map(|l| l.scaled(factor)),
            letter_spacing: self.letter_spacing.scaled(factor),
            word_spacing: self.word_spacing.scaled(factor),
            baseline_shift: self.baseline_shift.scaled(factor),
            underline: self.underline.map(|d| scale_decoration(d, factor)),
            strikethrough: self.strikethrough.map(|d| scale_decoration(d, factor)),
            ..self.clone()
        }
    }

    /// Whether `opsz` is being tracked from the font size rather than typed.
    ///
    /// ⚠️ **The rule this predicate serves lives in `text::axis_settings`, and
    /// there used to be a second copy of it here** (§15 D571). `[S6.1-L3-04]`:
    /// `TextStyle::optical_size` computed the same value, clamped it to the axis
    /// range where `axis_settings` passes a stored coordinate through raw, carried
    /// the only test of the Manual arm anywhere — and had **zero production
    /// callers**, so the tested copy was not the one that ran. `dead_code` cannot
    /// say so: a `pub` item in a library crate is API whatever calls it.
    ///
    /// It is deleted. This predicate is the half `axis_settings` actually asks, and
    /// keeping it here rather than beside the synthesis is deliberate — it is a
    /// question about the *style*, and the panel asks it too.
    pub fn optical_size_is_auto(&self) -> bool {
        !self.variations.iter().any(|a| a.tag == OPSZ)
    }
}

/// A decoration rounded to what the file and the controls can express, and with
/// its **thickness floored at zero**.
///
/// ⚠️ **The offset is signed and the thickness is not, and they used to share a
/// range** (§15 D546). `[S6.3-L1-02]`: the panel's `optional_length_field` serves
/// both fields and was handing thickness the *offset's* symmetric range, so
/// `−5 px` was accepted, shown, and saved. **One value then rendered three
/// ways** — `text::decoration_ink` floors the resolved size with `.max(0.0)` and
/// drew nothing, `export::svg::run_attrs` has no counterpart and wrote
/// `text-decoration-thickness="-5px"` verbatim, and re-importing that file gave
/// a third answer, because `svg_in` reads no decoration thickness at all and the
/// line came back at the face's own weight. No warning on any of them.
///
/// **The clamp is here rather than only at the panel, and that is the point.**
/// `TextStyle::set`'s `Size` and `Weight` arms clamp two lines above this for the
/// same reason — the model outlives the control, and a range the model does not
/// enforce is a range a hand-edited file, an MCP client or a future writer
/// ignores. Flooring at the panel alone would have left every already-saved file
/// carrying a negative thickness.
///
/// **Zero is kept rather than turned into `None`**, because they are different
/// answers: `None` is *"the font decides"* and draws the face's own weight, while
/// `Some(0.0)` is *"no line"* — which is what a negative was already drawing on
/// the canvas, so this makes the file say what the screen already showed.
fn canonical_decoration(mut d: Decoration) -> Decoration {
    d.thickness = d.thickness.map(Length::canonical).map(|t| match t {
        Length::Px(v) => Length::Px(v.max(0.0)),
        Length::Em(v) => Length::Em(v.max(0.0)),
    });
    d.offset = d.offset.map(Length::canonical);
    d.color = d.color.map(canonical_color);
    d
}

/// A colour rounded to what the file and the controls can actually express, so
/// that equality is exact and spans coalesce.
///
/// **`Color` is `[f32; 4]`, so equality is float equality** — this is
/// [`Length::canonical`]'s lesson in another type. The same colour typed as hex and
/// dragged off the picker's plane can differ in the last bit, and two spans that
/// differ in the last bit never merge: the list grows, and every save churns a diff
/// that says nothing.
///
/// **8 bits per channel, alpha included.** The hex field is 8-bit and the opacity
/// field is whole percents, so nothing the UI can express is lost; `from_rgba8` is
/// the round-trip both of those already go through. Quantizing alpha to 1/255 is a
/// real consequence and the deliberate one — a stored alpha finer than the control
/// that set it is a value the user cannot reproduce or clear.
///
/// This is also what `Decoration::color` was missing: it canonicalized its two
/// `Length`s from the start and left its colour raw.
fn canonical_color(c: Color) -> Color {
    let rgba = c.to_rgba8();
    Color::from_rgba8(rgba.r, rgba.g, rgba.b, rgba.a)
}

// ---------------------------------------------------------------------------
// Spans
// ---------------------------------------------------------------------------

/// One attribute of a scope whose values may be overridden over a byte range.
///
/// **Implemented twice, so [`Spans`] is written once**: by [`CharAttr`] over
/// [`TextStyle`] for the character scope, and by [`ParaAttr`] over
/// [`ParagraphStyle`] for the paragraph one. Every invariant in `Spans`'
/// documentation — the coalescing, the boundary rule, the byte-stable sort —
/// is a property of the *list* rather than of the values in it, and a second
/// copy of that list is how the two scopes would come to disagree about the
/// question every rich-text bug lives in.
///
/// The methods are deliberately **not** named `kind`/`get`/`set`/`scaled`. The
/// concrete types keep inherent methods under those names and the app calls them
/// from everywhere; a trait method shadowing one would be a way to change what a
/// call site means without changing what it says.
pub trait SpanAttr: Clone + PartialEq {
    /// The set of defaults a span of this attribute overrides.
    type Defaults: Clone;
    /// This attribute's identity without its value.
    ///
    /// `Ord` because the variant order *is* the `(attribute, start)` sort a
    /// saved file is written in (§5.11, invariant 9) — which is why a new
    /// variant is appended rather than inserted.
    type Kind: Copy + Eq + Ord;

    fn attr_kind(&self) -> Self::Kind;
    /// The defaults' own value for one attribute — the value a span would be
    /// invisible against, and therefore the one that is never stored.
    fn from_defaults(defaults: &Self::Defaults, kind: Self::Kind) -> Self;
    /// Write this value into a set of defaults, canonicalizing exactly as that
    /// scope's own setter does. Span coalescing is exact equality, so a value
    /// that reached the list by another road never merges with one that came
    /// through here.
    fn set_on(self, defaults: &mut Self::Defaults);
    /// This value under the photographic scale tool (§5.6): absolute lengths
    /// move, everything unitless stays.
    fn scaled_by(self, factor: f64) -> Self;
}

/// One attribute override over one byte range of the content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span<A> {
    pub start: usize,
    pub end: usize,
    pub attr: A,
}

/// One character attribute over one byte range.
pub type CharSpan = Span<CharAttr>;
/// One paragraph attribute over one byte range.
pub type ParaSpan = Span<ParaAttr>;

impl<A> Span<A> {
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    fn contains(&self, at: usize) -> bool {
        at >= self.start && at < self.end
    }
}

/// A text node's per-range overrides of one scope's defaults.
///
/// **Invariants, all established by [`Spans::normalize`] and relied on
/// everywhere else:**
///
/// 1. no span is empty;
/// 2. two spans of the same [`SpanAttr::Kind`] never overlap or touch while
///    carrying equal values (they are merged);
/// 3. no span carries the node default (it would be invisible and would grow
///    the file);
/// 4. the list is sorted by `(kind, start)` — the byte-stable order §5.11 wants.
///
/// (2) and (3) are what make "does this range agree?" a cheap question and what
/// stops a drag on a field from fragmenting the list into one span per frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spans<A>(Vec<Span<A>>);

/// A text node's per-range character overrides.
pub type CharSpans = Spans<CharAttr>;

/// A text node's per-**paragraph** overrides — the indents and the spacing, and
/// deliberately nothing else (§15 D77).
///
/// Keyed by byte range like [`CharSpans`], not by paragraph index, because the
/// question "where is this override now?" after an edit is the same question in
/// both scopes and [`Spans::edited`] is the one answer. A paragraph reads the
/// value in force at its **first byte** (`text::Paragraphs`).
pub type ParaSpans = Spans<ParaAttr>;

// Hand-written rather than derived: `#[derive(Default)]` on a generic newtype
// asks for `A: Default`, and an *attribute* has no default — only the scope's
// defaults do.
impl<A> Default for Spans<A> {
    fn default() -> Self {
        Spans(Vec::new())
    }
}

/// One attribute value put through its own scope's setter and read back (§15 D443).
///
/// **This is the only way to canonicalize an attribute without a per-variant
/// list**, and it needs no maintenance: [`SpanAttr::set_on`] is documented as
/// canonicalizing *"exactly as that scope's own setter does"* — because it **is**
/// that setter — and [`SpanAttr::from_defaults`] reads the stored value back out.
/// A variant added to `CharAttr` is covered the moment its `set` arm is written.
///
/// ⚠️ **`set_on`'s own doc predicted the bug this closes**: *"Span coalescing is
/// exact equality, so a value that reached the list by another road never merges
/// with one that came through here."* Deserialization is that other road —
/// `Spans<A>` is `#[derive(Deserialize)]` over the raw vector — so until
/// 2026-09-06 a hand-edited `"letter_spacing":{"Em":0.020000000000000004}`
/// survived the load intact, and a user who then set `Em(0.02)` over part of the
/// run got two spans that could never merge — breaking [`Spans`]'s own invariant
/// (2), so every save wrote two spans where the document has one value.
///
/// ⚠️ **Not the *Mixed* bug, though the review filed it as one.** `[S6.1-L3-05]`
/// states the cost as `shared_in` answering `None`. It does not: `values_in`
/// reads each cut through [`Spans::resolve`], which applies `set_on`, **and
/// `set_on` canonicalizes** — so the read path was always immune and the panel
/// never said *Mixed*. The loss is storage, which is smaller and still real.
/// Measured, and the measurement is in
/// `a_hand_edited_span_value_is_canonicalized_on_the_way_in`, whose `shared_in`
/// assertion is kept green under the flip precisely to hold this correction down.
///
/// **The residue, stated so it is not assumed away:** this canonicalizes the
/// *spans*, against whatever defaults it is handed. A non-canonical value in the
/// **defaults themselves** is untouched — closing that needs a list of every
/// `Kind`, which is the hand-maintained enumeration this function exists to
/// avoid — so a span whose canonical value equals such a default fails to elide.
/// Narrower than what is fixed here, and it cannot produce the *Mixed* symptom,
/// because that comes from two spans rather than from a span and a default.
fn canonical_attr<A: SpanAttr>(attr: A, defaults: &A::Defaults) -> A {
    let kind = attr.attr_kind();
    let mut scope = defaults.clone();
    attr.set_on(&mut scope);
    A::from_defaults(&scope, kind)
}

impl<A: SpanAttr> Spans<A> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[Span<A>] {
        &self.0
    }

    /// Rebuild from loose parts, normalizing — so a hand-edited file cannot
    /// install overlapping spans.
    ///
    /// ⚠️ **Not IO's way in**, which this said until 2026-09-06. Serde's derived
    /// impl on the newtype builds the `Vec` directly and never calls a
    /// constructor; the load path reaches [`Self::normalize`] through
    /// [`Self::clamped`] instead. The invariants are re-established either way —
    /// it is the sentence about *who calls this* that was wrong, and it mattered
    /// because it made the loader look guarded at a function the loader never
    /// touches.
    pub fn from_parts(spans: Vec<Span<A>>, default: &A::Defaults) -> Self {
        let mut s = Spans(spans);
        s.normalize(default);
        s
    }

    /// The defaults as they stand at byte `at`, every override applied.
    pub fn resolve(&self, default: &A::Defaults, at: usize) -> A::Defaults {
        let mut style = default.clone();
        for span in &self.0 {
            if span.contains(at) {
                span.attr.clone().set_on(&mut style);
            }
        }
        style
    }

    /// The content split into maximal runs that share every attribute, with the
    /// style each carries.
    ///
    /// The form the layout translation and the decoration walk both want: one
    /// entry per run, in content order, covering `0..len` exactly. `len` of zero
    /// still yields one empty run, because an empty text node still has a font
    /// size and a line height to be measured by.
    pub fn runs(&self, default: &A::Defaults, len: usize) -> Vec<(Range<usize>, A::Defaults)> {
        // Every span edge is a potential run boundary; nothing else can be.
        let mut cuts: Vec<usize> = Vec::with_capacity(self.0.len() * 2 + 2);
        cuts.push(0);
        cuts.push(len);
        for span in &self.0 {
            if span.start > 0 && span.start < len {
                cuts.push(span.start);
            }
            if span.end > 0 && span.end < len {
                cuts.push(span.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        if len == 0 {
            return vec![(0..0, default.clone())];
        }
        cuts.windows(2)
            .map(|w| (w[0]..w[1], self.resolve(default, w[0])))
            .collect()
    }

    /// Every distinct value of one attribute over `range`, in the order they
    /// first appear.
    ///
    /// What [`Self::shared_in`] is computed from, and through it the panel's
    /// **mixed** state: one value means the control shows it, more than one means it
    /// shows *mixed*. An empty `range` (a bare caret) reports the one value at that
    /// point, which is what a caret's fields should show.
    ///
    /// ⚠️ **That first clause read *"what the panel's mixed state is computed
    /// from"* and named a caller this function does not have** (§15 D571,
    /// `[S6.1-L3-04]`). The panel asks `shared_in`; `shared_in` is this function's
    /// **only** caller in the workspace. Harmless as prose and not as a lead: it is
    /// the sentence that makes a reader looking for the mixed rule stop one function
    /// short of where the collapse to `Option` actually happens.
    pub fn values_in(&self, default: &A::Defaults, kind: A::Kind, range: Range<usize>) -> Vec<A> {
        let mut out: Vec<A> = Vec::new();
        let mut push = |v: A| {
            if !out.contains(&v) {
                out.push(v);
            }
        };
        if range.is_empty() {
            push(A::from_defaults(&self.resolve(default, range.start), kind));
            return out;
        }
        for (_, v) in self.runs_of(default, kind, range) {
            push(v);
        }
        out
    }

    /// One attribute's runs across `range`, as `(sub-range, value)` — what
    /// [`Self::values_in`] answers, without the deduplication and with the ranges
    /// kept (§15 D752).
    ///
    /// **The ranges are the point.** `values_in` says *what values are in here*,
    /// which is enough to draw a mixed readout; a caller that wants to **write**
    /// each run something different — computed from what that run already had —
    /// needs to know where each run is. Merging one OpenType tag into a selection
    /// without flattening the others is the case this was added for.
    ///
    /// ⚠️ **The cut walk lives here and `values_in` calls it**, rather than the
    /// two each walking the spans. They have to agree about where a run begins,
    /// and two copies of a boundary rule is how a readout comes to disagree with
    /// the write it is a readout of.
    ///
    /// An empty `range` yields nothing. `values_in` keeps its own answer for that
    /// case — the value resolved *at* the caret, which is a question about a point
    /// rather than about a span, and is not a run.
    pub fn runs_of(
        &self,
        default: &A::Defaults,
        kind: A::Kind,
        range: Range<usize>,
    ) -> Vec<(Range<usize>, A)> {
        if range.is_empty() {
            return Vec::new();
        }
        let mut cuts = vec![range.start, range.end];
        for span in self.0.iter().filter(|s| s.attr.attr_kind() == kind) {
            if span.start > range.start && span.start < range.end {
                cuts.push(span.start);
            }
            if span.end > range.start && span.end < range.end {
                cuts.push(span.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2)
            .map(|w| {
                (
                    w[0]..w[1],
                    A::from_defaults(&self.resolve(default, w[0]), kind),
                )
            })
            .collect()
    }

    /// The single value of one attribute over `range`, or `None` when the range
    /// disagrees with itself — the mixed state.
    pub fn shared_in(
        &self,
        default: &A::Defaults,
        kind: A::Kind,
        range: Range<usize>,
    ) -> Option<A> {
        let mut v = self.values_in(default, kind, range);
        (v.len() == 1).then(|| v.remove(0))
    }

    /// Set one attribute over `range`, replacing whatever that attribute was
    /// doing there and leaving every other attribute alone.
    ///
    /// A span equal to the node default is not stored: it would be invisible and
    /// would grow every save. That is also why `default` is a parameter rather
    /// than something the caller checks — forgetting it is how a span list turns
    /// into one entry per keystroke.
    pub fn set(&mut self, range: Range<usize>, attr: A, default: &A::Defaults) {
        if range.is_empty() {
            return;
        }
        let kind = attr.attr_kind();
        let mut kept: Vec<Span<A>> = Vec::with_capacity(self.0.len() + 2);
        for span in self.0.drain(..) {
            if span.attr.attr_kind() != kind {
                kept.push(span);
                continue;
            }
            // Whatever of this span lies outside the new range survives; a span
            // wholly inside it is replaced.
            let left = span.start..span.end.min(range.start);
            if left.start < left.end {
                kept.push(Span {
                    start: left.start,
                    end: left.end,
                    attr: span.attr.clone(),
                });
            }
            let right = span.start.max(range.end)..span.end;
            if right.start < right.end {
                kept.push(Span {
                    start: right.start,
                    end: right.end,
                    attr: span.attr,
                });
            }
        }
        // Through the scope's own setter so the value is canonicalized exactly as
        // a default would be — otherwise a typed span never equals a dragged one.
        let canonical = {
            let mut probe = default.clone();
            attr.set_on(&mut probe);
            A::from_defaults(&probe, kind)
        };
        kept.push(Span {
            start: range.start,
            end: range.end,
            attr: canonical,
        });
        self.0 = kept;
        self.normalize(default);
    }

    /// Drop every override of one attribute over `range`, putting those bytes
    /// back on the node default.
    pub fn clear(&mut self, range: Range<usize>, kind: A::Kind, default: &A::Defaults) {
        if range.is_empty() {
            return;
        }
        let sentinel = A::from_defaults(default, kind);
        self.set(range, sentinel, default);
    }

    /// Re-base the spans after `at..at + removed` bytes of content became
    /// `inserted` bytes.
    ///
    /// **This is the boundary rule** (see the module docs): a span ending exactly
    /// at the edit point grows over the inserted text, one starting there does
    /// not — typing inherits from the character to the left.
    pub fn edited(
        &self,
        at: usize,
        removed: usize,
        inserted: usize,
        default: &A::Defaults,
    ) -> Self {
        let end_of_removal = at + removed;
        // Only ever applied to a position at or past the end of the removal, so
        // the subtraction cannot underflow.
        let shift = |p: usize| p - removed + inserted;
        // Two mappings, because the two ends of a span answer the boundary
        // question differently.
        let map_start = |p: usize| {
            if p < at {
                p
            } else if p >= end_of_removal {
                shift(p)
            } else {
                at
            }
        };
        let map_end = |p: usize| {
            if p < at {
                p
            } else if p == at {
                at + inserted
            } else if p >= end_of_removal {
                shift(p)
            } else {
                at + inserted
            }
        };
        let spans = self
            .0
            .iter()
            .map(|s| Span {
                start: map_start(s.start),
                end: map_end(s.end),
                attr: s.attr.clone(),
            })
            .collect();
        Self::from_parts(spans, default)
    }

    /// Re-establish every invariant in the type's doc comment.
    ///
    /// **Overlaps are resolved by painting, not by truncating** (§15 D643,
    /// `[S6.1-L1-06]`). Spans of one kind are laid down in sorted order and each
    /// byte takes the value of the last one covering it; a span that is painted
    /// over keeps whatever lies **outside** the shared bytes, on both sides.
    ///
    /// 🚨 **The previous rule was neither of the two things its own comment
    /// claimed.** It said *"equal values join, unequal ones let the later span
    /// keep the shared bytes (it is the one a caller set most recently)"* — but
    /// the list has just been sorted, so *later* meant "starts further right",
    /// which has nothing to do with recency; and for two spans sharing a start
    /// the sort was stable, so the winner was decided by **the order the file's
    /// array happened to be in**. Measured, on a stock `TextStyle`:
    ///
    /// | file order | result | bytes 5..10 |
    /// | --- | --- | --- |
    /// | `[{0..10, W700}, {0..5, W300}]` | `[{0..5, W300}]` | lost their 700 |
    /// | `[{0..5, W300}, {0..10, W700}]` | `[{0..10, W700}]` | the 300 is gone |
    ///
    /// Two documents from one set. The truncating arm set `prev.end =
    /// span.start` and then popped `prev` when that emptied it, which discards
    /// `prev`'s whole range **including the part beyond `span.end`** — bytes
    /// nothing overlapped.
    ///
    /// ⚠️ **`Reverse(end)` in the sort key is what makes the answer
    /// order-independent**, and it is the only reason two spans with the same
    /// start have a defined winner: widest first, so the narrower one paints over
    /// it and keeps the shared bytes. Two spans with an identical *range* and
    /// different values are still decided by array order, because nothing here
    /// compares values — that case has no defensible answer and is not one this
    /// crate can produce.
    ///
    /// ⚠️ **The sort is still `(kind, start)` as far as §5.11's byte-stable save
    /// is concerned.** After this runs, one kind's spans do not overlap, so no
    /// two share a start and the third key never breaks a tie in the output.
    ///
    /// **Reachability is IO only, and that is the honest bound.** `set` splits
    /// the old span around the new range, `edited`'s maps are monotonic, and
    /// `clamped`/`restated`/`scaled` preserve ranges — so a containment comes
    /// from a hand-edited or foreign-written file, which `schema.rs` chooses to
    /// normalize rather than reject.
    fn normalize(&mut self, default: &A::Defaults) {
        self.0
            .retain(|s| s.start < s.end && s.attr != A::from_defaults(default, s.attr.attr_kind()));
        self.0
            .sort_by_key(|s| (s.attr.attr_kind(), s.start, std::cmp::Reverse(s.end)));
        // The runs `span` paints over are always a **suffix** of `merged`: the
        // list is non-overlapping and sorted, and `span.start` is at least every
        // start already laid down. At most one of them can begin before
        // `span.start` (it is the one containing that byte) and at most one can
        // end after `span.end` (the last, which has the greatest end) — so the
        // whole repair is one head, one tail, and everything between them gone.
        let mut merged: Vec<Span<A>> = Vec::with_capacity(self.0.len());
        for span in std::mem::take(&mut self.0) {
            let kind = span.attr.attr_kind();
            let (mut head, mut tail) = (None, None);
            while let Some(prev) = merged.last() {
                if prev.attr.attr_kind() != kind || prev.end <= span.start {
                    break;
                }
                let prev = merged.pop().expect("last() just answered Some");
                if prev.end > span.end && tail.is_none() {
                    tail = Some(Span {
                        start: span.end,
                        end: prev.end,
                        attr: prev.attr.clone(),
                    });
                }
                if prev.start < span.start {
                    head = Some(Span {
                        start: prev.start,
                        end: span.start,
                        attr: prev.attr,
                    });
                    break;
                }
            }
            merged.extend(head);
            merged.push(span);
            merged.extend(tail);
        }
        // Touching or overlapping runs of one kind carrying equal values are one
        // span. This used to be an arm of the loop above; painting can produce
        // the same situation by splitting a run around one of equal value, so it
        // is a pass of its own now.
        let mut out: Vec<Span<A>> = Vec::with_capacity(merged.len());
        for span in merged {
            match out.last_mut() {
                Some(prev) if prev.attr == span.attr && prev.end >= span.start => {
                    prev.end = prev.end.max(span.end);
                }
                _ => out.push(span),
            }
        }
        out.retain(|s| s.start < s.end);
        self.0 = out;
    }

    /// This list re-normalized against `default` — what a change to the
    /// **defaults** owes the overrides.
    ///
    /// A span equal to the *old* default was never stored, so a default that moves
    /// would silently restyle those bytes too. Re-stating is what keeps "this run is
    /// 24pt" true when the node's own size changes underneath it, and it drops the
    /// spans that were only echoing the default.
    ///
    /// **A method rather than three lines at each caller**, because there are now
    /// four of them in two crates: `Document`'s two style ops, and the render
    /// *preview* of each. The preview used to leave the list alone — invisible in the
    /// drawing, since what this drops is by definition an echo of the new defaults,
    /// and visible to the Type panel, which reads the override's kind back through
    /// `display_node` (§15 D163).
    pub fn restated(&self, default: &A::Defaults) -> Self {
        Self::from_parts(self.as_slice().to_vec(), default)
    }

    /// Clamp every span into `content` **and onto its character boundaries**
    /// (§15 D422) — what the loader owes a file whose spans outrun its text, and
    /// what `SetText` owes content it did not author.
    ///
    /// ⚠️ **The boundary half is the one that was missing, and without it a file
    /// crashed the app at every launch.** A `.ondin` with `content: "aéb"` and a
    /// span of `0..2` — byte 2 is *inside* `é`, which occupies bytes 1..3 —
    /// loaded with no complaint and then panicked on every render:
    /// `text::cased_text` slices `content[range]` directly, and one line later
    /// parley's style builder does the same. Both are `ondin-core`'s own runs,
    /// so the early return for `TextCase::Original` is not a guard; it only
    /// moves which of the two fires.
    ///
    /// **The blast radius is the dashboard.** `library::cover::rasterize` loads
    /// and rebuilds every document the grid draws, synchronously inside the egui
    /// pass and with no `catch_unwind` anywhere on the path — so one such file
    /// in the base folder crashes the app at every launch, and `Cover::Failed`
    /// never records it because a panic does not get that far.
    ///
    /// **Snapped outward rather than dropped**, which keeps §5.11's stated
    /// posture — *"a stale range is a hand-edit or an older writer's rounding,
    /// not a corrupt tree"* — and keeps the styling the file asked for over the
    /// whole character it half-named. A span that collapses is then dropped by
    /// [`Self::normalize`], which already retains only `start < end`.
    ///
    /// ⚠️ **Takes the content, not its length, and that is the fix rather than a
    /// convenience.** The magnitude clamp cannot *create* an off-boundary index
    /// — `len` is always a boundary — but it could not repair one either, and
    /// this function is the *only* correct site: `op_set_text` and
    /// `op_set_text_style` call it too, so a repair placed at the loader alone
    /// would have left the live-editing route open.
    pub fn clamped(&self, content: &str, default: &A::Defaults) -> Self {
        let len = content.len();
        // `is_char_boundary` is true at 0 and at `len`, so both walks terminate.
        let down = |mut i: usize| {
            while !content.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let up = |mut i: usize| {
            while !content.is_char_boundary(i) {
                i += 1;
            }
            i
        };
        let spans = self
            .0
            .iter()
            .map(|s| Span {
                start: down(s.start.min(len)),
                end: up(s.end.min(len)),
                attr: canonical_attr(s.attr.clone(), default),
            })
            .collect();
        Self::from_parts(spans, default)
    }

    /// Multiply every absolute length by `factor` — the photographic scale
    /// tool's pass over the spans, mirroring what it does to the defaults.
    pub fn scaled(&self, factor: f64, default: &A::Defaults) -> Self {
        let spans = self
            .0
            .iter()
            .map(|s| Span {
                start: s.start,
                end: s.end,
                attr: s.attr.clone().scaled_by(factor),
            })
            .collect();
        Self::from_parts(spans, default)
    }
}

impl SpanAttr for CharAttr {
    type Defaults = TextStyle;
    type Kind = CharAttrKind;

    fn attr_kind(&self) -> CharAttrKind {
        self.kind()
    }

    fn from_defaults(defaults: &TextStyle, kind: CharAttrKind) -> Self {
        defaults.get(kind)
    }

    fn set_on(self, defaults: &mut TextStyle) {
        defaults.set(self);
    }

    /// Sizes and absolute lengths move; everything else is unitless and stays.
    fn scaled_by(self, factor: f64) -> Self {
        match self {
            CharAttr::Size(v) => CharAttr::Size((v * factor).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)),
            CharAttr::LineHeight(v) => CharAttr::LineHeight(v.map(|l| l.scaled(factor))),
            CharAttr::LetterSpacing(v) => CharAttr::LetterSpacing(v.scaled(factor)),
            CharAttr::WordSpacing(v) => CharAttr::WordSpacing(v.scaled(factor)),
            CharAttr::BaselineShift(v) => CharAttr::BaselineShift(v.scaled(factor)),
            CharAttr::Underline(d) => CharAttr::Underline(d.map(|d| scale_decoration(d, factor))),
            CharAttr::Strikethrough(d) => {
                CharAttr::Strikethrough(d.map(|d| scale_decoration(d, factor)))
            }
            other => other,
        }
    }
}

fn scale_decoration(mut d: Decoration, factor: f64) -> Decoration {
    d.thickness = d.thickness.map(|l| l.scaled(factor));
    d.offset = d.offset.map(|l| l.scaled(factor));
    d
}

// ---------------------------------------------------------------------------
// Paragraph attributes
// ---------------------------------------------------------------------------

/// Horizontal alignment of a line within its box.
///
/// `Start`/`End` are preferred over `Left`/`Right` in new documents — parley's
/// own docs recommend them for direction-aware text, and they are what makes an
/// RTL paragraph align correctly without a second field. The physical pair is
/// kept because every file written before this existed holds one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    /// Left in LTR text, right in RTL.
    #[default]
    Start,
    /// Right in LTR text, left in RTL.
    End,
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlign {
    /// What the panel offers, in the order it offers them. `Left`/`Right` are
    /// deliberately absent: the four cells are start, centre, end, justify, and
    /// the physical pair exists only to load old files.
    pub const OFFERED: [TextAlign; 4] = [
        TextAlign::Start,
        TextAlign::Center,
        TextAlign::End,
        TextAlign::Justify,
    ];

    /// Which of the four offered cells this alignment lights, mapping the
    /// physical pair onto the logical one so an old document's panel is not
    /// blank.
    pub fn cell(self) -> usize {
        match self {
            TextAlign::Start | TextAlign::Left => 0,
            TextAlign::Center => 1,
            TextAlign::End | TextAlign::Right => 2,
            TextAlign::Justify => 3,
        }
    }
}

/// Where the *last* line of a justified paragraph goes.
///
/// `Start` is parley's own `Justify` — it leaves the last line unstretched and
/// start-aligned. The other two are ours: the line is not re-spaced, only
/// translated, which is why they are affordable where "justify all" is not (that
/// would be reimplementing the part of justification parley owns).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JustifyLast {
    #[default]
    Start,
    Center,
    End,
}

impl JustifyLast {
    pub const ALL: [JustifyLast; 3] = [JustifyLast::Start, JustifyLast::Center, JustifyLast::End];

    pub fn label(self) -> &'static str {
        match self {
            JustifyLast::Start => "Start",
            JustifyLast::Center => "Center",
            JustifyLast::End => "End",
        }
    }
}

/// Base direction of the text. `Auto` takes it from the content's own bidi
/// classes, which is what parley does when nothing overrides it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}

impl TextDirection {
    pub const ALL: [TextDirection; 3] =
        [TextDirection::Auto, TextDirection::Ltr, TextDirection::Rtl];

    pub fn label(self) -> &'static str {
        match self {
            TextDirection::Auto => "Auto",
            TextDirection::Ltr => "LTR",
            TextDirection::Rtl => "RTL",
        }
    }
}

/// Whether lines wrap at all — parley's `TextWrapMode`, mirrored so it can be
/// serialized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WrapMode {
    #[default]
    Wrap,
    NoWrap,
}

impl WrapMode {
    /// Off before on, as every other segmented track in the popup reads: the
    /// cell that does nothing sits on the left.
    pub const ALL: [WrapMode; 2] = [WrapMode::NoWrap, WrapMode::Wrap];

    pub fn label(self) -> &'static str {
        match self {
            WrapMode::Wrap => "Wrap",
            WrapMode::NoWrap => "No wrap",
        }
    }
}

/// Where a line may break — parley's `WordBreak`, mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
}

impl WordBreak {
    pub const ALL: [WordBreak; 3] = [WordBreak::Normal, WordBreak::BreakAll, WordBreak::KeepAll];

    /// **"By script", not "Normal".** All three cells are normal for *some*
    /// writing system, and what `Normal` actually means is "use the line-breaking
    /// rules of whatever script this is" — which is the one thing a designer
    /// needs to know to choose between it and the two overrides beside it.
    pub fn label(self) -> &'static str {
        match self {
            WordBreak::Normal => "By script",
            WordBreak::BreakAll => "Break all",
            WordBreak::KeepAll => "Keep all",
        }
    }
}

/// Emergency breaking for a word wider than its box — parley's `OverflowWrap`,
/// mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverflowWrap {
    #[default]
    Normal,
    Anywhere,
    BreakWord,
}

impl OverflowWrap {
    /// Ordered by how much damage the cell permits: never break the word, break
    /// it only when there is no other way, break it anywhere. Declaration order
    /// puts `Anywhere` in the middle, which reads as a scale that goes back on
    /// itself.
    pub const ALL: [OverflowWrap; 3] = [
        OverflowWrap::Normal,
        OverflowWrap::BreakWord,
        OverflowWrap::Anywhere,
    ];

    /// **"Never", not "Normal".** This control only ever answers one question —
    /// may a word too wide for its box be cut? — so the resting cell says no
    /// rather than naming itself.
    pub fn label(self) -> &'static str {
        match self {
            OverflowWrap::Normal => "Never",
            OverflowWrap::Anywhere => "Anywhere",
            OverflowWrap::BreakWord => "Break word",
        }
    }
}

/// What a list item draws in its gutter.
///
/// **The core of CSS's `list-style-type`, and deliberately not a free string.** A
/// custom marker is a `String` on what is otherwise a `Copy` attribute, and it
/// raises a second question this does not have to answer — whether a custom marker
/// counts, and what its ordinal would mean — so it is left out rather than
/// half-built.
///
/// The three unordered glyphs were checked against the bundled Inter before being
/// offered: U+2022, U+25E6 and U+25AA all resolve (and to the same advance, so
/// switching between them moves no text), while the tempting U+25FE and U+2219 come
/// back as `.notdef`. *A marker set is a claim about the font, not about CSS.*
///
/// **Nesting does not rotate the glyph**, the way HTML's default stylesheet turns a
/// disc into a circle and then a square one `<ul>` down. There it is a default over
/// markup that names no marker of its own; here every paragraph stores the marker it
/// asked for, so deriving one from [`ParagraphStyle::level`] would draw something
/// other than the value in the model — and that value was picked from a list of
/// eight. Wanting hollow bullets for an inner list is two clicks, not a rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListMarker {
    /// `•`
    #[default]
    Disc,
    /// `◦`
    Circle,
    /// `▪`
    Square,
    /// `1.`
    Decimal,
    /// `a.`
    LowerAlpha,
    /// `A.`
    UpperAlpha,
    /// `i.`
    LowerRoman,
    /// `I.`
    UpperRoman,
}

impl ListMarker {
    pub const ALL: [ListMarker; 8] = [
        ListMarker::Disc,
        ListMarker::Circle,
        ListMarker::Square,
        ListMarker::Decimal,
        ListMarker::LowerAlpha,
        ListMarker::UpperAlpha,
        ListMarker::LowerRoman,
        ListMarker::UpperRoman,
    ];

    /// Whether this marker counts, which is also **what starts and ends a list**:
    /// consecutive paragraphs agree about a list only if they agree about this.
    ///
    /// A run of bullets interrupted by one numbered item is two lists and a third,
    /// and the numbering restarts — the same rule CSS gets from the markup, taken
    /// here from the one thing the model has instead of markup.
    pub fn ordered(self) -> bool {
        !matches!(
            self,
            ListMarker::Disc | ListMarker::Circle | ListMarker::Square
        )
    }

    /// The label for the control that picks this.
    pub fn label(self) -> &'static str {
        match self {
            ListMarker::Disc => "Disc",
            ListMarker::Circle => "Circle",
            ListMarker::Square => "Square",
            ListMarker::Decimal => "1, 2, 3",
            ListMarker::LowerAlpha => "a, b, c",
            ListMarker::UpperAlpha => "A, B, C",
            ListMarker::LowerRoman => "i, ii, iii",
            ListMarker::UpperRoman => "I, II, III",
        }
    }

    /// The text this marker draws for the `n`th item of its list, counting from 1.
    ///
    /// **The counter systems fall back to decimal rather than to nothing** where
    /// they run out — roman past 3999, and either past a zero — because a marker
    /// that vanishes reads as a broken list where an out-of-system number reads as
    /// a long one. CSS says the same for `upper-roman`.
    pub fn text(self, n: usize) -> String {
        match self {
            ListMarker::Disc => "\u{2022}".into(),
            ListMarker::Circle => "\u{25E6}".into(),
            ListMarker::Square => "\u{25AA}".into(),
            ListMarker::Decimal => format!("{n}."),
            ListMarker::LowerAlpha => format!("{}.", alphabetic(n, false)),
            ListMarker::UpperAlpha => format!("{}.", alphabetic(n, true)),
            ListMarker::LowerRoman => format!("{}.", roman(n, false)),
            ListMarker::UpperRoman => format!("{}.", roman(n, true)),
        }
    }
}

/// CSS's `lower-alpha`: **bijective** base 26, so 26 is `z` and 27 is `aa`.
///
/// Not plain base 26, which has no digit for zero to sit in and would give `a` for
/// both 1 and 27. The `n - 1` before each division is the whole difference.
fn alphabetic(n: usize, upper: bool) -> String {
    if n == 0 {
        return "0".into();
    }
    let base = if upper { b'A' } else { b'a' };
    let mut out = Vec::new();
    let mut n = n;
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push(base + rem as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

/// CSS's `lower-roman`, decimal outside 1..=3999.
fn roman(n: usize, upper: bool) -> String {
    const VALUES: [(usize, &str, &str); 13] = [
        (1000, "M", "m"),
        (900, "CM", "cm"),
        (500, "D", "d"),
        (400, "CD", "cd"),
        (100, "C", "c"),
        (90, "XC", "xc"),
        (50, "L", "l"),
        (40, "XL", "xl"),
        (10, "X", "x"),
        (9, "IX", "ix"),
        (5, "V", "v"),
        (4, "IV", "iv"),
        (1, "I", "i"),
    ];
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    let mut out = String::new();
    let mut n = n;
    for (value, up, low) in VALUES {
        while n >= value {
            out.push_str(if upper { up } else { low });
            n -= value;
        }
    }
    out
}

/// A text node's paragraph attributes — the value every paragraph carries unless
/// a [`ParaSpans`] entry says otherwise.
///
/// **Only some of these can be overridden per paragraph, and that split is the
/// design rather than an unfinished job** (§15 D77). parley lays the whole node
/// out as one `Layout`, and its per-line controls (`line_x`, `line_max_advance`)
/// can vary a paragraph's *measure* — the fields [`ParaAttr`] names. What is
/// genuinely one value per layout is the `Alignment` handed to `Layout::align`
/// (and with it `Justify`, which adjusts cluster advances) and the bidi base
/// direction; a span carrying either would be a value the renderer had to ignore,
/// which is worse than not being able to say it, because then the model lies
/// about what is on screen. Figma does not offer per-paragraph alignment either.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParagraphStyle {
    #[serde(default, skip_serializing_if = "is_default")]
    pub align: TextAlign,
    /// Space **before** this paragraph, ignored on the first one. Absolute by
    /// default: it belongs to the layout's spacing scale, not to the type.
    ///
    /// *Before* rather than *after* is what makes the node-level value mean
    /// exactly what it has always meant — extra space at every hard break, and
    /// none above the opening line — while leaving one paragraph free to ask for
    /// its own gap. Applied by `text::y_map`, not by parley.
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub spacing: Length,
    /// First-line indent, signed. With [`Self::hanging`] it indents the
    /// continuation lines instead.
    ///
    /// **It reaches every paragraph unless a [`ParaSpans`] entry overrides one.**
    /// CSS spells the distinction `text-indent: … each-line` and this used to
    /// carry the flag, but with it off the indent reached only the first line of
    /// the whole *layout* — type an indent into a three-paragraph node and one
    /// line moved while the other two stayed flush, which reads as a bug rather
    /// than as a setting (§15 D103). Naming a paragraph is now the model's job
    /// rather than a boolean's.
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub indent: Length,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hanging: bool,
    /// The paragraph's own start edge, measured in from the box's — a block
    /// indent, holding every line rather than only the first.
    ///
    /// On a [`crate::node::TextSizing::Auto`] node there is no box to measure
    /// from, so this is a plain x offset and the emergent width grows by it.
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub indent_start: Length,
    /// The paragraph's own end edge, measured in from the box's.
    ///
    /// **Inert on an auto-width node**, which has no wrap width for it to narrow
    /// — the honest answer rather than a number that quietly does nothing on one
    /// sizing mode and something on another.
    #[serde(default, skip_serializing_if = "Length::is_default_zero")]
    pub indent_end: Length,
    #[serde(default, skip_serializing_if = "is_default")]
    pub justify_last: JustifyLast,
    #[serde(default, skip_serializing_if = "is_default")]
    pub wrap: WrapMode,
    #[serde(default, skip_serializing_if = "is_default")]
    pub word_break: WordBreak,
    #[serde(default, skip_serializing_if = "is_default")]
    pub overflow_wrap: OverflowWrap,
    #[serde(default, skip_serializing_if = "is_default")]
    pub direction: TextDirection,
    /// The list marker this paragraph draws in its gutter, or `None` for an
    /// ordinary paragraph.
    ///
    /// **The marker is never in [`crate::node::NodeKind::Text`]'s `content`**, and
    /// that is the whole reason lists cost so little here: byte indices are the
    /// currency of the span lists, the caret, the hit test and the case transform,
    /// so a marker in the string would need every one of them to know it was not
    /// text. Ink without bytes cannot be selected, cannot be typed into, and cannot
    /// move a span.
    ///
    /// **Nor is it a parley inline box**, which was the route the plan named.
    /// `InlineBoxKind::OutOfFlow` is the only kind that reserves no space in the
    /// line — what a hanging marker wants — and parley honours the kind when it
    /// *draws* and ignores it in `Cluster::visual_offset`, `Cluster::from_point` and
    /// `Selection::geometry`. Measured: a 30px out-of-flow box at byte 0 leaves the
    /// first glyph at x 0 and puts the caret and the highlight's left edge at x 30.
    /// The gutter comes from [`Self::indent_start`] instead, which every line of the
    /// paragraph already honours — that *is* the hanging measure, and it was built
    /// in §15 D163.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<ListMarker>,
    /// How deeply nested this paragraph is: `0` is the outer level, and each level
    /// steps the start edge in by one more [`Self::indent_start`].
    ///
    /// **The step is `indent_start` rather than a number of its own**, which is the
    /// answer to the one thing nesting had to decide — what a level-2 item's gutter
    /// is measured from. It is measured from the level-1 edge, and the amount is the
    /// gutter the marker already sits in, so every level is one gutter wider than
    /// the last. CSS's default stylesheet says the same thing with one number
    /// (`padding-left: 40px` per `<ul>`, which is also where the marker goes), and a
    /// second length here would be a second number to keep in step with the first
    /// for no gain: a list whose levels step by an amount unrelated to its gutter is
    /// not what anyone means by a nested list.
    ///
    /// **It is a field and not a reading of `indent_start`.** Grouping the numbering
    /// by indent was the tempting version and is what this rejects: `indent_start` is
    /// a `Length` a user *drags*, so nudging one item's indent by a hair would
    /// silently renumber the list around it.
    ///
    /// **A level on a paragraph with no marker still indents it**, which is how a
    /// continuation paragraph sits under the item it belongs to. Making the multiplier
    /// conditional on [`Self::marker`] would store a number the renderer ignores,
    /// which §15 D77's own reasoning calls worse than not being able to say it. It
    /// does not keep that paragraph *inside* the list — a paragraph with no marker
    /// still ends every open run, exactly as it does at level 0.
    #[serde(default, skip_serializing_if = "is_default")]
    pub level: u8,
}

/// How deeply a list may nest. Past this the innermost item's start edge is eight
/// gutters in and the level has stopped describing a list; it is a bound on the
/// control and on a hand-edited file, not a layout limit.
pub const MAX_LIST_LEVEL: u8 = 8;

impl ParagraphStyle {
    /// This style's value for one spannable attribute, as a span would carry it.
    pub fn get(&self, kind: ParaAttrKind) -> ParaAttr {
        match kind {
            ParaAttrKind::Spacing => ParaAttr::Spacing(self.spacing),
            ParaAttrKind::Indent => ParaAttr::Indent(self.indent),
            ParaAttrKind::Hanging => ParaAttr::Hanging(self.hanging),
            ParaAttrKind::IndentStart => ParaAttr::IndentStart(self.indent_start),
            ParaAttrKind::IndentEnd => ParaAttr::IndentEnd(self.indent_end),
            ParaAttrKind::Marker => ParaAttr::Marker(self.marker),
            ParaAttrKind::Level => ParaAttr::Level(self.level),
        }
    }

    /// Where this paragraph's lines start, in px from the box's own start edge:
    /// [`Self::indent_start`] stepped by [`Self::level`].
    ///
    /// **One function because two readers have to agree.** The lines get it through
    /// `line_geometry`, which hands parley a `line_x`; the list marker gets it as the
    /// x its right edge lands on. Two copies of the multiplication is exactly how a
    /// marker ends up on a gutter its own text is not using — the class of bug §15
    /// D163 was, one edge derived twice.
    ///
    /// `saturating_add` because a hand-edited file can carry any `u8`, and the sum is
    /// a multiplier rather than an index — a level of 255 draws absurdly far in, which
    /// is the honest picture of what the file says, but it must not wrap to zero.
    pub fn start_edge(&self, font_size: f64) -> f64 {
        self.indent_start.resolve(font_size) * f64::from(self.level.saturating_add(1))
    }

    /// Overwrite one spannable attribute, canonicalizing its length so that a
    /// dragged value and a typed one coalesce (see [`Length::canonical`]).
    pub fn set(&mut self, attr: ParaAttr) {
        match attr {
            ParaAttr::Spacing(v) => self.spacing = v.canonical(),
            ParaAttr::Indent(v) => self.indent = v.canonical(),
            ParaAttr::Hanging(v) => self.hanging = v,
            ParaAttr::IndentStart(v) => self.indent_start = v.canonical(),
            ParaAttr::IndentEnd(v) => self.indent_end = v.canonical(),
            ParaAttr::Marker(v) => self.marker = v,
            // Clamped for the same reason the lengths are canonicalized: this is the
            // one door every edit comes through, so the bound belongs here rather
            // than at each control that writes one.
            ParaAttr::Level(v) => self.level = v.min(MAX_LIST_LEVEL),
        }
    }

    /// The same style with one attribute replaced.
    pub fn with(&self, attr: ParaAttr) -> Self {
        let mut next = self.clone();
        next.set(attr);
        next
    }

    /// The photographic scale tool's pass: the absolute lengths move, the modes
    /// do not.
    pub fn scaled(&self, factor: f64) -> Self {
        ParagraphStyle {
            spacing: self.spacing.scaled(factor),
            indent: self.indent.scaled(factor),
            indent_start: self.indent_start.scaled(factor),
            indent_end: self.indent_end.scaled(factor),
            ..self.clone()
        }
    }
}

/// One paragraph attribute, as a [`ParaSpans`] entry carries it.
///
/// **The membership of this enum is the whole design of per-paragraph attributes**
/// (§15 D77): every variant here is something one `Layout` can honour per
/// paragraph, through parley's per-line `line_x`/`line_max_advance` or through
/// `text::y_map`. Alignment, justification and direction are absent because they
/// are one value per layout, and a span holding a value the renderer must ignore
/// makes the model lie about the screen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParaAttr {
    Spacing(Length),
    Indent(Length),
    Hanging(bool),
    IndentStart(Length),
    IndentEnd(Length),
    /// `None` is "not a list item", which is a value like any other — a span
    /// carrying it is how one paragraph opts *out* of a node-level marker.
    Marker(Option<ListMarker>),
    /// Nesting depth — see [`ParagraphStyle::level`]. Spannable because a nested
    /// item is by definition a *paragraph* that disagrees with its neighbours.
    Level(u8),
}

/// Which attribute a [`ParaAttr`] is, without its value.
///
/// As with [`CharAttrKind`], this variant order *is* the `(attribute, start)`
/// sort a saved file is written in (§5.11, invariant 9), so a new attribute is
/// appended rather than inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParaAttrKind {
    Spacing,
    Indent,
    Hanging,
    IndentStart,
    IndentEnd,
    Marker,
    Level,
}

impl ParaAttrKind {
    /// Every kind, in the declaration order that *is* the on-disk span sort.
    ///
    /// [`CharAttrKind::ALL`]'s twin, for the same reason and with the same reading
    /// of what it does and does not catch (§15 D598). The paragraph list had no
    /// guard at all — not even the fourteen-comparison one — so a swap here was
    /// invisible from the start.
    pub const ALL: [ParaAttrKind; 7] = [
        ParaAttrKind::Spacing,
        ParaAttrKind::Indent,
        ParaAttrKind::Hanging,
        ParaAttrKind::IndentStart,
        ParaAttrKind::IndentEnd,
        ParaAttrKind::Marker,
        ParaAttrKind::Level,
    ];
}

impl ParaAttr {
    pub fn kind(&self) -> ParaAttrKind {
        match self {
            ParaAttr::Spacing(_) => ParaAttrKind::Spacing,
            ParaAttr::Indent(_) => ParaAttrKind::Indent,
            ParaAttr::Hanging(_) => ParaAttrKind::Hanging,
            ParaAttr::IndentStart(_) => ParaAttrKind::IndentStart,
            ParaAttr::IndentEnd(_) => ParaAttrKind::IndentEnd,
            ParaAttr::Marker(_) => ParaAttrKind::Marker,
            ParaAttr::Level(_) => ParaAttrKind::Level,
        }
    }
}

impl SpanAttr for ParaAttr {
    type Defaults = ParagraphStyle;
    type Kind = ParaAttrKind;

    fn attr_kind(&self) -> ParaAttrKind {
        self.kind()
    }

    fn from_defaults(defaults: &ParagraphStyle, kind: ParaAttrKind) -> Self {
        defaults.get(kind)
    }

    fn set_on(self, defaults: &mut ParagraphStyle) {
        defaults.set(self);
    }

    /// Every length here is a measure, so all four move; `Hanging` is a
    /// direction and stays, and `Marker` is a kind — its ink scales because the
    /// *type* scales, which is `TextStyle::font_size`'s business and not this one's.
    /// `Level` is a count of gutters and stays for the same reason: the gutter it
    /// counts is `IndentStart`, which scales here, so the nesting scales with it.
    fn scaled_by(self, factor: f64) -> Self {
        match self {
            ParaAttr::Spacing(v) => ParaAttr::Spacing(v.scaled(factor)),
            ParaAttr::Indent(v) => ParaAttr::Indent(v.scaled(factor)),
            ParaAttr::Hanging(v) => ParaAttr::Hanging(v),
            ParaAttr::IndentStart(v) => ParaAttr::IndentStart(v.scaled(factor)),
            ParaAttr::IndentEnd(v) => ParaAttr::IndentEnd(v.scaled(factor)),
            ParaAttr::Marker(v) => ParaAttr::Marker(v),
            ParaAttr::Level(v) => ParaAttr::Level(v),
        }
    }
}

// ---------------------------------------------------------------------------
// Block attributes
// ---------------------------------------------------------------------------

/// Where the laid-out text sits inside a box taller than it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl VerticalAlign {
    pub const ALL: [VerticalAlign; 3] = [
        VerticalAlign::Top,
        VerticalAlign::Middle,
        VerticalAlign::Bottom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VerticalAlign::Top => "Top",
            VerticalAlign::Middle => "Middle",
            VerticalAlign::Bottom => "Bottom",
        }
    }
}

/// Which font metric the node's box is measured to.
///
/// **A box attribute, not a drawing one: trimming never moves the ink.** The
/// glyphs stay where they are and the reported box tightens around them, which
/// is what makes trim useful for aligning one label's cap height to another's —
/// and what stops toggling it from shifting every text node in every saved file.
/// The offset that buys this is [`crate::TextLayout::origin`]; see §15 for the
/// two decisions it settles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoxTrim {
    /// The full line box, leading included — what every file written before this
    /// existed means, and how those files drew.
    ///
    /// **This being the `#[default]` does not mean trim is off.** The field
    /// serializes with `skip_serializing_if`, so the default is what an *absent* key
    /// means and it has to go on meaning this or every text node in every saved file
    /// re-measures on open (§15 D78). New text is trimmed, and the app is what
    /// authors that — `FontService::default_text_parts`, §15 D199.
    #[default]
    None,
    /// Cap height down to the baseline: the box a designer sees.
    CapToBaseline,
    /// Ascent to descent — the type's own extent, without the line gap.
    AscentToDescent,
}

impl BoxTrim {
    pub const ALL: [BoxTrim; 3] = [
        BoxTrim::None,
        BoxTrim::CapToBaseline,
        BoxTrim::AscentToDescent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BoxTrim::None => "None",
            BoxTrim::CapToBaseline => "Cap",
            BoxTrim::AscentToDescent => "Ascent",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            BoxTrim::None => "The full line box, leading included",
            BoxTrim::CapToBaseline => "Cap height to baseline",
            BoxTrim::AscentToDescent => "Ascent to descent, no line gap",
        }
    }
}

/// What happens to text that does not fit its box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextOverflow {
    #[default]
    Visible,
    Clip,
    /// Truncate and draw an ellipsis. Needs a line limit or a fixed height to
    /// mean anything, which is why the two controls sit together.
    Ellipsis,
}

impl TextOverflow {
    pub const ALL: [TextOverflow; 3] = [
        TextOverflow::Visible,
        TextOverflow::Clip,
        TextOverflow::Ellipsis,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextOverflow::Visible => "Visible",
            TextOverflow::Clip => "Clip",
            TextOverflow::Ellipsis => "Ellipsis",
        }
    }
}

/// A text node's block attributes — the whole node, never a range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockStyle {
    #[serde(default, skip_serializing_if = "is_default")]
    pub vertical_align: VerticalAlign,
    #[serde(default, skip_serializing_if = "is_default")]
    pub trim: BoxTrim,
    #[serde(default, skip_serializing_if = "is_default")]
    pub overflow: TextOverflow,
    /// `0` is unlimited.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub max_lines: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Most lines the field will accept. Past this the limit has stopped describing
/// a paragraph and the incremental break loop is doing work for nothing.
pub const MAX_LINE_LIMIT: u32 = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> TextStyle {
        TextStyle::default()
    }

    /// **`ssty` is not stylistic set anything**, and the prefix test everybody
    /// writes says it is. It is math script-style alternates — a mechanic the
    /// shaper applies — and the panel sorted it in with the sets a designer
    /// chooses between; read-fonts will likewise parse its (nonexistent) params
    /// table as a stylistic set's, because it too dispatches on two bytes.
    #[test]
    fn a_numbered_tag_range_is_read_as_a_number_and_not_as_a_prefix() {
        assert_eq!(stylistic_set(Tag::parse("ss01").unwrap()), Some(1));
        assert_eq!(stylistic_set(Tag::parse("ss20").unwrap()), Some(20));
        assert_eq!(stylistic_set(Tag::parse("ssty").unwrap()), None);
        // Past the registry's own end of the range, so it is a private tag.
        assert_eq!(stylistic_set(Tag::parse("ss21").unwrap()), None);
        assert_eq!(stylistic_set(Tag::parse("ss00").unwrap()), None);
        assert_eq!(character_variant(Tag::parse("cv01").unwrap()), Some(1));
        assert_eq!(character_variant(Tag::parse("cv99").unwrap()), Some(99));
        assert_eq!(character_variant(Tag::parse("cvXX").unwrap()), None);
        // `u8::from_str` accepts a leading sign and a tag may legally hold one.
        assert_eq!(stylistic_set(Tag::parse("ss+1").unwrap()), None);
        // Neither range claims the other's tags, or anything else's.
        assert_eq!(stylistic_set(Tag::parse("cv01").unwrap()), None);
        assert_eq!(character_variant(Tag::parse("ss01").unwrap()), None);
        assert_eq!(stylistic_set(Tag::parse("tnum").unwrap()), None);
    }

    #[test]
    fn an_em_length_is_stored_as_a_fraction_and_shown_as_a_percentage() {
        // The stored number is the em; ×100 is a display concern only.
        let l = Length::Em(0.02);
        assert_eq!(l.amount(), 0.02);
        assert_eq!(l.resolve(50.0), 1.0);
    }

    #[test]
    fn a_drags_float_noise_canonicalizes_to_what_was_typed() {
        // The exact value a 2%-per-pixel scrub produces. Without this the span
        // list fragments and every save churns.
        let dragged = Length::Em(0.020000000000000004).canonical();
        assert_eq!(dragged, Length::Em(0.02));
    }

    /// A hand-edited file's non-canonical span value must not survive the load:
    /// it fragments the stored list, breaking `Spans`'s own invariant (2) —
    /// *"two spans of the same `SpanAttr::Kind` never overlap or touch while
    /// carrying equal values (they are merged)"* — so every save writes two spans
    /// where the document has one letter spacing.
    ///
    /// `clamped` is the seam — `io::schema::normalize_kind` calls it on load, and
    /// `op_set_text`/`op_set_text_style` call it live — and it now puts every
    /// span's attribute through `canonical_attr`.
    ///
    /// ⚠️ **The review filed this with a symptom that does not reproduce, and the
    /// correction is the useful half.** `[S6.1-L3-05]` states the cost as
    /// `shared_in` returning `None` — *"the panel says Mixed for a node whose
    /// letter spacing is one value to sixteen decimal places"*. It does not:
    /// `values_in` reads each cut through `Spans::resolve`, which applies
    /// `set_on`, **and `set_on` canonicalizes**. So the read path was already
    /// immune and `shared_in` answers `Some(Em(0.02))` even with the two spans
    /// unmerged. What is actually lost is the *storage* invariant, which is
    /// smaller and still real — and asserting only the symptom the finding named
    /// would have made this test vacuous.
    ///
    /// ⚠️ **Flipped** by restoring `attr: s.attr.clone()` in `clamped`'s map. The
    /// predicted failure site was the `shared_in` assertion and **that was wrong**
    /// — it bit on the span count, `left: 2 right: 1`, which is how the paragraph
    /// above came to be written. The `shared_in` assertion is kept anyway, and
    /// deliberately: it is the thing that stays green under the flip, and so the
    /// standing record that this bug is not the *Mixed* bug.
    #[test]
    fn a_hand_edited_span_value_is_canonicalized_on_the_way_in() {
        let style = TextStyle::default();
        let content = "abcdefgh";

        // What a hand-edited file (or an older writer's rounding) can carry: the
        // exact float a 2%-per-pixel scrub produces, written out unrounded.
        let smuggled = CharSpans::from_parts(
            vec![
                Span {
                    start: 0,
                    end: 4,
                    attr: CharAttr::LetterSpacing(Length::Em(0.020000000000000004)),
                },
                Span {
                    start: 4,
                    end: 8,
                    attr: CharAttr::LetterSpacing(Length::Em(0.02)),
                },
            ],
            &style,
        );
        // The fixture is in the state the test is about: two spans, unmerged,
        // because 0.02 and 0.020000000000000004 are not equal `f64`s.
        assert_eq!(smuggled.as_slice().len(), 2, "fixture");

        let loaded = smuggled.clamped(content, &style);

        assert_eq!(loaded.as_slice().len(), 1, "the two spans merged");
        assert_eq!(
            loaded.as_slice()[0].attr,
            CharAttr::LetterSpacing(Length::Em(0.02)),
            "at the canonical value, not the smuggled one"
        );
        // Green before the fix as well as after — see the ⚠️ above. Kept as the
        // record that the read path was never the broken half.
        assert_eq!(
            loaded.shared_in(&style, CharAttrKind::LetterSpacing, 0..8),
            Some(CharAttr::LetterSpacing(Length::Em(0.02)))
        );
    }

    #[test]
    fn scaling_moves_px_and_leaves_em() {
        assert_eq!(Length::Px(10.0).scaled(2.0), Length::Px(20.0));
        assert_eq!(Length::Em(0.5).scaled(2.0), Length::Em(0.5));
    }

    #[test]
    fn a_unit_flip_keeps_the_measured_length() {
        let px = Length::Px(4.8);
        let em = px.in_unit(LengthUnit::Em, 24.0);
        assert_eq!(em, Length::Em(0.2));
        assert_eq!(em.in_unit(LengthUnit::Px, 24.0), px);
    }

    #[test]
    fn a_tag_round_trips_and_rejects_the_wrong_length() {
        assert_eq!(
            Tag::parse("wght").map(|t| t.to_string()),
            Some("wght".into())
        );
        assert!(Tag::parse("wgh").is_none());
        assert!(Tag::parse("weight").is_none());
    }

    #[test]
    fn a_span_equal_to_the_default_is_not_stored() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..3, CharAttr::Size(d.font_size), &d);
        assert!(spans.is_empty(), "got {:?}", spans);
    }

    #[test]
    fn setting_one_attribute_leaves_the_others_alone() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..5, CharAttr::Weight(700), &d);
        spans.set(2..8, CharAttr::Size(32.0), &d);
        // Two independent spans, not four style structs.
        assert_eq!(spans.as_slice().len(), 2);
        let at3 = spans.resolve(&d, 3);
        assert_eq!(at3.weight, 700);
        assert_eq!(at3.font_size, 32.0);
        let at6 = spans.resolve(&d, 6);
        assert_eq!(at6.weight, d.weight, "the weight span ended at 5");
        assert_eq!(at6.font_size, 32.0);
    }

    #[test]
    fn re_setting_an_attribute_replaces_rather_than_stacks() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..10, CharAttr::Weight(700), &d);
        spans.set(3..6, CharAttr::Weight(300), &d);
        assert_eq!(spans.as_slice().len(), 3, "split into 0..3, 3..6, 6..10");
        assert_eq!(spans.resolve(&d, 1).weight, 700);
        assert_eq!(spans.resolve(&d, 4).weight, 300);
        assert_eq!(spans.resolve(&d, 8).weight, 700);
    }

    #[test]
    fn two_touching_spans_with_the_same_value_merge() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::Weight(700), &d);
        spans.set(4..8, CharAttr::Weight(700), &d);
        assert_eq!(spans.as_slice().len(), 1, "got {:?}", spans);
        assert_eq!(spans.as_slice()[0].range(), 0..8);
    }

    #[test]
    fn a_typed_span_equals_a_dragged_one_so_the_two_coalesce() {
        // The reason `set` canonicalizes through `TextStyle::set`: the list must
        // not fragment because one value came off a slider.
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::LetterSpacing(Length::Em(0.02)), &d);
        spans.set(
            4..8,
            CharAttr::LetterSpacing(Length::Em(0.020000000000000004)),
            &d,
        );
        assert_eq!(spans.as_slice().len(), 1, "got {:?}", spans);
    }

    /// The colour sibling of the test above, and the reason `canonical_color`
    /// exists. `Color` is `[f32; 4]`: the same colour typed as hex and nudged off
    /// the picker's plane differs in the last bit, two spans that differ in the last
    /// bit never merge, and the list grows on every touch.
    #[test]
    fn a_typed_colour_and_a_picked_one_coalesce() {
        let d = style();
        let typed = Color::from_rgba8(145, 132, 217, 255);
        let mut dragged = typed;
        dragged.components[0] += 1e-7;
        dragged.components[2] -= 1e-7;
        assert_ne!(
            typed.components, dragged.components,
            "the fixture has to actually differ, or this proves nothing"
        );

        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::Color(Some(typed)), &d);
        spans.set(4..8, CharAttr::Color(Some(dragged)), &d);
        assert_eq!(
            spans.as_slice().len(),
            1,
            "8-bit canonicalization must make the two exactly equal: {spans:?}"
        );
        assert_eq!(spans.as_slice()[0].range(), 0..8);
    }

    /// **The declaration order of these kinds *is* the `(attribute, start)` sort a
    /// saved file is written in** (§5.11, invariant 9), so two of them exchanged
    /// reorders the spans of every document already on disk — each one churning a
    /// diff on its next save while saying nothing. Stated as a test because the
    /// failure is invisible: the code compiles, the document loads, and only a diff
    /// of two saves shows it.
    ///
    /// ⚠️ **This replaces `colour_sorts_after_every_other_attribute`, which could
    /// not see either change §15 D154 warns about** (§15 D598). That test asserted
    /// `Color > kind` for a hand-written list of the other fourteen, and every one of
    /// those fourteen comparisons survives any permutation of the fourteen. The
    /// measured flip — `CharAttrKind::Family` and `CharAttrKind::Size` exchanged, one
    /// line, no other change — left **1,479 tests green**, the `every-kind.json`
    /// golden included. D154 names that test by hand as the enforcement of the rule,
    /// which is `[S6.1-L6-02]`: a verdict whose reason names a code fact, where the
    /// code fact was not true.
    ///
    /// ⚠️ **And D154's *reason* was a proxy, which is why the replacement asserts
    /// something narrower than "`Color` is last".** A kind that *arrives* in the
    /// middle moves nothing — the fourteen keep their relative order, so a document
    /// that does not use the newcomer serializes byte-for-byte as before. What churns
    /// every file is a kind that *moves*. Both assertions are here, and only the
    /// first one is load-bearing; the second restates D154's rule for the reader who
    /// comes to it from the entry.
    ///
    /// Flip-check, run: the `Family`/`Size` swap fails here at the ascending
    /// assertion, naming both kinds and their index — *"Family and Size are out of
    /// declaration order at index 0"* — where the test this replaces was green.
    ///
    /// ⚠️ **It fails here and nowhere else, which was the predicted site and was
    /// *not* the prediction written first.** The draft of this comment said the swap
    /// would also take
    /// `spans_sort_by_attribute_then_start_for_byte_stable_saves` below, and it does
    /// not: that test's two spans are `Size` and `Weight`, whose relative order a
    /// `Family`/`Size` exchange does not touch. **A permutation is only visible to a
    /// test that holds both of the kinds it permutes** — which is the whole argument
    /// for asserting the list rather than sampling it, arriving from the direction
    /// that makes it concrete. The sibling below bites the *other* flip (the sort key
    /// with the attribute deleted), and between them the two flips cover the order
    /// and the bytes.
    #[test]
    fn char_attribute_kinds_keep_the_order_a_saved_file_is_written_in() {
        for (i, w) in CharAttrKind::ALL.windows(2).enumerate() {
            assert!(
                w[0] < w[1],
                "{:?} and {:?} are out of declaration order at index {i}: \
                 every document on disk holding both re-sorts on its next save",
                w[0],
                w[1],
            );
        }
        assert_eq!(
            CharAttrKind::ALL.last(),
            Some(&CharAttrKind::Color),
            "D154's own rule: Color was appended and stays appended"
        );
    }

    /// The paragraph list carries the same guarantee and had no guard of any kind
    /// (§15 D598). Same reading as the character test above.
    #[test]
    fn paragraph_attribute_kinds_keep_the_order_a_saved_file_is_written_in() {
        for (i, w) in ParaAttrKind::ALL.windows(2).enumerate() {
            assert!(
                w[0] < w[1],
                "{:?} and {:?} are out of declaration order at index {i}",
                w[0],
                w[1],
            );
        }
    }

    /// A decoration's colour goes through the same rounding, which it did not
    /// before: `canonical_decoration` canonicalized its two `Length`s from the start
    /// and left the colour raw, so an underline recoloured by a drag fragmented its
    /// spans exactly as a run colour would have.
    #[test]
    fn a_decorations_colour_is_canonicalized_too() {
        let d = style();
        let typed = Color::from_rgba8(200, 30, 30, 255);
        let mut dragged = typed;
        dragged.components[1] += 1e-7;
        let deco = |c: Color| {
            CharAttr::Underline(Some(Decoration {
                color: Some(c),
                ..Decoration::default()
            }))
        };
        let mut spans = CharSpans::default();
        spans.set(0..4, deco(typed), &d);
        spans.set(4..8, deco(dragged), &d);
        assert_eq!(
            spans.as_slice().len(),
            1,
            "a decoration's colour must round like every other value: {spans:?}"
        );
    }

    /// **A decoration's thickness is floored at zero and its offset is not** —
    /// `[S6.3-L1-02]`, §15 D546.
    ///
    /// The panel's `optional_length_field` serves both fields and was handing
    /// thickness the **offset's** symmetric range, so `−5 px` was accepted, shown
    /// and saved. **One value then rendered three ways**: `text::decoration_ink`
    /// floors the resolved size and drew nothing, `export::svg::run_attrs` had no
    /// counterpart and wrote `text-decoration-thickness="-5px"` verbatim, and
    /// re-importing that file gave a third answer because `svg_in` reads no
    /// decoration thickness at all. No warning on any of them.
    ///
    /// **The clamp is at the model as well as at the panel**, for the reason
    /// `TextStyle::set`'s `Size` and `Weight` arms already carry: the model
    /// outlives the control, and flooring at the panel alone leaves every
    /// already-saved file and every non-panel writer open.
    ///
    /// ⚠️ **The offset assertion is the one that would catch an over-broad
    /// fix**, and it is the whole reason both fields are here. The offset is
    /// legitimately signed — positive-**up**, so clearing the descenders means
    /// typing a negative — and a `canonical_decoration` that floored both would
    /// pass every thickness assertion while silently breaking the field the range
    /// was written for. §15 D151 fixes that sign convention.
    ///
    /// ⚠️ **`Em` as well as `Px`**, because the two are separate arms of the
    /// `map` and a fix applied to one compiles perfectly with the other left
    /// signed.
    ///
    /// ⚠️ **Zero is kept rather than turned into `None`.** They are different
    /// answers — `None` is *"the font decides"* and draws the face's own weight,
    /// `Some(0.0)` is *"no line"* — so a fix that mapped a negative to `None`
    /// would make the line **reappear**, which is the third of the three
    /// renderings this is closing.
    ///
    /// ⚠️ **Flipped** by dropping the `.max(0.0)` map: fails on the first
    /// assertion at `Some(Px(-5.0))` against `Some(Px(0.0))`. The offset control
    /// stays green.
    ///
    /// (Plain backticks rather than `[links]`, §15 D319's convention.)
    #[test]
    fn a_decorations_thickness_is_floored_at_zero_and_its_offset_is_not() {
        let d = style();
        let stored = |deco: Decoration| {
            let mut spans = CharSpans::default();
            spans.set(0..4, CharAttr::Underline(Some(deco)), &d);
            match spans.resolve(&d, 1).underline {
                Some(u) => u,
                None => panic!("the fixture sets an underline"),
            }
        };

        for negative in [Length::Px(-5.0), Length::Px(-9999.0), Length::Em(-1.5)] {
            let got = stored(Decoration {
                thickness: Some(negative),
                ..Decoration::default()
            });
            assert_eq!(
                got.thickness.map(|t| t.resolve(16.0)),
                Some(0.0),
                "a thickness of {negative:?} has no meaning and must not reach the \
                 file — the canvas already drew nothing for it"
            );
            assert!(
                got.thickness.is_some(),
                "and it must stay `Some`: `None` means \"the font decides\" and \
                 would make the line reappear"
            );
        }

        // **Controls.** A positive thickness is untouched, and the offset keeps
        // its sign — it is positive-up by design (§15 D151), and a floor applied
        // to both would pass every assertion above.
        let got = stored(Decoration {
            thickness: Some(Length::Px(3.0)),
            offset: Some(Length::Px(-2.0)),
            ..Decoration::default()
        });
        assert_eq!(
            got.thickness,
            Some(Length::Px(3.0)),
            "control: 3px survives"
        );
        assert_eq!(
            got.offset,
            Some(Length::Px(-2.0)),
            "control: a negative offset is a lower line, not an error"
        );
    }

    #[test]
    fn clearing_a_range_puts_it_back_on_the_default() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..10, CharAttr::Weight(700), &d);
        spans.clear(3..6, CharAttrKind::Weight, &d);
        assert_eq!(spans.resolve(&d, 4).weight, d.weight);
        assert_eq!(spans.resolve(&d, 1).weight, 700);
    }

    #[test]
    fn runs_cover_the_content_exactly_and_split_only_where_a_span_does() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(2..5, CharAttr::Weight(700), &d);
        let runs = spans.runs(&d, 9);
        assert_eq!(
            runs.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..2, 2..5, 5..9]
        );
        assert_eq!(runs[1].1.weight, 700);
    }

    #[test]
    fn an_empty_node_still_has_one_run() {
        // Its font size and line height are what measure the empty box.
        let d = style();
        let runs = CharSpans::default().runs(&d, 0);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, 0..0);
    }

    #[test]
    fn a_range_spanning_two_sizes_reads_as_mixed() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::Size(16.0), &d);
        spans.set(4..8, CharAttr::Size(24.0), &d);
        assert!(
            spans.shared_in(&d, CharAttrKind::Size, 0..8).is_none(),
            "16pt and 24pt together must not report one size"
        );
        assert_eq!(
            spans.shared_in(&d, CharAttrKind::Size, 4..8),
            Some(CharAttr::Size(24.0))
        );
    }

    #[test]
    fn a_bare_caret_reads_the_value_under_it() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::Weight(700), &d);
        assert_eq!(
            spans.shared_in(&d, CharAttrKind::Weight, 2..2),
            Some(CharAttr::Weight(700))
        );
    }

    #[test]
    fn spans_grow_leftwards_when_typing_at_a_seam() {
        // The boundary rule, both directions at one seam: text typed at 4 joins
        // the *bold* run that ends there, not the plain one that starts there.
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(0..4, CharAttr::Weight(700), &d);
        let after = spans.edited(4, 0, 3, &d);
        assert_eq!(after.as_slice()[0].range(), 0..7);
        assert_eq!(after.resolve(&d, 5).weight, 700);
    }

    #[test]
    fn a_span_starting_at_the_caret_is_pushed_right() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(4..8, CharAttr::Weight(700), &d);
        let after = spans.edited(4, 0, 3, &d);
        assert_eq!(
            after.as_slice()[0].range(),
            7..11,
            "the inserted text inherits from the left, so the span moves"
        );
    }

    #[test]
    fn a_deletion_across_a_span_closes_it_up() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(5..10, CharAttr::Weight(700), &d);
        // Delete 3..7 — the span's first two bytes go with it.
        let after = spans.edited(3, 4, 0, &d);
        assert_eq!(after.as_slice()[0].range(), 3..6);
    }

    #[test]
    fn deleting_a_whole_span_removes_it() {
        let d = style();
        let mut spans = CharSpans::default();
        spans.set(2..6, CharAttr::Weight(700), &d);
        let after = spans.edited(0, 10, 0, &d);
        assert!(after.is_empty(), "got {:?}", after);
    }

    /// The sort key is `(attribute, start)`, and **the two spans share a start so
    /// that the attribute half is what decides** (§15 D598).
    ///
    /// ⚠️ **The fixture used to be `Size 0..2` and `Weight 4..8`, whose *starts*
    /// already order them the way the kind does** — so the test named for the sort
    /// key never exercised the key. Measured, before the rebuild: deleting the
    /// attribute from the key outright (`sort_by_key(|s| (s.start, s.start))`) left
    /// this test **green**, and the only casualty anywhere in the workspace was
    /// `a_paragraph_span_list_round_trips_through_json` — one failure out of 1,479,
    /// in a test that is not about the sort at all. `[S6.1-L6-02]`.
    ///
    /// The explicit order assertion is the load-bearing one; the `a == b` that was
    /// here before is kept because it says the *insertion* order does not survive,
    /// which is the property a byte-stable save actually needs.
    ///
    /// Flip-check, run: with the key back at `(s.start, s.start)` this now fails at
    /// the order assertion — `left: [Weight, Size]`, `right: [Size, Weight]` — and
    /// `a_paragraph_span_list_round_trips_through_json` fails beside it, as it did
    /// before. **Two failures where the finding measured one, and the new one is at
    /// the test named for the key.**
    #[test]
    fn spans_sort_by_attribute_then_start_for_byte_stable_saves() {
        let d = style();
        let mut a = CharSpans::default();
        a.set(0..4, CharAttr::Weight(700), &d);
        a.set(0..4, CharAttr::Size(9.0), &d);
        let mut b = CharSpans::default();
        b.set(0..4, CharAttr::Size(9.0), &d);
        b.set(0..4, CharAttr::Weight(700), &d);
        assert_eq!(
            a.as_slice()
                .iter()
                .map(|s| s.attr.kind())
                .collect::<Vec<_>>(),
            vec![CharAttrKind::Size, CharAttrKind::Weight],
            "with one start between them the kind is the whole key"
        );
        assert_eq!(a, b, "the same overrides must serialize identically");
    }

    /// **One set of spans is one document, whatever order the file lists them
    /// in — and a span painted over keeps the bytes nobody claimed** (§15 D643,
    /// `[S6.1-L1-06]`).
    ///
    /// Built with `from_parts` rather than `set`, on purpose: `set` splits the
    /// old span around the new range and cannot produce a containment at all, so
    /// the only producer is a hand-edited or foreign-written file, which
    /// `schema.rs` normalizes rather than rejects. A test driven through `set`
    /// would be green against the old code.
    ///
    /// The three assertions are three different claims and each one failed
    /// against a different half of the old arm:
    ///
    /// 1. **Order-independence.** The two array orders gave `[{0..5, W300}]` and
    ///    `[{0..10, W700}]` — two documents from one set.
    /// 2. **Nothing outside the overlap changes hands.** Bytes 5..10 were never
    ///    covered by the 300 and lost their 700 to the node default anyway,
    ///    because emptying `prev` popped its *whole* range.
    /// 3. **The straddling control is unchanged**, which is what says this is a
    ///    repair to one arm rather than a new rule: it behaved as documented
    ///    before and behaves the same way now.
    ///
    /// Two flips, run, and **each bites at exactly one assertion with the other
    /// green** — which is what says the tie-break and the split are two separate
    /// halves of the fix rather than one written twice.
    ///
    /// - Sort key back to `(kind, start)`: red at assertion 1, `left` the fixed
    ///   pair and `right` `[{0..10, W700}]`, because with the narrow span first
    ///   the wide one paints straight over it.
    /// - Tail dropped from the split (`prev.end > span.end` blinded): assertion 1
    ///   **green** — both orders agree on the wrong answer — and red at assertion
    ///   2 with `[{0..5, W300}]` alone.
    ///
    /// Assertion 3 survives both, which is the control doing its job.
    #[test]
    fn an_overlap_resolves_the_same_way_whichever_order_the_file_lists_it_in() {
        let d = style();
        let wide = Span {
            start: 0,
            end: 10,
            attr: CharAttr::Weight(700),
        };
        let narrow = Span {
            start: 0,
            end: 5,
            attr: CharAttr::Weight(300),
        };

        let a = CharSpans::from_parts(vec![wide.clone(), narrow.clone()], &d);
        let b = CharSpans::from_parts(vec![narrow, wide], &d);
        assert_eq!(a, b, "the array order decided the document");

        // The narrower span wins the bytes it covers; the wider one keeps its
        // tail, which nothing overlapped.
        assert_eq!(
            a.as_slice(),
            [
                Span {
                    start: 0,
                    end: 5,
                    attr: CharAttr::Weight(300)
                },
                Span {
                    start: 5,
                    end: 10,
                    attr: CharAttr::Weight(700)
                },
            ],
            "bytes 5..10 were never overlapped"
        );

        // The control: a *straddling* overlap, which the old comment described
        // correctly and which this change does not touch.
        let straddle = CharSpans::from_parts(
            vec![
                Span {
                    start: 0,
                    end: 6,
                    attr: CharAttr::Weight(700),
                },
                Span {
                    start: 4,
                    end: 10,
                    attr: CharAttr::Weight(300),
                },
            ],
            &d,
        );
        assert_eq!(
            straddle.as_slice(),
            [
                Span {
                    start: 0,
                    end: 4,
                    attr: CharAttr::Weight(700)
                },
                Span {
                    start: 4,
                    end: 10,
                    attr: CharAttr::Weight(300)
                },
            ]
        );
    }

    /// **The predicate only** — the value it gates is `text::axis_settings`', and
    /// the test of *that* is
    /// `text::tests::a_stored_opsz_reaches_the_shaper_raw_and_an_absent_one_tracks_the_size`
    /// (§15 D571).
    ///
    /// ⚠️ **This test used to cover `TextStyle::optical_size`, which nothing
    /// called.** Its three value assertions were the only coverage the Manual arm
    /// had anywhere, and they were assertions about a function that could not affect
    /// a pixel — including the clamp, which the live rule deliberately does not do.
    /// Moved rather than deleted, which is the half a straight removal loses.
    #[test]
    fn optical_size_is_auto_until_a_coordinate_is_typed() {
        let mut s = style();
        s.font_size = 48.0;
        assert!(s.optical_size_is_auto());
        s.set(CharAttr::Variations(vec![AxisSetting::new(OPSZ, 14.0)]));
        assert!(!s.optical_size_is_auto());
        // Any `opsz` at all, including one that happens to equal the font size:
        // the question is whether a coordinate was *stored*, not what it says.
        s.set(CharAttr::Variations(vec![AxisSetting::new(OPSZ, 48.0)]));
        assert!(!s.optical_size_is_auto());
        // And another axis is not this one.
        s.set(CharAttr::Variations(vec![AxisSetting::new(WGHT, 700.0)]));
        assert!(s.optical_size_is_auto());
    }

    #[test]
    fn axis_values_quantize_to_tenths() {
        let a = AxisSetting::new(WGHT, 412.3456);
        assert_eq!(a.value, 412.3);
    }

    #[test]
    fn variations_sort_by_tag_so_two_orders_save_the_same() {
        let mut a = style();
        a.set(CharAttr::Variations(vec![
            AxisSetting::new(OPSZ, 14.0),
            AxisSetting::new(WGHT, 500.0),
        ]));
        let mut b = style();
        b.set(CharAttr::Variations(vec![
            AxisSetting::new(WGHT, 500.0),
            AxisSetting::new(OPSZ, 14.0),
        ]));
        assert_eq!(a.variations, b.variations);
    }

    #[test]
    fn the_physical_align_pair_still_lights_a_cell() {
        // An old file's `Left`/`Right` must not leave the panel blank.
        assert_eq!(TextAlign::Left.cell(), TextAlign::Start.cell());
        assert_eq!(TextAlign::Right.cell(), TextAlign::End.cell());
    }

    // --- paragraph spans ---------------------------------------------------
    //
    // The list machinery itself is `Spans<A>`, already covered above through
    // `CharSpans`; what these pin is the half that is *this* scope's — that
    // `ParagraphStyle::get`/`set` round-trip every spannable attribute, that the
    // canonicalization and the default-elision reach it, and that the two block
    // indents move under the scale tool. A generic list with a wrong `SpanAttr`
    // impl under it is a list that quietly stores nothing.

    #[test]
    fn every_paragraph_attribute_round_trips_through_get_and_set() {
        // The exhaustive check `TextStyle` gets by having a variant per field —
        // spelled out here because a new `ParaAttr` that `get` and `set` disagree
        // about is a span that reads back as something else.
        let values = [
            ParaAttr::Spacing(Length::Px(12.0)),
            ParaAttr::Indent(Length::Em(1.5)),
            ParaAttr::Hanging(true),
            ParaAttr::IndentStart(Length::Px(24.0)),
            ParaAttr::IndentEnd(Length::Px(8.0)),
        ];
        for v in values {
            let p = ParagraphStyle::default().with(v);
            assert_eq!(p.get(v.kind()), v, "{v:?} did not survive the round trip");
        }
        // And every kind is reachable from a value, so the loop above is complete.
        let kinds: Vec<ParaAttrKind> = values.iter().map(ParaAttr::kind).collect();
        for kind in [
            ParaAttrKind::Spacing,
            ParaAttrKind::Indent,
            ParaAttrKind::Hanging,
            ParaAttrKind::IndentStart,
            ParaAttrKind::IndentEnd,
        ] {
            assert!(kinds.contains(&kind), "{kind:?} is untested above");
        }
    }

    #[test]
    fn a_paragraph_span_is_canonicalized_so_two_of_them_coalesce() {
        let d = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        // The rounding `Length::canonical` does, reached through `set` — a dragged
        // value and a typed one have to become the same span or the list grows one
        // entry per frame.
        spans.set(
            0..4,
            ParaAttr::IndentStart(Length::Px(24.000000000000004)),
            &d,
        );
        spans.set(4..8, ParaAttr::IndentStart(Length::Px(24.0)), &d);
        assert_eq!(
            spans.as_slice().len(),
            1,
            "the two should have merged: {:?}",
            spans.as_slice()
        );
        assert_eq!(spans.as_slice()[0].range(), 0..8);
    }

    #[test]
    fn a_paragraph_span_equal_to_the_default_is_not_stored() {
        let d = ParagraphStyle {
            indent_start: Length::Px(10.0),
            ..ParagraphStyle::default()
        };
        let mut spans = ParaSpans::default();
        spans.set(0..4, ParaAttr::IndentStart(Length::Px(10.0)), &d);
        assert!(
            spans.is_empty(),
            "an override equal to the node's own value is invisible and grows the file"
        );
        spans.set(0..4, ParaAttr::IndentStart(Length::Px(30.0)), &d);
        assert_eq!(spans.as_slice().len(), 1);
        // And clearing puts those bytes back on the default rather than storing it.
        spans.clear(0..4, ParaAttrKind::IndentStart, &d);
        assert!(spans.is_empty());
    }

    #[test]
    fn a_paragraph_span_re_bases_over_an_edit() {
        let d = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        // "one\ntwo" — the span is on the second paragraph.
        spans.set(4..7, ParaAttr::IndentStart(Length::Px(20.0)), &d);
        // Type one character into the *first* paragraph. Without this re-basing,
        // the second paragraph now starts at byte 5 and the span still claims 4..7,
        // which is the whole reason `SetText` carries this list.
        let after = spans.edited(1, 0, 1, &d);
        assert_eq!(after.as_slice()[0].range(), 5..8);
        // Splitting a paragraph carries the override onto the new one, because the
        // span grows over inserted text at its edge — the boundary rule.
        let split = spans.edited(6, 0, 1, &d);
        assert_eq!(split.as_slice()[0].range(), 4..8);
    }

    #[test]
    fn the_scale_tool_moves_all_four_paragraph_lengths() {
        let p = ParagraphStyle {
            spacing: Length::Px(10.0),
            indent: Length::Px(20.0),
            indent_start: Length::Px(30.0),
            indent_end: Length::Em(0.5),
            hanging: true,
            ..ParagraphStyle::default()
        };
        let doubled = p.scaled(2.0);
        assert_eq!(doubled.spacing, Length::Px(20.0));
        assert_eq!(doubled.indent, Length::Px(40.0));
        assert_eq!(doubled.indent_start, Length::Px(60.0));
        // `Em` is already relative to the font size, which scaled too.
        assert_eq!(doubled.indent_end, Length::Em(0.5));
        assert!(doubled.hanging, "a direction is not a length");
    }

    /// **The level is a count of gutters, and the gutter is the one that scaled.**
    /// Doubling it here would step a nested item four gutters in on a scale that
    /// already doubled the gutter itself.
    #[test]
    fn the_scale_tool_leaves_the_nesting_level_alone() {
        let p = ParagraphStyle {
            indent_start: Length::Px(30.0),
            level: 2,
            ..ParagraphStyle::default()
        };
        let doubled = p.scaled(2.0);
        assert_eq!(doubled.level, 2);
        assert_eq!(
            doubled.start_edge(16.0),
            180.0,
            "three gutters of 60, not of 30 and not six of them"
        );
    }

    /// The bound lives on the one door every edit comes through, so a control cannot
    /// route around it — and a hand-edited file's absurd level cannot wrap the
    /// multiplier to zero and draw flush.
    #[test]
    fn a_levels_bound_is_enforced_where_it_is_written_and_survives_a_wild_file() {
        let mut p = ParagraphStyle::default();
        p.set(ParaAttr::Level(200));
        assert_eq!(p.level, MAX_LIST_LEVEL);
        assert_eq!(p.get(ParaAttrKind::Level), ParaAttr::Level(MAX_LIST_LEVEL));

        let wild = ParagraphStyle {
            indent_start: Length::Px(10.0),
            level: u8::MAX,
            ..ParagraphStyle::default()
        };
        // 255 rather than 256: `saturating_add` is what stops the multiplier wrapping
        // to zero, and one lost gutter at level 255 is the whole cost of that.
        assert_eq!(wild.start_edge(16.0), 2550.0, "255 gutters, not none");
    }

    #[test]
    fn a_paragraph_span_list_round_trips_through_json() {
        let d = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        spans.set(0..4, ParaAttr::IndentStart(Length::Px(24.0)), &d);
        spans.set(4..9, ParaAttr::Spacing(Length::Em(0.5)), &d);
        spans.set(4..9, ParaAttr::Hanging(true), &d);
        let json = serde_json::to_string(&spans).expect("serializes");
        let back: ParaSpans = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, spans);
        // Byte-stable: the same overrides written in another order are one file.
        let mut other = ParaSpans::default();
        other.set(4..9, ParaAttr::Hanging(true), &d);
        other.set(4..9, ParaAttr::Spacing(Length::Em(0.5)), &d);
        other.set(0..4, ParaAttr::IndentStart(Length::Px(24.0)), &d);
        assert_eq!(
            serde_json::to_string(&other).expect("serializes"),
            json,
            "the (kind, start) sort is what keeps a save from churning"
        );
    }

    /// **A zero length keeps its unit across a save** — `[S6.1-L1-03]`, §15 D537.
    ///
    /// `Length::is_zero` was the `skip_serializing_if` on all seven optional
    /// lengths in this module and answers `true` for `Em(0.0)` as readily as for
    /// `Px(0.0)`, so a letter-spacing field set to `%` and left at zero was
    /// **absent from the file** and came back `Px`. The drawing is identical
    /// either way — `resolve` is `0.0` for both — which is exactly why nothing
    /// rendered ever disagreed and why the only place the loss shows is the
    /// suffix on a panel, one reopen later.
    ///
    /// **The elided control is half the test.** `is_default_zero` has to keep
    /// eliding the *default* zero or the fix is "write every field always",
    /// which would grow every file in the library for a case that has no data in
    /// it. Asserted on the JSON text rather than on the round-tripped value,
    /// because a value that round-trips says nothing about whether the key was
    /// written.
    ///
    /// **And a paragraph field, because the seven move together.** The predicate
    /// is named seven times across `TextStyle` and `ParagraphStyle`; a test that
    /// pinned only `letter_spacing` would be green against a fix applied to one
    /// of them.
    ///
    /// ⚠️ **Flipped** by putting `is_zero` back on `letter_spacing`: fails on the
    /// first assertion — the key is absent — which is the predicted site. Flipped
    /// the other way too, by making `is_default_zero` return `false`
    /// unconditionally: the *control* fails on the elision, at
    /// `letter_spacing` present in a default style. Both halves bite.
    ///
    /// (Plain backticks, not `[links]` — `cargo doc` builds without the `test`
    /// cfg, so a link here is decoration no gate can validate, §15 D319.)
    #[test]
    fn a_zero_length_keeps_its_unit_across_a_save() {
        let em0 = TextStyle {
            letter_spacing: Length::Em(0.0),
            ..TextStyle::default()
        };
        let json = serde_json::to_string(&em0).expect("serializes");
        assert!(
            json.contains("letter_spacing"),
            "a zero whose unit is not the default one has to reach the file, or \
             the unit is what the save costs: {json}"
        );
        let back: TextStyle = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(
            back.letter_spacing,
            Length::Em(0.0),
            "and it comes back the unit it went in as"
        );

        // Control: the default zero is still absent, so no existing file grows.
        let plain = serde_json::to_string(&TextStyle::default()).expect("serializes");
        assert!(
            !plain.contains("letter_spacing"),
            "control: a field never touched is still elided: {plain}"
        );

        // And the paragraph side, because the predicate is named on both types.
        let indented = ParagraphStyle {
            indent: Length::Em(0.0),
            ..ParagraphStyle::default()
        };
        let json = serde_json::to_string(&indented).expect("serializes");
        assert!(
            json.contains("indent"),
            "the seven fields move together: {json}"
        );
        let back: ParagraphStyle = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.indent, Length::Em(0.0));
    }

    /// Canonicalizing a **finite** length leaves it finite, however large.
    ///
    /// ⚠️ **The failure this pins is the sanitizer manufacturing the bad value**,
    /// which is the opposite direction from every other finiteness guard in the
    /// workspace: `Length::canonical` opens with `if !v.is_finite() { return 0.0 }`
    /// and then multiplies by `10^LENGTH_PLACES`, so the guard answers about the
    /// input and the overflow happens two lines later. `Px(inf)` is written by
    /// `serde_json` as `null` and refused on the way back in — a document that
    /// never opens again, produced by a documented door
    /// (`Spans::set` routes every value through the canonicalizer).
    ///
    /// **The controls either side of the threshold are the point of the
    /// bracketing.** 1e304 was always fine and 1e305 was always `inf`, so a test
    /// that only tried one of them would have been green for the wrong reason at
    /// whichever end it picked.
    ///
    /// ⚠️ **`AxisSetting` is the second site of one mechanism**, not a second
    /// bug: `quantize` carries the identical guard and the identical settle, and
    /// is the only sanitizer a variable-axis value meets.
    ///
    /// Flip-check, run: putting `(v * scale).round() / scale` back unguarded
    /// fails on the first assertion, printing `Px(inf)`.
    #[test]
    fn canonicalizing_a_finite_length_leaves_it_finite() {
        for v in [1e304, 1e305, 1e307, f64::MAX, -1e305, -f64::MAX] {
            let out = Length::Px(v).canonical();
            let Length::Px(got) = out else { unreachable!() };
            assert!(got.is_finite(), "canonical(Px({v:e})) = {out:?}");
            let Length::Em(got_em) = Length::Em(v).canonical() else {
                unreachable!()
            };
            assert!(got_em.is_finite(), "canonical(Em({v:e}))");
        }

        // The control below the threshold, which always worked and has to keep
        // working: rounding to four places is the whole point of the function.
        assert_eq!(Length::Px(1.000_049).canonical(), Length::Px(1.0));
        assert_eq!(Length::Px(1.000_051).canonical(), Length::Px(1.0001));
        assert_eq!(Length::Px(1e303).canonical(), Length::Px(1e303));

        // The same mechanism at its second site.
        let axis = AxisSetting::new(WGHT, 1e306);
        assert!(axis.value.is_finite(), "AxisSetting::new(wght, 1e306)");
        let ordinary = AxisSetting::new(WGHT, 412.345_678);
        assert_eq!(
            ordinary.value,
            quantize(412.345_678, AXIS_QUANTUM),
            "and an ordinary axis value is still quantized"
        );
    }

    /// **`TextStyle::scaled` and `CharAttr::scaled_by` are one rule, and this is
    /// what holds them to it** — §15 D664, `[S6.1-L3-07]`.
    ///
    /// The node default and the spans over it are scaled by two different
    /// functions, because they are two different shapes: one is a struct and the
    /// other a stream of attribute values. `Spans::normalize` then compares the
    /// two — a span equal to the default is not stored — so the moment they
    /// disagree about one attribute, a scale either drops a span that mattered or
    /// keeps one that did not.
    ///
    /// 🚨 **They lived in two crates until 2026-09-09.** Core published
    /// `ParagraphStyle::scaled` for the paragraph scope and nothing for this one,
    /// so `ondin-app::tools::scaled_text_scopes` re-stated all six decisions in a
    /// `TextStyle` literal, above a byte-identical private twin of
    /// `scale_decoration`. They agreed; nothing made them agree tomorrow, which
    /// is what this test now does.
    ///
    /// **Asked over `CharAttrKind::ALL`**, so a sixteenth attribute is covered the
    /// day it exists rather than the day somebody remembers this test.
    ///
    /// ⚠️ **The fixture has to be non-default in every scalable field**, or the
    /// test is about nothing: `TextStyle::default`'s lengths are zero and
    /// `0 * 1.75 == 0`, so a `scaled` that returned `self.clone()` would pass over
    /// the defaults. The `assert_ne!` below is what says the fixture moved.
    ///
    /// ⚠️ **Flip run**, `baseline_shift` left unscaled in `TextStyle::scaled` —
    /// the plausible slip, since it is the one field of the six whose name does
    /// not say "length". Fails at `BaselineShift(Px(-3.0))` against
    /// `BaselineShift(Px(-5.25))`, naming the attribute in the message, which is
    /// the whole reason the loop asserts per kind rather than comparing two
    /// structs.
    #[test]
    fn a_scaled_style_agrees_with_the_span_rule_attribute_by_attribute() {
        let base = TextStyle {
            font_size: 12.0,
            line_height: Some(Length::Px(18.0)),
            letter_spacing: Length::Px(1.5),
            word_spacing: Length::Em(0.25),
            baseline_shift: Length::Px(-3.0),
            underline: Some(Decoration {
                thickness: Some(Length::Px(2.0)),
                offset: Some(Length::Px(4.0)),
                ..Decoration::default()
            }),
            strikethrough: Some(Decoration {
                thickness: Some(Length::Em(0.1)),
                offset: Some(Length::Px(-1.0)),
                ..Decoration::default()
            }),
            ..TextStyle::default()
        };
        let factor = 1.75;
        let scaled = base.scaled(factor);
        assert_ne!(
            scaled, base,
            "the fixture must actually move, or every assertion below is vacuous"
        );

        for kind in CharAttrKind::ALL {
            assert_eq!(
                scaled.get(kind),
                base.get(kind).scaled_by(factor),
                "the two scopes disagree about {kind:?}, so a span carrying it \
                 will normalize against a default that was scaled by a different \
                 rule"
            );
        }
    }
}
