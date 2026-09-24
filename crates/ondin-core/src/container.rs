//! Container layout (§5.3c) — so far, build step 2: **absolute insets on
//! frames**, which is what this project calls constraints (§15 D871).
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
fn placed_at(slot: Point, bx: Rect, linear_of: Affine) -> Affine {
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
