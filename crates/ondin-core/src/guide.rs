//! Ruler guides — the straight lines a designer pulls out of a ruler to line
//! work up against.
//!
//! Guides are document state, saved with the file (§15 D18/D28): people place them
//! for the artwork, not for the sitting.
//!
//! They are deliberately not nodes: a guide has no parent, transform, paint or
//! bounds, never renders into the scene or takes part in hit-testing, and must not
//! appear in the layers tree or be swept up by Select All. As a `Node` each of
//! those would need special-casing away.
//!
//! A guide references a node through [`Guide::owner`], the frame it is scoped to;
//! that is a reference, not membership in the tree.

use crate::id::NodeId;
use peniko::Color;

/// The colour a guide is drawn in until someone overrides it: a dark red.
///
/// Deliberately not the selection blue, which would make a permanent line say
/// "this is selected".
pub const DEFAULT_GUIDE_COLOR: Color = Color::from_rgba8(0xa3, 0x33, 0x2d, 255);

/// Stable identity for a guide.
///
/// A [`NodeId`] under the newtype, minted from the session's one [`IdSource`]
/// (§5.2), so the two never collide and one reservation covers both. The wrapper
/// stops either being passed to the other's functions.
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
/// Named for the line: a horizontal guide is a horizontal line, placed by a `y`
/// (the confusion [`Self::position_label`] settles).
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
    /// Where the line sits — `y` for a horizontal guide, `x` for a vertical one —
    /// in [`Self::owner`]'s local space when it has one, else in world space.
    /// Never view space: panning and zooming change nothing about a guide.
    pub position: f64,
    /// An override for [`DEFAULT_GUIDE_COLOR`]. `None` means "the default", not a
    /// copy of today's, so retuning the default carries every guide never
    /// recoloured and the inspector's reset has something to reset to.
    pub color: Option<Color>,
    /// The frame this guide is scoped to, or `None` for a guide that runs the
    /// whole canvas.
    ///
    /// Kept as an owner on a flat list rather than a `Vec<Guide>` per node, so
    /// [`Document::guides`] stays one id-sorted list (a one-guide edit is a
    /// one-line diff, invariant 9) and changing scope is a field edit, one undoable
    /// operation ([`Operation::SetGuideScope`]).
    ///
    /// The owner is a live node ([`Document::apply`] refuses otherwise and the
    /// loader rejects a dangling reference), and deleting a frame takes its guides
    /// with it in the same transaction ([`crate::build::guides_of`]).
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
/// The caller supplies the frame's box for a scoped guide or the visible world for
/// a global one; this is the one place that knows a horizontal guide is placed by
/// its `y` and drawn along its `x`.
///
/// The position is deliberately not clamped to the box: a guide outgrown by a
/// shrunk frame keeps its number and the clip hides the line, so re-growing the
/// frame brings it back.
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
/// The inverse of [`guide_span`]'s placement, spelled once because getting the
/// component backwards is silent: the guide just tracks the wrong axis.
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
/// `world * guide_point(axis, local)` read back through [`guide_coord_of`], which
/// is only meaningful when the owner is upright ([`crate::build::Orientation`],
/// asked at `OndinApp::guide_lines`).
pub fn guide_point(axis: GuideAxis, position: f64) -> kurbo::Point {
    match axis {
        GuideAxis::Horizontal => kurbo::Point::new(0.0, position),
        GuideAxis::Vertical => kurbo::Point::new(position, 0.0),
    }
}
