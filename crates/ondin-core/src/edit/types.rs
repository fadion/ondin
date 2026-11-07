//! The values the editing arithmetic is phrased in: a box's resize handles, a
//! line's ends, and a path read back as anchors with their two control handles.

use crate::kurbo::{Point, Vec2};

/// Which end of a `Line` a gesture has hold of.
///
/// Named rather than a `bool`, so `true` need not be remembered as meaning one end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineEnd {
    /// The node's own origin.
    Start,
    /// `NodeKind::Line::end`, in the node's local space.
    End,
}

/// Which part of the selection box is being dragged to resize.
///
/// Four corners **drawn** as squares, plus the four sides, which are grabbable
/// but invisible: eight squares around a small shape is a wall of chrome (§15
/// D14). The band along a side is the affordance; the cursor naming the
/// direction is the announcement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
    Top,
    Right,
    Bottom,
    Left,
}

impl Handle {
    /// The corners, clockwise from the top-left. The order the overlay draws
    /// and hit-tests them in — and the only four that are drawn.
    pub const CORNERS: [Handle; 4] = [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomRight,
        Handle::BottomLeft,
    ];

    /// The sides, in the order the corners bound them: top, right, bottom,
    /// left — so `EDGES[i]` runs from `CORNERS[i]` to `CORNERS[(i + 1) % 4]`.
    pub const EDGES: [Handle; 4] = [Handle::Top, Handle::Right, Handle::Bottom, Handle::Left];

    /// Handle position as a unit fraction of the node's local box. An edge
    /// reports the midpoint of the side it owns, so `0.5` marks the axis it
    /// leaves alone.
    pub fn unit(self) -> (f64, f64) {
        match self {
            Handle::TopLeft => (0.0, 0.0),
            Handle::TopRight => (1.0, 0.0),
            Handle::BottomRight => (1.0, 1.0),
            Handle::BottomLeft => (0.0, 1.0),
            Handle::Top => (0.5, 0.0),
            Handle::Right => (1.0, 0.5),
            Handle::Bottom => (0.5, 1.0),
            Handle::Left => (0.0, 0.5),
        }
    }

    /// The local-space coordinate this handle holds still on each axis, or
    /// `None` for an axis it does not touch.
    ///
    /// Dragging the right side moves `x = w`, holds `x = 0` and says nothing about
    /// `y`. `(w, h)` is the box's current size.
    pub fn anchored(self, w: f64, h: f64) -> (Option<f64>, Option<f64>) {
        let (ux, uy) = self.unit();
        let hold = |u: f64, extent: f64| match u {
            0.0 => Some(extent),
            1.0 => Some(0.0),
            _ => None,
        };
        (hold(ux, w), hold(uy, h))
    }

    /// The direction the handle faces, out of the box, in the node's **local**
    /// space. Rotate it through the node's world transform to get the screen
    /// direction the resize cursor should point along.
    pub fn outward(self) -> (f64, f64) {
        let (x, y) = self.unit();
        (x * 2.0 - 1.0, y * 2.0 - 1.0)
    }
}

/// Which of an anchor's two handles a gesture has hold of.
///
/// Named rather than a `bool`, as [`LineEnd`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// The handle leaving the anchor — `PenAnchor::out`.
    Out,
    /// The handle arriving at it — `PenAnchor::back`.
    Back,
}

impl Side {
    pub const BOTH: [Side; 2] = [Side::Out, Side::Back];
}

/// Below this a handle is no handle at all. Far under anything visible at any
/// zoom, and the one place "does this anchor bend" is decided.
const FLAT: f64 = 1e-9;

fn flat(v: Vec2) -> bool {
    v.hypot() < FLAT
}

/// One anchor of a pen path, in world space.
///
/// `out` and `back` are the two control handles, each an offset from `at`. An
/// anchor is **smooth** when they mirror (`back == −out`) and *broken* when they do
/// not — a state the pen authors on purpose, so the mirror is a property to read
/// ([`Self::is_smooth`]) rather than an invariant.
///
/// A corner has both handles zero, so the straight case falls out of the curved
/// one rather than being a mode.
#[derive(Clone, Copy, Debug)]
pub struct PenAnchor {
    pub at: Point,
    /// The handle leaving this anchor, as an offset from it.
    pub out: Vec2,
    /// The handle *arriving* at it, also as an offset — so a mirrored anchor has
    /// `back == -out` and a broken one does not.
    ///
    /// Two vectors rather than one mirrored one, so an anchor where the segment
    /// comes in straight and leaves on a curve (`back` zero, `out` long) can be
    /// expressed. Alt while shaping breaks them apart (`canvas::pen_input`).
    pub back: Vec2,
    /// How far this anchor's corner is rounded — `NodeKind::Path::corner_radii`,
    /// carried **on** the anchor rather than in a list beside it.
    ///
    /// The model stores radii as a parallel `Vec` indexed by anchor, which every
    /// insert or remove would have to renumber, and the failure is silent. Riding
    /// on the anchor it moves, deletes and reverses with the point. The flattening
    /// back to a list happens once, at the model boundary ([`super::pen_radii`]).
    pub radius: f64,
}

impl PenAnchor {
    pub fn corner(at: Point) -> Self {
        Self {
            at,
            out: Vec2::ZERO,
            back: Vec2::ZERO,
            radius: 0.0,
        }
    }

    /// An anchor whose two handles mirror each other — what a plain drag makes.
    pub fn smooth(at: Point, out: Vec2) -> Self {
        Self {
            at,
            out,
            back: -out,
            radius: 0.0,
        }
    }

    /// Aim the handle on `side` at `to`, mirroring the other unless `break_pair` —
    /// the whole of what a drag does to an anchor.
    ///
    /// Here rather than inline in the canvas's pen input so the rule is testable
    /// without a window and stated once. Either handle, because the node tool can
    /// grab the incoming one; the other becomes its negation unless the pair is
    /// broken.
    pub fn pull_side(&mut self, side: Side, to: Point, break_pair: bool) {
        let v = to - self.at;
        // The mirrored arms rebuild the anchor from a constructor, which would drop
        // the radius; saved and put back so a new field meets this line.
        let radius = self.radius;
        match (side, break_pair) {
            // The mirrored case *is* [`Self::smooth`], from either side.
            (Side::Out, false) => *self = Self::smooth(self.at, v),
            (Side::Back, false) => *self = Self::smooth(self.at, -v),
            (Side::Out, true) => self.out = v,
            (Side::Back, true) => self.back = v,
        }
        self.radius = radius;
    }

    /// [`Self::pull_side`] with the **retract snap**: within `snap` of the anchor
    /// the handle lands exactly *on* it, which is how a smooth point becomes a
    /// corner again.
    ///
    /// `is_corner` wants both handles at zero to better than 1e-9, which no hand
    /// can drag to. Retracting *is* aiming the handle at the anchor, so the mirror
    /// zeroes the other side and `Alt` still buys a one-sided retract.
    ///
    /// The pen needs it too, so it is here rather than in [`super::pull_handle`]: a
    /// pen press plants its anchor on the *snapped* point and shapes it from the
    /// *raw* pointer, so every click would leave a handle as long as the snap
    /// adjustment.
    pub fn pull_within(&mut self, side: Side, to: Point, break_pair: bool, snap: f64) {
        let to = if (to - self.at).hypot() <= snap {
            self.at
        } else {
            to
        };
        self.pull_side(side, to, break_pair);
    }

    /// The control point on `side`.
    pub fn handle(&self, side: Side) -> Point {
        match side {
            Side::Out => self.leaving(),
            Side::Back => self.arriving(),
        }
    }

    /// The control point leaving this anchor, and the one arriving at it.
    pub fn leaving(&self) -> Point {
        self.at + self.out
    }
    pub fn arriving(&self) -> Point {
        self.at + self.back
    }
    /// Whether this anchor bends anything at all.
    ///
    /// Inferred from the handles, and deliberately lossy: a smooth point whose
    /// handles are dragged back to nothing is a corner. A stored flag would have to
    /// live in the model to survive a commit, and `NodeKind::Path` holds only a
    /// `BezPath`.
    pub fn is_corner(&self) -> bool {
        flat(self.out) && flat(self.back)
    }

    /// Whether the two handles are each other's mirror — the anchor is smooth, or
    /// is a corner, which is the same statement with both of them at zero.
    ///
    /// What a numeric handle edit asks before deciding whether to mirror: a field
    /// has no `Alt` to hold, so a broken anchor stays broken and a smooth one
    /// stays smooth.
    pub fn is_smooth(&self) -> bool {
        flat(self.out + self.back)
    }

    /// Whether the segment running from this anchor to `next` is a straight line.
    ///
    /// Asks the two handles that segment uses, not whether both anchors are
    /// corners. An anchor that *arrives* straight and *leaves* on a curve is not a
    /// corner, yet the segment into it is a line; `is_corner` on both ends emitted
    /// a cubic with control points on its endpoints, which the reader does not
    /// give back, so a path grew on every read and write.
    pub fn straight_to(&self, next: &Self) -> bool {
        flat(self.out) && flat(next.back)
    }

    /// The same anchor with its two handles swapped — what it becomes when the
    /// subpath through it is traversed the other way.
    pub fn reversed(self) -> Self {
        Self {
            at: self.at,
            out: self.back,
            back: self.out,
            radius: self.radius,
        }
    }
}

/// One subpath of a path, as anchors — what [`super::pen_anchors`] reads a
/// `BezPath` back into and [`super::pen_path`] writes out again.
///
/// A flattened boolean or an import holds several subpaths, each open or closed
/// on its own, so the reader returns a list of these; one anchor run would drop
/// everything after the first `MoveTo`.
#[derive(Clone)]
pub struct PenSubpath {
    pub anchors: Vec<PenAnchor>,
    /// Whether the last anchor joins back to the first. The joining segment is
    /// *not* an anchor: `pen_outline` builds it from the two it already has, and
    /// the reader folds the duplicate point a `ClosePath` leaves behind.
    pub closed: bool,
}

impl PenSubpath {
    /// Traverse this subpath the other way round.
    ///
    /// Used to resume drawing from an open path's **first** anchor, since the pen
    /// only appends. Each anchor's handles swap with it, or every curve would flip.
    /// On a closed subpath it reverses the **winding**, which a fill rule can see.
    pub fn reverse(&mut self) {
        self.anchors.reverse();
        for a in &mut self.anchors {
            *a = a.reversed();
        }
    }
}

/// One anchor of a path, by position in [`super::pen_anchors`]' output.
///
/// Indices, not identity (§15 D114): the model gives an anchor no stable name,
/// so a point is *where it is in the list* and every structural edit invalidates
/// the ones after it. The app's point selection truncates on such an edit.
pub type PointRef = (usize, usize);

#[cfg(test)]
mod tests {
    use super::*;

    /// Dragging either handle mirrors the other, and Alt breaks the pair.
    #[test]
    fn pulling_the_incoming_handle_mirrors_the_outgoing_one() {
        let at = Point::new(10.0, 10.0);
        let mut a = PenAnchor::corner(at);

        a.pull_side(Side::Back, Point::new(4.0, 10.0), false);
        assert_eq!(
            a.back,
            Vec2::new(-6.0, 0.0),
            "the handle went to the pointer"
        );
        assert_eq!(a.out, Vec2::new(6.0, 0.0), "and the other side mirrored it");

        a.pull_side(Side::Back, Point::new(10.0, 2.0), true);
        assert_eq!(a.back, Vec2::new(0.0, -8.0));
        assert_eq!(
            a.out,
            Vec2::new(6.0, 0.0),
            "a broken pair leaves the outgoing handle alone"
        );

        // `handle` reads back what `pull_side` wrote, which is what the overlay
        // draws and what the press hit-tests — so the two cannot disagree.
        assert_eq!(a.handle(Side::Back), Point::new(10.0, 2.0));
        assert_eq!(a.handle(Side::Out), Point::new(16.0, 10.0));
    }
}
