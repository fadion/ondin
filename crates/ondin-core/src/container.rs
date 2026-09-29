//! Container layout (§5.3c) — build steps 2 to 4: **absolute insets**, which is
//! what this project calls constraints (§15 D871), **flex** (§15 D875) and
//! **grid** (§15 D913, D914), the engine for both of which is further down
//! (`LayoutView`, `FlexTree`, `lay_out` — one taffy tree, each container laid by
//! its own `display`). The insets half is described first because it came first.
//!
//! **CSS, under CSS's names.** A child of a frame may carry `top`, `right`,
//! `bottom` and `left` insets, each in px or % of the frame, and `margin: auto`
//! on any side. Where it does, the child's **used** geometry — where it is drawn,
//! §15 D868 — is computed from the insets and the frame's size, so resizing the
//! frame moves or stretches the child. A child with no insets keeps the position
//! its transform gives it, which is the *implicit* `left`/`top`, so every document
//! written before this module is unchanged by it.
//!
//! Three of the maintainer's rulings shape the arithmetic, and one decision
//! frames it (§15 D874 — the fourth ruling is the inspector's pin diagram and
//! fields, which is `panels::inspector`'s business):
//!
//! - **Both insets on an axis stretch** the child between them — frame size less
//!   both insets — for every kind with a size of its own. CSS keeps a *replaced*
//!   element at its intrinsic size there, and §15 D872 made shapes replaced
//!   elements; this is the one place that ruling gives way, because pinning both
//!   edges and not stretching is not what anyone pinning both edges means. A kind
//!   that cannot stretch is over-constrained the CSS way: the end inset is ignored.
//! - **`margin: auto` on an axis with both insets centres** the child (or pushes
//!   it, with one side auto) at its own size instead of stretching it — CSS's
//!   absolute-centring idiom, and the only margin value supported yet.
//! - **Rotation, skew and flip turn about the box centre**, CSS's default
//!   `transform-origin`: layout places the unrotated box and the node's linear
//!   transform is applied around its middle.
//! - **The insets are the specified values and the used geometry is derived.**
//!   Nothing here writes to the document; [`crate::build::keep_insets`] is the one
//!   place an edit to a child's drawn placement becomes new insets.
//!
//! **What stretches** is a kind whose size is a field: `Rect`, `Ellipse`,
//! `Polygon`, `Star`, `Artboard`, and `Text` through its [`TextSizing`]. A path, a
//! line or a boolean is positioned by its insets and keeps its size — stretching
//! one means scaling its points, and pen-editing a stretched path then needs the
//! inverse; that is later work. **A group or a boolean takes no insets at all**
//! yet: its box is its children's, which [`crate::Resolved`] measures *after* it
//! places them, so there is nothing to place it by. The field is stored on either
//! and inert, as `Node::grids` is on a layer nobody offers grids for.

use crate::geometry;
use crate::node::{NodeKind, TextSizing};
use kurbo::{Affine, Point, Rect, Size, Vec2};
use serde::{Deserialize, Serialize};

/// A CSS `<length-percentage>`: world units, or a percentage of the frame.
///
/// **Percent is stored as typed** — `Percent(25.0)` is `25%` — rather than as a
/// fraction, because it is a number the user reads back in a field and CSS writes
/// it that way. Horizontal insets resolve against the frame's width and vertical
/// ones against its height, which is CSS's rule for `left`/`right` and
/// `top`/`bottom` (not its rule for padding, which is width-only).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LengthPct {
    Px(f64),
    Percent(f64),
}

impl LengthPct {
    /// This length in world units, against a container `extent` wide.
    pub fn resolve(self, extent: f64) -> f64 {
        match self {
            LengthPct::Px(v) => v,
            LengthPct::Percent(p) => extent * p / 100.0,
        }
    }

    /// `px` world units spelled in **this length's unit** — the inverse of
    /// [`Self::resolve`], which is how moving a child keeps a percentage inset a
    /// percentage. A zero `extent` has no percentage to express, so the length is
    /// kept as it was rather than divided by nothing.
    pub fn rewritten(self, px: f64, extent: f64) -> LengthPct {
        match self {
            LengthPct::Px(_) => LengthPct::Px(px),
            LengthPct::Percent(p) if extent == 0.0 => LengthPct::Percent(p),
            LengthPct::Percent(_) => LengthPct::Percent(px / extent * 100.0),
        }
    }

    fn is_finite(self) -> bool {
        match self {
            LengthPct::Px(v) | LengthPct::Percent(v) => v.is_finite(),
        }
    }
}

/// Which sides carry `margin: auto` — the only margin value there is yet
/// (§15 D874). A bare `bool` per side because `auto` is the whole of the vocabulary;
/// a numeric margin is deferred with the rest of §5.3c's list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoMargins {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub top: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub right: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bottom: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub left: bool,
}

impl AutoMargins {
    pub fn is_none(&self) -> bool {
        *self == AutoMargins::default()
    }
}

/// A child's CSS insets and auto margins — its `position: absolute` properties
/// (§15 D871). All unset is the default and means *placed by its transform*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<LengthPct>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<LengthPct>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<LengthPct>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<LengthPct>,
    #[serde(default, skip_serializing_if = "AutoMargins::is_none")]
    pub margin_auto: AutoMargins,
}

impl Insets {
    /// Nothing set at all — the default, and what the save format skips.
    pub fn is_unset(&self) -> bool {
        *self == Insets::default()
    }

    /// Whether any inset is set. **Margins alone do not count**: an auto margin
    /// only means something between two insets, so a node with margins and no
    /// insets is placed exactly as one with neither.
    pub fn is_authored(&self) -> bool {
        self.top.is_some() || self.right.is_some() || self.bottom.is_some() || self.left.is_some()
    }

    /// Every set length finite. The operation that writes these refuses anything
    /// else, as `SetTransform` refuses a non-finite matrix.
    pub fn is_finite(&self) -> bool {
        [self.top, self.right, self.bottom, self.left]
            .into_iter()
            .flatten()
            .all(LengthPct::is_finite)
    }

    fn horizontal(&self) -> AxisInsets {
        AxisInsets {
            start: self.left,
            end: self.right,
            auto_start: self.margin_auto.left,
            auto_end: self.margin_auto.right,
        }
    }

    fn vertical(&self) -> AxisInsets {
        AxisInsets {
            start: self.top,
            end: self.bottom,
            auto_start: self.margin_auto.top,
            auto_end: self.margin_auto.bottom,
        }
    }
}

/// One axis of [`Insets`]: the start and end insets and their auto margins.
#[derive(Clone, Copy)]
struct AxisInsets {
    start: Option<LengthPct>,
    end: Option<LengthPct>,
    auto_start: bool,
    auto_end: bool,
}

impl AxisInsets {
    /// Where the box starts and how long it is along this axis, in a container
    /// `extent` long, for a box `size` long whose position with no insets would
    /// be `implicit`. The whole of CSS's absolute-positioning rule for one axis,
    /// with §15 D874's two rulings in it: both insets stretch when `can_stretch`,
    /// and auto margins between two insets centre or push instead.
    fn resolve(self, extent: f64, size: f64, implicit: f64, can_stretch: bool) -> (f64, f64) {
        match (self.start, self.end) {
            (None, None) => (implicit, size),
            (Some(start), None) => (start.resolve(extent), size),
            (None, Some(end)) => (extent - end.resolve(extent) - size, size),
            (Some(start), Some(end)) => {
                let (s, e) = (start.resolve(extent), end.resolve(extent));
                if self.auto_start || self.auto_end {
                    let free = extent - s - e - size;
                    let share = match (self.auto_start, self.auto_end) {
                        (true, true) => free / 2.0,
                        (true, false) => free,
                        _ => 0.0,
                    };
                    (s + share, size)
                } else if can_stretch {
                    // Clamped at zero: insets wider than the frame leave nothing
                    // to stretch into, and a negative size is an inside-out box.
                    (s, (extent - s - e).max(0.0))
                } else {
                    // Over-constrained the CSS way: the end inset is ignored.
                    (s, size)
                }
            }
        }
    }

    /// The insets that put this axis's box at `pos`, `size` long — the inverse of
    /// [`Self::resolve`], each inset kept in its own unit.
    ///
    /// An axis with no insets is left alone: its position is the transform's, and
    /// [`crate::build::keep_insets`] writes that. Between two auto-margined insets
    /// the move is taken up by shifting **both** insets the same way, which keeps
    /// the box centred (or pushed) in a region that moved with it — the one reading
    /// of "drag a centred thing" that leaves it centred.
    fn inverse(self, extent: f64, pos: f64, size: f64, can_stretch: bool) -> AxisInsets {
        // **An axis the edit did not move comes back untouched**, not rewritten
        // through a divide and a multiply: a nudge along x must not turn a `33.3%`
        // top inset into its float-noise neighbour and commit that as a change.
        let (now, now_size) = self.resolve(extent, size, pos, can_stretch);
        if (now - pos).abs() < 1e-9 && (now_size - size).abs() < 1e-9 {
            return self;
        }
        let mut out = self;
        match (self.start, self.end) {
            (None, None) => {}
            (Some(start), None) => out.start = Some(start.rewritten(pos, extent)),
            (None, Some(end)) => out.end = Some(end.rewritten(extent - pos - size, extent)),
            (Some(start), Some(end)) => {
                if self.auto_start || self.auto_end {
                    let (now, _) = self.resolve(extent, size, 0.0, can_stretch);
                    let delta = pos - now;
                    let s = start.resolve(extent) + delta;
                    let e = end.resolve(extent) - delta;
                    out.start = Some(start.rewritten(s, extent));
                    out.end = Some(end.rewritten(e, extent));
                } else if can_stretch {
                    out.start = Some(start.rewritten(pos, extent));
                    out.end = Some(end.rewritten(extent - pos - size, extent));
                } else {
                    // The end inset is ignored on this axis, so only the start moves.
                    out.start = Some(start.rewritten(pos, extent));
                }
            }
        }
        out
    }
}

/// Whether `kind` has a size of its own to stretch — see the module doc.
pub fn can_stretch(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Artboard { .. }
            | NodeKind::Text { .. }
    )
}

/// Whether `kind` can be placed by insets at all — every kind with a box of its
/// own. A group and a boolean take their box from their children and are not
/// (see the module doc).
pub fn takes_insets(kind: &NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Group | NodeKind::Root | NodeKind::Boolean { .. }
    )
}

/// `kind` resized to `size` on the axes that `stretched` says moved.
///
/// A size kind takes the size whole. **Text keeps its sizing mode's meaning** as
/// far as it can: a stretched width is a wrap width, so auto-width text wraps at
/// it (`AutoHeight`); a stretched height has nowhere to go but a fixed box, so the
/// node becomes `Fixed` at its current width and the stretched height. Anything
/// else comes back unchanged — [`can_stretch`] is the gate, and this is only asked
/// behind it.
pub fn resized(kind: &NodeKind, size: Size, stretched: (bool, bool)) -> NodeKind {
    let mut out = kind.clone();
    match &mut out {
        NodeKind::Rect { size: s, .. }
        | NodeKind::Ellipse { size: s }
        | NodeKind::Polygon { size: s, .. }
        | NodeKind::Star { size: s, .. }
        | NodeKind::Artboard { size: s, .. } => *s = size,
        NodeKind::Text { sizing, .. } => {
            *sizing = match (*sizing, stretched) {
                (_, (false, false)) => *sizing,
                (TextSizing::Fixed(s), (true, false)) => {
                    TextSizing::Fixed(Size::new(size.width, s.height))
                }
                (_, (true, false)) => TextSizing::AutoHeight(size.width),
                (_, (_, true)) => TextSizing::Fixed(size),
            };
        }
        _ => {}
    }
    out
}

/// A text node's kind at the `size` a flex or grid container gave it, **each
/// sizing mode keeping its meaning** (§15 D875, the maintainer's ruling; D913's
/// first for grid): auto width stays auto (one line) unless the container
/// stretched or grew it, when it becomes a fixed box at that size — **never
/// narrower than its line**, so it cannot wrap (§15 D917: a stretch can give less
/// room as well as more); auto height wraps at the width it was given, becoming
/// fixed only if the container also made it taller than its lines; a fixed box
/// takes the size whole.
pub fn flexed_text(kind: &NodeKind, size: Size) -> NodeKind {
    let NodeKind::Text { sizing, .. } = kind else {
        return kind.clone();
    };
    let near = |a: f64, b: f64| (a - b).abs() < 1.0 / 128.0;
    let with = |s: TextSizing| {
        let mut k = kind.clone();
        if let NodeKind::Text { sizing, .. } = &mut k {
            *sizing = s;
        }
        k
    };
    match sizing {
        // `local_bounds` answers for every text kind (`TextRef::of` refuses only
        // a kind that is not text), so the floor below always applies; the
        // fallback is the size whole and is unreachable.
        TextSizing::Auto => match geometry::local_bounds(kind, None) {
            Some(b) if near(b.width(), size.width) && near(b.height(), size.height) => kind.clone(),
            // 🚨 **Never narrower than its line** (§15 D917): a stretch can hand an
            // auto-width label *less* room than its line — a flex column or a grid
            // column narrower than the text — and a fixed box that narrow wraps it,
            // which auto width never does (D875, D913's first ruling). The box keeps
            // the line's width and overflows its slot, `white-space: nowrap`'s CSS
            // reading. The sentence above said a stretched box "cannot wrap", true
            // only of one the container made *wider*.
            Some(b) => with(TextSizing::Fixed(Size::new(
                size.width.max(b.width()),
                size.height,
            ))),
            None => with(TextSizing::Fixed(size)),
        },
        TextSizing::AutoHeight(_) => {
            let k = with(TextSizing::AutoHeight(size.width));
            match geometry::local_bounds(&k, None) {
                Some(b) if near(b.height(), size.height) => k,
                _ => with(TextSizing::Fixed(size)),
            }
        }
        TextSizing::Fixed(_) => with(TextSizing::Fixed(size)),
    }
}

/// The linear part of `a` — its rotation, skew, flip and any scale, with the
/// translation taken out.
fn linear(a: Affine) -> Affine {
    let [xx, yx, xy, yy, _, _] = a.as_coeffs();
    Affine::new([xx, yx, xy, yy, 0.0, 0.0])
}

/// Where a box's **slot** starts — the top-left of the unrotated box — for a
/// node drawn with local transform `local` whose box in its own space is `bx`.
///
/// The rotation-about-the-centre rule read backwards: the centre of the box is
/// where the transform puts it, and the slot is half the box up and left of that.
/// For an unrotated node this is simply the translation, which is why a document
/// with no rotation sees no difference at all.
pub fn slot_of(local: Affine, bx: Rect) -> Point {
    let half = Vec2::new(bx.width() / 2.0, bx.height() / 2.0);
    local * (bx.origin() + half) - half
}

/// The local transform that draws a box `bx` (in the node's own space) in the
/// slot starting at `slot`, turned by `linear_of`'s linear part about the box's
/// centre — [`slot_of`]'s inverse.
pub(crate) fn placed_at(slot: Point, bx: Rect, linear_of: Affine) -> Affine {
    let half = Vec2::new(bx.width() / 2.0, bx.height() / 2.0);
    Affine::translate((slot + half).to_vec2())
        * linear(linear_of)
        * Affine::translate(-(bx.origin() + half).to_vec2())
}

/// A child's used geometry under its insets, in a frame `frame` big — or `None`
/// when the insets leave it where its transform puts it.
///
/// `stored` and `kind` are the node's specified transform and kind; the result is
/// the used local transform and, when a stretch changed its size, the used kind.
/// `None` for a kind that takes no insets, for insets that set nothing, and for a
/// kind with no box to place (an empty text node measures to nothing).
pub fn place(
    insets: &Insets,
    frame: Size,
    stored: Affine,
    kind: &NodeKind,
) -> Option<(Affine, Option<NodeKind>)> {
    if !insets.is_authored() || !takes_insets(kind) {
        return None;
    }
    let bx = geometry::local_bounds(kind, None)?;
    let implicit = slot_of(stored, bx);
    let stretchy = can_stretch(kind);
    let (x, w) = insets
        .horizontal()
        .resolve(frame.width, bx.width(), implicit.x, stretchy);
    let (y, h) = insets
        .vertical()
        .resolve(frame.height, bx.height(), implicit.y, stretchy);
    let stretched = (w != bx.width(), h != bx.height());
    let (used_kind, used_box) = if stretched.0 || stretched.1 {
        let k = resized(kind, Size::new(w, h), stretched);
        // Measured again rather than assumed: a text node's box after a new
        // wrap width is whatever the lines come to, and its origin need not be 0.
        let b = geometry::local_bounds(&k, None)?;
        (Some(k), b)
    } else {
        (None, bx)
    };
    Some((placed_at(Point::new(x, y), used_box, stored), used_kind))
}

/// The insets that draw a child where `local` puts a box `bx` (both **used**:
/// the placement an edit wants), in a frame `frame` big — each inset kept in its
/// own unit, and an axis with no insets left to the transform.
///
/// [`place`] read backwards, and what [`crate::build::keep_insets`] calls. `kind`
/// decides only whether the child can stretch, which decides how two insets on one
/// axis are rewritten.
pub fn inverse(insets: &Insets, frame: Size, local: Affine, bx: Rect, kind: &NodeKind) -> Insets {
    let slot = slot_of(local, bx);
    let stretchy = can_stretch(kind);
    let h = insets
        .horizontal()
        .inverse(frame.width, slot.x, bx.width(), stretchy);
    let v = insets
        .vertical()
        .inverse(frame.height, slot.y, bx.height(), stretchy);
    Insets {
        left: h.start,
        right: h.end,
        top: v.start,
        bottom: v.end,
        margin_auto: insets.margin_auto,
    }
}

// ---------------------------------------------------------------------------
// Flexbox (build step 3, §15 D867)
// ---------------------------------------------------------------------------

/// CSS `flex-direction`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    /// Whether the main axis is horizontal.
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }
}

/// CSS `flex-wrap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

/// CSS `justify-content` — the subset a flex container reads.
///
/// **`Start` and `End` are CSS's `flex-start` and `flex-end`**, here and in
/// [`AlignItems`] and [`AlignContent`] (§15 D909): the flow's own start, which
/// `row-reverse` puts on the right and `wrap-reverse` at the bottom of a row. The
/// cards name them so (§15 D884) and draw them so (`Orient`). They were mapped to
/// taffy's `START`/`END` — CSS's physical `start`/`end`, which ignore both
/// reversals — so under either the canvas drew the opposite of the card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JustifyContent {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// CSS `align-items` / `align-self`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignItems {
    #[default]
    Stretch,
    Start,
    End,
    Center,
    Baseline,
}

/// CSS `align-content` — how the lines of a wrapping container share its cross
/// axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignContent {
    #[default]
    Stretch,
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// A flex container's properties — CSS's, under CSS's names (§15 D867).
///
/// **Padding and gap are world units**, not length-percentages: CSS resolves a
/// percentage padding against the container's *width* on both axes, which is a
/// rule nobody reaches for on purpose, and a percentage gap needs a definite
/// container size to mean anything. Both can grow a unit later without changing
/// what an existing file means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Flex {
    #[serde(default, skip_serializing_if = "is_default")]
    pub direction: FlexDirection,
    #[serde(default, skip_serializing_if = "is_default")]
    pub wrap: FlexWrap,
    #[serde(default, skip_serializing_if = "is_default")]
    pub justify_content: JustifyContent,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align_items: AlignItems,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align_content: AlignContent,
    /// The gap between lines (CSS `row-gap`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub row_gap: f64,
    /// The gap between items on a line (CSS `column-gap`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub column_gap: f64,
    /// Top, right, bottom, left — CSS's order.
    #[serde(default, skip_serializing_if = "is_zero4")]
    pub padding: [f64; 4],
}

impl Flex {
    fn is_finite(&self) -> bool {
        self.row_gap.is_finite()
            && self.column_gap.is_finite()
            && self.padding.iter().all(|p| p.is_finite())
    }
}

// ---------------------------------------------------------------------------
// Grid (build step 4, §15 D913, D914)
// ---------------------------------------------------------------------------

/// CSS `grid-auto-flow` — which axis auto-placement fills first. `dense` is not
/// offered (§5.3c's property set).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GridAutoFlow {
    #[default]
    Row,
    Column,
}

/// One CSS `<track-breadth>`, or `auto`: what a single track, or either end of a
/// `minmax()`, sizes by.
///
/// **`Fr` is a max only**: CSS refuses `minmax(1fr, …)`, and so does
/// [`Grid::is_valid`]. Percent is stored as typed, [`LengthPct`]'s reason.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrackBreadth {
    Px(f64),
    Percent(f64),
    Fr(f64),
    Auto,
    MinContent,
    MaxContent,
}

impl TrackBreadth {
    fn is_finite(self) -> bool {
        match self {
            TrackBreadth::Px(v) | TrackBreadth::Percent(v) | TrackBreadth::Fr(v) => v.is_finite(),
            TrackBreadth::Auto | TrackBreadth::MinContent | TrackBreadth::MaxContent => true,
        }
    }

    /// CSS's own refusal: no length, percentage or `fr` below zero.
    fn is_valid(self) -> bool {
        match self {
            TrackBreadth::Px(v) | TrackBreadth::Percent(v) | TrackBreadth::Fr(v) => v >= 0.0,
            TrackBreadth::Auto | TrackBreadth::MinContent | TrackBreadth::MaxContent => true,
        }
    }
}

/// One track's size: a breadth, or `minmax(min, max)`.
///
/// **Untagged in the file**, so a track list reads close to its CSS:
/// `{"Px":200.0}`, `"Auto"`, `{"min":{"Px":100.0},"max":{"Fr":2.0}}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TrackSize {
    Breadth(TrackBreadth),
    MinMax {
        min: TrackBreadth,
        max: TrackBreadth,
    },
}

impl TrackSize {
    fn is_finite(self) -> bool {
        match self {
            TrackSize::Breadth(b) => b.is_finite(),
            TrackSize::MinMax { min, max } => min.is_finite() && max.is_finite(),
        }
    }

    fn is_valid(self) -> bool {
        match self {
            TrackSize::Breadth(b) => b.is_valid(),
            TrackSize::MinMax { min, max } => {
                min.is_valid() && max.is_valid() && !matches!(min, TrackBreadth::Fr(_))
            }
        }
    }
}

/// One entry of `grid-template-columns` or `-rows`: a track, or `repeat(n, …)`.
///
/// **A repeat holds sizes, not entries**, so it cannot nest — CSS's own grammar,
/// made unrepresentable rather than refused. `auto-fill`/`auto-fit` are deferred
/// (§5.3c), so the count is a number. Untagged in the file, [`TrackSize`]'s
/// reason: `{"repeat":3,"tracks":["Auto"]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Track {
    Size(TrackSize),
    Repeat { repeat: u16, tracks: Vec<TrackSize> },
}

impl Track {
    fn is_finite(&self) -> bool {
        match self {
            Track::Size(s) => s.is_finite(),
            Track::Repeat { tracks, .. } => tracks.iter().all(|s| s.is_finite()),
        }
    }

    /// A repeat must repeat something at least once — `repeat(0, …)` and
    /// `repeat(2, )` are both refused by CSS.
    fn is_valid(&self) -> bool {
        match self {
            Track::Size(s) => s.is_valid(),
            Track::Repeat { repeat, tracks } => {
                *repeat > 0 && !tracks.is_empty() && tracks.iter().all(|s| s.is_valid())
            }
        }
    }
}

/// A grid container's properties — CSS's, under CSS's names (§15 D867, D914).
///
/// **Content alignment is [`AlignContent`] on both axes**, `justify-content`
/// included, because a grid container's `normal` content distribution behaves as
/// `stretch` on both axes (CSS Box Alignment; taffy's own default, too): an `auto`
/// track grows into the free space unless told otherwise, and
/// [`JustifyContent`] — flex's set — has no `stretch` to say that with. `Start`
/// and `End` are the grid's own start and end; a grid has no reversal for
/// `flex-start` to follow, so the two readings are one here (§15 D909's question
/// does not arise).
///
/// Padding and gap are world units, [`Flex`]'s reason. An empty track list is
/// CSS's `none`: every track is implicit and `auto`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// `grid-template-columns`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Track>,
    /// `grid-template-rows`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Track>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub auto_flow: GridAutoFlow,
    #[serde(default, skip_serializing_if = "is_default")]
    pub justify_content: AlignContent,
    /// `justify-items` — each item across its area, horizontally. **Not in
    /// §5.3c's first cut, and added with grid** (§15 D914): in a grid
    /// `align-items` is the vertical axis alone, so without it the only way to
    /// stop every item stretching sideways is `justify-self` on each one.
    ///
    /// **`None` is CSS's `normal`**, and it is not `stretch` (§15 D915, the
    /// maintainer's CSS-parity ruling): a replaced item — a shape (§15 D872) —
    /// sits at the start of its area at its own size, and a box (text, a laid
    /// group) stretches across it. `Some(Stretch)` stretches both. Flex keeps a
    /// plain [`AlignItems`], its `normal` being `stretch` for everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justify_items: Option<AlignItems>,
    /// `align-items` — vertically; `None` is `normal`, [`Self::justify_items`]'
    /// reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_items: Option<AlignItems>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align_content: AlignContent,
    /// The gap between rows (CSS `row-gap`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub row_gap: f64,
    /// The gap between columns (CSS `column-gap`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub column_gap: f64,
    /// Top, right, bottom, left — CSS's order.
    #[serde(default, skip_serializing_if = "is_zero4")]
    pub padding: [f64; 4],
}

impl Grid {
    fn is_finite(&self) -> bool {
        self.row_gap.is_finite()
            && self.column_gap.is_finite()
            && self.padding.iter().all(|p| p.is_finite())
            && self.columns.iter().chain(&self.rows).all(Track::is_finite)
    }

    /// Every track one CSS accepts. The operation that writes a grid refuses
    /// anything else (`OpError::BadLayout`); a file that carries one anyway opens,
    /// and is laid as [`style_of`] reads it — read around at layout, **kept as
    /// stored** and saved back as written. §15 D492's asymmetry (refused at the
    /// operation, not at the door) without its repair: D492's loader drops a bad
    /// export spec, and nothing here is rewritten at load.
    ///
    /// **And no more than [`MAX_TEMPLATE_TRACKS`] explicit tracks on an axis**
    /// (§15 D919).
    pub fn is_valid(&self) -> bool {
        self.columns.iter().chain(&self.rows).all(Track::is_valid)
            && track_count(&self.columns) <= MAX_TEMPLATE_TRACKS
            && track_count(&self.rows) <= MAX_TEMPLATE_TRACKS
    }
}

/// The most explicit tracks a template makes on one axis (§15 D919). CSS lets a
/// user agent clamp an overly large grid (*"Clamping Overly Large Grids"*), and
/// browsers do; a thousand is far past any layout drawn by hand and far short of
/// what a `repeat(65535, …)` — the most a `u16` count asks for — times a few
/// entries would make taffy allocate. The operation refuses a template over it;
/// a file's is laid to it ([`template`]).
///
/// **Not `layout::MAX_TRACKS`**, the most bands one of §5.3b's layout grids
/// draws (§15 D488). This was called `MAX_TRACKS` too until §15 D924 — two
/// unrelated caps, one name, in the one project where "grid" already means two
/// things.
pub const MAX_TEMPLATE_TRACKS: usize = 1000;

/// How many explicit tracks `tracks` makes, saturating — a repeat counts its
/// tracks `repeat` times. The Container card's *N tracks*.
pub fn track_count(tracks: &[Track]) -> usize {
    tracks
        .iter()
        .map(|t| match t {
            Track::Size(_) => 1,
            Track::Repeat { repeat, tracks } => usize::from(*repeat).saturating_mul(tracks.len()),
        })
        .fold(0usize, usize::saturating_add)
}

/// One end of an item's `grid-column` or `grid-row`: `auto`, a line number, or
/// `span n`. Line numbers count from 1, and from −1 at the far end, as CSS's do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GridPlacement {
    #[default]
    Auto,
    Line(i16),
    Span(u16),
}

impl GridPlacement {
    /// CSS refuses line 0 and `span 0`.
    fn is_valid(self) -> bool {
        match self {
            GridPlacement::Auto => true,
            GridPlacement::Line(n) => n != 0,
            GridPlacement::Span(n) => n != 0,
        }
    }
}

/// An item's `grid-column` or `grid-row`: its start and end, CSS's `start / end`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridLines {
    #[serde(default, skip_serializing_if = "is_default")]
    pub start: GridPlacement,
    #[serde(default, skip_serializing_if = "is_default")]
    pub end: GridPlacement,
}

impl GridLines {
    fn is_valid(self) -> bool {
        self.start.is_valid() && self.end.is_valid()
    }
}

// --- grid values as CSS text (§15 D920) -------------------------------------
//
// The Container card's track list has an editable CSS line under it, for pasting
// a template from a browser's inspector, and the Item card's lines are typed as
// CSS writes them. The text is CSS's own, so what is pasted in is what a browser
// would read; the parser is small because the property set is (§5.3c): lengths,
// percentages, `fr`, the three keywords, `minmax()` and `repeat(n, …)`. It does
// not validate — [`Grid::is_valid`] is the one judge, at the operation — so a
// parse that succeeds can still be refused.

/// A number as CSS writes it: no trailing zeros, no trailing point, at most four
/// decimals (the 1/64 grid needs six, and nothing typed needs more than four).
fn css_number(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn breadth_css(b: TrackBreadth) -> String {
    match b {
        TrackBreadth::Px(v) => format!("{}px", css_number(v)),
        TrackBreadth::Percent(v) => format!("{}%", css_number(v)),
        TrackBreadth::Fr(v) => format!("{}fr", css_number(v)),
        TrackBreadth::Auto => "auto".into(),
        TrackBreadth::MinContent => "min-content".into(),
        TrackBreadth::MaxContent => "max-content".into(),
    }
}

/// One track size as CSS text — `100px`, `minmax(100px, 2fr)`.
pub fn track_size_css(s: TrackSize) -> String {
    match s {
        TrackSize::Breadth(b) => breadth_css(b),
        TrackSize::MinMax { min, max } => {
            format!("minmax({}, {})", breadth_css(min), breadth_css(max))
        }
    }
}

/// A track list as CSS writes `grid-template-columns` — `200px 1fr repeat(3,
/// auto)`, and `none` for an empty one.
pub fn tracks_css(tracks: &[Track]) -> String {
    if tracks.is_empty() {
        return "none".into();
    }
    tracks
        .iter()
        .map(|t| match t {
            Track::Size(s) => track_size_css(*s),
            Track::Repeat { repeat, tracks } => format!(
                "repeat({repeat}, {})",
                tracks
                    .iter()
                    .map(|s| track_size_css(*s))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A track list from CSS text, the inverse of [`tracks_css`]: whitespace-
/// separated sizes, `minmax(min, max)` and `repeat(n, sizes…)`, keywords in any
/// case, and a unitless `0` for `0px` as CSS allows. `none` or nothing is the
/// empty list. The error names what could not be read.
pub fn parse_tracks(text: &str) -> Result<Vec<Track>, String> {
    let mut p = CssText::new(text);
    if p.rest().eq_ignore_ascii_case("none") || p.rest().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    while !p.done() {
        if p.eat_word("repeat(") {
            let count = p.integer()?;
            let repeat = u16::try_from(count).map_err(|_| format!("repeat count {count}"))?;
            p.expect(',')?;
            let mut tracks = Vec::new();
            while !p.eat(')') {
                if p.done() {
                    return Err("repeat( is not closed".into());
                }
                tracks.push(p.size()?);
            }
            out.push(Track::Repeat { repeat, tracks });
        } else {
            out.push(Track::Size(p.size()?));
        }
    }
    Ok(out)
}

/// One end of `grid-column`/`grid-row` as CSS text — `auto`, `2`, `-1`, `span 2`.
pub fn placement_css(p: GridPlacement) -> String {
    match p {
        GridPlacement::Auto => "auto".into(),
        GridPlacement::Line(n) => n.to_string(),
        GridPlacement::Span(n) => format!("span {n}"),
    }
}

/// One end of `grid-column`/`grid-row` from CSS text, the inverse of
/// [`placement_css`]. `None` for anything else — line 0 and `span 0` included,
/// which CSS refuses.
pub fn parse_placement(text: &str) -> Option<GridPlacement> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("auto") {
        return Some(GridPlacement::Auto);
    }
    if let Some(n) = t
        .get(..4)
        .filter(|w| w.eq_ignore_ascii_case("span"))
        .map(|_| t[4..].trim())
    {
        return n
            .parse::<u16>()
            .ok()
            .filter(|n| *n != 0)
            .map(GridPlacement::Span);
    }
    t.parse::<i16>()
        .ok()
        .filter(|n| *n != 0)
        .map(GridPlacement::Line)
}

/// A cursor over CSS text for [`parse_tracks`] — whitespace skipped before every
/// token.
struct CssText<'a> {
    rest: &'a str,
}

impl<'a> CssText<'a> {
    fn new(text: &'a str) -> Self {
        CssText { rest: text.trim() }
    }

    fn rest(&self) -> &'a str {
        self.rest
    }

    fn skip(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn done(&mut self) -> bool {
        self.skip();
        self.rest.is_empty()
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip();
        match self.rest.strip_prefix(c) {
            Some(r) => {
                self.rest = r;
                true
            }
            None => false,
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected '{c}' at \"{}\"", self.rest))
        }
    }

    /// `word`, case-insensitively, if the text starts with it.
    fn eat_word(&mut self, word: &str) -> bool {
        self.skip();
        match self.rest.get(..word.len()) {
            Some(w) if w.eq_ignore_ascii_case(word) => {
                self.rest = &self.rest[word.len()..];
                true
            }
            _ => false,
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip();
        let end = self
            .rest
            .char_indices()
            .find(|(i, c)| {
                !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+')))
            })
            .map_or(self.rest.len(), |(i, _)| i);
        let (n, r) = self.rest.split_at(end);
        let v = n
            .parse::<f64>()
            .map_err(|_| format!("expected a number at \"{}\"", self.rest))?;
        self.rest = r;
        Ok(v)
    }

    fn integer(&mut self) -> Result<i64, String> {
        let v = self.number()?;
        if v.fract() == 0.0 {
            Ok(v as i64)
        } else {
            Err(format!("repeat count {v} is not whole"))
        }
    }

    fn breadth(&mut self) -> Result<TrackBreadth, String> {
        for (word, b) in [
            ("min-content", TrackBreadth::MinContent),
            ("max-content", TrackBreadth::MaxContent),
            ("auto", TrackBreadth::Auto),
        ] {
            if self.eat_word(word) {
                return Ok(b);
            }
        }
        let v = self.number()?;
        if self.eat_word("px") {
            Ok(TrackBreadth::Px(v))
        } else if self.eat_word("fr") {
            Ok(TrackBreadth::Fr(v))
        } else if self.eat('%') {
            Ok(TrackBreadth::Percent(v))
        } else if v == 0.0 {
            Ok(TrackBreadth::Px(0.0))
        } else {
            Err(format!("{} needs a unit: px, % or fr", css_number(v)))
        }
    }

    fn size(&mut self) -> Result<TrackSize, String> {
        if self.eat_word("minmax(") {
            let min = self.breadth()?;
            self.expect(',')?;
            let max = self.breadth()?;
            self.expect(')')?;
            Ok(TrackSize::MinMax { min, max })
        } else {
            Ok(TrackSize::Breadth(self.breadth()?))
        }
    }
}

/// How a container lays out its children — CSS `display`. **`None` on the node
/// is not CSS's `display: none`** (which hides): it is a container with no layout
/// of its own, whose children are placed by their transforms and insets, which is
/// what every frame was before this and what a group has always been.
///
/// **Not `Copy` since grid** (§15 D914): a track list is a `Vec`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Display {
    Flex(Flex),
    Grid(Grid),
}

impl Display {
    pub fn is_finite(&self) -> bool {
        match self {
            Display::Flex(f) => f.is_finite(),
            Display::Grid(g) => g.is_finite(),
        }
    }

    /// Every value one CSS accepts — [`Grid::is_valid`]; a flex container has no
    /// value CSS refuses that is not also non-finite.
    pub fn is_valid(&self) -> bool {
        match self {
            Display::Flex(_) => true,
            Display::Grid(g) => g.is_valid(),
        }
    }

    /// Top, right, bottom, left — either layout's.
    pub fn padding(&self) -> [f64; 4] {
        match self {
            Display::Flex(f) => f.padding,
            Display::Grid(g) => g.padding,
        }
    }

    /// Either layout's padding, to write.
    pub fn padding_mut(&mut self) -> &mut [f64; 4] {
        match self {
            Display::Flex(f) => &mut f.padding,
            Display::Grid(g) => &mut g.padding,
        }
    }

    /// Either layout's `column-gap` and `row-gap`, to write — the two properties
    /// flex and grid share by name (the Container card's gap row serves both).
    pub fn gaps_mut(&mut self) -> (&mut f64, &mut f64) {
        match self {
            Display::Flex(f) => (&mut f.column_gap, &mut f.row_gap),
            Display::Grid(g) => (&mut g.column_gap, &mut g.row_gap),
        }
    }

    /// `(column-gap, row-gap)`.
    pub fn gaps(&self) -> (f64, f64) {
        match self {
            Display::Flex(f) => (f.column_gap, f.row_gap),
            Display::Grid(g) => (g.column_gap, g.row_gap),
        }
    }
}

/// A CSS size: `auto`, a length, a percentage of the container, or `fit-content`
/// (hug the content — a container's shrink-wrap).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Dimension {
    #[default]
    Auto,
    Px(f64),
    Percent(f64),
    FitContent,
}

impl Dimension {
    fn is_finite(self) -> bool {
        match self {
            Dimension::Px(v) | Dimension::Percent(v) => v.is_finite(),
            Dimension::Auto | Dimension::FitContent => true,
        }
    }
}

/// A layer's properties **as an item of a layout** — CSS's size properties,
/// `flex-*`, `grid-column`/`grid-row` and the self-alignments (§15 D867, D914).
///
/// **One record for both layouts, as CSS keeps them on the element**: which fields
/// are read depends on the parent's `display` — `grow`, `shrink` and `basis` under
/// flex, `grid_column`, `grid_row` and `justify_self` under grid, the sizes and
/// `align_self` under both — so a layer whose container turns from flex to grid
/// keeps its grid placement, and back. It was `FlexItem` until grid (§15 D914).
///
/// **A shape's or a frame's own stored size is its CSS width and height** — the
/// replaced-element reading §15 D872 decided — so `width`/`height` stay `Auto`
/// for them and resizing one goes on writing its geometry. The two fields exist
/// for what has no size of its own: a group with `display`, whose box is `Auto`
/// (hug) until a size is typed, and a frame asked to hug with `FitContent`.
///
/// **`flex-shrink` defaults to 1, CSS's default**, and does not squeeze a shape
/// below its own size, because a replaced element's automatic minimum is its
/// intrinsic size — the §15 D872 spike's 40-stays-40.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutItem {
    #[serde(default, skip_serializing_if = "is_default")]
    pub width: Dimension,
    #[serde(default, skip_serializing_if = "is_default")]
    pub height: Dimension,
    #[serde(default, skip_serializing_if = "is_default")]
    pub min_width: Dimension,
    #[serde(default, skip_serializing_if = "is_default")]
    pub min_height: Dimension,
    #[serde(default, skip_serializing_if = "is_default")]
    pub max_width: Dimension,
    #[serde(default, skip_serializing_if = "is_default")]
    pub max_height: Dimension,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub grow: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub shrink: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub basis: Dimension,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_self: Option<AlignItems>,
    /// `justify-self`, read under grid; `None` is `auto` — the container's
    /// `justify-items` ([`Grid::justify_items`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justify_self: Option<AlignItems>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub grid_column: GridLines,
    #[serde(default, skip_serializing_if = "is_default")]
    pub grid_row: GridLines,
}

impl Default for LayoutItem {
    fn default() -> Self {
        LayoutItem {
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Auto,
            min_height: Dimension::Auto,
            max_width: Dimension::Auto,
            max_height: Dimension::Auto,
            grow: 0.0,
            shrink: 1.0,
            basis: Dimension::Auto,
            align_self: None,
            justify_self: None,
            grid_column: GridLines::default(),
            grid_row: GridLines::default(),
        }
    }
}

impl LayoutItem {
    /// All at CSS's defaults — what the save format skips.
    pub fn is_default(&self) -> bool {
        *self == LayoutItem::default()
    }

    /// Every number finite; the operation that writes this refuses anything else.
    pub fn is_finite(&self) -> bool {
        [
            self.width,
            self.height,
            self.min_width,
            self.min_height,
            self.max_width,
            self.max_height,
            self.basis,
        ]
        .into_iter()
        .all(Dimension::is_finite)
            && self.grow.is_finite()
            && self.shrink.is_finite()
    }

    /// Every grid line one CSS accepts: no line 0 and no `span 0`. The operation
    /// that writes this refuses anything else; the engine reads one from a file as
    /// `auto` ([`style_of`]).
    pub fn is_valid(&self) -> bool {
        self.grid_column.is_valid() && self.grid_row.is_valid()
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_zero4(v: &[f64; 4]) -> bool {
    v.iter().all(|x| *x == 0.0)
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

/// Round a length taffy computed in `f32` onto the 1/64 px grid (§15 D873, the
/// maintainer's ruling) — Chrome's own layout unit, so what comes back is
/// CSS-faithful and no float-noise digit reaches a field or a file.
pub fn quantize(v: f32) -> f64 {
    (f64::from(v) * 64.0).round() / 64.0
}

/// The questions the layout engine asks of a tree — answered from the committed
/// document by `Resolved`, and from a preview's overrides by `RenderOverrides`,
/// so the two run one layout rather than two (`boolean::Operands`' shape).
///
/// Every answer is a **specified** value — the kind with its stored size, the
/// transform as typed — because layout is what turns specified into used.
pub trait LayoutView {
    fn parent(&self, id: crate::NodeId) -> Option<crate::NodeId>;
    fn children(&self, id: crate::NodeId) -> Vec<crate::NodeId>;
    fn kind(&self, id: crate::NodeId) -> Option<NodeKind>;
    /// Borrowed, since grid (§15 D914): a grid's track list is a `Vec`, and most
    /// callers only ask whether there is a layout at all.
    fn display(&self, id: crate::NodeId) -> Option<&Display>;
    fn item(&self, id: crate::NodeId) -> LayoutItem;
    fn insets(&self, id: crate::NodeId) -> Insets;
    fn visible(&self, id: crate::NodeId) -> bool;
    fn mask(&self, id: crate::NodeId) -> bool;
    fn local(&self, id: crate::NodeId) -> Affine;
}

/// Whether `kind` can be a layout container, flex or grid: a frame, or a group
/// (§15 D869).
pub fn is_container(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Artboard { .. } | NodeKind::Group)
}

/// Whether `id` takes part in its container's flow: visible, not a mask, and not
/// taken out by insets (which place it absolutely, `position: absolute`'s
/// reading, §15 D874). **A hidden layer leaves the flow** — CSS's `display: none`
/// rather than `visibility: hidden`, the design-tool reading of an eye switched
/// off: the session's default in §15 D875, **ruled by the maintainer in §15
/// D881**, when the Item card came to say so in words.
pub fn in_flow(view: &dyn LayoutView, id: crate::NodeId) -> bool {
    view.visible(id) && !view.mask(id) && !view.insets(id).is_authored()
}

/// Whether `id` is a **replaced element** to the layout engine (§15 D872) — what
/// `FlexTree::push` makes a `Leaf::Replaced`: anything but text and a container
/// with a layout of its own. Asked where CSS treats the two differently, which is
/// grid's `normal` (§15 D915).
pub fn is_replaced(view: &dyn LayoutView, id: crate::NodeId) -> bool {
    match view.kind(id) {
        Some(NodeKind::Text { .. }) | None => false,
        Some(k) => !(is_container(&k) && view.display(id).is_some()),
    }
}

/// Whether `id`'s parent lays its children out — a frame or a group with a
/// `display`.
pub fn parent_lays_out(view: &dyn LayoutView, id: crate::NodeId) -> bool {
    view.parent(id).is_some_and(|p| {
        view.display(p).is_some() && view.kind(p).is_some_and(|k| is_container(&k))
    })
}

/// Whether `id` roots a layout pass: a container with a layout that is **not**
/// itself laid out by one — the top of a chain of nested layout containers, which
/// [`lay_out`] lays out whole.
pub fn is_layout_root(view: &dyn LayoutView, id: crate::NodeId) -> bool {
    view.display(id).is_some()
        && view.kind(id).is_some_and(|k| is_container(&k))
        && !(parent_lays_out(view, id) && in_flow(view, id))
}

/// The topmost layout root a change to `id` has to re-lay — `id` itself, or the
/// topmost container above it along an unbroken chain of in-flow items. **The
/// widening step 1 owed**: under flex a node moves when a sibling grows, so a
/// change to any item re-lays its whole chain from here.
///
/// ⚠️ **Not always the only pass that places `id`** (§15 D911): since the chain
/// climbs through plain groups (below), a layout nested in one between `id` and
/// the answer is a root of its own, which this root's pass stops short of. A
/// caller re-laying a change runs every layout root on the way up too.
///
/// ⚠️ **The first hop counts for any child of a flex container, in flow or not**:
/// a layer pinned, hidden or made a mask *leaves* the flow, and its siblings close
/// up — so a change to it is a change to its container's layout even though, by
/// then, it is not in it. The rebuild comparison caught the first cut, which asked
/// the dirty node whether it was in flow and stopped at itself. Only the hops
/// above need the container to be in its own parent's flow.
///
/// 🚨 **And the chain runs through a group or a boolean with no layout of its
/// own** (§15 D899): such a node is one atomic box to its container, measured
/// from its children ([`atomic_box`]), so moving a shape inside it resizes it and
/// reflows the row it sits in. Stopping there left `update` re-laying nothing —
/// found by the randomized layout guard at its seventh seed, a rect moved inside
/// a plain group in a flex column, the group's slot stale until a rebuild. **The
/// climb only counts if it reaches a layout**: `root` moves only on a hop into a
/// container that lays its children out, so a deep tree of plain groups — an SVG
/// import — still answers the node itself, and an edit there re-derives what it
/// always did.
pub fn chain_root(view: &dyn LayoutView, id: crate::NodeId) -> crate::NodeId {
    let mut root = id;
    let mut cursor = id;
    let mut first = true;
    while let Some(parent) = view.parent(cursor) {
        if parent_lays_out(view, cursor) {
            if !(first || in_flow(view, cursor)) {
                break;
            }
            root = parent;
        } else if !matches!(
            view.kind(parent),
            Some(NodeKind::Group | NodeKind::Boolean { .. })
        ) {
            break;
        }
        // Up one: into the container, or through an atomic box its item is
        // measured by, `root` staying put until a layout is found above it.
        cursor = parent;
        first = false;
    }
    root
}

/// What layout makes of one node: the local transform it is drawn with, the kind
/// it is sized with, and — for a group with a layout — its box. Each `None` means
/// *as specified*.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub local: Option<Affine>,
    pub kind: Option<NodeKind>,
    pub frame: Option<Size>,
}

/// Approximately equal on the 1/64 grid layout results land on.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0 / 128.0
}

/// A flex item's used geometry from where its container laid it (`slot`, in the
/// parent's space) and at what `size` (§15 D867).
///
/// A group with a layout keeps its size as a box ([`Placed::frame`]); a group or
/// a boolean without one is placed whole and keeps its own size; a sized kind is
/// resized to what it was given; text keeps each sizing mode's meaning
/// ([`flexed_text`], §15 D875). Every placement turns about the box's centre, the
/// step-2 rule (§15 D874). `None` when the result is exactly the specified
/// geometry.
pub fn item_placed(
    view: &dyn LayoutView,
    id: crate::NodeId,
    slot: Point,
    size: Size,
) -> Option<Placed> {
    let kind = view.kind(id)?;
    let stored = view.local(id);
    let (used_kind, bx, frame) = match &kind {
        NodeKind::Group if view.display(id).is_some() => {
            (None, Rect::from_origin_size(Point::ZERO, size), Some(size))
        }
        NodeKind::Group | NodeKind::Boolean { .. } => (None, atomic_box(view, id, &kind)?, None),
        NodeKind::Text { .. } => {
            let k = flexed_text(&kind, size);
            let bx = geometry::local_bounds(&k, None)?;
            ((k != kind).then_some(k), bx, None)
        }
        _ => {
            let own = geometry::local_bounds(&kind, None)?;
            let stretched = (
                !near(own.width(), size.width),
                !near(own.height(), size.height),
            );
            if (stretched.0 || stretched.1) && can_stretch(&kind) {
                let k = resized(&kind, size, stretched);
                let bx = geometry::local_bounds(&k, None)?;
                (Some(k), bx, None)
            } else {
                (None, own, None)
            }
        }
    };
    let local = placed_at(slot, bx, stored);
    let moved = local
        .as_coeffs()
        .iter()
        .zip(stored.as_coeffs())
        .any(|(a, b)| (a - b).abs() > 1e-9);
    (moved || used_kind.is_some() || frame.is_some()).then_some(Placed {
        local: moved.then_some(local),
        kind: used_kind,
        frame,
    })
}

/// A layout root's size as its own pass made it: a frame asked to hug grows its
/// kind, a group with a layout takes the box. Where the root is *placed* is not
/// the pass's business — that is its parent's (its transform, or insets).
pub fn root_sized(
    view: &dyn LayoutView,
    id: crate::NodeId,
    size: Size,
) -> (Option<NodeKind>, Option<Size>) {
    match view.kind(id) {
        Some(NodeKind::Group) => (None, Some(size)),
        Some(NodeKind::Artboard { size: own })
            if !near(own.width, size.width) || !near(own.height, size.height) =>
        {
            (Some(NodeKind::Artboard { size }), None)
        }
        _ => (None, None),
    }
}

/// One node's result from [`lay_out`]: where its box's top-left lands in its
/// parent's local space, and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Laid {
    pub id: crate::NodeId,
    pub slot: Point,
    pub size: Size,
}

/// Lay out the container `root` — flex or grid — and every container nested in
/// its flow, returning a [`Laid`] for `root` (its size — a container that hugs
/// grows to fit) and for every in-flow node beneath it, each in its parent's space.
///
/// Leaves are measured, not read (§15 D872): a shape's intrinsic size is its
/// geometry, as an image's is, so CSS's automatic minimum keeps it from being
/// squeezed below it; a text node by its sizing mode, each mode meaning what it
/// means outside a container (§15 D875), in a grid cell as in a flex line (§15
/// D913's first ruling); a group or a boolean without a layout
/// as one atomic box, its children's union. Results come back on the 1/64 px grid
/// ([`quantize`]).
pub fn lay_out(view: &dyn LayoutView, root: crate::NodeId) -> Vec<Laid> {
    let mut tree = FlexTree::new(view);
    let Some(r) = tree.push(root) else {
        return Vec::new();
    };
    taffy::compute_root_layout(
        &mut tree,
        taffy::NodeId::from(r),
        taffy::Size {
            width: taffy::AvailableSpace::MaxContent,
            height: taffy::AvailableSpace::MaxContent,
        },
    );
    tree.ids
        .iter()
        .zip(&tree.layouts)
        .map(|(id, l)| Laid {
            id: *id,
            slot: Point::new(quantize(l.location.x), quantize(l.location.y)),
            size: Size::new(quantize(l.size.width), quantize(l.size.height)),
        })
        .collect()
}

/// One axis of a laid grid: each track's start and end in the container's own
/// space, on the 1/64 px grid, implicit tracks included.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidTracks {
    pub spans: Vec<(f64, f64)>,
    /// Implicit tracks **before** the explicit grid — which only an item placed at
    /// a negative line beyond it makes. CSS's line 1 is the start of track
    /// `before`.
    pub before: u16,
    /// Tracks the template made; the rest are implicit.
    pub explicit: u16,
}

impl LaidTracks {
    /// The track under `v`, or the nearest one — a point in a gap goes to the
    /// closer edge, one past either end to the end track. `None` with no tracks.
    pub fn index_at(&self, v: f64) -> Option<usize> {
        let distance = |(a, b): (f64, f64)| {
            if v < a {
                a - v
            } else if v > b {
                v - b
            } else {
                0.0
            }
        };
        (0..self.spans.len()).min_by(|i, j| {
            distance(self.spans[*i])
                .partial_cmp(&distance(self.spans[*j]))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Track `index`'s start line as CSS numbers it — 1 at the explicit grid's
    /// start, so a leading implicit track answers 0 or less.
    pub fn line_of(&self, index: usize) -> i32 {
        index as i32 - i32::from(self.before) + 1
    }
}

/// A grid container as its layout pass left it (§15 D916): its tracks on both
/// axes, and the area each in-flow child was placed in, as CSS line numbers
/// `(column start, column end, row start, row end)`.
///
/// **Derived on demand and never stored** — `Resolved` keeps used geometry and
/// not the passes that made it, so this re-runs the pass (§15 D868's derived,
/// never-saved rule, kept by not keeping it at all). Asked by a drop and by the
/// canvas's track lines, not per frame of every document.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidGrid {
    pub columns: LaidTracks,
    pub rows: LaidTracks,
    pub areas: Vec<(crate::NodeId, [i32; 4])>,
}

/// The tracks and item areas of the grid container `id`, from the pass that
/// lays it out — its own if it is a layout root, else the pass of the root
/// above it along an unbroken chain of in-flow items, which is where its size is
/// decided. `None` if `id` is not a container with a grid.
pub fn laid_grid(view: &dyn LayoutView, id: crate::NodeId) -> Option<LaidGrid> {
    if !matches!(view.display(id), Some(Display::Grid(_))) || !is_container(&view.kind(id)?) {
        return None;
    }
    let mut root = id;
    while parent_lays_out(view, root) && in_flow(view, root) {
        root = view.parent(root)?;
    }
    let mut tree = FlexTree::new(view);
    tree.want = Some(id);
    let r = tree.push(root)?;
    taffy::compute_root_layout(
        &mut tree,
        taffy::NodeId::from(r),
        taffy::Size {
            width: taffy::AvailableSpace::MaxContent,
            height: taffy::AvailableSpace::MaxContent,
        },
    );
    let info = tree.grid.take()?;
    let axis = |t: &taffy::DetailedGridTracksInfo<String>| LaidTracks {
        spans: t
            .positions
            .iter()
            .map(|l| (quantize(l.start), quantize(l.end)))
            .collect(),
        before: t.negative_implicit_tracks,
        explicit: t.explicit_tracks,
    };
    let (columns, rows) = (axis(&info.columns), axis(&info.rows));
    // taffy numbers lines from the whole grid's first; CSS from the explicit's.
    let css = |line: u16, before: u16| i32::from(line) - i32::from(before);
    let index = tree.ids.iter().position(|n| *n == id)?;
    let areas = tree.children[index]
        .iter()
        .zip(&info.items)
        .map(|(child, a)| {
            (
                tree.ids[usize::from(*child)],
                [
                    css(a.column_start, columns.before),
                    css(a.column_end, columns.before),
                    css(a.row_start, rows.before),
                    css(a.row_end, rows.before),
                ],
            )
        })
        .collect();
    Some(LaidGrid {
        columns,
        rows,
        areas,
    })
}

/// What a node in a [`FlexTree`] is, for the engine's purposes.
enum Leaf {
    /// A container with a layout, flex or grid, laid out by taffy.
    Container,
    /// A box with a size of its own — a shape, a frame with no layout, a group or
    /// a boolean taken whole.
    Replaced(Size),
    /// A text node, measured by its sizing mode. Boxed: the parts are several
    /// times the size of the other variants.
    Text(Box<crate::text::TextParts>, String),
}

/// taffy's view of one layout pass: the in-flow part of the subtree under a root
/// container, indexed by position, with each node's style, cache and result.
///
/// **Built per pass and dropped after**, not kept between commits: the document
/// is the one tree, and the incremental part is `Resolved`'s — which containers it
/// re-lays at all.
struct FlexTree<'v> {
    view: &'v dyn LayoutView,
    ids: Vec<crate::NodeId>,
    children: Vec<Vec<taffy::NodeId>>,
    styles: Vec<taffy::Style>,
    leaves: Vec<Leaf>,
    caches: Vec<taffy::Cache>,
    layouts: Vec<taffy::Layout>,
    /// Text measurements already made this pass, keyed by node and the width
    /// asked about — the memo §15 D872 owed: taffy asks a text node the same
    /// question several times a pass, and each answer is a parley shape.
    measured: rustc_hash::FxHashMap<(usize, u32), taffy::Size<f32>>,
    /// The grid container whose tracks this pass was asked for ([`laid_grid`]),
    /// and what taffy reported for it — kept from its **last** full layout, which
    /// is the one its parent places it by. ⚠️ **Not the first**: a parent's
    /// *measure* can lay the grid out in full at another size — taffy's flex
    /// baseline step does, for an item of a hugging row — so the first report can
    /// be of a width the grid never has (`tests/grid.rs`'
    /// `a_grid_laid_out_twice_reports_its_last_layout`, §15 D916's amendment).
    want: Option<crate::NodeId>,
    grid: Option<taffy::DetailedGridInfo<String>>,
}

impl<'v> FlexTree<'v> {
    fn new(view: &'v dyn LayoutView) -> Self {
        FlexTree {
            view,
            ids: Vec::new(),
            children: Vec::new(),
            styles: Vec::new(),
            leaves: Vec::new(),
            caches: Vec::new(),
            layouts: Vec::new(),
            measured: Default::default(),
            want: None,
            grid: None,
        }
    }

    /// Add `id` (and, if it is a container, its in-flow children) and answer its
    /// index.
    fn push(&mut self, id: crate::NodeId) -> Option<usize> {
        let kind = self.view.kind(id)?;
        // Through a copy of the `&'v` so the borrow is the view's, not `self`'s,
        // and outlives the recursion below.
        let view = self.view;
        let display = view.display(id).filter(|_| is_container(&kind));
        let item = self.view.item(id);
        let index = self.ids.len();
        self.ids.push(id);
        self.children.push(Vec::new());
        self.styles.push(style_of(&kind, display, &item));
        self.caches.push(taffy::Cache::new());
        self.layouts.push(taffy::Layout::with_order(index as u32));
        let leaf = match (&display, &kind) {
            (Some(_), _) => Leaf::Container,
            (None, NodeKind::Text { content, .. }) => match crate::text::TextParts::of(&kind) {
                Some(parts) => Leaf::Text(Box::new(parts), content.clone()),
                None => Leaf::Replaced(Size::ZERO),
            },
            (None, _) => {
                Leaf::Replaced(atomic_box(self.view, id, &kind).map_or(Size::ZERO, |b| b.size()))
            }
        };
        self.leaves.push(leaf);
        if let Some(display) = display {
            for child in self.view.children(id) {
                if in_flow(self.view, child)
                    && let Some(c) = self.push(child)
                {
                    self.children[index].push(taffy::NodeId::from(c));
                    match display {
                        Display::Flex(flex) => {
                            if fit_content_across(&self.view.item(child), flex) {
                                self.styles[c].align_self = Some(taffy::AlignItems::FLEX_START);
                            }
                        }
                        Display::Grid(grid) => {
                            // `is_replaced`'s answer, read off the leaf just built.
                            let replaced = matches!(self.leaves[c], Leaf::Replaced(_));
                            let (across, down) = grid_held(&self.view.item(child), grid, replaced);
                            if across {
                                self.styles[c].justify_self = Some(taffy::AlignItems::START);
                            }
                            if down {
                                self.styles[c].align_self = Some(taffy::AlignItems::START);
                            }
                        }
                    }
                }
            }
        }
        Some(index)
    }

    fn measure(
        &mut self,
        index: usize,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        let (parts, content) = match &self.leaves[index] {
            Leaf::Replaced(s) => {
                return taffy::Size {
                    width: known.width.unwrap_or(s.width as f32),
                    height: known.height.unwrap_or(s.height as f32),
                };
            }
            Leaf::Container => return taffy::Size::ZERO,
            Leaf::Text(parts, content) => ((**parts).clone(), content.clone()),
        };
        let at = |sizing: TextSizing| {
            let mut p = parts.clone();
            p.sizing = sizing;
            crate::text::measure(p.as_ref(&content))
        };
        match parts.sizing {
            // **Never wraps** (§15 D875): a label keeps its one line whatever room
            // it is given, `white-space: nowrap`'s reading of auto width.
            TextSizing::Auto => {
                let key = (index, u32::MAX);
                let m = match self.measured.get(&key) {
                    Some(m) => *m,
                    None => {
                        let r = at(TextSizing::Auto);
                        let m = taffy::Size {
                            width: r.width() as f32,
                            height: r.height() as f32,
                        };
                        self.measured.insert(key, m);
                        m
                    }
                };
                taffy::Size {
                    width: known.width.unwrap_or(m.width),
                    height: known.height.unwrap_or(m.height),
                }
            }
            // **Wraps at the width it is given**; its stored width is only the
            // width it prefers — its max-content — and it will not go narrower
            // than its widest word.
            TextSizing::AutoHeight(preferred) => {
                let width = match known.width {
                    Some(w) => w,
                    None => {
                        let (min, _) = crate::text::content_widths(parts.as_ref(&content));
                        let preferred = preferred as f32;
                        match available.width {
                            taffy::AvailableSpace::MinContent => min as f32,
                            taffy::AvailableSpace::MaxContent => preferred,
                            taffy::AvailableSpace::Definite(a) => a.min(preferred).max(min as f32),
                        }
                    }
                };
                let key = (index, width.to_bits());
                let m = match self.measured.get(&key) {
                    Some(m) => *m,
                    None => {
                        let r = at(TextSizing::AutoHeight(f64::from(width)));
                        let m = taffy::Size {
                            width,
                            height: r.height() as f32,
                        };
                        self.measured.insert(key, m);
                        m
                    }
                };
                taffy::Size {
                    width,
                    height: known.height.unwrap_or(m.height),
                }
            }
            // A fixed box is a fixed box.
            TextSizing::Fixed(s) => taffy::Size {
                width: known.width.unwrap_or(s.width as f32),
                height: known.height.unwrap_or(s.height as f32),
            },
        }
    }
}

/// A group's or a boolean's box, taken whole — the union of its children's
/// boxes through their transforms, in its own space; any other kind's own box.
///
/// **Specified geometry, not used**: a group with no layout is one atomic box to
/// its container, and what is inside it is not re-laid by this pass. A boolean is
/// measured by its operands' union, which is at least its result's box.
///
/// 🚨 **Except a container with a layout of its own**, which is measured by that
/// layout — its own pass, its padding and its hugging — as the box `Resolved`
/// gives it (§15 D899). It was measured like any group, by its children's
/// *specified* union, so a flex row nested in a plain group lent its container a
/// box that ignored its padding and its layout: §15 D875's ⚠️, now closed. Its
/// children's specified placements still stand for everything else here, since
/// nothing inside a plain group is in a flow.
pub fn atomic_box(view: &dyn LayoutView, id: crate::NodeId, kind: &NodeKind) -> Option<Rect> {
    if is_container(kind) && view.display(id).is_some() {
        let size = lay_out(view, id).first().map(|l| l.size)?;
        return Some(Rect::from_origin_size(Point::ZERO, size));
    }
    match kind {
        NodeKind::Group | NodeKind::Boolean { .. } | NodeKind::Root => view
            .children(id)
            .into_iter()
            .filter(|c| view.visible(*c) && !view.mask(*c))
            .filter_map(|c| {
                let k = view.kind(c)?;
                atomic_box(view, c, &k).map(|b| geometry::transform_rect(view.local(c), b))
            })
            .reduce(|a, b| a.union(b)),
        _ => geometry::local_bounds(kind, None),
    }
}

/// A node's taffy style, from its kind, its layout and its item properties.
///
/// **A frame's stored size is its CSS size** unless it is asked to hug
/// (`FitContent`); a group with a layout hugs unless a size is typed; a leaf's
/// size is `auto` and comes from its measure.
fn style_of(kind: &NodeKind, display: Option<&Display>, item: &LayoutItem) -> taffy::Style {
    use taffy::prelude::{auto, length, percent};
    let dim = |d: Dimension, stored: Option<f64>| -> taffy::Dimension {
        match d {
            Dimension::Px(v) => length(v as f32),
            Dimension::Percent(p) => percent((p / 100.0) as f32),
            Dimension::FitContent => auto(),
            Dimension::Auto => match stored {
                Some(v) => length(v as f32),
                None => auto(),
            },
        }
    };
    // The limits take a narrower type than the sizes, with no `fit-content`: a
    // `FitContent` limit is no limit.
    let limit = |d: Dimension| -> taffy::LengthPercentageAuto {
        match d {
            Dimension::Px(v) => taffy::LengthPercentageAuto::length(v as f32),
            Dimension::Percent(p) => taffy::LengthPercentageAuto::percent((p / 100.0) as f32),
            Dimension::Auto | Dimension::FitContent => taffy::LengthPercentageAuto::auto(),
        }
    };
    let stored = match kind {
        NodeKind::Artboard { size } => Some(*size),
        _ => None,
    };
    let mut style = taffy::Style {
        size: taffy::Size {
            width: dim(item.width, stored.map(|s| s.width)),
            height: dim(item.height, stored.map(|s| s.height)),
        },
        min_size: taffy::Size {
            width: limit(item.min_width),
            height: limit(item.min_height),
        },
        max_size: taffy::Size {
            width: limit(item.max_width),
            height: limit(item.max_height),
        },
        flex_grow: item.grow as f32,
        flex_shrink: item.shrink as f32,
        flex_basis: dim(item.basis, None),
        align_self: item.align_self.map(align_items),
        justify_self: item.justify_self.map(align_items),
        grid_column: grid_lines(item.grid_column),
        grid_row: grid_lines(item.grid_row),
        ..Default::default()
    };
    match display {
        Some(Display::Flex(f)) => {
            style.display = taffy::Display::Flex;
            style.flex_direction = match f.direction {
                FlexDirection::Row => taffy::FlexDirection::Row,
                FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
                FlexDirection::Column => taffy::FlexDirection::Column,
                FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
            };
            style.flex_wrap = match f.wrap {
                FlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
                FlexWrap::Wrap => taffy::FlexWrap::Wrap,
                FlexWrap::WrapReverse => taffy::FlexWrap::WrapReverse,
            };
            style.justify_content = Some(match f.justify_content {
                // `flex-start`/`flex-end`, not `start`/`end` (§15 D909).
                JustifyContent::Start => taffy::JustifyContent::FLEX_START,
                JustifyContent::End => taffy::JustifyContent::FLEX_END,
                JustifyContent::Center => taffy::JustifyContent::CENTER,
                JustifyContent::SpaceBetween => taffy::JustifyContent::SPACE_BETWEEN,
                JustifyContent::SpaceAround => taffy::JustifyContent::SPACE_AROUND,
                JustifyContent::SpaceEvenly => taffy::JustifyContent::SPACE_EVENLY,
            });
            style.align_items = Some(align_items(f.align_items));
            style.align_content = Some(content_alignment(f.align_content));
            style.gap = taffy::Size {
                width: length(f.column_gap as f32),
                height: length(f.row_gap as f32),
            };
            let [top, right, bottom, left] = f.padding.map(|p| length(p as f32));
            style.padding = taffy::Rect {
                left,
                right,
                top,
                bottom,
            };
        }
        Some(Display::Grid(g)) => {
            style.display = taffy::Display::Grid;
            style.grid_template_columns = template(&g.columns);
            style.grid_template_rows = template(&g.rows);
            style.grid_auto_flow = match g.auto_flow {
                GridAutoFlow::Row => taffy::GridAutoFlow::Row,
                GridAutoFlow::Column => taffy::GridAutoFlow::Column,
            };
            style.justify_content = Some(content_alignment(g.justify_content));
            // `None` goes to taffy as `normal`; the replaced items it must not
            // stretch are held at the start in `FlexTree::push` (§15 D915).
            style.justify_items = g.justify_items.map(align_items);
            style.align_items = g.align_items.map(align_items);
            style.align_content = Some(content_alignment(g.align_content));
            style.gap = taffy::Size {
                width: length(g.column_gap as f32),
                height: length(g.row_gap as f32),
            };
            let [top, right, bottom, left] = g.padding.map(|p| length(p as f32));
            style.padding = taffy::Rect {
                left,
                right,
                top,
                bottom,
            };
        }
        // A leaf keeps taffy's default display: its layout is `compute_leaf_layout`
        // over its measure either way (`FlexTree::compute_child_layout`), and the
        // one value that would matter — `None`, which hides — is never set.
        None => {}
    }
    style
}

/// Whether `item`, in a `flex` container, is **`fit-content` across the line and
/// would be stretched** — so it must not be (§15 D893, the maintainer's ruling).
///
/// CSS stretches an item across its line only when its cross size is `auto`; a
/// definite one, which `fit-content` is, makes `stretch` behave as `flex-start`
/// (CSS Flexbox §8.3). Figma's *Hug* inside a stretching parent likewise keeps
/// hugging. taffy is handed `auto` for `FitContent` ([`style_of`] — it has no
/// such size), so without this a hugging frame in a stretching row stretched,
/// which §15 D879 flagged. The main axis is untouched: `fit-content` there is the
/// flex basis's content size, and `flex-grow` may still grow it, as in CSS.
fn fit_content_across(item: &LayoutItem, parent: &Flex) -> bool {
    let across = if parent.direction.is_row() {
        item.height
    } else {
        item.width
    };
    across == Dimension::FitContent
        && item.align_self.unwrap_or(parent.align_items) == AlignItems::Stretch
}

/// Which axes of a grid item — across, down — must be **held at the start of
/// its area rather than stretched** (§15 D915, the maintainer's CSS-parity
/// ruling):
///
/// - **Under `normal`**, the item's self-alignment and the container's both
///   unset, a *replaced* item keeps its own size at the start — CSS Box
///   Alignment's `normal` for a grid item with a natural size, and a shape is a
///   replaced element here (§15 D872). taffy's `normal` stretches anything sized
///   `auto`, and a shape goes to it `auto` and measured, so the start is written
///   in. A box — text, a laid group — stretches under `normal`, as a `div` does.
/// - **`fit-content` is never stretched**, under `normal` or `stretch`: CSS
///   stretches only an `auto` size, and taffy is handed `auto` for
///   `FitContent`. §15 D893's rule for flex, on both of a grid's axes.
///
/// An explicit `stretch` stretches a replaced item as it does in CSS; every
/// other alignment is taffy's to apply.
fn grid_held(item: &LayoutItem, parent: &Grid, replaced: bool) -> (bool, bool) {
    let axis = |own: Option<AlignItems>, items: Option<AlignItems>, size: Dimension| {
        let normal = own.is_none() && items.is_none();
        let stretched = normal || own.or(items) == Some(AlignItems::Stretch);
        (normal && replaced) || (stretched && size == Dimension::FitContent)
    };
    (
        axis(item.justify_self, parent.justify_items, item.width),
        axis(item.align_self, parent.align_items, item.height),
    )
}

/// `a` for taffy — `Start` and `End` as `flex-start` and `flex-end`, which follow
/// `wrap-reverse` (§15 D909).
fn align_items(a: AlignItems) -> taffy::AlignItems {
    match a {
        AlignItems::Stretch => taffy::AlignItems::STRETCH,
        AlignItems::Start => taffy::AlignItems::FLEX_START,
        AlignItems::End => taffy::AlignItems::FLEX_END,
        AlignItems::Center => taffy::AlignItems::CENTER,
        AlignItems::Baseline => taffy::AlignItems::BASELINE,
    }
}

/// `a` for taffy, on either axis. `Start` and `End` go as `flex-start` and
/// `flex-end` (§15 D909), which a grid reads as its own start and end — taffy's
/// grid treats the two pairs as one, having no reversal to tell them apart.
fn content_alignment(a: AlignContent) -> taffy::AlignContent {
    match a {
        AlignContent::Stretch => taffy::AlignContent::STRETCH,
        AlignContent::Start => taffy::AlignContent::FLEX_START,
        AlignContent::End => taffy::AlignContent::FLEX_END,
        AlignContent::Center => taffy::AlignContent::CENTER,
        AlignContent::SpaceBetween => taffy::AlignContent::SPACE_BETWEEN,
        AlignContent::SpaceAround => taffy::AlignContent::SPACE_AROUND,
        AlignContent::SpaceEvenly => taffy::AlignContent::SPACE_EVENLY,
    }
}

/// A track list for taffy. **Reads what CSS would refuse without refusing it**
/// ([`Grid::is_valid`]'s reason — a file can carry one): a negative number is
/// read as 0, a `minmax()` whose min is `fr` as `auto` at that end, and an empty
/// or zero-count `repeat()` as nothing. **And the list stops at
/// [`MAX_TEMPLATE_TRACKS`]** (§15 D919): a repeat that would pass it repeats as
/// many whole times as fit, and what follows is laid only as far as there is
/// room — a multi-track repeat can leave a remainder a later single track still
/// fits in.
fn template(tracks: &[Track]) -> Vec<taffy::GridTemplateComponent<String>> {
    let mut room = MAX_TEMPLATE_TRACKS;
    let mut out = Vec::new();
    for t in tracks {
        match t {
            Track::Size(s) if room > 0 => {
                room -= 1;
                out.push(taffy::GridTemplateComponent::Single(track_size(*s)));
            }
            Track::Repeat { repeat, tracks } if *repeat > 0 && !tracks.is_empty() => {
                let times = usize::from(*repeat).min(room / tracks.len());
                if times == 0 {
                    continue;
                }
                room -= times * tracks.len();
                out.push(taffy::GridTemplateComponent::Repeat(
                    taffy::GridTemplateRepetition {
                        count: taffy::RepetitionCount::Count(times as u16),
                        tracks: tracks.iter().map(|s| track_size(*s)).collect(),
                        line_names: Vec::new(),
                    },
                ));
            }
            _ => {}
        }
    }
    out
}

/// One track size for taffy — a single breadth `b` is CSS's `minmax(b, b)`, save
/// `fr`, which is `minmax(auto, fr)`: taffy's reading and CSS's.
fn track_size(s: TrackSize) -> taffy::TrackSizingFunction {
    let (min, max) = match s {
        TrackSize::Breadth(b) => (b, b),
        TrackSize::MinMax { min, max } => (min, max),
    };
    taffy::MinMax {
        min: min_breadth(min),
        max: max_breadth(max),
    }
}

fn min_breadth(b: TrackBreadth) -> taffy::MinTrackSizingFunction {
    use taffy::MinTrackSizingFunction as Min;
    match b {
        TrackBreadth::Px(v) => Min::length(v.max(0.0) as f32),
        TrackBreadth::Percent(p) => Min::percent((p.max(0.0) / 100.0) as f32),
        TrackBreadth::Fr(_) | TrackBreadth::Auto => Min::auto(),
        TrackBreadth::MinContent => Min::min_content(),
        TrackBreadth::MaxContent => Min::max_content(),
    }
}

fn max_breadth(b: TrackBreadth) -> taffy::MaxTrackSizingFunction {
    use taffy::MaxTrackSizingFunction as Max;
    match b {
        TrackBreadth::Px(v) => Max::length(v.max(0.0) as f32),
        TrackBreadth::Percent(p) => Max::percent((p.max(0.0) / 100.0) as f32),
        TrackBreadth::Fr(v) => taffy::prelude::fr(v.max(0.0) as f32),
        TrackBreadth::Auto => Max::auto(),
        TrackBreadth::MinContent => Max::min_content(),
        TrackBreadth::MaxContent => Max::max_content(),
    }
}

/// An item's `grid-column` or `grid-row` for taffy; line 0 and `span 0`, which
/// CSS refuses, read as `auto`.
fn grid_lines(l: GridLines) -> taffy::Line<taffy::GridPlacement<String>> {
    let one = |p: GridPlacement| match p {
        GridPlacement::Line(n) if n != 0 => taffy::GridPlacement::Line(n.into()),
        GridPlacement::Span(n) if n != 0 => taffy::GridPlacement::Span(n),
        _ => taffy::GridPlacement::Auto,
    };
    taffy::Line {
        start: one(l.start),
        end: one(l.end),
    }
}

impl taffy::TraversePartialTree for FlexTree<'_> {
    type ChildIter<'a>
        = std::iter::Copied<std::slice::Iter<'a, taffy::NodeId>>
    where
        Self: 'a;

    fn child_ids(&self, node: taffy::NodeId) -> Self::ChildIter<'_> {
        self.children[usize::from(node)].iter().copied()
    }

    fn child_count(&self, node: taffy::NodeId) -> usize {
        self.children[usize::from(node)].len()
    }

    fn get_child_id(&self, node: taffy::NodeId, index: usize) -> taffy::NodeId {
        self.children[usize::from(node)][index]
    }
}

impl taffy::TraverseTree for FlexTree<'_> {}

impl taffy::LayoutPartialTree for FlexTree<'_> {
    type CustomIdent = String;

    type CoreContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_core_container_style(&self, node: taffy::NodeId) -> Self::CoreContainerStyle<'_> {
        &self.styles[usize::from(node)]
    }

    fn set_unrounded_layout(&mut self, node: taffy::NodeId, layout: &taffy::Layout) {
        self.layouts[usize::from(node)] = *layout;
    }

    fn resolve_calc_value(&self, _val: *const (), _basis: f32) -> f32 {
        // `calc` is off (`default-features = false`), so nothing can hand one in.
        0.0
    }

    fn compute_child_layout(
        &mut self,
        node: taffy::NodeId,
        inputs: taffy::tree::LayoutInput,
    ) -> taffy::tree::LayoutOutput {
        taffy::compute_cached_layout(self, node, inputs, |tree, node, inputs| {
            let index = usize::from(node);
            match tree.leaves[index] {
                Leaf::Container if tree.styles[index].display == taffy::Display::Grid => {
                    taffy::compute_grid_layout(tree, node, inputs)
                }
                Leaf::Container => taffy::compute_flexbox_layout(tree, node, inputs),
                _ => {
                    let style = tree.styles[index].clone();
                    taffy::compute_leaf_layout(
                        inputs,
                        &style,
                        |_, _| 0.0,
                        |known, available| tree.measure(index, known, available),
                    )
                }
            }
        })
    }
}

impl taffy::CacheTree for FlexTree<'_> {
    fn cache_get(
        &mut self,
        node: taffy::NodeId,
        inputs: &taffy::tree::LayoutInput,
    ) -> Option<taffy::tree::LayoutOutput> {
        self.caches[usize::from(node)].get(inputs)
    }

    fn cache_store(
        &mut self,
        node: taffy::NodeId,
        inputs: &taffy::tree::LayoutInput,
        output: taffy::tree::LayoutOutput,
    ) {
        self.caches[usize::from(node)].store(inputs, output)
    }

    fn cache_clear(&mut self, node: taffy::NodeId) {
        self.caches[usize::from(node)].clear();
    }
}

impl taffy::LayoutFlexboxContainer for FlexTree<'_> {
    type FlexboxContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    type FlexboxItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node: taffy::NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.styles[usize::from(node)]
    }

    fn get_flexbox_child_style(&self, child: taffy::NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.styles[usize::from(child)]
    }
}

impl taffy::LayoutGridContainer for FlexTree<'_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    type GridItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node: taffy::NodeId) -> Self::GridContainerStyle<'_> {
        &self.styles[usize::from(node)]
    }

    fn get_grid_child_style(&self, child: taffy::NodeId) -> Self::GridItemStyle<'_> {
        &self.styles[usize::from(child)]
    }

    fn set_detailed_grid_info(
        &mut self,
        node: taffy::NodeId,
        info: taffy::DetailedGridInfo<Self::CustomIdent>,
    ) {
        if self.want == Some(self.ids[usize::from(node)]) {
            self.grid = Some(info);
        }
    }
}

/// One edge of a box, as the inspector's pin diagram names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// `insets` with `edge` pinned or unpinned **without moving the layer** — the
/// pin diagram's click (§15 D874).
///
/// Pinning sets the edge at the distance it is at now, in px, by marking it and
/// then reading every inset back from where the layer is drawn (`local`, `bx` —
/// its used placement) through [`inverse`]. So pinning the second edge of an axis
/// stretches the layer at the size it already has, and nothing on the canvas
/// jumps. Unpinning clears the edge and, when that leaves its axis with one inset,
/// drops the axis's auto margins, which only mean anything between two.
#[allow(clippy::too_many_arguments)]
pub fn with_edge(
    insets: &Insets,
    edge: Edge,
    pinned: bool,
    frame: Size,
    local: Affine,
    bx: Rect,
    kind: &NodeKind,
) -> Insets {
    let mut out = *insets;
    let slot = match edge {
        Edge::Top => &mut out.top,
        Edge::Right => &mut out.right,
        Edge::Bottom => &mut out.bottom,
        Edge::Left => &mut out.left,
    };
    if !pinned {
        *slot = None;
        if out.left.is_none() || out.right.is_none() {
            out.margin_auto.left = false;
            out.margin_auto.right = false;
        }
        if out.top.is_none() || out.bottom.is_none() {
            out.margin_auto.top = false;
            out.margin_auto.bottom = false;
        }
        return out;
    }
    if slot.is_some() {
        return out;
    }
    // Marking the edge set is what makes `inverse` compute it, and it computes it
    // from the placement. `Px(0)` is a safe placeholder: the one way it survives
    // the inverse unrewritten is that the edge really is at 0.
    *slot = Some(LengthPct::Px(0.0));
    let read = inverse(&out, frame, local, bx, kind);
    // Only the new edge's axis is the placement's to rewrite — the new edge, and
    // its partner if the axis now has two, since a second inset changes how the
    // first is read (stretch, or centre). The other axis keeps exactly what it had.
    match edge {
        Edge::Left | Edge::Right => {
            out.left = read.left;
            out.right = read.right;
        }
        Edge::Top | Edge::Bottom => {
            out.top = read.top;
            out.bottom = read.bottom;
        }
    }
    out
}

/// The inset arithmetic, one rule per test (§15 D871, D874).
///
/// Every fixture is a 100×50 rect in a 300×200 frame, so each rule's answer is a
/// number a reader can check by hand, and each assertion sits where the plausible
/// wrong rule gives a different one.
#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::RoundedRectRadii;

    const FRAME: Size = Size::new(300.0, 200.0);

    fn rect(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
        }
    }

    fn px(v: f64) -> Option<LengthPct> {
        Some(LengthPct::Px(v))
    }

    /// The slot the used transform puts the box's top-left in, and the used size.
    fn placed(insets: Insets, stored: Affine, kind: &NodeKind) -> (Point, Size) {
        let (local, used) = place(&insets, FRAME, stored, kind).expect("placed");
        let k = used.unwrap_or_else(|| kind.clone());
        let bx = geometry::local_bounds(&k, None).unwrap();
        (slot_of(local, bx), bx.size())
    }

    /// One inset on each axis pins that edge; none leaves the axis where the
    /// transform puts it. `right: 10` in a 300 frame puts a 100-wide box at 190.
    #[test]
    fn a_single_inset_pins_its_edge_and_an_unset_axis_keeps_the_transform() {
        let insets = Insets {
            right: px(10.0),
            ..Default::default()
        };
        let (slot, size) = placed(insets, Affine::translate((40.0, 70.0)), &rect(100.0, 50.0));
        assert_eq!(
            slot,
            Point::new(190.0, 70.0),
            "pinned right; y is the transform's"
        );
        assert_eq!(size, Size::new(100.0, 50.0));
    }

    /// Both insets on an axis stretch a kind with a size (§15 D874's first
    /// ruling) and over-constrain one without: a path keeps its size and the end
    /// inset is ignored, CSS's rule.
    #[test]
    fn two_insets_stretch_a_sized_kind_and_are_over_constrained_for_a_path() {
        let insets = Insets {
            left: px(10.0),
            right: px(20.0),
            ..Default::default()
        };
        let (slot, size) = placed(insets, Affine::IDENTITY, &rect(100.0, 50.0));
        assert_eq!((slot.x, size.width), (10.0, 270.0), "300 less 10 less 20");

        let mut line = kurbo::BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((100.0, 50.0));
        let path = NodeKind::Path {
            path: line,
            corner_radii: Vec::new(),
        };
        let (slot, size) = placed(insets, Affine::IDENTITY, &path);
        assert_eq!(
            (slot.x, size.width),
            (10.0, 100.0),
            "a path cannot stretch: its left holds and its right is ignored"
        );
    }

    /// `margin: auto` between two insets centres at the box's own size, and one
    /// auto side pushes the box against the other (§15 D874's centring ruling).
    #[test]
    fn auto_margins_between_two_insets_centre_or_push_instead_of_stretching() {
        let both = Insets {
            left: px(0.0),
            right: px(0.0),
            margin_auto: AutoMargins {
                left: true,
                right: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let (slot, size) = placed(both, Affine::IDENTITY, &rect(100.0, 50.0));
        assert_eq!((slot.x, size.width), (100.0, 100.0), "(300 - 100) / 2");

        let pushed = Insets {
            margin_auto: AutoMargins {
                left: true,
                ..Default::default()
            },
            ..both
        };
        let (slot, _) = placed(pushed, Affine::IDENTITY, &rect(100.0, 50.0));
        assert_eq!(slot.x, 200.0, "the auto left side takes all the room");
    }

    /// Percentages resolve against the axis they are on — width for left/right,
    /// height for top/bottom — so `25%`/`25%` is Figma's *scale*.
    #[test]
    fn percent_insets_resolve_against_their_own_axis() {
        let insets = Insets {
            left: Some(LengthPct::Percent(25.0)),
            right: Some(LengthPct::Percent(25.0)),
            top: Some(LengthPct::Percent(10.0)),
            ..Default::default()
        };
        let (slot, size) = placed(insets, Affine::IDENTITY, &rect(100.0, 50.0));
        assert_eq!(slot, Point::new(75.0, 20.0), "25% of 300; 10% of 200");
        assert_eq!(size.width, 150.0);
    }

    /// A rotated child is placed by its **unrotated** box and turned about that
    /// box's centre (§15 D874's rotation ruling): pinned `right: 10`, a 100×50
    /// rect's centre lands at x 240 whatever its angle.
    ///
    /// **Flip:** dropping the half-box from `placed_at`, so the rotation turns
    /// about the box's top-left, fails here with the centre at (217.2, 68.9) — and
    /// **only** here: for an unrotated box the half cancels, which is why no other
    /// test in this module notices and why a document with no rotation cannot.
    #[test]
    fn a_rotated_child_turns_about_the_centre_of_the_box_its_insets_place() {
        let insets = Insets {
            right: px(10.0),
            top: px(20.0),
            ..Default::default()
        };
        let turned = Affine::translate((5.0, 5.0)) * Affine::rotate(0.6);
        let kind = rect(100.0, 50.0);
        let (local, _) = place(&insets, FRAME, turned, &kind).unwrap();
        let centre = local * Point::new(50.0, 25.0);
        assert!(
            (centre.x - 240.0).abs() < 1e-9 && (centre.y - 45.0).abs() < 1e-9,
            "centre at {centre:?}, not (240, 45)"
        );
        let [xx, yx, xy, yy, _, _] = local.as_coeffs();
        let [rx, ry, rxy, ryy, _, _] = Affine::rotate(0.6).as_coeffs();
        assert_eq!(
            [xx, yx, xy, yy],
            [rx, ry, rxy, ryy],
            "and the angle is kept"
        );
    }

    /// A stretched width is a wrap width for text; a stretched height makes the
    /// box fixed. The mode is kept wherever it can be.
    #[test]
    fn stretched_text_wraps_at_the_width_and_fixes_only_for_a_height() {
        let text = |sizing| NodeKind::Text {
            content: "x".into(),
            style: Box::default(),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let sizing = |k: NodeKind| match k {
            NodeKind::Text { sizing, .. } => sizing,
            _ => unreachable!(),
        };
        let to = Size::new(120.0, 80.0);
        assert_eq!(
            sizing(resized(&text(TextSizing::Auto), to, (true, false))),
            TextSizing::AutoHeight(120.0)
        );
        assert_eq!(
            sizing(resized(
                &text(TextSizing::Fixed(Size::new(50.0, 30.0))),
                to,
                (true, false)
            )),
            TextSizing::Fixed(Size::new(120.0, 30.0)),
            "a fixed box keeps its height when only its width stretches"
        );
        assert_eq!(
            sizing(resized(&text(TextSizing::Auto), to, (false, true))),
            TextSizing::Fixed(to)
        );
    }

    /// [`inverse`] undoes [`place`], and a move rewrites each inset **in its own
    /// unit**: dragging a right-pinned child 30 left adds 30 to `right`, and a
    /// percentage stays a percentage.
    ///
    /// **Flip:** `LengthPct::rewritten` writing px for a percentage fails the
    /// move, `Px(40.0)` against `Percent(20.0)`. ⚠️ **It failed the round trip
    /// instead until the unmoved-axis guard went into `AxisInsets::inverse`** —
    /// before it, reading an unmoved child back rewrote every inset — so the round
    /// trip now passes by returning early, and it is the guard's own assertion
    /// rather than the unit's.
    #[test]
    fn the_inverse_rewrites_each_inset_in_its_own_unit() {
        let insets = Insets {
            right: px(10.0),
            top: Some(LengthPct::Percent(10.0)),
            ..Default::default()
        };
        let kind = rect(100.0, 50.0);
        let bx = geometry::local_bounds(&kind, None).unwrap();
        let (local, _) = place(&insets, FRAME, Affine::IDENTITY, &kind).unwrap();
        assert_eq!(
            inverse(&insets, FRAME, local, bx, &kind),
            insets,
            "placing and reading back changes nothing"
        );

        let moved = Affine::translate((-30.0, 20.0)) * local;
        let back = inverse(&insets, FRAME, moved, bx, &kind);
        assert_eq!(back.right, px(40.0));
        assert_eq!(
            back.top,
            Some(LengthPct::Percent(20.0)),
            "20 + 20 down is 40 of 200"
        );
        assert_eq!(back.left, None, "an axis's unset side stays unset");
    }

    /// Pinning an edge from the diagram never moves the layer: each pin is set at
    /// the distance the edge is at, and pinning the second edge of an axis stretches
    /// the layer at the size it already has.
    ///
    /// **Asserted as "`place` draws it where it was"**, after every click, rather
    /// than as the inset values — the values are the means, not being moved is the
    /// promise.
    #[test]
    fn pinning_from_the_diagram_never_moves_the_layer() {
        let kind = rect(100.0, 50.0);
        let bx = geometry::local_bounds(&kind, None).unwrap();
        let at = Affine::translate((40.0, 30.0));
        let mut insets = Insets::default();
        for edge in [Edge::Right, Edge::Left, Edge::Bottom] {
            insets = with_edge(&insets, edge, true, FRAME, at, bx, &kind);
            let (slot, size) = placed(insets, at, &kind);
            assert_eq!(
                (slot, size),
                (Point::new(40.0, 30.0), Size::new(100.0, 50.0)),
                "pinning {edge:?} moved it"
            );
        }
        assert_eq!(insets.right, px(160.0), "300 - 40 - 100");
        assert_eq!(insets.left, px(40.0));
        assert_eq!(insets.bottom, px(120.0), "200 - 30 - 50");
        assert_eq!(insets.top, None, "an edge never clicked stays unpinned");

        // Unpinning one of two drops the pair's auto margins with it.
        let centred = Insets {
            margin_auto: AutoMargins {
                left: true,
                right: true,
                ..Default::default()
            },
            ..insets
        };
        let out = with_edge(&centred, Edge::Left, false, FRAME, at, bx, &kind);
        assert_eq!(out.left, None);
        assert!(out.margin_auto.is_none());
    }

    /// A group and a boolean take no insets — their box is their children's,
    /// measured after placement — and `place` leaves them where they are even with
    /// insets set; every kind with a box of its own takes them.
    #[test]
    fn a_group_or_a_boolean_takes_no_insets() {
        let pinned = Insets {
            right: px(10.0),
            ..Default::default()
        };
        for kind in [
            NodeKind::Group,
            NodeKind::Boolean {
                op: crate::node::BoolOp::Union,
            },
        ] {
            assert!(!takes_insets(&kind), "{kind:?} takes insets");
            assert!(
                place(&pinned, FRAME, Affine::IDENTITY, &kind).is_none(),
                "{kind:?} was placed"
            );
        }
        assert!(takes_insets(&rect(1.0, 1.0)));
    }

    /// Between two auto-margined insets a move shifts both, so the child stays
    /// centred in a region that moved with it.
    #[test]
    fn moving_a_centred_child_shifts_both_insets() {
        let insets = Insets {
            left: px(0.0),
            right: px(0.0),
            margin_auto: AutoMargins {
                left: true,
                right: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let kind = rect(100.0, 50.0);
        let bx = geometry::local_bounds(&kind, None).unwrap();
        let moved = Affine::translate((130.0, 0.0));
        let back = inverse(&insets, FRAME, moved, bx, &kind);
        assert_eq!((back.left, back.right), (px(30.0), px(-30.0)));
        let (slot, _) = placed(back, Affine::IDENTITY, &kind);
        assert_eq!(slot.x, 130.0, "and it is still centred between them");
    }
}

/// The flex engine on its own, against an in-memory [`LayoutView`] — what taffy
/// computes through `FlexTree`, before `Resolved` is involved (§15 D867, D875).
#[cfg(test)]
mod flex_tests {
    use super::*;
    use crate::id::{IdSource, NodeId};
    use kurbo::RoundedRectRadii;
    use rustc_hash::FxHashMap;

    #[derive(Clone)]
    struct Fake {
        kind: NodeKind,
        display: Option<Display>,
        item: LayoutItem,
        insets: Insets,
        visible: bool,
        mask: bool,
        children: Vec<NodeId>,
    }

    #[derive(Default)]
    struct View {
        nodes: FxHashMap<NodeId, Fake>,
        ids: Option<IdSource>,
    }

    impl View {
        fn add(&mut self, parent: Option<NodeId>, kind: NodeKind) -> NodeId {
            let id = self.ids.get_or_insert_with(|| IdSource::new(3)).mint();
            self.nodes.insert(
                id,
                Fake {
                    kind,
                    display: None,
                    item: LayoutItem::default(),
                    insets: Insets::default(),
                    visible: true,
                    mask: false,
                    children: Vec::new(),
                },
            );
            if let Some(p) = parent {
                self.nodes.get_mut(&p).unwrap().children.push(id);
            }
            id
        }
        fn set(&mut self, id: NodeId, f: impl FnOnce(&mut Fake)) {
            f(self.nodes.get_mut(&id).unwrap());
        }
    }

    impl LayoutView for View {
        fn parent(&self, id: NodeId) -> Option<NodeId> {
            self.nodes
                .iter()
                .find(|(_, n)| n.children.contains(&id))
                .map(|(p, _)| *p)
        }
        fn children(&self, id: NodeId) -> Vec<NodeId> {
            self.nodes[&id].children.clone()
        }
        fn kind(&self, id: NodeId) -> Option<NodeKind> {
            self.nodes.get(&id).map(|n| n.kind.clone())
        }
        fn display(&self, id: NodeId) -> Option<&Display> {
            self.nodes[&id].display.as_ref()
        }
        fn item(&self, id: NodeId) -> LayoutItem {
            self.nodes[&id].item
        }
        fn insets(&self, id: NodeId) -> Insets {
            self.nodes[&id].insets
        }
        fn visible(&self, id: NodeId) -> bool {
            self.nodes[&id].visible
        }
        fn mask(&self, id: NodeId) -> bool {
            self.nodes[&id].mask
        }
        fn local(&self, _id: NodeId) -> Affine {
            Affine::IDENTITY
        }
    }

    fn rect(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
        }
    }

    fn frame(w: f64, h: f64) -> NodeKind {
        NodeKind::Artboard {
            size: Size::new(w, h),
        }
    }

    fn row(gap: f64, pad: f64) -> Display {
        Display::Flex(Flex {
            column_gap: gap,
            row_gap: gap,
            padding: [pad; 4],
            align_items: AlignItems::Start,
            ..Default::default()
        })
    }

    fn laid(out: &[Laid], id: NodeId) -> (Point, Size) {
        let l = out.iter().find(|l| l.id == id).expect("laid out");
        (l.slot, l.size)
    }

    /// A row places its items one after another from the padding, a gap apart,
    /// each at its own size — the whole of flexbox's simplest case.
    #[test]
    fn a_row_places_items_from_the_padding_a_gap_apart() {
        let mut v = View::default();
        let f = v.add(None, frame(300.0, 100.0));
        v.set(f, |n| n.display = Some(row(10.0, 20.0)));
        let a = v.add(Some(f), rect(40.0, 30.0));
        let b = v.add(Some(f), rect(60.0, 30.0));
        let out = lay_out(&v, f);
        assert_eq!(
            laid(&out, a),
            (Point::new(20.0, 20.0), Size::new(40.0, 30.0))
        );
        assert_eq!(
            laid(&out, b),
            (Point::new(70.0, 20.0), Size::new(60.0, 30.0))
        );
        assert_eq!(
            laid(&out, f).1,
            Size::new(300.0, 100.0),
            "a frame keeps its stored size"
        );
    }

    /// `flex-grow` shares the free space in proportion, and the share lands on the
    /// 1/64 grid (§15 D873) — a third of 100 is not a number f32 or f64 can hold.
    #[test]
    fn grow_shares_the_free_space_on_the_sixty_fourth_grid() {
        let mut v = View::default();
        let f = v.add(None, frame(100.0, 50.0));
        v.set(f, |n| n.display = Some(row(0.0, 0.0)));
        let a = v.add(Some(f), rect(0.0, 10.0));
        let b = v.add(Some(f), rect(0.0, 10.0));
        v.set(a, |n| n.item.grow = 1.0);
        v.set(b, |n| n.item.grow = 2.0);
        let out = lay_out(&v, f);
        let (_, sa) = laid(&out, a);
        let (_, sb) = laid(&out, b);
        assert_eq!(sa.width, (100.0_f64 / 3.0 * 64.0).round() / 64.0);
        assert_eq!(sa.width * 64.0, (sa.width * 64.0).round(), "on the grid");
        assert!((sa.width + sb.width - 100.0).abs() < 1.0 / 32.0);
    }

    /// A shape is not squeezed below its own size in an overflowing row — CSS's
    /// automatic minimum for a replaced element (§15 D872's spike, 40 stays 40).
    ///
    /// **Flip:** a shape answering a min-content width of 0, as an empty box would,
    /// fails here at 30 against 40 — the spike's 30.643, on the 1/64 grid.
    #[test]
    fn a_shape_holds_its_size_in_an_overflowing_row() {
        let mut v = View::default();
        let f = v.add(None, frame(60.0, 50.0));
        v.set(f, |n| n.display = Some(row(0.0, 0.0)));
        let a = v.add(Some(f), rect(40.0, 10.0));
        let b = v.add(Some(f), rect(40.0, 10.0));
        let out = lay_out(&v, f);
        assert_eq!(laid(&out, a).1.width, 40.0);
        assert_eq!(laid(&out, b).1.width, 40.0);
    }

    /// A group with a layout hugs what it holds: its box is its padding plus its
    /// items plus the gaps between them.
    #[test]
    fn a_group_with_a_layout_hugs_its_items() {
        let mut v = View::default();
        let g = v.add(None, NodeKind::Group);
        v.set(g, |n| n.display = Some(row(10.0, 5.0)));
        v.add(Some(g), rect(40.0, 30.0));
        v.add(Some(g), rect(60.0, 20.0));
        let out = lay_out(&v, g);
        assert_eq!(
            laid(&out, g).1,
            Size::new(5.0 + 40.0 + 10.0 + 60.0 + 5.0, 5.0 + 30.0 + 5.0)
        );
    }

    /// Hidden layers, masks and pinned layers are out of the flow: the row closes
    /// up around them, and they get no result.
    #[test]
    fn hidden_masked_and_pinned_layers_leave_the_flow() {
        let mut v = View::default();
        let f = v.add(None, frame(300.0, 100.0));
        v.set(f, |n| n.display = Some(row(0.0, 0.0)));
        let hidden = v.add(Some(f), rect(50.0, 10.0));
        let mask = v.add(Some(f), rect(50.0, 10.0));
        let pinned = v.add(Some(f), rect(50.0, 10.0));
        let kept = v.add(Some(f), rect(50.0, 10.0));
        v.set(hidden, |n| n.visible = false);
        v.set(mask, |n| n.mask = true);
        v.set(pinned, |n| {
            n.insets.right = Some(LengthPct::Px(0.0));
        });
        let out = lay_out(&v, f);
        assert_eq!(
            laid(&out, kept).0.x,
            0.0,
            "the row closed up to the first in-flow layer"
        );
        for id in [hidden, mask, pinned] {
            assert!(out.iter().all(|l| l.id != id), "{id:?} was laid out");
        }
    }

    /// Text keeps each mode's meaning in a row (§15 D875): auto width never wraps
    /// however narrow the row, auto height wraps at the width it is given but not
    /// below its widest word.
    ///
    /// **Flips.** Answering auto-width text's min-content with its widest word and
    /// wrapping it at whatever width taffy settles on — the "CSS default" option
    /// the maintainer turned down — fails the one-line assertion, 116.2 tall
    /// against 19.4. ⚠️ **Wrapping it at a *definite* available width instead does
    /// not bite**, and was the first flip tried: taffy never asks an auto-width
    /// item about a definite width, only its min- and max-content and then the
    /// width it chose, so that input never arrives. The teeth are in min-content.
    #[test]
    fn text_keeps_each_modes_meaning_in_a_narrow_row() {
        let text = |sizing| NodeKind::Text {
            content: "several short words that can wrap".into(),
            style: Box::default(),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let one_line =
            crate::text::measure(crate::node::TextRef::of(&text(TextSizing::Auto)).unwrap());

        let mut v = View::default();
        let f = v.add(None, frame(60.0, 400.0));
        v.set(f, |n| n.display = Some(row(0.0, 0.0)));
        let auto = v.add(Some(f), text(TextSizing::Auto));
        let out = lay_out(&v, f);
        assert_eq!(
            laid(&out, auto).1.height,
            quantize(one_line.height() as f32),
            "auto width stays one line in a 60-wide row"
        );

        let mut v = View::default();
        let f = v.add(None, frame(60.0, 400.0));
        v.set(f, |n| n.display = Some(row(0.0, 0.0)));
        let wraps = v.add(Some(f), text(TextSizing::AutoHeight(500.0)));
        let out = lay_out(&v, f);
        let (_, size) = laid(&out, wraps);
        assert!(
            size.height > one_line.height() * 2.0,
            "auto height wrapped: {size:?}"
        );
        let (widest, _) =
            crate::text::content_widths(crate::node::TextRef::of(&text(TextSizing::Auto)).unwrap());
        assert!(
            size.width >= quantize(widest as f32),
            "and no narrower than its widest word ({widest})"
        );
    }

    /// A group with its own layout nested in a row is laid out in the same pass:
    /// its size comes from the row, its children from it.
    #[test]
    fn a_nested_container_is_laid_out_in_the_same_pass() {
        let mut v = View::default();
        let outer = v.add(None, frame(300.0, 100.0));
        v.set(outer, |n| n.display = Some(row(0.0, 10.0)));
        let inner = v.add(Some(outer), NodeKind::Group);
        v.set(inner, |n| {
            n.display = Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                row_gap: 4.0,
                align_items: AlignItems::Start,
                ..Default::default()
            }))
        });
        let a = v.add(Some(inner), rect(20.0, 20.0));
        let b = v.add(Some(inner), rect(30.0, 20.0));
        let out = lay_out(&v, outer);
        assert_eq!(laid(&out, inner).0, Point::new(10.0, 10.0));
        assert_eq!(
            laid(&out, a).0,
            Point::new(0.0, 0.0),
            "in the inner group's own space"
        );
        assert_eq!(laid(&out, b).0, Point::new(0.0, 24.0), "a column, 4 apart");
    }
}
