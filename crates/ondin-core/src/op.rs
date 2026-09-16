//! Operations, transactions, and errors (§5.7).
//!
//! Ids are minted by the caller and carried inside ops; `apply` never allocates
//! (invariant 3). A `Transaction` applies atomically = one undo entry.

use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideId};
use crate::id::NodeId;
use crate::image::{ImageEntry, ImageId};
use crate::node::{Fill, FillRule, MaskMode, Node, NodeKind, Pivot, Stroke, TextStyle};
use kurbo::{Affine, BezPath, Point, RoundedRectRadii, Size};
use peniko::Color;
use rustc_hash::FxHashSet;

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    CreateNode {
        id: NodeId,
        parent: NodeId,
        index: usize,
        kind: NodeKind,
        transform: Option<Affine>,
        name: Option<String>,
    },
    DeleteNode {
        id: NodeId,
    },
    /// Paste / duplicate / import: nodes arrive with freshly-minted ids and are
    /// validated as a well-formed subtree.
    InsertSubtree {
        nodes: Vec<Node>,
        parent: NodeId,
        index: usize,
    },
    Reparent {
        id: NodeId,
        new_parent: NodeId,
        index: usize,
    },
    Reorder {
        id: NodeId,
        index: usize,
    },
    SetTransform {
        id: NodeId,
        transform: Affine,
    },
    SetGeometry {
        id: NodeId,
        geometry: GeometryPatch,
    },
    /// New content **and both span lists that go with it**.
    ///
    /// All three travel together because a span is a byte range into the content:
    /// commit the string on its own and every override past the edit point is
    /// pointing at the wrong characters. `TextEdit` re-bases both lists as it
    /// goes ([`crate::Spans::edited`]) and hands them back at the end.
    ///
    /// **The paragraph list is here for exactly the same reason as the character
    /// one, not by analogy**: a `ParaSpans` entry is a byte range too, and a
    /// paragraph reads the value in force at its first byte — so one character
    /// typed anywhere before it moves that byte out from under the span.
    SetText {
        id: NodeId,
        content: String,
        spans: crate::node::CharSpans,
        para_spans: crate::node::ParaSpans,
    },
    /// The node's character *defaults* — what a byte with no span carries.
    ///
    /// **`spans` is how an undo of this puts back an override it dropped.**
    /// Applying this re-states the list against the new defaults, because a span
    /// equal to the *old* default was never stored: node size 16, a span puts one
    /// run at 24, the user sets the node to 24, and that span is correctly and
    /// invisibly dropped as redundant. Undo restoring the defaults alone then took
    /// the run to 16 with everything else, because the inverse had no way to say
    /// what the list used to be — the re-statement was one-way (§15 D163).
    ///
    /// `None` means "re-state", which is what every forward caller means and the
    /// only thing any of them could compute; `Some` installs exactly this list, and
    /// the inverse always carries one.
    SetTextStyle {
        id: NodeId,
        style: TextStyle,
        spans: Option<crate::node::CharSpans>,
    },
    /// The node's character overrides, without touching the content.
    ///
    /// Separate from [`Operation::SetText`] so styling a range is not an edit to
    /// the string: an undo of "make this bold" must not be able to restore
    /// different text.
    SetTextSpans {
        id: NodeId,
        spans: crate::node::CharSpans,
    },
    /// Alignment, indent, wrapping and the rest of the paragraph scope (§5.4) —
    /// the node's paragraph *defaults*.
    ///
    /// `spans` is [`Operation::SetTextStyle`]'s, for the identical reason and not by
    /// analogy: this op re-states the per-paragraph overrides against the new
    /// defaults too, so without a list in its inverse the drop is one-way in this
    /// scope as well (§15 D163).
    SetParagraphStyle {
        id: NodeId,
        paragraph: crate::node::ParagraphStyle,
        spans: Option<crate::node::ParaSpans>,
    },
    /// The node's per-paragraph overrides, without touching the content.
    ///
    /// The paragraph twin of [`Operation::SetTextSpans`], and separate from
    /// [`Operation::SetText`] for the same reason: an undo of "indent this
    /// paragraph" must not be able to restore different text.
    SetParagraphSpans {
        id: NodeId,
        spans: crate::node::ParaSpans,
    },
    /// Vertical alignment, box trim, overflow and the line limit — the whole
    /// node, never a range.
    SetBlockStyle {
        id: NodeId,
        block: crate::node::BlockStyle,
    },
    SetName {
        id: NodeId,
        name: String,
    },
    SetVisible {
        id: NodeId,
        visible: bool,
    },
    SetLocked {
        id: NodeId,
        locked: bool,
    },
    /// The Transform panel's chain link: whether a resize of the node holds its
    /// aspect ratio, so a handle drag keeps the proportions as though Shift were
    /// held (`canvas::keep_ratio`).
    ///
    /// A different thing from [`Operation::SetLocked`] above, which locks the
    /// layer against being edited at all.
    SetProportionsLocked {
        id: NodeId,
        locked: bool,
    },
    SetOpacity {
        id: NodeId,
        opacity: f32,
    },
    /// Move the node's transform origin — the point rotation and mirroring hold
    /// still. `None` puts it back on the centre of the node's box, which is where
    /// both pivoted before the field existed (§5.3, [`Pivot`]).
    ///
    /// **Resize and skew deliberately ignore it**, each because it already has a
    /// fixed point of its own that the gesture is built on: the opposite edge for a
    /// resize, the facing side for a skew. Alt-resize about the pivot was built and
    /// then reversed — §15 D60 has the three constraints that cannot all hold.
    ///
    /// **Nothing composes this into the world transform.** The pivot is read by
    /// the gesture that produces a transform and then baked into the result, so
    /// the render boundary, `Resolved`, hit-testing and the SVG writer never see
    /// it — see §15.
    SetPivot {
        id: NodeId,
        pivot: Option<Pivot>,
    },
    /// Whether a container clips its children to its own frame (§5.3).
    SetClip {
        id: NodeId,
        clip: bool,
    },
    /// Whether this layer is a mask for the siblings above it (§5.3).
    ///
    /// **One node, not a run.** The masked layers carry nothing — being masked
    /// is a fact about where they sit, so moving one out from over the mask is a
    /// reorder and not a second edit here. That is also what makes the inverse a
    /// plain `SetMask` back.
    SetMask {
        id: NodeId,
        mask: bool,
    },
    /// How this layer masks — its outline, or its own alpha (§5.3).
    ///
    /// **Separate from `SetMask`, and writable while the layer is not a mask at
    /// all.** The mode is remembered across the flag going off and on, so the
    /// operation that sets it cannot be an argument of the one that turns masking
    /// on: that would make every release throw the mode away.
    SetMaskMode {
        id: NodeId,
        mode: MaskMode,
    },
    /// How this layer's geometry decides its interior — non-zero or even-odd
    /// (§15 D239).
    ///
    /// **Its own operation rather than part of a geometry patch**, because it is a
    /// property of no particular kind: a `Path`, a `Boolean` and a frame all have an
    /// interior and `GeometryPatch` is matched against the kind. It is also the one
    /// thing about a shape that a *boolean* sets on the user's behalf — `build::boolean`
    /// gives an `Exclude` an even-odd rule, that being what a symmetric difference is.
    SetFillRule {
        id: NodeId,
        rule: FillRule,
    },
    SetFills {
        id: NodeId,
        fills: Vec<Fill>,
    },
    SetStrokes {
        id: NodeId,
        strokes: Vec<Stroke>,
    },
    /// The whole effect stack (§5.3a), for `SetStrokes`' reason: a reorder, a
    /// removal, a visibility toggle and a tuned number are then one operation
    /// with one inverse, and the panel never has to describe *which* of those it
    /// made.
    SetEffects {
        id: NodeId,
        effects: Vec<Effect>,
    },
    /// The list of files this layer produces (§7). The whole list, not a
    /// per-entry edit, for `SetStrokes`' reason: a reorder, a removal and an
    /// edit are then one operation with one inverse, and the panel never has to
    /// describe *which* change it made.
    SetExports {
        id: NodeId,
        exports: Vec<ExportSpec>,
    },
    /// The columns and rows drawn over this layer (`crate::layout`). The whole
    /// list, for [`Operation::SetStrokes`]' reason.
    ///
    /// **Chrome, like the guide operations**: it answers `false` to
    /// [`Self::changes_ink`], because a grid is drawn over the artwork and is
    /// never part of it.
    SetLayoutGrids {
        id: NodeId,
        grids: Vec<crate::layout::LayoutGrid>,
    },
    /// The ground behind and around the frames — the one operation with no node
    /// to name, because the ground belongs to the document rather than to
    /// anything in it (§15 D18). A solid colour, not a `Brush`: there is
    /// nothing behind the canvas, so a gradient would have nowhere to go and an
    /// alpha nothing to blend with.
    SetCanvasBackground {
        background: Color,
    },
    /// Place a ruler guide. The id is minted by the caller like a node's, so a
    /// guide pulled out of a ruler has a stable identity from the first frame
    /// of the drag (§5.2, invariant 3).
    AddGuide {
        guide: Guide,
    },
    RemoveGuide {
        id: GuideId,
    },
    /// Move a guide along the axis it constrains.
    SetGuidePosition {
        id: GuideId,
        position: f64,
    },
    /// Recolour a guide, or (`None`) put it back on the shared default.
    SetGuideColor {
        id: GuideId,
        color: Option<Color>,
    },
    /// Rescope a guide: which frame owns it, and where it sits in that owner's
    /// space (`None` owner meaning the canvas, in world space).
    ///
    /// **The two fields move together or not at all**, which is the whole reason
    /// this is one operation and not [`Self::SetGuidePosition`] beside a
    /// scope-only setter. `position` is *read in* the owner's space
    /// (`Guide::position`), so a transaction that changed the owner and left the
    /// number would be reinterpreting the same coordinate in a different space —
    /// a guide 40 units down a frame would jump to 40 units down the canvas for
    /// however long the intermediate state lasted, and an inverse built from that
    /// state would restore the jump rather than the guide.
    ///
    /// Rescoping is what dragging a guide across a frame's boundary does, in
    /// either direction, and it inverts to itself carrying the old pair — so the
    /// mode change is one undo step and the return trip is exact.
    SetGuideScope {
        id: GuideId,
        owner: Option<NodeId>,
        position: f64,
    },
    /// Put an image's bytes in the document's table (`crate::image`).
    ///
    /// **The bytes travel in the operation, which is what makes undo whole.** A
    /// delete of the last layer showing an image must be undoable, and an inverse
    /// that only knew the id would restore a reference to nothing. So this
    /// inverts to [`Self::RemoveImage`] and that inverts back to *this*, entry
    /// included — the same shape as [`Self::AddGuide`], for a heavier payload and
    /// the same reason.
    ///
    /// The id is minted by the caller, as a guide's is: it is a content hash of
    /// the encoded bytes, so the caller is the only thing that can compute one,
    /// and computing it there is what makes placing the same file twice
    /// deduplicate rather than duplicate.
    AddImage {
        id: ImageId,
        entry: ImageEntry,
    },
    /// Take an image out of the table.
    ///
    /// **Not reached by deleting a layer** — nothing collects the table (see
    /// `Document::images`) — so this is *Purge unused*, *Unlink*, and the second
    /// half of a replace.
    RemoveImage {
        id: ImageId,
    },
}

impl Operation {
    /// The node this operation **overwrites a field of**, or `None` when it is
    /// not that kind of operation at all.
    ///
    /// The question [`crate::History`] asks to decide whether one edit may be
    /// folded into the step before it, and the reason it is phrased about
    /// overwriting: merging keeps the *older* inverse and throws the newer one
    /// away, which restores the original state only if the second edit replaced
    /// exactly what the first one did. That holds for an absolute setter on a
    /// node and fails for everything structural — a create, a delete, a reparent
    /// or a reorder has an inverse that undoes a *change in the tree*, and
    /// dropping one loses a node.
    ///
    /// The guide operations answer `None` for a duller reason: they name a
    /// `GuideId` rather than a `NodeId`, so they have no subject of this shape.
    ///
    /// ⚠️ **This used to add "a guide drag is one commit on release anyway, so
    /// nothing wants it yet", and something did** (§15 D482). An arrow-key nudge
    /// on a guide is a *repeating* gesture and needs to fold, which is
    /// [`Self::shape_key`]'s business — and deliberately not this function's, whose
    /// two callers ask *"which node does this transaction write"* and would take a
    /// guide's underlying id for a layer's. **Two questions that look like one**:
    /// the subject of an edit, and the key a run merges on.
    pub fn overwrites(&self) -> Option<NodeId> {
        match self {
            Operation::SetTransform { id, .. }
            | Operation::SetGeometry { id, .. }
            | Operation::SetText { id, .. }
            | Operation::SetTextStyle { id, .. }
            | Operation::SetTextSpans { id, .. }
            | Operation::SetParagraphStyle { id, .. }
            | Operation::SetParagraphSpans { id, .. }
            | Operation::SetBlockStyle { id, .. }
            | Operation::SetName { id, .. }
            | Operation::SetVisible { id, .. }
            | Operation::SetLocked { id, .. }
            | Operation::SetProportionsLocked { id, .. }
            | Operation::SetOpacity { id, .. }
            | Operation::SetPivot { id, .. }
            | Operation::SetClip { id, .. }
            | Operation::SetMask { id, .. }
            | Operation::SetMaskMode { id, .. }
            | Operation::SetFillRule { id, .. }
            | Operation::SetFills { id, .. }
            | Operation::SetStrokes { id, .. }
            | Operation::SetEffects { id, .. }
            | Operation::SetExports { id, .. }
            | Operation::SetLayoutGrids { id, .. } => Some(*id),
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::Reparent { .. }
            | Operation::Reorder { .. }
            | Operation::SetCanvasBackground { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. }
            // The image table is document-level, like the guides and the ground:
            // these name an `ImageId` and no node at all.
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. } => None,
        }
    }

    /// The key `History` merges a run on: the operation, the **field** inside it
    /// where there is one, and the subject (§15 D482).
    ///
    /// `History::commit_into_run` may keep the older inverse and drop the newer
    /// one only when the second edit overwrote *exactly* the fields the first one
    /// did. [`Self::overwrites`] answers a subject and the operation's discriminant
    /// answers the variant, and for 22 of the 23 overwriting operations that pair
    /// **is** the field set — each writes a fixed group of fields. Checked variant
    /// by variant.
    ///
    /// ⚠️ **`SetGeometry` is the one exception, and it is the reason this function
    /// exists** (`[S2.2-L2-02]`). Its payload is itself a field selector:
    /// [`GeometryPatch`] has twelve variants aimed at different fields, so two
    /// commits carrying `Size` and `CornerRadius` looked identical to a
    /// discriminant-only key and merged. Measured through core's public API on a
    /// 10×10 rect — `Size(20×20)` then `CornerRadius(5.0)`, `undo_depth` 1, and one
    /// undo restoring the size while stranding the radius at 5, **with an empty
    /// stack**: a state that was never committed and nothing left to reach the real
    /// one. Not reachable through the app today, `commit_run`'s verbs each emitting
    /// one patch variant per node, but it is core's public contract and
    /// `commit_into_run`'s doc claims it is enforced.
    ///
    /// ⚠️ **`SetGuidePosition` is admitted here and not by
    /// [`Self::overwrites`]**, which answers `None` for every guide operation. A
    /// guide's id wraps a `NodeId` (§5.2), so it makes a perfectly good merge key —
    /// but it is not a *node*, and `overwrites`' two callers ask which node an edit
    /// writes: `app`'s `touched` count behind *"Pasted properties onto n layers"*,
    /// and `inspector`'s assertion that a recolour names no frame. Widening that one
    /// would hand both of them a guide's underlying id **as though it were a
    /// layer's**.
    ///
    /// ⚠️ **Not because the ids could collide — they cannot.** This comment said so
    /// and was wrong: [`crate::guide::GuideId`]'s own doc records that guides and
    /// nodes are minted from the session's **one** `IdSource`, *"so a guide id and
    /// a node id can never collide"*. Neither caller is reachable with a guide
    /// operation today either, so the cost is a latent lie rather than a measured
    /// bug — which is the reason to keep the two questions apart rather than a
    /// reason to shrug.
    ///
    /// The other guide operations stay out: `AddGuide` and
    /// `RemoveGuide` are structural, and nothing repeats a colour or a scope, so
    /// admitting them would widen the merge for a gesture that does not exist.
    pub fn shape_key(
        &self,
    ) -> Option<(
        std::mem::Discriminant<Operation>,
        Option<std::mem::Discriminant<GeometryPatch>>,
        NodeId,
    )> {
        let subject = match self {
            Operation::SetGuidePosition { id, .. } => id.0,
            other => other.overwrites()?,
        };
        let field = match self {
            Operation::SetGeometry { geometry, .. } => Some(std::mem::discriminant(geometry)),
            _ => None,
        };
        Some((std::mem::discriminant(self), field, subject))
    }

    /// Whether applying this changes what is **drawn** — the artwork, not the
    /// chrome around it.
    ///
    /// **The question that decides whether an edit is worth getting the selection
    /// box out of the way for** (§15 D128). An edit whose result is invisible has
    /// nothing to look at, and an edit whose *only* result is chrome would have its
    /// one visible effect taken away by hiding it. Both fall out of this: the
    /// proportion lock changes what a future handle drag does and nothing on screen
    /// now, a pivot is a marker the canvas draws as chrome, a name is a row in a
    /// panel and a tag over a frame, and a guide is a line over the artwork rather
    /// than part of it.
    ///
    /// **The same eleven operations `RenderOverrides::absorb` treats as a no-op**, and
    /// deliberately the same list: "the renderer has nothing to do with this" and
    /// "this changes nothing drawn" are one fact asked by two callers. They are not
    /// one function because `absorb` also has to separate the *patchable* ops from
    /// the structural ones, which is a distinction this question does not make —
    /// but if the two lists ever disagree, one of them is wrong.
    ///
    /// **The two image operations are the standing exception, and it is a gap
    /// rather than a disagreement.** They answer `true` here and are absorbed as
    /// no-ops there, because `RenderOverrides` has no image table for `absorb` to
    /// patch — so the honest reading of that arm is "cannot preview this", not
    /// "nothing to preview". It is written out at both ends.
    ///
    /// No wildcard, so a new `Operation` stops compiling here and has to be
    /// classified rather than defaulting to either answer. That is the
    /// `matches!`-over-a-list trap (§15 D87–D88) in the shape it takes for an op.
    pub fn changes_ink(&self) -> bool {
        match self {
            Operation::SetName { .. }
            | Operation::SetLocked { .. }
            | Operation::SetProportionsLocked { .. }
            | Operation::SetPivot { .. }
            // Export settings say what a *file* would contain, and nothing on
            // the canvas draws from them — adding a 2× PNG to a layer changes
            // the panel and not one pixel of the artwork.
            | Operation::SetExports { .. }
            // A layout grid is drawn over a frame and is never in it: no scene
            // node, no export, nothing in the snapshot. The same sentence the
            // guide arms below are filed under.
            | Operation::SetLayoutGrids { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. } => false,
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::Reparent { .. }
            | Operation::Reorder { .. }
            | Operation::SetTransform { .. }
            | Operation::SetGeometry { .. }
            | Operation::SetText { .. }
            | Operation::SetTextStyle { .. }
            | Operation::SetTextSpans { .. }
            | Operation::SetParagraphStyle { .. }
            | Operation::SetParagraphSpans { .. }
            | Operation::SetBlockStyle { .. }
            | Operation::SetVisible { .. }
            | Operation::SetOpacity { .. }
            | Operation::SetClip { .. }
            // **The most visible flag in this list.** Setting it takes one layer
            // off the page and cuts every layer above it down to that layer's
            // outline — two changes to the drawing at once, from one bool.
            | Operation::SetMask { .. }
            // **True even though it can be written to a layer that is not a
            // mask**, where it changes nothing on screen. The honest reading of
            // this question is about the operation, not about the state it lands
            // in: `SetGeometry` on a shape already that size is `true` as well.
            | Operation::SetMaskMode { .. }
            // **True, and it is a *geometry* change wearing a paint-shaped op.**
            // Switching the rule changes which parts of the shape are inside, so
            // the hit test and the mask it feeds change with the picture.
            | Operation::SetFillRule { .. }
            | Operation::SetFills { .. }
            | Operation::SetStrokes { .. }
            // **True, and it is the one paint-adjacent op that can change what
            // is drawn *outside* the layer it names.** A drop shadow reaches
            // past its own geometry, so this op dirties a region wider than the
            // node's box — which is why `effect::stack_escape` exists and why
            // the bounds cache reads it rather than the node's size alone.
            | Operation::SetEffects { .. }
            | Operation::SetCanvasBackground { .. }
            // **True, though neither touches a node.** The table is not a
            // bookkeeping side-car: a fill referring to an id the document does
            // not carry paints nothing, so putting the entry back — undoing a
            // purge, finishing a relink — is the difference between an empty
            // shape and a photograph on screen. Answering `false` here would
            // leave the selection box sitting over the one edit that just changed
            // the most.
            //
            // (What it draws without the entry is the placeholder — a grey cross
            // rather than a blank, §15 D179. Either way the entry going back is
            // what changes the picture.)
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. } => true,
        }
    }

    /// Whether applying this to `doc` would leave the document exactly as it
    /// stands.
    ///
    /// The per-operation half of [`Transaction::changes_nothing`], which is
    /// where the reason for asking is written down.
    ///
    /// **`false` is the safe answer, and every arm that cannot be certain gives
    /// it.** A wrong `true` silently drops an edit; a wrong `false` only commits
    /// an undo step nobody wanted, which is the state the app was in before this
    /// existed. So the match is exhaustive with no wildcard — a variant added
    /// later has to answer for itself rather than inherit a guess.
    ///
    /// **An operation whose subject is missing answers `false`**, so a commit
    /// that would have failed still fails and still says so. The same goes for
    /// the two kind gates `apply` enforces ([`Self::SetFills`] and
    /// [`Self::SetStrokes`] on an unpaintable kind, [`Self::SetEffects`] on the
    /// root): this reports "changes something" rather than swallowing the error
    /// that is about to be raised.
    ///
    /// ⚠️ **Equality of the value written, not of the bytes stored.** `-0.0` and
    /// `0.0` compare equal, so a transform that changes only a zero's sign is
    /// called a no-op — nothing that reads the document can tell those apart.
    /// `NaN` compares equal to nothing, which is the useful direction: a
    /// non-finite operation is never a no-op and still reaches
    /// [`OpError::NonFinite`].
    pub fn changes_nothing(&self, doc: &crate::Document) -> bool {
        let node = |id: &NodeId| doc.get(*id);
        let kind = |id: &NodeId| doc.get(*id).map(Node::kind);
        match self {
            // **Structural, so never a no-op by inspection.** A create, a delete,
            // an insert or an image/guide add-remove either changes the tree or
            // fails; there is no third answer to find here, and answering `true`
            // for an insert of zero nodes would suppress the `MalformedSubtree`
            // that says so.
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::InsertSubtree { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::AddImage { .. }
            | Operation::RemoveImage { .. } => false,

            // **The index is read the way `apply` writes it** — the child is
            // removed from its parent's list and re-inserted at `index`, so the
            // list comes back identical exactly when `index` is where it already
            // sits. That is also true of a reparent into the parent it already
            // has, which is why the two arms differ only in the parent test.
            Operation::Reparent {
                id,
                new_parent,
                index,
            } => {
                node(id).and_then(Node::parent) == Some(*new_parent)
                    && child_index(doc, *id) == Some(*index)
            }
            Operation::Reorder { id, index } => child_index(doc, *id) == Some(*index),

            Operation::SetTransform { id, transform } => {
                node(id).is_some_and(|n| n.transform() == *transform)
            }
            // **Asked of the result rather than of the patch**, which is what
            // makes this arm exact for all fourteen patch shapes at once: a patch
            // that does not match the kind answers `None` and so answers `false`
            // here, and one that does is compared against the kind it would
            // produce.
            Operation::SetGeometry { id, geometry } => node(id).is_some_and(|n| {
                geometry
                    .applied_to(n.kind())
                    .is_some_and(|would_be| would_be == *n.kind())
            }),

            // **The four span-carrying arms compare what `apply` would store, not
            // what the caller handed over.** Every one of them clamps or re-states
            // its list on the way in, so a list that differs from the stored one
            // only by a span the clamp would drop is a no-op, and asking the
            // question any other way would call it an edit.
            Operation::SetText {
                id,
                content,
                spans,
                para_spans,
            } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content: c,
                    spans: sp,
                    para_spans: psp,
                    style,
                    paragraph,
                    ..
                }) if c == content
                    && *sp == spans.clamped(content, style)
                    && *psp == para_spans.clamped(content, paragraph)
            ),
            Operation::SetTextStyle { id, style, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    style: s,
                    spans: sp,
                    ..
                }) if **s == *style
                    && match spans {
                        // `None` means "re-state against the defaults", and the
                        // defaults are unchanged on this branch, so the question
                        // is whether the stored list is already a fixed point of
                        // the re-statement.
                        None => *sp == sp.restated(style),
                        Some(given) => *sp == given.clamped(content, style),
                    }
            ),
            Operation::SetTextSpans { id, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    style,
                    spans: sp,
                    ..
                }) if *sp == spans.clamped(content, style)
            ),
            Operation::SetParagraphStyle {
                id,
                paragraph,
                spans,
            } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    paragraph: p,
                    para_spans: sp,
                    ..
                }) if p == paragraph
                    && match spans {
                        None => *sp == sp.restated(paragraph),
                        Some(given) => *sp == given.clamped(content, paragraph),
                    }
            ),
            Operation::SetParagraphSpans { id, spans } => matches!(
                kind(id),
                Some(NodeKind::Text {
                    content,
                    paragraph,
                    para_spans: sp,
                    ..
                }) if *sp == spans.clamped(content, paragraph)
            ),
            Operation::SetBlockStyle { id, block } => matches!(
                kind(id),
                Some(NodeKind::Text { block: b, .. }) if b == block
            ),

            Operation::SetName { id, name } => node(id).is_some_and(|n| n.name() == name),
            Operation::SetVisible { id, visible } => {
                node(id).is_some_and(|n| n.visible() == *visible)
            }
            Operation::SetLocked { id, locked } => node(id).is_some_and(|n| n.locked() == *locked),
            Operation::SetProportionsLocked { id, locked } => {
                node(id).is_some_and(|n| n.proportions_locked() == *locked)
            }
            Operation::SetOpacity { id, opacity } => {
                node(id).is_some_and(|n| n.opacity() == *opacity)
            }
            Operation::SetPivot { id, pivot } => node(id).is_some_and(|n| n.pivot() == *pivot),
            Operation::SetClip { id, clip } => node(id).is_some_and(|n| n.clip() == *clip),
            Operation::SetMask { id, mask } => node(id).is_some_and(|n| n.mask() == *mask),
            Operation::SetMaskMode { id, mode } => node(id).is_some_and(|n| n.mask_mode() == *mode),
            Operation::SetFillRule { id, rule } => node(id).is_some_and(|n| n.fill_rule() == *rule),

            // `takes_paint` is the gate `apply` applies, spelled here so an
            // unpaintable subject reaches its `WrongKindForOp` rather than being
            // called a no-op because both lists happen to be empty.
            Operation::SetFills { id, fills } => {
                node(id).is_some_and(|n| n.kind().takes_paint() && n.paint().fills == *fills)
            }
            Operation::SetStrokes { id, strokes } => {
                node(id).is_some_and(|n| n.kind().takes_paint() && n.paint().strokes == *strokes)
            }
            // The root is the one kind `op_set_effects` refuses; the rest carry
            // the stack whether they paint or not.
            Operation::SetEffects { id, effects } => node(id)
                .is_some_and(|n| !matches!(n.kind(), NodeKind::Root) && n.effects() == effects),
            Operation::SetExports { id, exports } => {
                node(id).is_some_and(|n| n.exports() == exports)
            }
            Operation::SetLayoutGrids { id, grids } => node(id).is_some_and(|n| n.grids() == grids),

            Operation::SetCanvasBackground { background } => doc.canvas_background() == *background,
            Operation::SetGuidePosition { id, position } => {
                doc.guide(*id).is_some_and(|g| g.position == *position)
            }
            Operation::SetGuideColor { id, color } => {
                doc.guide(*id).is_some_and(|g| g.color == *color)
            }
            Operation::SetGuideScope {
                id,
                owner,
                position,
            } => doc
                .guide(*id)
                .is_some_and(|g| g.owner == *owner && g.position == *position),
        }
    }
}

/// Where `id` sits in its parent's child list, or `None` when it has no parent
/// (the root) or is not in the document.
///
/// **The number [`Operation::Reorder`] is compared against**, and it is spelled
/// here rather than inline because the reparent arm asks the identical question
/// of the identical list.
fn child_index(doc: &crate::Document, id: NodeId) -> Option<usize> {
    let parent = doc.get(id)?.parent()?;
    doc.get(parent)?.children().iter().position(|c| *c == id)
}

impl Transaction {
    /// Whether any operation in this changes what is drawn
    /// ([`Operation::changes_ink`]).
    ///
    /// **Any, not all.** A transaction that moves a layer *and* renames it has a
    /// visible result, so it is worth looking at; one that only renames does not.
    pub fn changes_ink(&self) -> bool {
        self.0.iter().any(Operation::changes_ink)
    }

    /// Whether applying this would leave `doc` exactly as it stands — a
    /// completed interaction that turned out to change nothing (§15 D428).
    ///
    /// **All, not any, and that is the whole specification.** A per-operation
    /// filter would break the case the app relies on most: *Hide* over a mixed
    /// selection emits a `SetVisible` per layer, and the ones already hidden are
    /// individually no-ops while the transaction as a whole hides something.
    /// Dropping those would still commit — but a later reader would find the
    /// undo step describing fewer layers than the user selected, and the inverse
    /// would put back a different set. So the question is asked of the
    /// transaction and nothing is ever dropped *from* one.
    ///
    /// **Sound because a no-op leaves the document it was measured against.**
    /// Each operation is tested against `doc` as it stands now rather than
    /// against the state its predecessors would leave; that is the same thing,
    /// because if every operation before it changes nothing then the document it
    /// would actually see is this one.
    ///
    /// **Empty is not the same question.** An empty transaction is a tool
    /// legitimately producing no edit and is dropped one line earlier at the
    /// commit seam; this answers `false` for one, so that "changes nothing" is
    /// only ever said about work that was really attempted.
    pub fn changes_nothing(&self, doc: &crate::Document) -> bool {
        !self.0.is_empty() && self.0.iter().all(|op| op.changes_nothing(doc))
    }
}

/// Geometry edit targeting the kind-specific payload of a node (§5.6).
#[derive(Clone, Debug, PartialEq)]
pub enum GeometryPatch {
    Size(Size),
    /// The same radius on all four corners — the common edit, spelled once so a
    /// caller that has no per-corner opinion cannot accidentally set three.
    CornerRadius(f64),
    /// Per-corner radii. The inverse of *either* corner patch is always this
    /// one, because a uniform set has to be undoable back to four unequal
    /// values.
    CornerRadii(RoundedRectRadii),
    /// Vertex count: a polygon's sides, or a star's points.
    Sides(u32),
    /// A star's inner radius as a fraction of its outer one, 0..=1.
    InnerRatio(f64),
    LineEnd(Point),
    /// A path's outline **and** its per-anchor radii, together.
    ///
    /// **One patch, not two, because they are one fact.** The radii are indexed
    /// by anchor (`NodeKind::Path::corner_radii`), so a patch that replaced the
    /// outline alone would leave the radii numbered against a path that no longer
    /// exists — the moment an anchor is inserted or deleted, every radius after it
    /// is on the wrong corner. Carrying both makes that unsayable, and makes a
    /// point edit one undo step rather than two that can be separated.
    Path {
        path: BezPath,
        corner_radii: Vec<f64>,
    },
    TextSizing(crate::node::TextSizing),
    /// The rail a `Text` node is set along, or `None` to take it off
    /// (`NodeKind::Text::on_path`, §15 D405).
    ///
    /// **A geometry patch and not a style one**, for the reason `TextSizing`
    /// beside it is: it changes the node's *box* — every glyph moves and the
    /// bounds are the bent line box — where the three typographic scopes change
    /// how the characters are drawn inside one. It is also what makes *Text on
    /// path* and *Detach* one undo step each and reversible by the same
    /// machinery, since a patch reports the value it displaced.
    TextPath(Option<BezPath>),
    /// Which way round a `Text` node runs along its rail
    /// (`NodeKind::Text::on_path_flip`, §15 D406).
    ///
    /// **Its own patch rather than a second field on [`Self::TextPath`]**, for the
    /// reason the model keeps the two apart: the flip outlives the rail, so a
    /// patch carrying both would make *Detach from path* — which sends
    /// `TextPath(None)` — say something about the flip as well, and the obvious
    /// something is `false`. That is the destructive toggle spelled as an
    /// operation.
    TextPathFlip(bool),
    /// Where along its rail a `Text` node's type starts, as a fraction of the
    /// rail's length (`NodeKind::Text::on_path_offset`, §15 D409).
    ///
    /// **Its own patch beside the flip, for the flip's own reason**: the offset
    /// outlives the rail, so a patch carrying rail and offset together would make
    /// *Detach from path* — which sends `TextPath(None)` — say something about the
    /// offset as well.
    ///
    /// ⚠️ **Written once today, by the Text tool's click, and that is a gap rather
    /// than a design.** There is no gesture for *moving* type along its rail yet;
    /// this is the door one would come through, and it is a geometry patch so that
    /// such a gesture would preview and undo like every other.
    TextPathOffset(f64),
    /// Which operation a `Boolean` container performs.
    ///
    /// A *geometry* patch and not a kind of its own, because that is what it is:
    /// it changes the node's outline and nothing else about it, and it travels
    /// through the same preview→commit valve every other geometry edit does —
    /// which is what lets a dropdown preview an operation on hover.
    BoolOp(crate::node::BoolOp),
}

impl GeometryPatch {
    /// A copy of `kind` with this patch applied, or `None` if the patch does
    /// not match the kind.
    ///
    /// The non-mutating twin of what `apply` does internally, for callers that
    /// need to know what a geometry edit *would* produce without performing it
    /// — chiefly render previews, which must not touch the `Document`
    /// (invariant 5).
    pub fn applied_to(&self, kind: &NodeKind) -> Option<NodeKind> {
        let mut copy = kind.clone();
        crate::document::apply_geometry_patch(&mut copy, self)
            .ok()
            .map(|_| copy)
    }

    /// Whether every number this patch carries is finite.
    ///
    /// **Matched exhaustively on purpose**, so a variant added later has to
    /// answer this rather than defaulting to "fine": every arm here writes a
    /// number straight into the document, and a non-finite one makes the file
    /// unopenable for good. See [`OpError::NonFinite`].
    pub fn is_finite(&self) -> bool {
        let size = |s: &kurbo::Size| s.width.is_finite() && s.height.is_finite();
        match self {
            Self::Size(s) => size(s),
            Self::CornerRadius(v) | Self::InnerRatio(v) | Self::TextPathOffset(v) => v.is_finite(),
            Self::CornerRadii(r) => [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
                .iter()
                .all(|v| v.is_finite()),
            Self::LineEnd(p) => p.x.is_finite() && p.y.is_finite(),
            Self::Path { path, corner_radii } => {
                crate::node::path_is_finite(path) && corner_radii.iter().all(|v| v.is_finite())
            }
            Self::TextSizing(s) => match s {
                crate::node::TextSizing::Auto => true,
                crate::node::TextSizing::AutoHeight(w) => w.is_finite(),
                crate::node::TextSizing::Fixed(s) => size(s),
            },
            Self::TextPath(p) => p.as_ref().is_none_or(crate::node::path_is_finite),
            // No number in them at all.
            Self::Sides(_) | Self::TextPathFlip(_) | Self::BoolOp(_) => true,
        }
    }
}

/// Applied atomically; one entry in history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transaction(pub Vec<Operation>);

/// Nodes whose world bounds / appearance changed (including descendants).
#[derive(Clone, Debug, Default)]
pub struct DirtySet(pub FxHashSet<NodeId>);

/// Result of a successful `apply`.
#[derive(Debug)]
pub struct ApplyOutcome {
    /// Apply this to undo.
    pub inverse: Transaction,
    pub dirty: DirtySet,
}

#[derive(thiserror::Error, Debug)]
pub enum OpError {
    #[error("no such node: {0:?}")]
    NoSuchNode(NodeId),
    #[error("duplicate id: {0:?}")]
    DuplicateId(NodeId),
    #[error("invalid parent")]
    InvalidParent,
    #[error("operation would create a cycle")]
    WouldCycle,
    #[error("index out of range")]
    IndexOutOfRange,
    #[error("opacity out of range (expected 0.0..=1.0)")]
    BadOpacity,
    /// An export spec carrying a value that cannot be a size or a quality
    /// (§15 D492).
    ///
    /// **Both other entry points already refuse exactly these**, and say why in
    /// their own comments: `ExportScale::from_label` filters `*px > 0` and
    /// `n.is_finite() && *n > 0.0`, and `main.rs`'s CLI parse rejects the same set
    /// *"rather than left to `factor`'s floor, which exists to keep the renderer
    /// safe and **would turn `--scale 0` into a one-pixel file nobody asked
    /// for**"*. That is exactly the file a `Width(0)` produced — the comment
    /// describes the outcome, and the guard was on the two paths that could not
    /// reach it (`[S8.2-L1-03]`).
    #[error("an export spec's scale or quality is not a usable value")]
    BadExportSpec,
    #[error("operation not valid for this node kind")]
    WrongKindForOp,
    /// A frame asked to sit somewhere only a shape may go. Frames nest inside
    /// frames (§5.3); what they cannot do is hang off a group, which has no box for
    /// a page to be clipped by.
    #[error("a frame belongs to the canvas or to another frame, not to a group")]
    ArtboardPlacement,
    #[error("malformed subtree")]
    MalformedSubtree,
    /// The operation had nothing to make a shape out of — a boolean whose operands
    /// cancel out entirely, asked to flatten. Distinct from [`Self::WrongKindForOp`]
    /// because the kind was right and the *geometry* was empty, which is a thing the
    /// user can fix by moving a shape rather than by selecting differently.
    #[error("there is no geometry here to flatten")]
    EmptyGeometry,
    /// The boolean arithmetic was **abandoned** — [`crate::boolean::evaluate`]'s
    /// `catch_unwind` guard caught an unwind out of flo_curves (§15 D239) — rather
    /// than having answered honestly with nothing.
    ///
    /// 🚨 **Distinct from [`Self::EmptyGeometry`] because that variant's own
    /// argument is about *what the user can do next*, and here the answer is
    /// different.** `EmptyGeometry` says the geometry was empty, *"which is a thing
    /// the user can fix by moving a shape rather than by selecting differently"* —
    /// and moving a shape does not help an unwind. Both cases arrive at the same
    /// `ok_or` as `None`, because `None` is also what a correct empty result answers;
    /// the counter is the only thing that tells them apart, which is the whole of
    /// [`crate::boolean::failures`]'s doc.
    ///
    /// ⚠️ **The builders are outside §15 D298's bracket and always were.** D298 wires
    /// the counter into `commit_inner`, *"the one door every edit goes through"*,
    /// reading it across `Resolved::update` — which is **resolution**, after a
    /// transaction exists. A builder calls `evaluate` while *constructing* the
    /// transaction, so a failure there refuses the edit before there is anything for
    /// that bracket to be read around. Not an omission at those call sites: a
    /// different call path (`[S10.1-L2-06]`, §15 D736).
    #[error("a boolean could not be computed, so there is nothing to flatten")]
    BooleanAbandoned,
    #[error("the root node cannot be deleted, reparented, or reordered")]
    CannotModifyRoot,
    #[error("no such guide: {0:?}")]
    NoSuchGuide(GuideId),
    #[error("duplicate guide id: {0:?}")]
    DuplicateGuide(GuideId),
    #[error("no such image: {0}")]
    NoSuchImage(ImageId),
    /// The table already carries this id. **Not the same event as placing the
    /// same picture twice**, which is the point of a content-hashed key and must
    /// be free: a caller that has hashed its bytes asks `Document::has_image`
    /// first and adds a second *reference*, not a second entry. Reaching here
    /// means two different entries claimed one id, which is a bug rather than a
    /// duplicate image.
    #[error("duplicate image id: {0}")]
    DuplicateImage(ImageId),
    /// A guide asked to be scoped to something that is not a live frame — a node
    /// that is not in the document, or one with no box for a guide to be clipped
    /// to.
    ///
    /// Validated rather than tolerated because a guide's `position` is *read in*
    /// its owner's space: with no owner to read it against, the guide has no
    /// place at all, and every drawing and snapping site would need a fallback
    /// for a state the model can simply refuse. It is also what makes the
    /// ordering inside a delete transaction matter — see
    /// [`crate::build::guides_of`].
    #[error("a guide may only be scoped to a live frame, not {0:?}")]
    BadGuideOwner(NodeId),
    /// A number that is `NaN` or infinite — in a node's geometry, in its
    /// transform, in its **pivot** (§15 D639), in an **effect** it carries — a
    /// shadow's offset, blur, spread and colour, a layer blur's radius, the four
    /// filter factors (§15 D641) — in a guide's position (§15 D421), or **in a
    /// paint**: a
    /// gradient's stop offsets and colours, its ramp geometry and its transform,
    /// a solid's colour channels, an image brush's alpha *and the
    /// [`crate::ImageRef`] behind it* — its crop rectangle, its tile scale and
    /// its seven adjustment sliders (§15 D452) — and a stroke's width and dash
    /// lengths (§15 D451).
    ///
    /// ⚠️ **This said *coordinate* and named only the first three until
    /// 2026-09-07**, which is the whole of D451: the rule was always "a number
    /// this document will write", and the code was scoped to the values that
    /// *read* like coordinates. A colour channel is as capable of being `NaN` as
    /// a radius is and goes into the same file.
    ///
    /// ⚠️ **Refused rather than repaired, because the alternative is a file that
    /// never opens again.** `serde_json` writes a non-finite `f64` as `null` and
    /// the reader has no arm for it, so such a value survives `apply`, survives
    /// `save`, and kills `load` permanently — silently, and through autosave and
    /// the crash snapshot as readily as through Ctrl+S. Invariant 8 puts the
    /// check at the operation for exactly this: it is the last place the value
    /// can be rejected while the user still has their work.
    ///
    /// **Not clamped**, unlike an out-of-range opacity on the load path. There is
    /// no defensible number to clamp a coordinate to — 0 moves the shape and
    /// `f64::MAX` is not what was meant — and unlike a stale text span there is
    /// no repairable file at stake yet: the value has not been written.
    #[error("a number is not finite")]
    NonFinite,
}

#[cfg(test)]
mod changes_ink_tests {
    use super::*;
    use crate::id::IdSource;

    /// **Exactly eleven operations change nothing drawn**, and the set is what the
    /// inspector's chrome hide turns on (§15 D128): an edit with no visible result
    /// has nothing to get the selection box out of the way *for*, and an edit whose
    /// only result *is* chrome would have that result taken away by hiding it.
    ///
    /// Pinned as a list rather than left to `changes_ink`'s own `match`, because the
    /// `match` is wildcard-free — so a new operation stops the build there and the
    /// author has to choose an arm, but nothing stops them choosing the *wrong* one.
    /// This is where the choice is checked against what the eleven have in common.
    ///
    /// They are the same eleven `RenderOverrides::absorb` treats as a no-op. If
    /// the two ever disagree, one of them is wrong.
    ///
    /// ⚠️ **This said "nine" and listed nine while the `match` held ten**, having
    /// missed `SetExports` when that op arrived — the exact drift the paragraph
    /// above claims the test exists to catch, sitting inside the test that claims
    /// it. Nothing failed, because a count in a doc comment and a list in a
    /// fixture are both prose as far as every gate is concerned. Recount both
    /// against the `match` when an operation is added; the compiler will not.
    /// **It went to eleven with `SetLayoutGrids`**, and the recount was done by
    /// running this test with a deliberately wrong count in it — which the
    /// fixture below now makes possible, since it asserts its own length.
    ///
    /// ⚠️ **And then the prose said "ten" twice while the sentence between them
    /// said "the same eleven"**, until 2026-09-06 — so this comment held both
    /// answers at once, two lines apart, and the fixture that asserts its own
    /// length could not see it, because the number it asserts against is the
    /// fixture's and not the prose's. **A count written in prose beside a count
    /// the code checks is not covered by the code checking it.**
    #[test]
    fn only_the_chrome_operations_change_no_ink() {
        let mut ids = IdSource::new(1);
        let id = ids.mint();
        let guide = crate::guide::GuideId(ids.mint());
        let invisible = [
            Operation::SetName {
                id,
                name: "x".into(),
            },
            Operation::SetLocked { id, locked: true },
            Operation::SetProportionsLocked { id, locked: true },
            Operation::SetPivot { id, pivot: None },
            Operation::SetExports {
                id,
                exports: vec![],
            },
            Operation::SetLayoutGrids { id, grids: vec![] },
            Operation::AddGuide {
                guide: crate::guide::Guide {
                    id: guide,
                    axis: crate::guide::GuideAxis::Vertical,
                    position: 0.0,
                    color: None,
                    owner: None,
                },
            },
            Operation::RemoveGuide { id: guide },
            Operation::SetGuidePosition {
                id: guide,
                position: 1.0,
            },
            Operation::SetGuideColor {
                id: guide,
                color: None,
            },
            Operation::SetGuideScope {
                id: guide,
                owner: None,
                position: 1.0,
            },
        ];
        // ⚠️ **The fixture asserts its own length**, which is the only thing that
        // makes the count in this test's doc comment checkable by anything but a
        // reader — and that comment has been wrong once, by exactly this
        // mechanism.
        assert_eq!(invisible.len(), 11, "the chrome list is eleven operations");
        for op in &invisible {
            assert!(
                !op.changes_ink(),
                "{op:?} is chrome or bookkeeping and must not arm the chrome hide"
            );
        }

        // And the ones that plainly do, including the two that are easy to file with
        // the bookkeeping above: `SetVisible` empties a layer's pixels, and
        // `SetCanvasBackground` repaints the ground the artwork sits on.
        let visible = [
            Operation::SetVisible { id, visible: false },
            Operation::SetCanvasBackground {
                background: peniko::Color::BLACK,
            },
            Operation::SetOpacity { id, opacity: 0.5 },
            Operation::SetClip { id, clip: true },
            Operation::SetTransform {
                id,
                transform: kurbo::Affine::IDENTITY,
            },
            Operation::SetFills { id, fills: vec![] },
            Operation::SetStrokes {
                id,
                strokes: vec![],
            },
            // Filed here and not with `SetExports` above, which is the mistake
            // worth guarding: both are a whole-list edit to a `Vec` on the node
            // that most layers leave empty, and only one of them draws.
            Operation::SetEffects {
                id,
                effects: vec![],
            },
            Operation::DeleteNode { id },
            Operation::Reorder { id, index: 0 },
        ];
        for op in &visible {
            assert!(op.changes_ink(), "{op:?} changes what is drawn");
        }
    }

    /// **Any, not all.** A transaction that moves a layer *and* renames it has a
    /// visible result and is worth looking at; one that only renames does not — which
    /// is what keeps the layers-panel rename and the identity card's from blanking the
    /// selection box for a second and a half.
    #[test]
    fn a_transaction_is_visible_if_any_of_its_operations_is() {
        let id = IdSource::new(1).mint();
        let rename = Operation::SetName {
            id,
            name: "x".into(),
        };
        let move_it = Operation::SetTransform {
            id,
            transform: kurbo::Affine::translate((1.0, 0.0)),
        };
        assert!(!Transaction(vec![]).changes_ink(), "nothing is not an edit");
        assert!(!Transaction(vec![rename.clone()]).changes_ink());
        assert!(Transaction(vec![rename.clone(), move_it.clone()]).changes_ink());
        assert!(Transaction(vec![move_it, rename]).changes_ink());
    }
}

#[cfg(test)]
mod changes_nothing_tests {
    //! `Operation::changes_nothing` and its whole-transaction twin, which
    //! together are the guard `EditorSession::commit_inner` asks before making an
    //! undo step (§15, G16).
    //!
    //! ⚠️ **What each flip here was run against, and where it bit.** The
    //! plausible wrong version of this feature is not "no guard at all" — that
    //! fails everything and proves nothing. It is the three near-misses:
    //!
    //! - **`any` in place of `all`** on the transaction. Predicted to fail in
    //!   `a_mixed_selection_still_commits_because_the_test_is_all_and_not_any`,
    //!   and it did — on the *first* assertion, the transaction one, which is the
    //!   loss. The per-operation assertion below it is the mechanism and stays
    //!   second on purpose.
    //! - **The `Reorder` index adjusted for the removal** — `if index > old
    //!   { index == old + 1 } else { index == old }`, which is what somebody who
    //!   has just read `op_reorder`'s remove-then-insert and over-corrected for
    //!   it writes. ⚠️ **Predicted to fail on the *positive* half of
    //!   `rows_dropped_where_they_already_sit_change_nothing` and it failed on
    //!   the negative one**, at *"moving the top row under the bottom one swaps
    //!   them"*. The adjustment does not break a row that is already where it is
    //!   asked to be; what it breaks is calling a real swap a no-op — which is
    //!   the **worse** failure of the two, since that is an edit silently
    //!   dropped. The correction is left here rather than in the prose above it,
    //!   because the assertion that bites is the one the next reader is shown.
    //! - **Dropping the `takes_paint` gate** from the `SetFills` arm. Predicted
    //!   to fail in `an_operation_whose_subject_refuses_it_is_not_a_no_op`, and
    //!   it did — a `SetFills { fills: vec![] }` aimed at the root compares two
    //!   empty lists and is only saved from being called a no-op by the gate.

    use super::*;
    use crate::Document;
    use crate::id::IdSource;
    use crate::image::Brush;
    use crate::node::TextSizing;

    /// Root → frame → two rects, at child indices 0 and 1.
    fn fixture() -> (Document, NodeId, NodeId, [NodeId; 2]) {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let a = ids.mint();
        let b = ids.mint();
        let mut tx = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: None,
            name: None,
        }];
        for (i, id) in [a, b].into_iter().enumerate() {
            tx.push(Operation::CreateNode {
                id,
                parent: frame,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(tx)).expect("seed");
        (doc, root, frame, [a, b])
    }

    /// **The property the whole guard rests on, stated as a property rather than
    /// as a list**: an operation that writes back the value already stored is a
    /// no-op, and the same operation carrying any other value is not.
    ///
    /// Each pair is `(the value already there, something else)`, so the negative
    /// half is a real edit of the same field rather than a different field
    /// altogether — which is what makes a wrong `true` visible here. The spread
    /// is one entry per *shape* of arm in the `match`, not one per variant: a
    /// plain node field, a geometry patch resolved through
    /// `GeometryPatch::applied_to`, a paint list behind a kind gate, a whole
    /// stack, and the one operation with no node to name.
    #[test]
    fn writing_back_the_stored_value_changes_nothing_and_any_other_value_does() {
        let (doc, _root, frame, [a, _b]) = fixture();
        let pairs: Vec<(Operation, Operation)> = vec![
            (
                Operation::SetTransform {
                    id: a,
                    transform: Affine::IDENTITY,
                },
                Operation::SetTransform {
                    id: a,
                    transform: Affine::translate((1.0, 0.0)),
                },
            ),
            (
                Operation::SetGeometry {
                    id: a,
                    geometry: GeometryPatch::Size(Size::new(10.0, 10.0)),
                },
                Operation::SetGeometry {
                    id: a,
                    geometry: GeometryPatch::Size(Size::new(10.0, 10.5)),
                },
            ),
            (
                Operation::SetName {
                    id: a,
                    name: doc.get(a).expect("seeded").name().to_string(),
                },
                Operation::SetName {
                    id: a,
                    name: "something else".into(),
                },
            ),
            (
                Operation::SetVisible {
                    id: a,
                    visible: true,
                },
                Operation::SetVisible {
                    id: a,
                    visible: false,
                },
            ),
            (
                Operation::SetLocked {
                    id: a,
                    locked: false,
                },
                Operation::SetLocked {
                    id: a,
                    locked: true,
                },
            ),
            (
                Operation::SetProportionsLocked {
                    id: a,
                    locked: false,
                },
                Operation::SetProportionsLocked {
                    id: a,
                    locked: true,
                },
            ),
            (
                Operation::SetOpacity {
                    id: a,
                    opacity: 1.0,
                },
                Operation::SetOpacity {
                    id: a,
                    opacity: 0.999,
                },
            ),
            (
                Operation::SetPivot { id: a, pivot: None },
                Operation::SetPivot {
                    id: a,
                    pivot: Some(crate::node::Pivot::Normalized(kurbo::Vec2::new(0.25, 0.25))),
                },
            ),
            (
                Operation::SetClip { id: a, clip: false },
                Operation::SetClip { id: a, clip: true },
            ),
            (
                Operation::SetMask { id: a, mask: false },
                Operation::SetMask { id: a, mask: true },
            ),
            (
                Operation::SetFillRule {
                    id: a,
                    rule: FillRule::default(),
                },
                Operation::SetFillRule {
                    id: a,
                    rule: match FillRule::default() {
                        FillRule::NonZero => FillRule::EvenOdd,
                        FillRule::EvenOdd => FillRule::NonZero,
                    },
                },
            ),
            (
                Operation::SetFills {
                    id: a,
                    fills: vec![],
                },
                Operation::SetFills {
                    id: a,
                    fills: vec![Fill {
                        brush: Brush::Solid(peniko::Color::BLACK),
                        visible: true,
                    }],
                },
            ),
            (
                Operation::SetEffects {
                    id: a,
                    effects: vec![],
                },
                Operation::SetEffects {
                    id: a,
                    effects: vec![Effect::new(crate::effect::EffectKind::LayerBlur {
                        radius: 2.0,
                    })],
                },
            ),
            (
                Operation::SetExports {
                    id: a,
                    exports: vec![],
                },
                Operation::SetExports {
                    id: a,
                    exports: vec![ExportSpec::new(
                        crate::export::ExportFormat::Png,
                        crate::export::ExportScale::Times(1.0),
                    )],
                },
            ),
            (
                Operation::SetLayoutGrids {
                    id: frame,
                    grids: vec![],
                },
                Operation::SetLayoutGrids {
                    id: frame,
                    grids: vec![crate::layout::LayoutGrid::new(
                        crate::layout::GridAxis::Columns,
                    )],
                },
            ),
            (
                Operation::SetCanvasBackground {
                    background: doc.canvas_background(),
                },
                Operation::SetCanvasBackground {
                    background: peniko::Color::from_rgb8(1, 2, 3),
                },
            ),
        ];
        assert_eq!(pairs.len(), 16, "one pair per shape of arm");
        for (still, moved) in &pairs {
            assert!(
                still.changes_nothing(&doc),
                "writing back the stored value should change nothing: {still:?}"
            );
            assert!(
                !moved.changes_nothing(&doc),
                "writing a different value is an edit: {moved:?}"
            );
        }
    }

    /// **`[S15.1-L1-03]`'s witness**, and the arm whose arithmetic is easiest to
    /// write the plausible wrong way.
    ///
    /// A multi-row drop that lands rows where they already are emits one
    /// `Reorder` per row, each net-zero, and the transaction is *not empty* — so
    /// the empty test at the commit seam let every one of them through and the
    /// user got an undo step that undoes to the same picture.
    ///
    /// The index is compared the way `apply` writes it: the child is removed from
    /// its parent's list and re-inserted at `index`, which comes back identical
    /// exactly when `index` is where the child already sits — so the comparison
    /// is against the list *as it stands*, with no adjustment for the removal.
    /// ⚠️ **The assertion that catches an adjustment is the negative one**, not
    /// the positive: an over-corrected index still agrees that a row asked for
    /// its own position changes nothing, and disagrees about a real swap. See
    /// the module's flip notes.
    #[test]
    fn rows_dropped_where_they_already_sit_change_nothing() {
        let (doc, _root, _frame, [a, b]) = fixture();
        let still = Transaction(vec![
            Operation::Reorder { id: a, index: 0 },
            Operation::Reorder { id: b, index: 1 },
        ]);
        assert!(
            still.changes_nothing(&doc),
            "two rows dropped on their own positions are one undo step for nothing"
        );
        assert!(
            !Transaction(vec![Operation::Reorder { id: a, index: 1 }]).changes_nothing(&doc),
            "moving the top row under the bottom one swaps them"
        );
        // The reparent arm asks the identical question of the identical list, so
        // the same drop spelled as a move into the parent it already has is a
        // no-op too — and one index along is not.
        assert!(
            Transaction(vec![Operation::Reparent {
                id: b,
                new_parent: _frame,
                index: 1,
            }])
            .changes_nothing(&doc)
        );
        assert!(
            !Transaction(vec![Operation::Reparent {
                id: b,
                new_parent: _frame,
                index: 0,
            }])
            .changes_nothing(&doc)
        );
    }

    /// **The whole specification of the transaction-level test, and the reason it
    /// is `all`.**
    ///
    /// *Hide* over a mixed selection emits a `SetVisible` per layer. The ones
    /// already hidden are individually no-ops; the transaction as a whole hides
    /// something. An `any` reading would call the transaction a no-op and throw
    /// the edit away — which is why the loss is asserted first and the mechanism
    /// second.
    #[test]
    fn a_mixed_selection_still_commits_because_the_test_is_all_and_not_any() {
        let (mut doc, _root, _frame, [a, b]) = fixture();
        doc.apply(&Transaction(vec![Operation::SetVisible {
            id: a,
            visible: false,
        }]))
        .expect("hide one of them");
        let hide_both = Transaction(vec![
            Operation::SetVisible {
                id: a,
                visible: false,
            },
            Operation::SetVisible {
                id: b,
                visible: false,
            },
        ]);
        assert!(
            !hide_both.changes_nothing(&doc),
            "hiding a mixed selection hides something and must commit"
        );
        assert!(
            hide_both.0[0].changes_nothing(&doc),
            "and it must do so with a member that is individually a no-op in it"
        );
        // An empty transaction is a different question, answered one line earlier
        // at the commit seam; this one is only ever asked about work attempted.
        assert!(!Transaction(vec![]).changes_nothing(&doc));
    }

    /// **An operation `apply` would refuse is not a no-op**, however equal the two
    /// values look — otherwise the guard would swallow the error instead of the
    /// edit, and the user would be told nothing at all.
    ///
    /// Two doors: a subject that is not in the document, and a subject whose
    /// *kind* refuses the operation. The second is the interesting one, because
    /// both lists really are equal — the root's fills are empty and so are the
    /// ones being written — and only the `takes_paint` gate tells them apart.
    #[test]
    fn an_operation_whose_subject_refuses_it_is_not_a_no_op() {
        let (doc, root, _frame, _rects) = fixture();
        let ghost = IdSource::new(0xDEAD).mint();
        assert!(
            !Operation::SetName {
                id: ghost,
                name: "n".into(),
            }
            .changes_nothing(&doc)
        );
        assert!(
            !Operation::SetFills {
                id: root,
                fills: vec![],
            }
            .changes_nothing(&doc),
            "the root is unpaintable, so this reaches WrongKindForOp rather than being dropped"
        );
        assert!(
            !Operation::SetEffects {
                id: root,
                effects: vec![],
            }
            .changes_nothing(&doc),
            "the root is the one kind op_set_effects refuses"
        );
        // And a non-finite value is never equal to anything, so it still reaches
        // `OpError::NonFinite` (§15 D415) rather than being called a no-op.
        assert!(
            !Operation::SetGuidePosition {
                id: crate::guide::GuideId(ghost),
                position: f64::NAN,
            }
            .changes_nothing(&doc)
        );
    }

    /// **The four span-carrying arms compare what `apply` would store**, not what
    /// the caller handed over — every one of them clamps or re-states its list on
    /// the way in.
    ///
    /// So a `SetTextSpans` carrying a list that reaches past the end of the
    /// content is a no-op when the clamp brings it back to the list already
    /// stored, and a `SetTextStyle` with `spans: None` is a no-op when the stored
    /// list is already a fixed point of the re-statement. Compared any other way,
    /// both would be called edits and would commit an undo step that changes
    /// nothing.
    #[test]
    fn the_text_arms_compare_the_clamped_list_and_not_the_one_handed_over() {
        let mut ids = IdSource::new(0x7E47);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let t = ids.mint();
        let style = crate::node::TextStyle::default();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: t,
            parent: root,
            index: 0,
            kind: NodeKind::Text {
                content: "abc".into(),
                style: Box::new(style.clone()),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
                paragraph: crate::node::ParagraphStyle::default(),
                block: crate::node::BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("seed the text node");

        assert!(
            Operation::SetText {
                id: t,
                content: "abc".into(),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
            }
            .changes_nothing(&doc)
        );
        assert!(
            !Operation::SetText {
                id: t,
                content: "abd".into(),
                spans: crate::node::CharSpans::default(),
                para_spans: crate::node::ParaSpans::default(),
            }
            .changes_nothing(&doc)
        );
        // `None` is the spelling every forward caller uses, and it re-states the
        // stored list against the defaults being written. Same defaults, and the
        // stored list is empty, so the re-statement is the identity.
        assert!(
            Operation::SetTextStyle {
                id: t,
                style: style.clone(),
                spans: None,
            }
            .changes_nothing(&doc)
        );
        let mut bigger = style.clone();
        bigger.font_size += 1.0;
        assert!(
            !Operation::SetTextStyle {
                id: t,
                style: bigger,
                spans: None,
            }
            .changes_nothing(&doc)
        );
        assert!(
            Operation::SetParagraphStyle {
                id: t,
                paragraph: crate::node::ParagraphStyle::default(),
                spans: None,
            }
            .changes_nothing(&doc)
        );
        assert!(
            Operation::SetBlockStyle {
                id: t,
                block: crate::node::BlockStyle::default(),
            }
            .changes_nothing(&doc)
        );
        // A rect has no block style to compare, so the arm answers `false` on it
        // rather than reaching for a field that is not there.
        assert!(
            !Operation::SetBlockStyle {
                id: root,
                block: crate::node::BlockStyle::default(),
            }
            .changes_nothing(&doc)
        );
    }
}
