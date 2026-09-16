//! Ruler guides — the straight lines a designer pulls out of a ruler to line
//! work up against.
//!
//! Guides are **document state**, saved with the file, for the same reason the
//! canvas ground is (§15 D18/D28): people place them for the artwork, not for
//! the sitting. A guide down the middle of a layout is part of how that layout
//! was built, and losing it on reopen would make it worthless. They are the
//! second thing in a `Document` that is not a node.
//!
//! They are not nodes, though, and deliberately so. A guide has no parent, no
//! transform, no paint and no bounds; it never renders into the scene, never
//! participates in hit-testing for artwork, and must not show up in the layers
//! tree or be swept up by Select All. Every one of those would have to be
//! special-cased away if a guide were a `Node`, which is a longer list than the
//! five operations it takes to keep them apart.
//!
//! A guide **does** now reference a node — [`Guide::owner`], the frame it is
//! scoped to — which is the one thing about them that has moved toward the tree.
//! It is a reference and not membership: a scoped guide still has no parent, no
//! transform of its own and no place in the layers tree.

use crate::id::NodeId;
use peniko::Color;

/// The colour a guide is drawn in until someone overrides it — the design's
/// dark red (`design/Editor.dc.html`).
///
/// Deliberately not the selection blue: guides sit over artwork permanently,
/// and a permanent line in the colour the app uses to say "this is selected"
/// would be a standing lie.
pub const DEFAULT_GUIDE_COLOR: Color = Color::from_rgba8(0xa3, 0x33, 0x2d, 255);

/// Stable identity for a guide.
///
/// A [`NodeId`] under the newtype, minted from the session's one [`IdSource`]
/// (§5.2) — so a guide id and a node id can never collide, and the reservation
/// a loaded file gets covers both without a second counter. The wrapper is what
/// stops the two being passed to each other's functions: nothing in the model
/// accepts "an id", only a node id or a guide id.
///
/// [`IdSource`]: crate::id::IdSource
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GuideId(pub NodeId);

impl std::fmt::Debug for GuideId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "guide {:?}", self.0)
    }
}

impl GuideId {
    /// The on-disk form, shared with [`NodeId::to_wire`].
    pub fn to_wire(self) -> String {
        self.0.to_wire()
    }

    /// Parse the wire form. `None` on malformed input.
    pub fn from_wire(s: &str) -> Option<GuideId> {
        NodeId::from_wire(s).map(GuideId)
    }
}

/// Which way a guide runs.
///
/// Named for the *line*, as a designer names it: a **horizontal** guide is a
/// horizontal line, and the number that places it is therefore a `y`. The axis
/// a guide constrains is the opposite of the one it is named for, which is
/// exactly the confusion [`Self::position_label`] exists to settle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuideAxis {
    /// A horizontal line, placed by its `y`. Pulled out of the top ruler.
    Horizontal,
    /// A vertical line, placed by its `x`. Pulled out of the left ruler.
    Vertical,
}

impl GuideAxis {
    /// The name of the coordinate that places this guide: `"Y"` for a
    /// horizontal one, `"X"` for a vertical one.
    pub fn position_label(self) -> &'static str {
        match self {
            GuideAxis::Horizontal => "Y",
            GuideAxis::Vertical => "X",
        }
    }
}

/// One guide: a line at a coordinate in its owner's space, optionally
/// recoloured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub id: GuideId,
    pub axis: GuideAxis,
    /// Where the line sits — `y` for a horizontal guide, `x` for a vertical one
    /// — read in [`Self::owner`]'s **local** space when it has one and in
    /// **world** space when it does not.
    ///
    /// Never view space either way: a guide is anchored to the artwork, so
    /// panning and zooming move it on screen and change nothing about it.
    pub position: f64,
    /// An override for [`DEFAULT_GUIDE_COLOR`]. `None` means "the default", not
    /// "a copy of today's default" — so retuning the default carries every
    /// guide that was never recoloured with it, and the inspector's reset has
    /// something to reset *to* rather than a colour to re-enter.
    pub color: Option<Color>,
    /// The frame this guide is scoped to, or `None` for a guide that runs the
    /// whole canvas.
    ///
    /// **A reference to a node, from the one thing in the document that is not
    /// one.** It is stored as an owner on a flat list rather than as a
    /// `Vec<Guide>` per node, which keeps [`Document::guides`] one id-sorted
    /// list — so a one-guide edit is still a one-line diff (§5.11 invariant 9)
    /// — and makes changing scope a **field edit** rather than a move between
    /// two collections. That is what lets a guide be dragged from global to
    /// scoped and back as one undoable operation
    /// ([`Operation::SetGuideScope`]).
    ///
    /// Two invariants ride on it, both enforced rather than hoped for: the
    /// owner is a live node ([`Document::apply`] refuses a guide owned by one
    /// that is not, and the loader rejects a dangling reference), and deleting a
    /// frame takes its guides with it in the same transaction
    /// ([`crate::build::guides_of`]).
    ///
    /// [`Document::guides`]: crate::document::Document::guides
    /// [`Document::apply`]: crate::document::Document::apply
    /// [`Operation::SetGuideScope`]: crate::op::Operation::SetGuideScope
    pub owner: Option<NodeId>,
}

impl Guide {
    /// The colour to draw this guide in.
    pub fn color(&self) -> Color {
        self.color.unwrap_or(DEFAULT_GUIDE_COLOR)
    }
}

/// A guide's line across `box_`, as two points in the same space `box_` and
/// `position` are measured in.
///
/// The span is the frame's own box for a scoped guide and the visible world for
/// a global one; the caller supplies whichever it means, so this stays the one
/// place that knows a horizontal guide is placed by its `y` and drawn along its
/// `x`.
///
/// **The span is not clamped to the box on the guide's own axis**, deliberately.
/// A guide whose offset has outgrown a shrunk frame keeps the number and lets
/// the clip hide the line, so re-growing the frame brings it back — the same
/// bargain a corner radius that exceeds half the shorter side makes (§5.6).
pub fn guide_span(
    axis: GuideAxis,
    position: f64,
    box_: kurbo::Rect,
) -> (kurbo::Point, kurbo::Point) {
    match axis {
        GuideAxis::Horizontal => (
            kurbo::Point::new(box_.x0, position),
            kurbo::Point::new(box_.x1, position),
        ),
        GuideAxis::Vertical => (
            kurbo::Point::new(position, box_.y0),
            kurbo::Point::new(position, box_.y1),
        ),
    }
}

/// The component of `p` that places a guide of this axis: `y` for a horizontal
/// one, `x` for a vertical one.
///
/// The inverse of [`guide_span`]'s placement, and the reason a pointer position
/// can be turned into a guide coordinate in any space without each caller
/// re-deciding which component it wants. Getting that backwards is silent — the
/// guide simply tracks the wrong axis — so it is spelled once.
pub fn guide_coord_of(axis: GuideAxis, p: kurbo::Point) -> f64 {
    match axis {
        GuideAxis::Horizontal => p.y,
        GuideAxis::Vertical => p.x,
    }
}

/// A point on the line a guide of this axis draws at `position`, with the other
/// coordinate at zero.
///
/// For projecting a guide between spaces: a scoped guide's world coordinate is
/// `world * guide_point(axis, local)` read back through [`guide_coord_of`], and
/// that is only meaningful when the owner is upright — which is exactly the
/// condition [`crate::build::Orientation`] is asked about at the one site that
/// does it (`OndinApp::guide_lines`).
pub fn guide_point(axis: GuideAxis, position: f64) -> kurbo::Point {
    match axis {
        GuideAxis::Horizontal => kurbo::Point::new(0.0, position),
        GuideAxis::Vertical => kurbo::Point::new(position, 0.0),
    }
}
