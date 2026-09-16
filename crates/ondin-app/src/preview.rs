//! Uncommitted gesture state (§9.3, invariant 5).
//!
//! A gesture — drag, resize, rotate, marquee, pen, text edit — lives here while
//! it is in progress and never touches the `Document`. Exactly one
//! `Transaction` is committed when it ends.
//!
//! How the in-progress result is *shown*: `EditorSession::set_preview` projects
//! the pending transaction into `RenderOverrides` (§6.2), per-node patches the
//! scene walk reads instead of the committed values. Building them from the very
//! transaction that will be committed is what guarantees the preview and the
//! result agree — there is only one description of the edit.
//!
//! Overlays that are not document content at all (marquee, pen rubber-band,
//! caret, snap guides, handles) are drawn directly with egui and never go
//! through the renderer or the model.
//!
//! One thing here is *not* a gesture: [`PointSet`], the points selected inside a
//! path. It outlives the drags that read it, the way a layer selection does. It
//! lives here because everything else the node tool touches does.

use crate::tools::Tool;
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::{CharSpans, GuideAxis, GuideId, NodeId, ParaSpans, TextEdit};
use std::collections::BTreeSet;
use std::ops::Range;

/// What a handle drag transforms.
///
/// One layer is transformed in **its own** frame, so a rotated rectangle resizes
/// along the edges the designer can see rather than along the screen's axes
/// (§15 D15). Several layers have no shared frame — a selection is not a group
/// and the layers in it may each be turned differently — so they are transformed
/// together in one **upright** box over all of them, which is the only frame the
/// set actually agrees on.
///
/// Carried by the gesture rather than looked up when it is applied, so a drag
/// cannot change what it is transforming halfway through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Layer(NodeId),
    Selection,
}

/// The pointer gesture currently in progress.
/// Which end of a `Line` a gesture has hold of.
///
/// Named rather than a `bool`, because the two are handled differently enough that
/// `true` would have to be remembered as meaning one of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineEnd {
    /// The node's own origin.
    Start,
    /// `NodeKind::Line::end`, in the node's local space.
    End,
}

pub enum Drag {
    None,
    /// Panning the camera (drag on empty canvas, or with the hand tool).
    Pan,
    /// Moving the selection; `anchor` is where the pointer went down.
    Move {
        anchor: Point,
    },
    /// Dragging a resize handle. `corner` identifies which one.
    Resize {
        target: Target,
        handle: Handle,
    },
    /// Leaning a side of the box sideways — Ctrl on the same band a resize
    /// grabs. Sides only: a corner has no side facing it to hold still, so it
    /// keeps resize and the rotate ring beyond it.
    ///
    /// **Carries the press point, and the day it did not was a reported bug.**
    /// This said "there is nothing to remember from where the press landed", on
    /// the analogy with a resize handle — but a resize handle *is* the thing you
    /// grabbed, so tracking the pointer absolutely puts it back under your
    /// finger, while a skew's lean was measured from the pointer's offset to the
    /// box **centre**. Grab a side anywhere but its midpoint and the first frame
    /// jumped to whatever lean that offset already implied: on a 200×120 rect
    /// grabbed 80% along the top edge, −26.76° before the pointer had moved
    /// (§15 D322).
    ///
    /// So `press` is the world point of the press, and the lean is measured from
    /// it. The grabbed point follows the pointer, which is the resize analogy
    /// actually honoured rather than merely invoked.
    Skew {
        target: Target,
        handle: Handle,
        press: Point,
        /// The world-space lean the gesture has asked for so far — **what the
        /// chrome draws with**, and `IDENTITY` until the pointer moves.
        ///
        /// Only a `Target::Selection` reads it (§15 D324). A set's frame is an
        /// upright union with an identity transform, so it cannot lean; without
        /// this it re-unioned the *previewed* bounds every frame and inflated as
        /// the members sheared, while the arithmetic measured the committed box.
        /// A single node's frame already leans, its own world transform carrying
        /// the shear.
        ///
        /// Written from the same computation that builds the transaction rather
        /// than re-derived at draw time, so the frame and the shapes cannot
        /// disagree about the angle.
        lean: Affine,
    },
    Rotate {
        target: Target,
        pivot: Point,
        start: f64,
        /// The world-space turn asked for so far — **what the chrome draws with**,
        /// and `IDENTITY` until the pointer moves. [`Drag::Skew`]'s `lean` for the
        /// other gesture that reorients a set (§15 D325).
        turn: Affine,
    },
    /// Dragging one end of a [`ondin_core::NodeKind::Line`].
    ///
    /// **A line has no box worth dragging**, so it does not get one: a bounding box
    /// around a diagonal line has four corners, none of which is on the line, and
    /// rotating or skewing it says nothing a second endpoint could not say better.
    /// The two ends replace all of it.
    ///
    /// Which end matters, and they are not symmetric in the model: the start *is* the
    /// node's origin (§5.3 stores only the far point), so moving it is a transform
    /// edit plus a re-expressed `end`, while moving the end is one geometry patch.
    LineEnd {
        id: NodeId,
        which: LineEnd,
    },
    /// Sliding type along the rail it is set on (§15 D410).
    ///
    /// **Carries nothing but the node, and that is the whole design.** A drag
    /// previews through `set_preview`, which writes `RenderOverrides` and leaves
    /// `Resolved` alone — so the *committed* offset stands still for the whole
    /// gesture and can simply be read each frame. The alternative, carrying the
    /// offset the press began at, was written first and is one invariant worse: it
    /// has to agree with the node's, and a stale copy drifts silently.
    ///
    /// `id` is named for the reason every gesture here names its target — a drag
    /// may not change what it is editing halfway through.
    RailOffset {
        id: NodeId,
    },
    /// Moving a layer's transform origin ([`ondin_core::Pivot`]).
    ///
    /// Carries no preview of its own, and needs none: the pivot is not scene
    /// content — nothing about it renders (§15) — so the canvas draws the marker
    /// straight from the live pointer, the way it does a marquee, and commits one
    /// `SetPivot` on release. The layer is named here rather than looked up on
    /// release for the reason every other gesture names its target: a drag may not
    /// change what it is editing halfway through.
    Pivot {
        id: NodeId,
    },
    /// Rubber-band selection from `origin` to the current pointer.
    Marquee {
        origin: Point,
        additive: bool,
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
        /// `↑`/`↓` change it mid-drag (`docs/shortcuts.md` §9, Illustrator exact).
        ///
        /// Carried on the gesture rather than read from the node, because there
        /// is no node yet: a create drag is a *preview* until the release, and
        /// `create_tx` rebuilds the whole thing from these fields on every
        /// frame. Meaningless for the shapes that have no side count, which is
        /// why it starts at the tool's own default rather than at zero — a
        /// rect's copy of it is simply never read.
        sides: u32,
        /// Where the pointer was on the previous frame of this drag.
        ///
        /// **Only `Space` needs it.** Every other thing a create drag does is a
        /// function of `start` and the pointer *now*; holding Space repositions
        /// the uncommitted shape, which is the one part that integrates motion
        /// rather than reading a position. Starts at `start`, so the first frame
        /// contributes nothing.
        last: Point,
    },
    /// Moving the selected anchors of a path, with the node tool.
    ///
    /// `grab` is where the pointer went down, so the anchors move by a **delta**
    /// rather than jumping to the pointer: the press can land up to
    /// `PEN_PICK_PX` off the anchor, and absolute tracking would snatch it that
    /// far on the first frame. `Drag::Move` carries its anchor point for the same
    /// reason.
    ///
    /// Which points move is [`PointSet`], read live — not carried here. That is
    /// the one departure from "a gesture names its target", and it is safe for
    /// the same reason it is necessary: the press *sets* the point selection
    /// (clicking an unselected anchor selects it), so the set is decided by the
    /// press and cannot change again until the button comes up.
    Points {
        id: NodeId,
        grab: Point,
        /// The anchor the press actually landed on — **what the snap measures**.
        ///
        /// A set of points has no single position to snap, and snapping the box
        /// over them would pull the whole set by whichever of its edges happened
        /// to catch. The point under the hand is the one being aimed, so it is the
        /// one offered to the lattice and the guides, and the adjustment it earns
        /// moves the set — the same shape as `snapped_move`, where the box snaps
        /// and every member follows.
        anchor: PointRef,
    },
    /// Dragging one handle of one anchor, with the node tool.
    ///
    /// Absolute, unlike [`Drag::Points`]: a handle *is* the pointer position, the
    /// way `PenAnchor::pull` treats the pen's own drag. Alt breaks the pair, read
    /// live every frame — the same rule the pen follows (§15 D114).
    Handle {
        id: NodeId,
        at: PointRef,
        side: Side,
    },
    /// Dragging a **segment** of a path, with the node tool.
    ///
    /// One gesture with two meanings, chosen by `Ctrl` and re-read every frame
    /// rather than latched at the press: plain **moves** the segment, `Ctrl`
    /// **bends** it. That is the same split `Ctrl` already has on an anchor —
    /// plain moves the point, `Ctrl` pulls its handles out — and the rule the
    /// tool now states once: *plain moves, `Ctrl` shapes the curvature* (§15
    /// D118). Reading the modifier live is `sync_alt_clone`'s rule (§15 D40): the
    /// hand can change its mind mid-drag, because both readings measure the same
    /// total from the same press.
    ///
    /// `at` names the segment by the anchor it **leaves**, which is
    /// `geometry::anchor_runs`' numbering and therefore the same index a corner
    /// radius is stored at. `t` is where along it the press landed, fixed at the
    /// press so the curve follows the part of it the hand actually took.
    Segment {
        id: NodeId,
        at: PointRef,
        t: f64,
        /// Where the pointer went down, so the drag is a **delta** — the same
        /// reason [`Drag::Points`] carries one.
        grab: Point,
    },
    /// Turning the selected points about the centre of the box over them.
    ///
    /// **The box's centre, and there is no other candidate.** A layer has a pivot
    /// it can move (§5.3); a set of points has nothing to hang one on, so this is a
    /// multi-selection's rule — turn about the middle of the box the handles were
    /// built from, so the set sweeps round as one thing.
    ///
    /// `start` is the angle from that centre to where the press landed, so the
    /// gesture measures a *change* and the set does not snap round to meet the
    /// pointer on the first frame. `Drag::Rotate` carries one for the same reason.
    PointRotate {
        id: NodeId,
        pivot: Point,
        start: f64,
    },
    /// Dragging a corner of the box over the selected points, with the node tool.
    ///
    /// **The box is carried, not re-derived.** It is the extent of the very points
    /// being scaled, so a box read fresh each frame would be the box this gesture
    /// is already changing — and the scale would compound. The same rule
    /// `resize_tx` follows by reading the committed tree.
    ///
    /// ⚠️ **These six lines were sitting on [`Drag::PointRotate`] until 2026-09-15**
    /// (§15 D760): `PointRotate` had been inserted between this variant and its own
    /// doc, so the run above it opened *"Dragging a corner of the box"* and this
    /// variant had no comment at all. `CLAUDE.md`'s enum-variant sweep — *does a
    /// variant's first doc line name a sibling?* — is the check that finds it;
    /// neither the length ranking nor the neighbour grep can, the merged run being
    /// sixteen lines against a 52-line floor and nothing having lost a `fn`.
    PointBox {
        id: NodeId,
        handle: Handle,
        /// The world-space box the points occupied when the press landed.
        box_: Rect,
        /// Where the pointer went down. Not an origin to measure from — a scale
        /// tracks the pointer absolutely, as a resize does — but the answer to
        /// "has this gesture moved at all", which is what keeps a plain click on a
        /// corner from snapping and scaling the set.
        grab: Point,
    },
    /// Sliding the picture around inside a fixed frame (`Tool::ImageEdit`).
    ///
    /// `start` is the crop rectangle as it was when the press landed, and every
    /// frame re-derives from it rather than from the last one. That is the rule
    /// `resize_tx` and `point_box_tx` already follow for the same reason: reading
    /// the *preview* back and adding to it compounds the delta every frame, so
    /// the picture would accelerate away from the pointer.
    CropPan {
        id: NodeId,
        /// Which fill of the node is the image being cropped.
        fill: usize,
        start: Rect,
        /// Where the press landed, in the node's **local** space — the space the
        /// frame and the drag arithmetic are both in.
        grab: Point,
        /// The same press in **world** space, and it is not a duplicate of
        /// `grab`: it is the answer to *"has this gesture moved at all"*, which
        /// `Drag::PointBox` carries for the same reason and states in the same
        /// words. World rather than local because that is the space
        /// `CanvasRenderer::gesture_travelled` measures in — the threshold is a
        /// screen distance, and a layer scaled 10× would otherwise need a tenth
        /// of the hand movement to count as a drag (§15 D528).
        grab_world: Point,
    },
    /// Dragging a handle of the frame to **crop** — the layer's box shrinks or
    /// grows and the picture stays exactly where it is (`Tool::ImageEdit`).
    ///
    /// **This is a resize gesture and a paint edit at once**, which is why it
    /// carries both starting values: `frame` is the layer's local box as the
    /// press found it and `start` the crop rectangle, and `ondin_core::
    /// crop_reframed` maps between them so the picture does not move. Reading
    /// either back off the preview would compound, exactly as `CropPan` says.
    ///
    /// `limit` is `ImageRef::picture_box` at the press — the rectangle the whole
    /// photograph occupies, which is the outline drawn on screen and the wall the
    /// handle stops at. Captured rather than re-derived per frame because it
    /// *would* move: it is computed from the crop this gesture is changing, so a
    /// fresh one each frame would be a limit chasing the thing it bounds.
    CropResize {
        id: NodeId,
        fill: usize,
        handle: Handle,
        frame: Rect,
        start: Rect,
        limit: Rect,
        /// Where the press landed in **world** space — `CropPan::grab_world`'s
        /// twin and there for the same reason: a click on a handle that never
        /// travels must commit nothing (§15 D528).
        grab_world: Point,
    },
    /// Dragging inside a text node to extend its selection.
    TextSelect,
    /// Pulling a ruler guide out of a ruler, or sliding an existing one.
    Guide(GuideDrag),
}

impl Drag {
    /// The transform a **multi-selection's** frame should be drawn with while this
    /// gesture is in flight, if the gesture reorients the set (§15 D324, D325).
    ///
    /// A set's box is the upright union of its members' world bounds, so it cannot
    /// lean or turn: re-unioning it each frame grew it instead, and widened it at
    /// *both* ends, which moved even the edge a skew is supposed to hold. The answer
    /// is the single-layer shape — a fixed box plus a transform — with the box taken
    /// from the **committed** tree and the transform from here.
    ///
    /// **One method rather than a match per gesture at the draw site**, so a third
    /// gesture that reorients a set has one obvious place to declare itself instead
    /// of quietly inheriting the upright box. `None` covers every other drag,
    /// including a resize and a move, whose frames *should* follow the re-unioned
    /// preview — the box genuinely changes size, and there is no rotation to carry.
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
    /// **The question is whether the gesture is reaching for somewhere**, and the
    /// ones that answer no are `Pan` on its own terms, **every gesture that turns
    /// about a fixed pivot**, and a guide for the separate reason below:
    ///
    /// - `Pan` already moves the view. Auto-scrolling it would add the band's speed
    ///   to the hand's, so the view would accelerate away as soon as the drag
    ///   reached an edge and never come back.
    /// - `Rotate` and [`Drag::PointRotate`] turn around a fixed pivot, so they can
    ///   always be completed where they started. A view that drifted while the hand
    ///   circled near an edge would be the tool moving with nothing having asked it
    ///   to. The reachable set is a circle about a centre already inside the box,
    ///   bounded by its circumradius, and it does not grow with pointer travel —
    ///   which is the *"reaching for somewhere"* test failing three ways at once.
    ///
    /// 🚨 **Stated as a rule rather than as a list of names, on purpose** (§15
    /// D760). `PointRotate` auto-panned for months — measured at 42.23 px of drift
    /// — **on the exact argument given here for exempting `Rotate`**, and its own
    /// doc cites `Drag::Rotate` while establishing two of the three clauses. The
    /// count in this paragraph said *"three"* and has already gone stale once for
    /// this reason (the ⚠️ below), so there is no number in it now: a fourth
    /// turning gesture is covered by the sentence without anybody editing it.
    ///
    /// Everything else can genuinely want more room than the window has: moving a
    /// layer somewhere off screen, resizing or skewing past the edge, marqueeing
    /// more than fits, drawing a shape bigger than the viewport, dragging a pivot
    /// across the document, or extending a text selection.
    ///
    /// ⚠️ **That list read "a pivot *or a guide*" until 2026-08-22, three lines
    /// above the paragraph saying a guide does not** — the phrase outlived the
    /// exclusion below and left one doc comment arguing both ways. A **second,
    /// duplicate** summary sentence had drifted into the middle of it as well,
    /// where it read as part of the list rather than as a heading; the real summary
    /// is at the top and was never wrong. The count above said "two" and the code
    /// has excluded three since the guide arm landed. All three found while writing
    /// `reaching_the_canvas_edge_scrolls_the_view_toward_the_pointer` (§15 D303) —
    /// the sort of thing writing a test finds and reading does not.
    ///
    /// **A guide drag does not**, and the reason is that its band and its home are
    /// the same place. The bars are `rulers::THICKNESS` (20pt) and the band is
    /// `canvas::AUTOPAN_BAND` (32), so a gesture that starts on a bar — which every
    /// new guide does — is inside the band before it begins, and the only way out of
    /// the bar is *through* the remaining 12pt of it. Suppressing the bar alone was
    /// tried and reported back as "much less than before, but still there": it made
    /// the strip the second cause instead of the first.
    ///
    /// What is given up is dragging a guide beyond the visible area in one gesture,
    /// and the Guide panel's position field is the answer to that — a number is a
    /// better way to put a guide somewhere you cannot see than a view that scrolls
    /// while you aim. Placing a guide is aiming at artwork, so scrolling the artwork
    /// underneath was working against the gesture even where it fired legitimately.
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
/// guide is not scene content: the renderer never sees one, so there is no
/// override for a patch to ride on. The canvas draws the guide from this while
/// the drag is up and from the document once it commits — the same
/// preview-then-commit shape as every other gesture (invariant 5), with egui
/// standing in for the render layer.
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
    /// **Tracked live rather than resolved on release**, for the reason
    /// [`Self::discarding`] is: the gesture has to say what letting go will do.
    /// Dragging over a frame clips the line to that frame as you cross its
    /// boundary, and dragging back out lets it run the whole canvas again.
    pub owner: Option<NodeId>,
    /// The **world** offset from the pointer to the guide's line, fixed at the
    /// press.
    ///
    /// **A guide is grabbed, not teleported.** Without this the press put the line
    /// at the pointer's coordinate, and the pointer is within
    /// `rulers::GUIDE_PICK_PT` of the line rather than on it — so
    /// *clicking a guide to select it* moved it a few points and committed that as
    /// an edit. The offset also has to survive the whole drag, not just the first
    /// frame: a dead zone released at the far end would jump by exactly this much
    /// the moment it was crossed.
    ///
    /// **A world vector rather than a scalar on the guide's own axis**, because the
    /// guide's axis is now the *owner's* and the owner changes mid-drag. A scalar
    /// would be a number in whichever space the press happened in, and crossing a
    /// frame boundary would reinterpret it in another one — the line jumping by the
    /// difference between the two origins at the moment the user crossed. In world
    /// space the point being held is the same point in every space, so the position
    /// is `local_of(pointer + grab)` and a scope change moves nothing at all.
    ///
    /// Zero for a guide being pulled out of a ruler, which has no previous position
    /// to keep and should appear under the pointer.
    pub grab: Vec2,
    /// Whether the pointer is over the ruler this guide belongs to.
    ///
    /// Dropping it there is how a guide is thrown away — the gesture that made
    /// it, run backwards. A new guide released over the ruler is simply never
    /// created; an existing one is removed. This drives the **commit**; what the
    /// gesture *announces* is [`Self::announces_discard`], which is not the same
    /// question.
    pub discarding: bool,
    /// Whether the pointer has been off **both** rulers at any point during this
    /// drag. The corner square counts as *on* them — `rulers::ruler_at` gives it to
    /// the left bar, because that is the one painted over it.
    ///
    /// **The gesture cannot announce a discard before there is anything to
    /// discard.** A new guide is dragged *out of* a ruler, so `discarding` is true
    /// from the press — the pointer is still on the bar it came from — and the bin
    /// cursor appeared the instant the button went down, on a guide that did not
    /// exist yet and was not being thrown away. There is a real difference between
    /// *not yet placed* and *about to be deleted*, and only the second is worth a
    /// cursor.
    ///
    /// So the announcement waits for the pointer to leave the bar once. An
    /// existing guide is grabbed on the canvas — `guide_at` never answers over a
    /// bar — so it is off the ruler at the press and announces immediately, which
    /// is right: it exists, and letting go there deletes it.
    pub left_ruler: bool,
    /// Whether Alt is held, making this drag place a **copy** and leave the
    /// original where it is.
    ///
    /// A guide needs none of the machinery `canvas::AltClone` does — no subtree to
    /// capture, no ids to mint up front — because a `Guide` *is* its own template
    /// and nothing is created until the release. So this is a live modifier read
    /// each frame rather than a prepared payload: the same rule
    /// `OndinApp::sync_alt_clone` follows for a layer, with nothing left to
    /// prepare.
    ///
    /// Named `copying` rather than `clone` so a field access can never be misread
    /// as a call to `Clone::clone`.
    pub copying: bool,
    /// Whether the rulers were on when the press happened.
    ///
    /// ⚠️ **Latched, because `rulers_on()` is two live chords and *both* of
    /// `update_guide_drag`'s ruler terms folded it** (§15 D555,
    /// `[S19.1-L1-02]`). This said *"the only term that decides `discarding`"* —
    /// true of `discarding` and half the story, which is exactly how the first
    /// version of the fix came to latch one term and leave the other. **Two
    /// consumers**: `over_own_ruler`, which decides the **commit**, and
    /// `off_rulers`, which feeds [`Self::left_ruler`] and therefore the
    /// **announcement**. Both read this field now.
    /// `rulers::rulers_on` is `show_rulers && !present`, `Shift+R` toggles the
    /// first and `Ctrl+\` the second, and `OndinApp` dispatches a resolved action
    /// with no drag guard — so pressing either mid-drag turned the guide **cancel**
    /// gesture into a guide **create** gesture. Measured: dragging back over the
    /// bar and releasing committed a guide at whatever coordinate the vanished bar
    /// sat over, selected it and dirtied the document, while the remaining frames
    /// of the drag drew **0 shapes** and the cursor still promised a live guide.
    ///
    /// **The same reason [`Self::left_ruler`] is latched, and its comment already
    /// said so**: *"one rule, evaluated the same way at the press and on every
    /// frame after it."* This is that rule's other half — the bars' *existence*,
    /// where `left_ruler` latches the pointer's position against them.
    pub rulers_shown: bool,
}

impl GuideDrag {
    /// Whether the gesture should *say* that letting go throws the guide away —
    /// the faded line and the bin cursor.
    ///
    /// One conjunction in one place, so the fade and the cursor cannot disagree
    /// about it, which was the whole point of computing `discarding` once.
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
/// but invisible. The design has no mid-edge squares and neither do we — eight
/// squares around a small shape is a wall of chrome (§15 D14) — but the sides
/// still have to resize one axis, because that is what dragging an edge means
/// in every other tool. The band along a side is the affordance; the cursor
/// naming the direction is the announcement.
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
    /// This is the whole difference between a corner and an edge, stated once:
    /// dragging the right side moves `x = w` and holds `x = 0`, and says
    /// nothing at all about `y`. `(w, h)` is the box's current size.
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
/// Named rather than a `bool` for the reason [`LineEnd`] is: the two are not
/// symmetric to read, and `true` would have to be remembered as meaning one of
/// them.
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
/// not — which is a state the pen authors on purpose, so the mirror is a property
/// to read ([`Self::is_smooth`]) rather than an invariant to rely on. This said
/// "the incoming one is its mirror" until 2026-08-20 and had been wrong since
/// `back` was added; the field's own comment beside it was right the whole time.
///
/// A corner has both handles zero, so a path of corners is exactly the polyline
/// the pen used to draw — the straight case falls out of the curved one rather
/// than being a mode.
#[derive(Clone, Copy, Debug)]
pub struct PenAnchor {
    pub at: Point,
    /// The handle leaving this anchor, as an offset from it.
    pub out: Vec2,
    /// The handle *arriving* at it, also as an offset — so a mirrored anchor has
    /// `back == -out` and a broken one does not.
    ///
    /// **Two vectors rather than one mirrored one**, which is what makes a
    /// corner-into-curve sayable: an anchor where the segment comes in straight
    /// and leaves on a curve has `back` zero and `out` long, and one mirrored
    /// vector cannot express it at all. Alt while shaping is what breaks them
    /// apart (`canvas::pen_input`).
    pub back: Vec2,
    /// How far this anchor's corner is rounded — `NodeKind::Path::corner_radii`,
    /// carried **on** the anchor rather than in a list beside it.
    ///
    /// This is what keeps the radii from drifting off their corners. The model
    /// stores them as a parallel `Vec` indexed by anchor, which every edit that
    /// inserts or removes an anchor would otherwise have to renumber by hand —
    /// and the failure is silent: a radius quietly moves to the neighbouring
    /// corner. Riding on the anchor it moves, deletes, reverses and reorders with
    /// the point for free, and `move_points`, `remove_points`, `align_points` and
    /// `PenSubpath::reverse` needed no change at all to stay correct. The
    /// flattening back to a list happens once, at the model boundary
    /// (`tools::pen_radii`).
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
    /// without a window, which is the same reason the pen's geometry lives in
    /// `tools`. A canvas that computed the mirror itself would be a second
    /// statement of it, and the one that drifted would be the one nothing runs.
    ///
    /// **Either handle, because the node tool can grab the incoming one** — which
    /// the pen never can, since it only ever draws forwards.
    ///
    /// The mirror runs the same way in both directions: the handle not being
    /// dragged becomes the negation of the one that is, unless the pair is
    /// broken. Stated once here rather than as two near-identical arms at the
    /// call site, because "which one is the mirror of which" is exactly the
    /// question a second copy gets backwards.
    pub fn pull_side(&mut self, side: Side, to: Point, break_pair: bool) {
        let v = to - self.at;
        // **The radius survives the rebuild.** The mirrored arms below replace the
        // whole anchor from a constructor, which is precisely how a per-anchor
        // value gets dropped by an edit that has nothing to do with it — and
        // dragging a handle on a rounded corner is a thing people do. Saved and
        // put back rather than reached around, so a third field added to the
        // anchor meets this line and not a silent loss.
        let radius = self.radius;
        match (side, break_pair) {
            // The mirrored case *is* [`Self::smooth`], from either side —
            // dragging the incoming handle to `v` is the same anchor as dragging
            // the outgoing one to `-v`.
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
    /// can drag to, so without this the conversion has no gesture at all. It needs
    /// no branch beyond the substitution: retracting *is* aiming the handle at the
    /// anchor, so the mirror zeroes the other side for free and `Alt` still buys a
    /// one-sided retract.
    ///
    /// **The pen needs it as much as the node tool does**, which is why it is here
    /// rather than in `tools::pull_handle` alone. A pen press plants its anchor on
    /// the *snapped* point and then shapes it from the *raw* pointer, so without
    /// the substitution every click would leave a handle as long as the snap
    /// adjustment — a corner that is not quite a corner, on every click.
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
    /// **Inferred from the handles, and deliberately lossy — see §15.** A smooth
    /// point whose handles are dragged back to nothing is a corner by this test
    /// and there is no way to tell it from one that was always a corner. The
    /// alternative is a stored flag, and a stored flag would have to live in the
    /// *model* to survive being committed: `NodeKind::Path` holds a `BezPath` and
    /// nothing else, so an app-side flag would be a fact that existed only until
    /// the path was deselected, which is worse than not having it.
    pub fn is_corner(&self) -> bool {
        flat(self.out) && flat(self.back)
    }

    /// Whether the two handles are each other's mirror — the anchor is smooth, or
    /// is a corner, which is the same statement with both of them at zero.
    ///
    /// **What a numeric handle edit asks before deciding whether to mirror.** A
    /// drag has `Alt` to say so live (§15 D114) and re-mirrors the moment the key
    /// comes up; a field has no key to hold, so it keeps whatever it finds — an
    /// anchor that was broken stays broken and one that was smooth stays smooth,
    /// which is the only reading that cannot surprise from a number.
    pub fn is_smooth(&self) -> bool {
        flat(self.out + self.back)
    }

    /// Whether the segment running from this anchor to `next` is a straight line.
    ///
    /// **The two handles that segment actually uses, not the two anchors being
    /// corners.** They differ in exactly one shape, and it is one the pen draws
    /// on purpose: an anchor that *arrives* straight and *leaves* on a curve. Its
    /// incoming handle is zero, so the segment into it is a line — but the anchor
    /// is not a corner, and asking `is_corner` on both ends emitted a cubic whose
    /// two control points sat on its own endpoints. Same curve, three times the
    /// data, and it is not what the reader gives back, so a path grew every time
    /// it was read and written (§15).
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
/// **A `Vec<PenAnchor>` alone cannot describe a path the pen did not draw.** A
/// flattened boolean or an import holds several subpaths, each open or closed on
/// its own, so the reader has to return a list of these rather than one anchor
/// run — otherwise resuming or editing such a path silently drops everything
/// after the first `MoveTo`.
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
    /// Used to resume drawing from an open path's **first** anchor: the pen only
    /// ever appends, so extending the far end means turning the run around
    /// first. Each anchor's handles swap with it, or every curve would flip.
    ///
    /// Sound for a closed subpath too — the segments are the same set traversed
    /// backwards — but it reverses the **winding**, which a fill rule can see.
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
    /// The node's **own** world transform, not its parent's. Writing the edited
    /// path back through this leaves the node's transform alone, so resuming a
    /// path cannot move it — which `create_path`'s origin-at-the-first-anchor
    /// rule would do the moment the pen extended the *start* of a path.
    pub world: Affine,
    /// Every subpath the node holds, in world space, with the one being edited
    /// still in place at `active`. Finishing swaps the pen's anchors in and
    /// writes the whole list back, so a multi-subpath path — a flattened
    /// boolean, an import — does not lose the subpaths nobody touched.
    ///
    /// **In the orientation they were read in.** The pen only appends, so
    /// resuming at the *start* of a subpath reverses the working copy in
    /// `PenState::anchors`; this list keeps the original, and `finish_pen`
    /// reverses back before splicing. That is what lets `tools::rewrite_path`
    /// recognise a gesture that changed nothing — and it means extending the
    /// start of a path *prepends* to it rather than flipping its direction,
    /// which a fill rule and a dash pattern can both see.
    pub subpaths: Vec<PenSubpath>,
    pub active: usize,
    /// Whether the pen grabbed the subpath's first anchor, so `anchors` is the
    /// reversed run and has to be turned back at commit.
    pub from_start: bool,
    /// Whether `subpaths[active]` is a **new, empty** run this gesture appended —
    /// what the pen bias's empty-canvas click starts (§15 D125).
    ///
    /// **It says which subpath `finish_pen` may throw away.** A run left with
    /// fewer than two anchors draws nothing, and `pen_path` would still write its
    /// lone `MoveTo` into the model, where it is junk in the save format and an
    /// extra entry in `corner_radii`. Pruning every short run instead would reach
    /// a *stored* one — an import can hold one — and a resume that changed nothing
    /// would then compare unequal to the stored path and commit an edit
    /// (`tools::rewrite_path`'s no-op test). So the flag scopes the pruning to the
    /// run this gesture minted, which is the only one it is entitled to remove.
    pub minted: bool,
}

impl PenEdit {
    /// The end the pen is holding — the anchor the press grabbed, and the one the
    /// path will grow from.
    ///
    /// This is what the hover ring is drawn on before the press, so the promise
    /// and the grab read the same anchor rather than two that agree by habit.
    ///
    /// `None` for a [`Self::minted`] run, which has no end yet — the anchors are
    /// in `PenState` and the entry here is the empty placeholder they will replace.
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
/// **Indices, not identity, and that is the whole cost of `BezPath` over a vector
/// network** (§15 D114): the model has nothing to give an anchor a stable name,
/// so a point is *where it is in the list*, and every structural edit invalidates
/// the ones after it. `PointSet::retain_valid` is what stops that being a bug —
/// see it for the rule.
pub type PointRef = (usize, usize);

/// Which points of which path are selected — a third kind of subject, beside
/// the layer `Selection` and the guide selection.
///
/// **Beside it, not inside it.** `Selection` is layers-or-guides and its
/// exclusivity is structural; points are neither, and folding them in would mean
/// every reader of a selection learning about a case that cannot co-occur with
/// the ones it already handles.
///
/// The node it belongs to is carried so a stale set cannot be read against a
/// different path — selecting another layer and coming back must not leave three
/// points lit on artwork that never had them.
///
/// **Anchors *or* segments, never both**, which is `Selection`'s own exclusivity
/// argument one level down: Delete has to have a single subject, and "remove these
/// points and also those segments" is a state with no operation. So the two sets
/// live in one type and every mutator clears the other, rather than a second field
/// beside this one that each of a dozen call sites would have to remember to keep
/// in step.
#[derive(Default)]
pub struct PointSet {
    id: Option<NodeId>,
    points: BTreeSet<PointRef>,
    /// The selected segments, each named by the anchor it **leaves** — the
    /// numbering `geometry::anchor_runs` owns, so they are indices like any other
    /// and [`Self::retain_valid`] guards them the same way.
    segments: BTreeSet<PointRef>,
}

impl PointSet {
    /// The points selected on `id`, or nothing if the set belongs elsewhere.
    ///
    /// Every read goes through the node id, so a set left behind by another path
    /// is invisible rather than wrong.
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
    /// The question Delete, Escape and the arrow keys ask before they claim the
    /// key from the layer they would otherwise act on, so it has to mean "this
    /// tool has no subject" rather than "no anchors".
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
    /// **Called after any structural edit, and it is a truncation rather than a
    /// remap.** Deleting an anchor renumbers every anchor after it in that
    /// subpath, so a set kept across the edit would silently point at the wrong
    /// points — near-miss selections being the kind of bug that only shows up
    /// three edits later. The honest answer is that the set is *gone*: the caller
    /// re-selects what it means to leave lit, and this only guards the reads in
    /// between.
    ///
    /// ⚠️ **Undo is a structural edit and this is the wrong guard for it** (§15
    /// D568). Rewinding the document can leave every index in range while the
    /// anchors under them have moved — delete the middle anchor of three, pick the
    /// last one, `Ctrl+Z` — so the truncation is a no-op and the set goes on naming
    /// a *different* point, which is the precise near-miss the paragraph above says
    /// this type exists to refuse. `OndinApp::document_rewound` clears instead.
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
    /// **A remap, where every other structural edit is a truncation** ([`Self::
    /// retain_valid`]) — and the difference is that this edit is a permutation.
    /// Reversing loses nothing and adds nothing, so the anchor that was third from
    /// the start is third from the end and there is a right answer; dropping the
    /// set instead would make the one verb whose whole purpose is to leave the
    /// shape alone the one verb that clears your selection. Leaving the indices
    /// *unchanged* is the failure this exists to prevent: they would still be
    /// valid, and lighting the mirrored points is exactly the silent near-miss
    /// `retain_valid` refuses to allow.
    ///
    /// Anchors mirror as `n − 1 − a`. **Segments mirror as `n − 2 − a`**, because a
    /// segment is named by the anchor it *leaves* (`geometry::anchor_runs`): the
    /// run from `a` to `a + 1` becomes the run from `n − 2 − a` to `n − 1 − a`, so
    /// the name moves by one more than the anchor's does. On a `closed` subpath the
    /// arithmetic wraps, which is what keeps the closing segment — the one named by
    /// the last anchor — the closing segment.
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
            // wrap, which `usize` needs and which is why this is not the open arm
            // with a `%` on the end.
            true if a < len => Some((2 * len - 2 - a) % len),
            true => None,
            false => (len - 2).checked_sub(a),
        });
    }
}

/// The caret's blink phase.
///
/// **Chrome, so it lives here and not in `ondin_core::TextEdit`.** The caret model is
/// about bytes and geometry; when the caret is *drawn* is a question about wall-clock
/// time and about nothing in the document, and putting a timestamp on `TextEdit`
/// would make a value the save format and the exporters have to ignore.
///
/// **The phase restarts on activity, and activity is *observed* rather than
/// announced.** A caret that happens to be in its dark half on the frame you clicked
/// reads as broken, so every mover and every edit owes a restart — and there are a
/// dozen of them in `canvas::text_mode_input`. Rather than a call at each, this
/// compares the caret's own position every frame: a call site cannot forget to do
/// something it does not do. It also buys the drag case for free, since a selection
/// being dragged changes its range on every frame of the gesture and so stays solid.
/// **A hidden caret is hidden until something happens**, so an unfocused window costs
/// no repaints at all — which is the whole reason [`CaretBlink::phase`] answers a
/// deadline rather than the canvas asking every frame.
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
/// **Window focus belongs beside the caret's position, not in a branch of its own.**
/// Coming back to a window is activity in exactly the sense a keystroke is, and owes
/// the same solid caret: put focus in a branch and regaining it shows a caret halfway
/// through whatever phase it left, which is the original complaint in miniature.
///
/// The value comes from `InputState::focused`, which is the OS's answer rather than
/// egui's own widget focus. `RawInput::default()` sets it **true** — "integrations opt
/// into global focus tracking" — so a session opened before any focus event still shows
/// its caret, and an integration that never tracked focus would simply go on blinking
/// rather than going dark. Both failure directions land on "visible", which is the
/// right way round for a caret.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlinkState {
    /// The selected range, and the content length beside it so that a *deletion* which
    /// leaves the offset alone still counts (`Delete` at a caret is exactly that).
    at: (Range<usize>, usize),
    focused: bool,
}

impl Default for CaretBlink {
    fn default() -> Self {
        // **A state no caret can be in**, so the first frame of a session restarts the
        // phase and the caret is solid the instant it opens. That is what makes
        // `Default` enough here and spares every construction site a timestamp it
        // would have to be handed.
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
    /// **500ms, and not read from the OS.** Windows defaults to 530 and macOS to about
    /// 500; neither winit nor egui exposes the setting, and a caret blinking at a rate
    /// a few tens of milliseconds off the platform's is a far smaller surprise than one
    /// that does not blink at all — which is what this fixes.
    const HALF: f64 = 0.5;

    /// Whether the caret is drawn at `now`, and how long until that answer changes —
    /// restarting the phase if anything in [`BlinkState`] has changed since the last
    /// call.
    ///
    /// `&mut self` because **asking is what notices the move**: the observation and the
    /// reset are one step, so there is no way to read the phase without keeping it
    /// honest. The deadline is `None` when nothing is due to change on its own, which
    /// is the unfocused case — the caret is hidden and stays hidden until a focus event,
    /// and egui delivers that as input and repaints for it anyway.
    fn phase(&mut self, now: f64, at: (Range<usize>, usize), focused: bool) -> (bool, Option<f64>) {
        let state = BlinkState { at, focused };
        if state != self.at {
            self.at = state;
            self.since = now;
        }
        if !focused {
            return (false, None);
        }
        // Clamped at zero because a `now` behind `since` would cast to nothing useful
        // — it should not happen, and a caret stuck dark is not the way to find out.
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
    /// ⚠️ **"Last held", not "held when the session opened"** (§15 D584). This
    /// was a snapshot taken once at `begin_edit_text` and never touched again,
    /// and the three helpers that re-open the editor against an outside commit
    /// (`canvas::text_session_restyled`, `…_after`, `restyle_session_with`) all
    /// re-based the *editor* and none of them re-based this — so from the first
    /// commit to the node from outside the session, `changed()` was comparing a
    /// re-based editor against a pre-session document and could only answer yes.
    /// One styling click cost two undo steps, the second byte-identical to the
    /// first, against §9.3's *"one editing session is one undo step"*.
    /// `canvas::restyle_session` is the one place it moves.
    ///
    /// **The spans are part of "nothing changed".** Bolding a word and typing
    /// nothing is a real edit, and comparing only the string would throw it away
    /// on Escape. The paragraph list is here for the same reason and not merely by
    /// symmetry: it is what the session commits alongside the content
    /// (`Operation::SetText`), so anything it leaves out is dropped on close.
    pub original: (String, CharSpans, ParaSpans),
    /// Whether this session **created** the node it is editing (§15 D518).
    ///
    /// ⚠️ **Carried rather than inferred, because `original` cannot answer it.**
    /// §9.3 says *"a **fresh** node left empty is removed instead"*, and
    /// `finish_text_edit` asked that question of `original.0.is_empty()` — which is
    /// true of a freshly created node **and equally true of any pre-existing node
    /// that is already empty**. Nothing distinguished the two, so a text layer
    /// whose content the user cleared and committed in one session was **deleted**
    /// by an `Enter` … `Escape` in a later one, with nothing typed and the status
    /// line saying *"Removed empty text"*.
    ///
    /// D170 is the entry that added `original`, and it says what the field is for:
    /// *"a node created empty must record an empty original, or closing it reads as
    /// a change and commits a second undo step over the create."* It answers the
    /// **change-detection** question. It was being read to answer a *freshness*
    /// question it cannot.
    pub created: bool,
    /// When the caret is drawn — see [`CaretBlink`]. `Default` is correct for a
    /// session that has just opened, whichever way it opened.
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
    /// The two reads are separate statements so the borrows are, which is the whole
    /// reason this is a method on the session rather than a call at the draw site.
    pub fn caret_phase(&mut self, now: f64, focused: bool) -> (bool, Option<f64>) {
        let at = (self.editor.selected_range(), self.editor.content().len());
        self.blink.phase(now, at, focused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::IdSource;

    /// **The caret blinks, and the phase restarts when it moves** (§15 D171).
    ///
    /// The restart is the half worth testing: a caret that is dark on the frame you
    /// clicked is exactly the affordance failure this fixes, so a move landing in the
    /// dark half must light it immediately rather than at the next tick.
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

    /// **An unfocused window hides the caret and asks for no repaints**, and coming back
    /// relights it rather than resuming mid-phase — which is why focus sits in
    /// `BlinkState` beside the caret's position instead of in a branch of its own.
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

    /// The deadline is what replaces a repaint per frame, so it has to land on the
    /// **next** flip — not merely on *a* flip.
    ///
    /// **"Adding the deadline changes the answer" is not enough**, and this test passed
    /// against a deadline of a whole half-cycle before it was written this way: adding
    /// exactly `HALF` flips the phase from anywhere in it, so that check cannot tell
    /// the distance-to-the-boundary from a fixed half. Sampling a hair either side of
    /// the deadline can, and a fixed half fails it — the caret would light up late by
    /// however far into the phase the frame fell.
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

    /// **A deletion that leaves the offset alone still counts as activity** — which is
    /// what the content length in the state is for, and the one case comparing the
    /// selection alone would miss. `Delete` at a caret is exactly it.
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

    /// **A point reference is a position in a list, so a structural edit
    /// invalidates it** — the cost `BezPath` charges for having no per-anchor
    /// identity (§15 D114). The guard has to be a truncation rather than a remap:
    /// an index that survives an edit *pointing at a different anchor* is the
    /// failure, and it is silent.
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

    /// A **minted** run has no endpoint to ring, and asking for one must not panic.
    ///
    /// The empty placeholder is real state, not a hypothetical: the pen bias's
    /// empty-canvas click appends one and keeps the anchors it is drawing in
    /// `PenState`, so anything that reaches for `subpaths[active]`'s last anchor —
    /// the hover ring does, every frame — is one `len() - 1` away from an underflow
    /// on a `usize` (§15 D125).
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

    /// **Reversing is the one structural edit that remaps instead of truncating**,
    /// and both halves of the arithmetic are easy to get wrong by one: an anchor
    /// mirrors as `n − 1 − a`, a segment as `n − 2 − a`, because a segment is named
    /// by the anchor it *leaves*.
    ///
    /// The failure this pins is not a crash — every index here stays in range either
    /// way. It is that doing nothing leaves the selection lighting the **mirrored**
    /// points: reverse a five-anchor run with the second point picked and the fourth
    /// lights up. That is the silent near-miss `retain_valid` refuses to allow.
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

    /// The set is read *through* its node, so one left behind by another path is
    /// invisible rather than wrong — three points lit on artwork that never had
    /// them is the shape of the bug this prevents.
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

    /// Dragging either handle mirrors the other, and Alt breaks the pair — the
    /// incoming side being draggable at all is what the node tool adds over the
    /// pen, which can only ever pull the outgoing one.
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

    /// **A guide being pulled out of a ruler must not announce a discard before it
    /// has ever been out there.** The bin cursor and the faded line appeared the
    /// instant the button went down on the bar — on a guide that did not exist yet
    /// and was not being thrown away — because `discarding` is a pure geometric
    /// test ("is the pointer over this guide's own ruler") and a new guide starts
    /// there by definition.
    ///
    /// The two questions are genuinely different: *not yet placed* and *about to be
    /// deleted*, and only the second is worth a cursor. So the announcement waits
    /// for the pointer to leave the bar once, and `discarding` keeps driving the
    /// commit untouched.
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

    /// An Alt-copy over the ruler still announces, and that is not an omission.
    ///
    /// The announcement is always about **the line in flight**, and a release over
    /// the ruler always throws that line away. For a copy the line in flight is the
    /// copy — the original is untouched — so the fade and the bin are describing
    /// exactly what happens.
    #[test]
    fn an_alt_copy_over_the_ruler_still_announces_the_discard() {
        assert!(drag(true, true, true).announces_discard());
        assert!(!drag(false, true, true).announces_discard());
    }
}
