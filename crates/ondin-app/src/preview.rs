//! Uncommitted gesture state (§9.3, invariant 5).
//!
//! A gesture — drag, resize, rotate, marquee, pen, text edit — lives here while
//! it is in progress and never touches the `Document`. Exactly one
//! `Transaction` is committed when it ends.
//!
//! `EditorSession::set_preview` projects the pending transaction into
//! `RenderOverrides` (§6.2), so the preview and the result come from one
//! description of the edit. Overlays that are not document content (marquee, pen
//! rubber-band, caret, snap guides, handles) are drawn directly with egui.
//!
//! [`PointSet`], the points selected inside a path, is not a gesture: it outlives
//! the drags that read it, like a layer selection, and lives here because the node
//! tool does.

use crate::tools::Tool;
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::{CharSpans, GuideAxis, GuideId, NodeId, ParaSpans, TextEdit};
use std::collections::BTreeSet;
use std::ops::Range;

/// What a handle drag transforms.
///
/// One layer is transformed in **its own** frame, so a rotated rectangle resizes
/// along the edges the designer can see. Several layers have no shared frame, so
/// they are transformed together in one **upright** box over all of them.
///
/// Carried by the gesture rather than looked up when it is applied, so a drag
/// cannot change what it is transforming halfway through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Layer(NodeId),
    Selection,
}

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

/// What a marquee does to the selection it starts from, chosen by the modifier held at
/// the press: none replaces, `Shift` toggles what it covers, `Alt` takes it away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarqueeMode {
    Replace,
    Toggle,
    Subtract,
}

impl MarqueeMode {
    pub fn from_modifiers(shift: bool, alt: bool) -> Self {
        match (shift, alt) {
            (true, _) => MarqueeMode::Toggle,
            (false, true) => MarqueeMode::Subtract,
            (false, false) => MarqueeMode::Replace,
        }
    }
}

/// The pointer gesture currently in progress.
pub enum Drag {
    None,
    /// Panning the camera (drag on empty canvas, or with the hand tool).
    Pan,
    /// Moving the selection; `anchor` is where the pointer went down.
    Move {
        anchor: Point,
    },
    /// Dragging a resize handle. `handle` identifies which one.
    Resize {
        target: Target,
        handle: Handle,
    },
    /// Leaning a side of the box sideways — Ctrl on the same band a resize
    /// grabs. Sides only: a corner has no side facing it to hold still, so it
    /// keeps resize and the rotate ring beyond it.
    ///
    /// `press` is the world point of the press, and the lean is measured from it:
    /// measured from the box centre instead, grabbing a side anywhere but its
    /// midpoint made the first frame jump to the lean that offset implied.
    Skew {
        target: Target,
        handle: Handle,
        press: Point,
        /// The world-space lean asked for so far — **what the chrome draws with**,
        /// and `IDENTITY` until the pointer moves.
        ///
        /// Only a `Target::Selection` reads it: a set's frame is an upright union
        /// that cannot lean, and re-unioning the previewed bounds inflated it as
        /// the members sheared. A single node's world transform carries the shear.
        /// Written from the computation that builds the transaction, so the frame
        /// and the shapes agree about the angle.
        lean: Affine,
    },
    Rotate {
        target: Target,
        pivot: Point,
        start: f64,
        /// The world-space turn asked for so far — **what the chrome draws with**,
        /// and `IDENTITY` until the pointer moves. [`Drag::Skew`]'s `lean` for
        /// rotation.
        turn: Affine,
    },
    /// Dragging one end of a [`ondin_core::NodeKind::Line`].
    ///
    /// A line has no box: its two ends replace the corners, rotate and skew. They
    /// are not symmetric in the model — the start *is* the node's origin (§5.3
    /// stores only the far point), so moving it is a transform edit plus a
    /// re-expressed `end`, while moving the end is one geometry patch.
    LineEnd {
        id: NodeId,
        which: LineEnd,
    },
    /// Sliding type along the rail it is set on.
    ///
    /// Carries only the node: a drag previews through `set_preview` and leaves
    /// `Resolved` alone, so the committed offset stands still for the gesture and
    /// is read each frame. A carried copy of the press-time offset could drift.
    RailOffset {
        id: NodeId,
    },
    /// Dragging a laid container's gap or padding band: `band` is what was
    /// grabbed, `anchor` the press in the container's local space, and `base` the
    /// value the band drew there — the delta along the band's axis is added to it.
    /// Each frame previews one `SetDisplay` and the release commits it.
    Spacing {
        id: NodeId,
        band: crate::spacing::BandKind,
        anchor: Point,
        base: f64,
    },
    /// Moving a layer's transform origin ([`ondin_core::Pivot`]).
    ///
    /// Carries no preview: the pivot is not scene content, so the canvas draws the
    /// marker straight from the live pointer, like a marquee, and commits one
    /// `SetPivot` on release.
    Pivot {
        id: NodeId,
    },
    /// Rubber-band selection from `origin` to the current pointer. A layer marquee
    /// selects live; a point marquee (node tool) resolves on release.
    Marquee {
        origin: Point,
        mode: MarqueeMode,
        /// The layer selection at the press. Each frame applies the band to it
        /// afresh, or a Shift marquee would flip a layer on every frame; a cancel
        /// puts it back.
        base: Box<crate::session::Selection>,
    },
    /// Drawing a new shape. `id` is minted up front so the preview is stable
    /// across frames; committed once on release.
    Create {
        id: NodeId,
        parent: NodeId,
        parent_world: Affine,
        start: Point,
        shape: Tool,
        /// How many sides the polygon or star being drawn has, live —
        /// `↑`/`↓` change it mid-drag (`docs/shortcuts.md` §9).
        ///
        /// On the gesture because there is no node yet: `create_tx` rebuilds the
        /// shape from these fields every frame. Never read for shapes without a
        /// side count.
        sides: u32,
        /// Where the pointer was on the previous frame of this drag.
        ///
        /// Only `Space` needs it: holding it repositions the uncommitted shape,
        /// the one part that integrates motion. Starts at `start`.
        last: Point,
    },
    /// Moving the selected anchors of a path, with the node tool.
    ///
    /// `grab` is where the pointer went down, so the anchors move by a **delta**:
    /// the press can land up to `PEN_PICK_PX` off the anchor, and absolute
    /// tracking would snatch it that far on the first frame.
    ///
    /// Which points move is [`PointSet`], read live rather than carried. That is
    /// safe because the press sets the point selection, which cannot change again
    /// until the button comes up.
    Points {
        id: NodeId,
        grab: Point,
        /// The anchor the press actually landed on — **what the snap measures**.
        ///
        /// A set of points has no single position to snap, and snapping their box
        /// would pull the set by whichever edge caught. This anchor is offered to
        /// the lattice and the guides, and the adjustment it earns moves the set.
        anchor: PointRef,
    },
    /// Dragging one handle of one anchor, with the node tool.
    ///
    /// Absolute, unlike [`Drag::Points`]: a handle *is* the pointer position.
    /// Alt breaks the pair, read live every frame, as in the pen (§15 D114).
    Handle {
        id: NodeId,
        at: PointRef,
        side: Side,
    },
    /// Dragging a **segment** of a path, with the node tool.
    ///
    /// Plain **moves** the segment, `Ctrl` **bends** it — the split `Ctrl` has on
    /// an anchor. The modifier is re-read every frame rather than latched at the
    /// press (`sync_alt_clone`'s rule, §15 D40): both readings measure the same
    /// total from the same press.
    ///
    /// `at` names the segment by the anchor it **leaves**, which is
    /// `geometry::anchor_runs`' numbering and the index a corner radius is stored
    /// at. `t` is where along it the press landed, fixed at the press.
    Segment {
        id: NodeId,
        at: PointRef,
        t: f64,
        /// Where the pointer went down, so the drag is a **delta**, as for
        /// [`Drag::Points`].
        grab: Point,
    },
    /// Turning the selected points about the centre of the box over them.
    ///
    /// A set of points has no pivot to move, so it turns about the middle of the
    /// box the handles were built from. `start` is the angle from that centre to
    /// the press, so the gesture measures a *change* and does not snap to the
    /// pointer on the first frame.
    PointRotate {
        id: NodeId,
        pivot: Point,
        start: f64,
    },
    /// Dragging a corner of the box over the selected points, with the node tool.
    ///
    /// The box is carried, not re-derived: it is the extent of the points being
    /// scaled, so reading it fresh each frame would compound the scale.
    PointBox {
        id: NodeId,
        handle: Handle,
        /// The world-space box the points occupied when the press landed.
        box_: Rect,
        /// Where the pointer went down. Not an origin — a scale tracks the pointer
        /// absolutely — but the answer to "has this gesture moved at all", which
        /// keeps a plain click on a corner from scaling the set.
        grab: Point,
    },
    /// Sliding the picture around inside a fixed frame (`Tool::ImageEdit`).
    ///
    /// Every frame re-derives from `start`, the crop rectangle at the press, as
    /// `resize_tx` and `point_box_tx` do: reading the preview back would compound
    /// the delta and the picture would accelerate away from the pointer.
    CropPan {
        id: NodeId,
        /// Which fill of the node is the image being cropped.
        fill: usize,
        start: Rect,
        /// Where the press landed, in the node's **local** space.
        grab: Point,
        /// The same press in **world** space, answering "has this gesture moved at
        /// all". World because `CanvasRenderer::gesture_travelled` measures there:
        /// the threshold is a screen distance, and a layer scaled 10× would
        /// otherwise need a tenth of the hand movement to count as a drag.
        grab_world: Point,
    },
    /// Dragging a handle of the frame to **crop** — the layer's box shrinks or
    /// grows and the picture stays exactly where it is (`Tool::ImageEdit`).
    ///
    /// Both starting values are carried: `frame` is the layer's local box at the
    /// press and `start` the crop rectangle, and `ondin_core::crop_reframed` maps
    /// between them. Reading either off the preview would compound.
    ///
    /// `limit` is `ImageRef::picture_box` at the press — the rectangle the whole
    /// photograph occupies, and the wall the handle stops at. Captured because it
    /// is computed from the crop this gesture is changing.
    CropResize {
        id: NodeId,
        fill: usize,
        handle: Handle,
        frame: Rect,
        start: Rect,
        limit: Rect,
        /// Where the press landed in **world** space, as `CropPan::grab_world`: a
        /// click on a handle that never travels must commit nothing.
        grab_world: Point,
    },
    /// Dragging inside a text node to extend its selection.
    TextSelect,
    /// Pulling a ruler guide out of a ruler, or sliding an existing one.
    Guide(GuideDrag),
}

impl Drag {
    /// The transform a **multi-selection's** frame should be drawn with while this
    /// gesture is in flight, if the gesture reorients the set.
    ///
    /// A set's box is the upright union of its members' world bounds, so it cannot
    /// lean or turn; the frame is a fixed box from the **committed** tree plus the
    /// transform from here. One method rather than a match per gesture at the draw
    /// site, so a new reorienting gesture declares itself here. `None` covers every
    /// other drag, including resize and move, whose frames follow the re-unioned
    /// preview.
    pub fn selection_frame(&self) -> Option<Affine> {
        match self {
            Drag::Skew {
                target: Target::Selection,
                lean,
                ..
            } => Some(*lean),
            Drag::Rotate {
                target: Target::Selection,
                turn,
                ..
            } => Some(*turn),
            _ => None,
        }
    }
}

impl Drag {
    /// Whether this gesture should scroll the view when it is dragged into the
    /// canvas edge (`canvas::autopan`).
    ///
    /// The ones that answer no are those not reaching for somewhere: `Pan`,
    /// **every gesture that turns about a fixed pivot**, and a guide. Stated as a
    /// rule rather than a list of names so a new turning gesture is covered.
    ///
    /// - `Pan` already moves the view; auto-scrolling would add the band's speed to
    ///   the hand's and the view would accelerate away at an edge.
    /// - `Rotate` and [`Drag::PointRotate`] turn around a fixed pivot, so they can
    ///   always be completed where they started, and a drifting view would be the
    ///   tool moving unasked.
    /// - A guide drag: its band and its home are the same place. The bars are
    ///   `rulers::THICKNESS` (20pt) and the band is `canvas::AUTOPAN_BAND` (32), so
    ///   a gesture that starts on a bar, as every new guide does, is inside the
    ///   band before it begins. The Guide panel's position field covers placing a
    ///   guide beyond the visible area.
    ///
    /// Everything else can want more room than the window has: moving, resizing or
    /// skewing past the edge, marqueeing, drawing, dragging a pivot, or extending a
    /// text selection.
    pub fn autopans(&self) -> bool {
        !matches!(
            self,
            Drag::None
                | Drag::Pan
                | Drag::Rotate { .. }
                | Drag::PointRotate { .. }
                | Drag::Guide(_)
        )
    }
}

/// A guide being dragged.
///
/// The live position lives here rather than in `RenderOverrides`, because a
/// guide is not scene content and the renderer never sees one. The canvas draws
/// the guide from this while the drag is up and from the document once it commits.
pub struct GuideDrag {
    /// `None` while the guide is being pulled out of a ruler and does not exist
    /// in the document yet.
    pub id: Option<GuideId>,
    pub axis: GuideAxis,
    /// Where the line currently is, read in [`Self::owner`]'s space — so a scoped
    /// guide's live position is a frame-local offset, exactly as the committed
    /// `Guide::position` is.
    pub position: f64,
    /// The frame the guide would belong to if the drag ended now, or `None` for
    /// the canvas.
    ///
    /// Tracked live rather than resolved on release, so the gesture says what
    /// letting go will do: dragging over a frame clips the line to it.
    pub owner: Option<NodeId>,
    /// The **world** offset from the pointer to the guide's line, fixed at the
    /// press.
    ///
    /// A guide is grabbed, not teleported: the pointer is within
    /// `rulers::GUIDE_PICK_PT` of the line rather than on it, so putting the line
    /// at the pointer would make a click move it. It must survive the whole drag,
    /// or a dead zone released at the far end would jump by this much.
    ///
    /// A world vector rather than a scalar, because the guide's axis is the
    /// *owner's* and the owner changes mid-drag; a scalar would be reinterpreted
    /// across a frame boundary. The position is `local_of(pointer + grab)`.
    ///
    /// Zero for a guide pulled out of a ruler, which should appear under the pointer.
    pub grab: Vec2,
    /// Whether the pointer is over the ruler this guide belongs to.
    ///
    /// Dropping a guide there throws it away: a new one is never created, an
    /// existing one is removed. This drives the **commit**; what the gesture
    /// *announces* is [`Self::announces_discard`].
    pub discarding: bool,
    /// Whether the pointer has been off **both** rulers at any point during this
    /// drag. The corner square counts as *on* them — `rulers::ruler_at` gives it to
    /// the left bar, because that is the one painted over it.
    ///
    /// A new guide is dragged *out of* a ruler, so `discarding` is true from the
    /// press, but the guide is not yet placed rather than about to be deleted. The
    /// announcement waits for the pointer to leave the bar once; an existing guide
    /// is grabbed on the canvas (`guide_at` never answers over a bar) and announces
    /// immediately.
    pub left_ruler: bool,
    /// Whether Alt is held, making this drag place a **copy** and leave the
    /// original where it is.
    ///
    /// A live modifier read each frame, like `OndinApp::sync_alt_clone`; a `Guide`
    /// is its own template, so there is nothing to prepare. Named `copying` so a
    /// field access cannot be misread as `Clone::clone`.
    pub copying: bool,
    /// Whether the rulers were on when the press happened.
    ///
    /// Latched because `rulers_on()` is two live chords (`Shift+R` and `Ctrl+\`)
    /// and `OndinApp` dispatches them mid-drag; reading it live turned a guide
    /// **cancel** into a **create**. Both consumers read this field:
    /// `over_own_ruler`, which decides the commit, and `off_rulers`, which feeds
    /// [`Self::left_ruler`]. This latches the bars' existence, where `left_ruler`
    /// latches the pointer's position against them.
    pub rulers_shown: bool,
}

impl GuideDrag {
    /// Whether the gesture should *say* that letting go throws the guide away —
    /// the faded line and the bin cursor.
    ///
    /// One conjunction in one place, so the fade and the cursor cannot disagree.
    pub fn announces_discard(&self) -> bool {
        self.discarding && self.left_ruler
    }
}

impl Drag {
    pub fn is_none(&self) -> bool {
        matches!(self, Drag::None)
    }
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
    /// back to a list happens once, at the model boundary (`tools::pen_radii`).
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
    /// Here rather than inline in `canvas::pen_input` so the rule is testable
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
    /// The pen needs it too, so it is here rather than in `tools::pull_handle`: a
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

/// One subpath of a path, as anchors — what `tools::pen_anchors` reads a
/// `BezPath` back into and `tools::pen_path` writes out again.
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

/// A pen path being drawn: its destination artboard and the anchors placed so
/// far. Committed as one `Path` node when finished — or, when [`PenState::editing`]
/// is set, written back into the node whose endpoint the pen resumed.
pub struct PenState {
    pub parent: NodeId,
    pub parent_world: Affine,
    pub anchors: Vec<PenAnchor>,
    /// True while the button is still down on the anchor just placed, so
    /// dragging pulls its handles out.
    pub shaping: bool,
    /// The existing open path this pen is extending, if it is extending one
    /// rather than starting a new node (`canvas::pen_resume`).
    pub editing: Option<PenEdit>,
}

/// The existing `Path` node a pen gesture is extending.
pub struct PenEdit {
    pub id: NodeId,
    /// The node's **own** world transform, not its parent's, so writing the edited
    /// path back leaves the node's transform alone; `create_path`'s
    /// origin-at-the-first-anchor rule would move it.
    pub world: Affine,
    /// Every subpath the node holds, in world space, with the one being edited
    /// still in place at `active`. Finishing swaps the pen's anchors in and writes
    /// the whole list back, so untouched subpaths survive.
    ///
    /// **In the orientation they were read in.** Resuming at the *start* of a
    /// subpath reverses the working copy in `PenState::anchors`; this list keeps
    /// the original and `finish_pen` reverses back before splicing. That lets
    /// `tools::rewrite_path` recognise a no-op gesture, and extending the start
    /// *prepends* rather than flipping direction, which a fill rule and a dash
    /// pattern can see.
    pub subpaths: Vec<PenSubpath>,
    pub active: usize,
    /// Whether the pen grabbed the subpath's first anchor, so `anchors` is the
    /// reversed run and has to be turned back at commit.
    pub from_start: bool,
    /// Whether `subpaths[active]` is a **new, empty** run this gesture appended —
    /// what the pen bias's empty-canvas click starts (§15 D125).
    ///
    /// Says which subpath `finish_pen` may throw away: a run left with fewer than
    /// two anchors would write a lone `MoveTo` and an extra `corner_radii` entry.
    /// Pruning every short run would reach a *stored* one (an import can hold one)
    /// and make a no-op resume commit an edit (`tools::rewrite_path`'s no-op test).
    pub minted: bool,
}

impl PenEdit {
    /// The end the pen is holding — the anchor the press grabbed, and the one the
    /// path will grow from.
    ///
    /// The hover ring is drawn on this before the press, so the promise and the
    /// grab read the same anchor.
    ///
    /// `None` for a [`Self::minted`] run, which has no end yet.
    pub fn endpoint(&self) -> Option<Point> {
        let sub = self.subpaths.get(self.active)?;
        let i = if self.from_start {
            0
        } else {
            sub.anchors.len().checked_sub(1)?
        };
        Some(sub.anchors.get(i)?.at)
    }

    /// The active subpath as the pen works on it: appending only, so the run is
    /// reversed when the grab was at the start.
    pub fn working(&self) -> Vec<PenAnchor> {
        let mut sub = self.subpaths[self.active].clone();
        if self.from_start {
            sub.reverse();
        }
        sub.anchors
    }
}

/// One anchor of a path, by position in `tools::pen_anchors`' output.
///
/// Indices, not identity (§15 D114): the model gives an anchor no stable name,
/// so a point is *where it is in the list* and every structural edit invalidates
/// the ones after it. See `PointSet::retain_valid`.
pub type PointRef = (usize, usize);

/// Which points of which path are selected — a third kind of subject, beside
/// the layer `Selection` and the guide selection.
///
/// Beside `Selection`, not inside it: that is layers-or-guides, and points cannot
/// co-occur with either.
///
/// The node it belongs to is carried so a stale set cannot be read against a
/// different path.
///
/// **Anchors *or* segments, never both**: Delete needs a single subject. The two
/// sets live in one type and every mutator clears the other.
#[derive(Default)]
pub struct PointSet {
    id: Option<NodeId>,
    points: BTreeSet<PointRef>,
    /// The selected segments, each named by the anchor it **leaves**
    /// (`geometry::anchor_runs`' numbering), guarded by [`Self::retain_valid`].
    segments: BTreeSet<PointRef>,
}

impl PointSet {
    /// The points selected on `id`, or nothing if the set belongs elsewhere.
    ///
    /// Every read goes through the node id, so a stale set is invisible.
    pub fn on(&self, id: NodeId) -> impl Iterator<Item = PointRef> + '_ {
        let mine = self.id == Some(id);
        self.points.iter().copied().filter(move |_| mine)
    }

    pub fn contains(&self, id: NodeId, at: PointRef) -> bool {
        self.id == Some(id) && self.points.contains(&at)
    }

    /// The selected segments of `id`, or nothing if the set belongs elsewhere.
    pub fn segments(&self, id: NodeId) -> impl Iterator<Item = PointRef> + '_ {
        let mine = self.id == Some(id);
        self.segments.iter().copied().filter(move |_| mine)
    }

    /// How many segments of `id` are selected — the question the **box** asks,
    /// since exactly one segment does not get one (§15 D121).
    pub fn segment_count(&self, id: NodeId) -> usize {
        match self.id == Some(id) {
            true => self.segments.len(),
            false => 0,
        }
    }

    pub fn contains_segment(&self, id: NodeId, at: PointRef) -> bool {
        self.id == Some(id) && self.segments.contains(&at)
    }

    /// Whether **nothing** is selected here — no anchors and no segments.
    ///
    /// What Delete, Escape and the arrow keys ask before claiming the key from the
    /// layer: "this tool has no subject", not "no anchors".
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.segments.is_empty()
    }

    pub fn clear(&mut self) {
        self.id = None;
        self.points.clear();
        self.segments.clear();
    }

    /// Select exactly `at` on `id`, dropping whatever was selected before.
    pub fn set_one(&mut self, id: NodeId, at: PointRef) {
        self.id = Some(id);
        self.points.clear();
        self.segments.clear();
        self.points.insert(at);
    }

    /// Select the segment leaving `at` on `id`, dropping the anchors and any other
    /// segments — see the exclusivity note on the type.
    pub fn set_segment(&mut self, id: NodeId, at: PointRef) {
        self.id = Some(id);
        self.points.clear();
        self.segments.clear();
        self.segments.insert(at);
    }

    /// Add or remove `at` — Shift+click. Switching paths starts a fresh set
    /// rather than mixing two nodes' points, which nothing downstream could
    /// commit as one transaction anyway.
    pub fn toggle(&mut self, id: NodeId, at: PointRef) {
        if self.id != Some(id) {
            self.set_one(id, at);
            return;
        }
        self.segments.clear();
        if !self.points.remove(&at) {
            self.points.insert(at);
        }
    }

    /// Shift+click on a segment, the anchor version's exact counterpart.
    pub fn toggle_segment(&mut self, id: NodeId, at: PointRef) {
        if self.id != Some(id) {
            self.set_segment(id, at);
            return;
        }
        self.points.clear();
        if !self.segments.remove(&at) {
            self.segments.insert(at);
        }
    }

    /// Drop every reference the path no longer has, given its subpath lengths.
    ///
    /// A truncation rather than a remap: deleting an anchor renumbers every anchor
    /// after it, so a set kept across the edit would silently name the wrong
    /// points. The caller re-selects what it means to leave lit.
    ///
    /// Undo is the wrong case for this guard: rewinding can leave every index in
    /// range while the anchors under them moved, so `OndinApp::document_rewound`
    /// clears instead.
    ///
    /// No production caller. `#[cfg(test)]` would break the doc gate, because a
    /// production intra-doc link names this item; `allow` also hides the loss if
    /// the tests that assert it go.
    #[allow(dead_code)]
    pub fn retain_valid(&mut self, lengths: &[usize]) {
        let live = |(sub, anchor): &PointRef| lengths.get(*sub).is_some_and(|n| anchor < n);
        self.points.retain(live);
        self.segments.retain(live);
        if self.is_empty() {
            self.id = None;
        }
    }

    /// Renumber the references into subpath `sub` after it has been reversed, so
    /// the **same points** stay lit.
    ///
    /// A remap, where every other structural edit is a truncation ([`Self::
    /// retain_valid`]): reversing is a permutation, so there is a right answer, and
    /// clearing the selection would be wrong for a verb that leaves the shape
    /// alone. Unchanged indices would stay valid but light the mirrored points.
    ///
    /// Anchors mirror as `n − 1 − a`. **Segments mirror as `n − 2 − a`**, because a
    /// segment is named by the anchor it *leaves* (`geometry::anchor_runs`). On a
    /// `closed` subpath the arithmetic wraps, keeping the closing segment — named
    /// by the last anchor — the closing segment.
    pub fn reverse_within(&mut self, id: NodeId, sub: usize, len: usize, closed: bool) {
        if self.id != Some(id) || len == 0 {
            return;
        }
        let flip = |set: &mut BTreeSet<PointRef>, mirror: &dyn Fn(usize) -> Option<usize>| {
            let moved: Vec<PointRef> = set
                .iter()
                .copied()
                .filter(|(s, _)| *s == sub)
                .filter_map(|(s, a)| mirror(a).map(|a| (s, a)))
                .collect();
            set.retain(|(s, _)| *s != sub);
            set.extend(moved);
        };
        flip(&mut self.points, &|a| (len - 1).checked_sub(a));
        flip(&mut self.segments, &|a| match closed {
            // `2·len − 2 − a` is `len − 2 − a` lifted clear of zero before the
            // wrap, which `usize` needs.
            true if a < len => Some((2 * len - 2 - a) % len),
            true => None,
            false => (len - 2).checked_sub(a),
        });
    }
}

/// The caret's blink phase.
///
/// Chrome, so it lives here and not in `ondin_core::TextEdit`: when the caret is
/// *drawn* is about wall-clock time, and a timestamp on `TextEdit` would be a value
/// the save format and exporters have to ignore.
///
/// The phase restarts on activity, and activity is *observed* rather than
/// announced: a caret in its dark half on the frame you clicked reads as broken,
/// and rather than a call at each of the dozen movers in
/// `canvas::text_mode_input`, this compares the caret's own position every frame.
/// A dragged selection changes its range every frame and so stays solid. A hidden
/// caret stays hidden until something happens, so an unfocused window costs no
/// repaints; [`CaretBlink::phase`] answers a deadline for that reason.
#[derive(Debug)]
pub struct CaretBlink {
    /// When the current phase began.
    since: f64,
    /// What was true then. Anything in here changing restarts the phase — see
    /// [`BlinkState`].
    at: BlinkState,
}

/// Everything a caret restarts its blink for, in one comparable value.
///
/// Window focus sits beside the caret's position: regaining it is activity like a
/// keystroke, and a separate branch would show a caret halfway through the phase it
/// left.
///
/// The value comes from `InputState::focused`, the OS's answer rather than egui's
/// widget focus. `RawInput::default()` sets it **true**, so a session opened before
/// any focus event, or an integration that never tracks focus, shows the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlinkState {
    /// The selected range, and the content length beside it so that a *deletion* which
    /// leaves the offset alone still counts (`Delete` at a caret is exactly that).
    at: (Range<usize>, usize),
    focused: bool,
}

impl Default for CaretBlink {
    fn default() -> Self {
        // A state no caret can be in, so the first frame restarts the phase and the
        // caret is solid the instant a session opens.
        CaretBlink {
            since: 0.0,
            at: BlinkState {
                at: (usize::MAX..usize::MAX, usize::MAX),
                focused: false,
            },
        }
    }
}

impl CaretBlink {
    /// Half a cycle: solid for this long, then hidden for as long.
    ///
    /// 500ms, not read from the OS: neither winit nor egui exposes the setting, and
    /// the platforms default to about 500 (macOS) and 530 (Windows).
    const HALF: f64 = 0.5;

    /// Whether the caret is drawn at `now`, and how long until that answer changes —
    /// restarting the phase if anything in [`BlinkState`] has changed since the last
    /// call.
    ///
    /// `&mut self` because asking is what notices the move: observation and reset
    /// are one step. The deadline is `None` when nothing is due to change on its
    /// own (unfocused); egui repaints for the focus event itself.
    fn phase(&mut self, now: f64, at: (Range<usize>, usize), focused: bool) -> (bool, Option<f64>) {
        let state = BlinkState { at, focused };
        if state != self.at {
            self.at = state;
            self.since = now;
        }
        if !focused {
            return (false, None);
        }
        // Clamped at zero: a `now` behind `since` should not happen, and must not
        // leave the caret stuck dark.
        let elapsed = (now - self.since).max(0.0);
        // Solid half first, so the frame the phase restarts on always draws.
        let on = ((elapsed / Self::HALF) as u64).is_multiple_of(2);
        // Time to the next flip, always in `(0, HALF]` — never zero, which would ask
        // the canvas to repaint immediately and again on the frame after that.
        let next = Self::HALF - elapsed % Self::HALF;
        (on, Some(next))
    }
}

/// A text node open for editing. The caret model is `ondin_core::TextEdit`; the
/// app only decides when the session starts, and commits one `SetText` when it
/// ends (§9.3 — one editing session is one undo step).
pub struct TextSession {
    pub id: NodeId,
    pub editor: TextEdit,
    /// Content and both span lists **as the document last held them**, so a
    /// session that changed nothing can be closed without creating an undo step.
    ///
    /// "Last held", not "held when the session opened": the helpers that re-open
    /// the editor against an outside commit (`canvas::text_session_restyled`,
    /// `…_after`, `restyle_session_with`) re-base the editor, and
    /// `canvas::restyle_session` is the one place this moves with it. Otherwise
    /// `changed()` could only answer yes and a styling click cost two undo steps
    /// (§9.3).
    ///
    /// The spans are part of "nothing changed": bolding a word and typing nothing
    /// is a real edit. The paragraph list is what the session commits alongside the
    /// content (`Operation::SetText`), so anything left out is dropped on close.
    pub original: (String, CharSpans, ParaSpans),
    /// Whether this session **created** the node it is editing (§15 D518).
    ///
    /// Carried rather than inferred, because `original` cannot answer it: it is
    /// empty for a fresh node and equally for any pre-existing empty one, and §9.3
    /// removes only a **fresh** node left empty. `original` answers change
    /// detection, not freshness.
    pub created: bool,
    /// When the caret is drawn — see [`CaretBlink`]. `Default` is correct for a
    /// session that has just opened.
    pub blink: CaretBlink,
}

impl TextSession {
    pub fn changed(&self) -> bool {
        self.editor.content() != self.original.0
            || *self.editor.spans() != self.original.1
            || *self.editor.para_spans() != self.original.2
    }

    /// Whether the caret is drawn this frame, and in how many seconds that changes —
    /// see [`CaretBlink::phase`]. `None` means nothing is due to change on its own.
    ///
    /// The two reads are separate statements so the borrows are.
    pub fn caret_phase(&mut self, now: f64, focused: bool) -> (bool, Option<f64>) {
        let at = (self.editor.selected_range(), self.editor.content().len());
        self.blink.phase(now, at, focused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::IdSource;

    /// The caret blinks, and a move landing in the dark half lights it at once.
    #[test]
    fn the_caret_blinks_and_a_move_relights_it() {
        let mut b = CaretBlink::default();
        let caret = |n: usize| (n..n, 10_usize);
        let on = |b: &mut CaretBlink, t: f64, n: usize| b.phase(t, caret(n), true).0;

        // Solid from the first frame — `Default` cannot match any real caret, so the
        // phase starts here whatever the clock says.
        assert!(on(&mut b, 100.0, 3), "solid on arrival");
        assert!(on(&mut b, 100.4, 3), "still solid inside the first half");
        assert!(!on(&mut b, 100.6, 3), "dark in the second");
        assert!(on(&mut b, 101.1, 3), "and on again in the next cycle");

        // Now the part that matters: move the caret while it is dark.
        assert!(!on(&mut b, 101.7, 3), "dark again");
        assert!(
            on(&mut b, 101.75, 4),
            "a moved caret is drawn on the very frame it moves, not at the next tick"
        );
        assert!(
            on(&mut b, 102.2, 4),
            "and the new phase runs from there rather than from the old start"
        );
    }

    /// An unfocused window hides the caret and schedules nothing; regaining focus
    /// relights it rather than resuming mid-phase.
    #[test]
    fn an_unfocused_window_hides_the_caret_and_costs_nothing() {
        let mut b = CaretBlink::default();
        let caret = (3..3, 10_usize);

        assert!(b.phase(0.0, caret.clone(), true).0, "solid, focused");
        let (on, next) = b.phase(0.1, caret.clone(), false);
        assert!(!on, "hidden the moment focus goes");
        assert_eq!(
            next, None,
            "and nothing scheduled — a hidden caret changes only on a focus event, \
             which egui repaints for by itself"
        );
        // Deep into what would have been the dark half, still hidden.
        assert_eq!(b.phase(0.8, caret.clone(), false), (false, None));

        // Back: solid immediately, not halfway through the phase it left.
        let (on, next) = b.phase(0.9, caret.clone(), true);
        assert!(on, "focus returning relights the caret at once");
        assert!(next.is_some(), "and the schedule resumes");
        assert!(
            b.phase(1.3, caret, true).0,
            "the new phase runs from the moment focus came back"
        );
    }

    /// The deadline lands on the **next** flip, not merely on *a* flip.
    ///
    /// Sampling a hair either side of it is what bites: adding a fixed `HALF` flips
    /// the phase from anywhere, so checking only that the answer changes cannot tell
    /// it from the distance to the boundary.
    #[test]
    fn the_blink_deadline_lands_on_the_next_flip() {
        let mut b = CaretBlink::default();
        let caret = (0..0, 4_usize);
        let eps = 0.005;
        // Prime the phase once, so `since` is fixed and the samples below fall at
        // assorted points inside it rather than all at its start.
        b.phase(0.0, caret.clone(), true);
        for t in [0.13, 0.27, 0.49, 0.51, 0.8, 1.24, 3.37] {
            let (on, next) = b.phase(t, caret.clone(), true);
            let next = next.expect("focused, so a deadline is due");
            assert!(
                next > 0.0 && next <= 0.5,
                "at t={t} the deadline must be inside one half-cycle, and never zero \
                 (which would ask for an immediate repaint, then another): {next}"
            );
            if next <= eps {
                continue;
            }
            assert_eq!(
                b.phase(t + next - eps, caret.clone(), true).0,
                on,
                "at t={t}, a hair before the deadline ({next}) nothing has changed yet"
            );
            assert_ne!(
                b.phase(t + next + eps, caret.clone(), true).0,
                on,
                "and a hair after it, the caret has flipped"
            );
        }
    }

    /// A deletion that leaves the offset alone (`Delete` at a caret) still counts as
    /// activity, which is what the content length in the state is for.
    #[test]
    fn forward_delete_relights_the_caret_without_moving_it() {
        let mut b = CaretBlink::default();
        assert!(b.phase(0.0, (3..3, 10), true).0);
        assert!(!b.phase(0.7, (3..3, 10), true).0, "dark");
        assert!(
            b.phase(0.75, (3..3, 9), true).0,
            "same offset, one byte shorter — the caret still has to light up"
        );
    }

    /// A drag extending a selection changes its range every frame, so the caret stays
    /// solid for the whole gesture without the drag having to say so.
    #[test]
    fn a_growing_selection_keeps_the_caret_solid() {
        let mut b = CaretBlink::default();
        let mut t = 0.0;
        for end in 0..40 {
            t += 0.1;
            assert!(
                b.phase(t, (0..end, 40), true).0,
                "frame {end} at t={t} should be solid — the selection is still growing"
            );
        }
    }

    /// A structural edit invalidates point references: `retain_valid` truncates
    /// rather than remaps.
    #[test]
    fn a_point_reference_does_not_survive_the_subpath_shrinking_under_it() {
        let id = IdSource::new(7).mint();
        let mut set = PointSet::default();
        set.set_one(id, (0, 3));
        set.toggle(id, (1, 0));
        assert_eq!(set.on(id).count(), 2);

        // Subpath 0 lost an anchor and subpath 1 is gone entirely.
        set.retain_valid(&[3]);
        assert!(
            set.is_empty(),
            "both references outlived what they pointed at"
        );
        assert_eq!(set.on(id).count(), 0);
    }

    /// A **minted** run has no endpoint to ring, and asking for one must not panic
    /// on a `len() - 1` underflow (§15 D125).
    #[test]
    fn a_freshly_minted_run_has_no_endpoint_and_does_not_panic_for_one() {
        use ondin_core::kurbo::Affine;
        let id = IdSource::new(3).mint();
        let real = PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 0.0)),
            ],
            closed: false,
        };
        let mut edit = PenEdit {
            id,
            world: Affine::IDENTITY,
            subpaths: vec![
                real,
                PenSubpath {
                    anchors: Vec::new(),
                    closed: false,
                },
            ],
            active: 1,
            from_start: false,
            minted: true,
        };
        assert!(edit.endpoint().is_none(), "an empty run has no end");
        assert!(edit.working().is_empty());
        // From the start, the other index that reads off the run.
        edit.from_start = true;
        assert!(edit.endpoint().is_none());
        // And the real run beside it still answers.
        edit.active = 0;
        edit.from_start = false;
        assert_eq!(edit.endpoint(), Some(Point::new(10.0, 0.0)));
    }

    /// Reversing remaps the selection (anchor `n − 1 − a`, segment `n − 2 − a`)
    /// rather than leaving in-range indices that light the **mirrored** points.
    #[test]
    fn reversing_a_subpath_carries_the_selection_to_the_same_points() {
        let id = IdSource::new(7).mint();
        // Five anchors, open: anchors 0..5, segments 0..4.
        let mut set = PointSet::default();
        set.set_one(id, (0, 1));
        set.toggle(id, (0, 4));
        // And a reference into a run this reversal does not touch.
        set.toggle(id, (1, 2));
        set.reverse_within(id, 0, 5, false);
        assert_eq!(
            set.on(id).collect::<Vec<_>>(),
            vec![(0, 0), (0, 3), (1, 2)],
            "anchor 1 of 5 is anchor 3 from the other end, and 4 is 0"
        );

        // Segments, open: the run leaving anchor 0 is the run leaving anchor 3 once
        // the traversal turns round, and the last one (3) becomes the first (0).
        let mut seg = PointSet::default();
        seg.set_segment(id, (0, 0));
        seg.toggle_segment(id, (0, 3));
        seg.reverse_within(id, 0, 5, false);
        assert_eq!(
            seg.segments(id).collect::<Vec<_>>(),
            vec![(0, 0), (0, 3)],
            "the two end segments must swap, not stay put"
        );
        let mut mid = PointSet::default();
        mid.set_segment(id, (0, 1));
        mid.reverse_within(id, 0, 5, false);
        assert_eq!(mid.segments(id).collect::<Vec<_>>(), vec![(0, 2)]);

        // Closed: there is one more segment — the closing one, named by the last
        // anchor — and it has to stay the closing one, which is what the wrap buys.
        let mut closed = PointSet::default();
        closed.set_segment(id, (0, 3));
        closed.toggle_segment(id, (0, 0));
        closed.reverse_within(id, 0, 4, true);
        assert_eq!(
            closed.segments(id).collect::<Vec<_>>(),
            vec![(0, 2), (0, 3)],
            "closing segment 3 must stay 3; segment 0 becomes 2"
        );

        // A set belonging to another path is never touched.
        let other = IdSource::new(9).mint();
        let mut foreign = PointSet::default();
        foreign.set_one(other, (0, 1));
        foreign.reverse_within(id, 0, 5, false);
        assert_eq!(foreign.on(other).collect::<Vec<_>>(), vec![(0, 1)]);
    }

    /// The set is read through its node, so one left behind by another path is
    /// invisible.
    #[test]
    fn points_belong_to_one_path_and_are_invisible_to_any_other() {
        let mut ids = IdSource::new(7);
        let (a, b) = (ids.mint(), ids.mint());
        let mut set = PointSet::default();
        set.set_one(a, (0, 0));
        assert!(set.contains(a, (0, 0)));
        assert!(!set.contains(b, (0, 0)));
        assert_eq!(set.on(b).count(), 0);

        // Toggling on a different path starts that path's set rather than mixing
        // two nodes' points into one selection nothing could commit.
        set.toggle(b, (0, 1));
        assert_eq!(set.on(a).count(), 0);
        assert_eq!(set.on(b).collect::<Vec<_>>(), vec![(0, 1)]);
    }

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

#[cfg(test)]
mod guide_drag_tests {
    use super::*;
    use ondin_core::IdSource;

    fn drag(discarding: bool, left_ruler: bool, copying: bool) -> GuideDrag {
        let mut ids = IdSource::new(1);
        GuideDrag {
            id: Some(GuideId(ids.mint())),
            axis: GuideAxis::Horizontal,
            owner: None,
            position: 100.0,
            grab: Vec2::ZERO,
            discarding,
            left_ruler,
            copying,
            // These tests are about `announces_discard`, which does not read it.
            rulers_shown: true,
        }
    }

    /// A guide pulled out of a ruler does not announce a discard until the pointer
    /// has left the bar once, while `discarding` still drives the commit.
    #[test]
    fn a_new_guide_does_not_announce_a_discard_until_it_has_left_the_ruler() {
        // The press: over its own ruler, never having left. Nothing announced.
        assert!(
            !drag(true, false, false).announces_discard(),
            "the press frame of a fresh ruler drag must be silent"
        );
        // Out on the canvas: nothing to announce either, and now armed.
        assert!(!drag(false, true, false).announces_discard());
        // Dragged back onto the bar having been out: *now* it says so.
        assert!(drag(true, true, false).announces_discard());
        // And the raw flag is untouched, so the commit still cancels a release on
        // the bar whether or not the guide ever left it.
        assert!(drag(true, false, false).discarding);
    }

    /// An Alt-copy over the ruler still announces: the line in flight is the copy,
    /// and a release there throws it away.
    #[test]
    fn an_alt_copy_over_the_ruler_still_announces_the_discard() {
        assert!(drag(true, true, true).announces_discard());
        assert!(!drag(false, true, true).announces_discard());
    }
}
