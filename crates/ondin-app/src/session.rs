//! `EditorSession` — everything an editing session is, with no GPU in it (§9.2).
//!
//! Owns the document, history, resolved layer, id source, selection, camera and
//! save state, and is the only place they are mutated. Deliberately free of
//! `wgpu`/`egui` types for three reasons:
//!
//! 1. it can be unit-tested without a graphics device, which is how the
//!    id-collision and dirty-tracking bugs below are pinned down;
//! 2. `ondin serve` (§8.2) needs exactly this state without a window, and a
//!    second implementation would drift from the GUI's;
//! 3. it is where §8.3's single-writer op queue plugs in — MCP handlers submit
//!    transactions and the owning loop drains them through [`EditorSession::commit`],
//!    the same path the UI uses.
//!
//! Rendering lives in `canvas.rs`, which borrows `render_inputs()` and owns the
//! wgpu resources.

use crate::view::Camera;
use ondin_core::Brush;
use ondin_core::kurbo::{Affine, Point, Rect, RoundedRectRadii, Size};
use ondin_core::node::{
    BlockStyle, CharSpans, Length, ParaSpans, ParagraphStyle, TextSizing, TextStyle,
};
use ondin_core::peniko::Color;
use ondin_core::text::TextLayout;
use ondin_core::{
    Document, Fill, GuideId, History, IdSource, Node, NodeId, NodeKind, OpError, Operation, Paint,
    Pivot, Resolved, Transaction, geometry,
};
use ondin_render::{NodeOverride, RenderOverrides};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a run of same-verb edits stays open between two of them
/// ([`EditorSession::commit_run`]).
const RUN_WINDOW: Duration = Duration::from_millis(600);

/// What the status line is reporting. Errors are kept distinct so the chrome
/// can colour them — a failed commit used to be discarded with `let _ =` and
/// the user simply saw nothing happen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
}

impl Default for Status {
    /// Empty. The status line reports what just happened, and at startup nothing
    /// has — a standing "Ready" is a label that never changes and so says
    /// nothing.
    fn default() -> Self {
        Self {
            text: String::new(),
            kind: StatusKind::Info,
        }
    }
}

/// What is selected: some nodes, or some guides.
///
/// A newtype rather than a bare `Vec` so multi-select rules (dedupe, prune on
/// delete, "the single selection") live in one place instead of being re-derived
/// at every call site — and, since guides arrived, so that the *exclusivity*
/// between the two kinds of subject is structural rather than a rule every call
/// site has to remember. A guide and a layer are never selected together:
/// nothing acts on both (there is no transform, no paint and no z-order in
/// common), so a mixed selection would be a state with no operations. Every
/// mutator below therefore clears the other half.
///
/// **Both halves are lists, and the exclusivity is unchanged by that.** Guides
/// multi-select by Shift+click, so what used to be one `Option<GuideId>` is a
/// `Vec`; what has not moved is that [`Self::is_empty`], [`Self::len`] and
/// [`Self::single`] answer about **layers** and read a guide selection as empty
/// however many guides it holds. Widening the guide half without holding that
/// line is what would let node code start treating guides as layers.
#[derive(Default, Clone, Debug)]
pub struct Selection {
    ids: Vec<NodeId>,
    /// The **key layer**: the one member a multi-selection is measured *against*
    /// rather than merely included in. Align moves everything to its edge, and
    /// `build::boolean` makes it the base operand.
    ///
    /// **Explicit, not "the last one clicked".** Deriving it from `ids` order was
    /// the first design and it fails on two counts. Align would stop aligning to
    /// the selection's union for everybody — losing "align these to each other",
    /// which is the common case and the one that has always worked — and the
    /// canvas mark would then be drawn on some member of *every* multi-selection,
    /// which is noise that reads as a bug rather than as an answer. `None` is
    /// therefore the ordinary state and means "no key": today's behaviour,
    /// unchanged, until someone asks for something else.
    key: Option<NodeId>,
    guides: Vec<GuideId>,
    /// The orientation a **multi-selection's** frame has been left in by a turn or
    /// a lean, with the ids it was measured for (§15 D326).
    ///
    /// A set has no stored frame of its own in the document — it is not a group —
    /// so the box drawn round one is derived every frame. Derived from *world* axes
    /// it can only ever be upright, which meant that releasing a rotation snapped
    /// the frame back square while the shapes stayed turned. This is the missing
    /// piece of state: what the frame is turned by, kept for as long as the
    /// selection is.
    ///
    /// **Guarded twice, because there are two ways to lose it and neither guard
    /// covers both.** Every method that changes `ids` calls
    /// [`Selection::forget_frame`], which is what makes *reselecting the same
    /// layers* a new selection — a thing the ids cannot see, since they are equal.
    /// And the orientation carries the ids it was measured for, which is what stops
    /// a future mutator that forgets to call it from leaving an orientation on a
    /// different set.
    ///
    /// The id key alone was tried first and is not enough on its own: `clear()`
    /// followed by selecting the same pair again matched the stored entry and
    /// **resurrected** the old orientation. A test covers that path by name.
    frame: Option<(Vec<NodeId>, Affine)>,
}

impl Selection {
    pub fn ids(&self) -> &[NodeId] {
        &self.ids
    }
    /// Every selected guide, in the order they were picked.
    pub fn guides(&self) -> &[GuideId] {
        &self.guides
    }
    /// Whether `id` is one of the selected guides.
    ///
    /// What the canvas asks per guide as it draws them. **There is deliberately no
    /// `guide() -> Option<GuideId>` beside this.** One existed — the guide half's
    /// [`Self::single`] — and nothing wanted it once the selection became a list:
    /// every reader either acts on all of them (the inspector's fields, Delete, the
    /// nudge, the picker's write) or asks about one by name, which is this. A
    /// `guide()` left in place is a method whose whole meaning is "and is it the
    /// only one", waiting for a caller that means `has_guide` to reach for it.
    pub fn has_guide(&self, id: GuideId) -> bool {
        self.guides.contains(&id)
    }
    /// Select one guide, dropping whatever was selected before — layers and
    /// other guides both.
    pub fn set_guide(&mut self, id: GuideId) {
        self.ids.clear();
        self.key = None;
        self.guides.clear();
        self.guides.push(id);
    }
    /// Drop the guide half and leave the layers alone.
    ///
    /// For *View ▸ Lock guides*, which makes guides unselectable and so has to let
    /// go of any it is holding — but has nothing to say about layers. [`Self::clear`]
    /// was the first implementation and was wrong for the reason the exclusivity
    /// invariant makes easy to miss: with layers selected there are no guides to
    /// drop, so the only thing clearing achieved there was deselecting the user's
    /// work.
    pub fn clear_guides(&mut self) {
        self.guides.clear();
    }
    /// Shift+click semantics for guides: add if absent, remove if already
    /// selected — [`Self::toggle`]'s counterpart, and it clears the layers for
    /// the same reason `toggle` clears the guides.
    pub fn toggle_guide(&mut self, id: GuideId) {
        self.ids.clear();
        self.key = None;
        match self.guides.iter().position(|g| *g == id) {
            Some(i) => {
                self.guides.remove(i);
            }
            None => self.guides.push(id),
        }
    }
    /// Whether any **layers** are selected.
    ///
    /// A selected guide reads as empty here, and so does [`Self::len`] and
    /// [`Self::single`]. That is deliberate and is what every caller wants:
    /// they gate node work — move, align, nudge, delete a layer — and a guide
    /// is not a subject for any of it. What it is *not* is "nothing is
    /// selected"; ask [`Self::nothing`] for that.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    /// Whether nothing at all is selected — no layers **and** no guides.
    ///
    /// The question the chrome asks when it wants to know whether the inspector
    /// has a subject, as opposed to whether the node operations have one.
    pub fn nothing(&self) -> bool {
        self.ids.is_empty() && self.guides.is_empty()
    }
    /// How many **layers** are selected. See [`Self::is_empty`].
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn contains(&self, id: NodeId) -> bool {
        self.ids.contains(&id)
    }
    /// The node when exactly one is selected — the inspector's subject and the
    /// default subject for a future command line (§13 decision 3).
    pub fn single(&self) -> Option<NodeId> {
        match self.ids.as_slice() {
            [id] => Some(*id),
            _ => None,
        }
    }
    /// The key layer — see [`Self::key`] the field.
    ///
    /// **`None` below two layers, whatever is stored.** A key says "this one, not
    /// the others", so with nothing to contrast against it has no meaning: align
    /// falls back to the container and the canvas has nothing to mark. Answering
    /// that here rather than at each call site is what stops the three consumers
    /// disagreeing about it. The stored value survives, so shrinking a selection
    /// to one and growing it back does not silently lose the designation.
    pub fn key(&self) -> Option<NodeId> {
        self.key.filter(|_| self.ids.len() >= 2)
    }
    /// Designate `id` the key layer, or clear the designation if it already is
    /// one — the same click both ways, so there is a way back to aligning
    /// against the selection's union without deselecting everything.
    ///
    /// Ignores an `id` that is not selected: the key is one *of* the selection,
    /// and a key sitting outside it would align the others to a layer the user
    /// cannot see is involved.
    pub fn set_key(&mut self, id: NodeId) {
        if !self.contains(id) {
            return;
        }
        self.key = (self.key != Some(id)).then_some(id);
    }
    pub fn clear(&mut self) {
        self.ids.clear();
        self.key = None;
        self.guides.clear();
        self.forget_frame();
    }
    pub fn set_one(&mut self, id: NodeId) {
        self.ids.clear();
        self.key = None;
        self.guides.clear();
        self.forget_frame();
        self.ids.push(id);
    }
    pub fn set(&mut self, ids: Vec<NodeId>) {
        self.ids = ids;
        self.key = None;
        self.guides.clear();
        self.forget_frame();
        self.dedupe();
    }
    /// Shift-click semantics: add if absent, remove if already selected.
    pub fn toggle(&mut self, id: NodeId) {
        self.guides.clear();
        self.forget_frame();
        match self.ids.iter().position(|c| *c == id) {
            Some(i) => {
                self.ids.remove(i);
                // Deselecting the key gives up the designation. Keeping it would
                // leave a key that is not in the selection, which `set_key`
                // refuses to create in the first place.
                self.key.take_if(|k| *k == id);
            }
            None => self.ids.push(id),
        }
    }
    pub fn add(&mut self, id: NodeId) {
        self.guides.clear();
        self.forget_frame();
        if !self.contains(id) {
            self.ids.push(id);
        }
    }
    /// What this selection's frame is turned by — `IDENTITY` unless a turn or a
    /// lean has left one on *exactly* these ids (§15 D326).
    ///
    /// The id comparison is the invalidation: change the selection and this answers
    /// the identity again, without any mutator having said so.
    pub fn frame(&self) -> Affine {
        match &self.frame {
            Some((ids, f)) if *ids == self.ids => *f,
            _ => Affine::IDENTITY,
        }
    }

    /// Leave `f` as this selection's frame orientation.
    ///
    /// Callers compose rather than replace — `set_frame(turn * selection.frame())` —
    /// so a lean after a turn keeps both.
    pub fn set_frame(&mut self, f: Affine) {
        self.frame = Some((self.ids.clone(), f));
    }

    /// Give up any stored frame orientation.
    ///
    /// **Called by every method that changes `ids`**, including the ones that end up
    /// with the *same* ids — selecting the same two layers again is a new selection
    /// to the hand that did it, and the id key cannot tell the two apart. A new
    /// mutator of `ids` owes a call to this; the id key is what limits the damage if
    /// it does not.
    fn forget_frame(&mut self) {
        self.frame = None;
    }

    /// Drop ids that are no longer in the document (after a delete or an undo).
    pub fn retain_existing(&mut self, doc: &Document) {
        let before = self.ids.len();
        self.ids.retain(|id| doc.contains(*id));
        self.key.take_if(|id| !doc.contains(*id));
        self.guides.retain(|id| doc.guide(*id).is_some());
        // **Only when it actually dropped something.** This runs after every commit
        // and every undo, so forgetting unconditionally would throw the orientation
        // away on the very commit that just set it (§15 D326).
        if self.ids.len() != before {
            self.forget_frame();
        }
    }
    fn dedupe(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.ids.retain(|id| seen.insert(*id));
    }
}

/// A node as it should currently be *shown*: the committed node with any live
/// gesture's override read through it.
///
/// Panels and overlays go through this instead of `Document`. Reading the
/// document directly during a drag makes a control snap back to the committed
/// value every frame, which breaks accumulating gestures like a `DragValue`.
pub struct DisplayNode<'a> {
    node: &'a Node,
    over: Option<&'a NodeOverride>,
}

impl<'a> DisplayNode<'a> {
    pub fn name(&self) -> &'a str {
        self.node.name()
    }
    pub fn locked(&self) -> bool {
        self.node.locked()
    }
    /// No override behind this one: clipping is a checkbox that commits at
    /// once, so there is never a pending version of it to read through.
    pub fn clip(&self) -> bool {
        self.node.clip()
    }
    /// No override behind this one either, and for the same reason: the chain
    /// button commits the moment it is clicked, so there is never a pending
    /// version of the lock to read through.
    pub fn proportions_locked(&self) -> bool {
        self.node.proportions_locked()
    }
    pub fn kind(&self) -> &'a NodeKind {
        self.over
            .and_then(|o| o.kind.as_ref())
            .unwrap_or_else(|| self.node.kind())
    }
    pub fn paint(&self) -> &'a Paint {
        self.over
            .and_then(|o| o.paint.as_ref())
            .unwrap_or_else(|| self.node.paint())
    }
    pub fn opacity(&self) -> f32 {
        self.over
            .and_then(|o| o.opacity)
            .unwrap_or_else(|| self.node.opacity())
    }
    pub fn visible(&self) -> bool {
        self.over
            .and_then(|o| o.visible)
            .unwrap_or_else(|| self.node.visible())
    }
    /// The layer's effect stack (§5.3a).
    ///
    /// **With an override behind it, unlike `clip` and `proportions_locked`
    /// above.** Every number in an effect popover is scrubbable, so each one needs
    /// a live preview or it is the unscrubbable field §15 D245 is about — which is
    /// the same reason `RenderOverrides` carries `SetEffects` at all rather than
    /// filing it with the operations that change nothing drawn.
    pub fn effects(&self) -> &'a [ondin_core::Effect] {
        self.over
            .and_then(|o| o.effects.as_deref())
            .unwrap_or_else(|| self.node.effects())
    }
}

pub struct EditorSession {
    pub doc: Document,
    pub history: History,
    pub resolved: Resolved,
    /// Mints ids for newly created nodes. Kept in step with the open document
    /// by [`Self::adopt_document`] so a load can never cause a `DuplicateId`.
    pub ids: IdSource,
    pub selection: Selection,
    pub camera: Camera,
    /// The file backing this document, if it has been saved or opened.
    pub path: Option<PathBuf>,

    /// Uncommitted gesture preview, as per-node patches over the committed
    /// state (§6.2, invariant 5). Empty when no gesture is live.
    ///
    /// Costs O(nodes the gesture touches). The earlier implementation cloned
    /// the whole document, applied the pending transaction and re-resolved it
    /// every frame — correct, but O(nodes) including re-shaping every text
    /// node, on every mouse move.
    overrides: RenderOverrides,
    /// The live text-editing session's own transaction, kept for as long as the
    /// session's content is what `overrides` stands for — see
    /// [`Self::set_session_preview`] and the clear in [`Self::try_commit`].
    ///
    /// 🚨 **It is kept so a gesture laid *over* it can be wound back to it rather
    /// than cleared** (§15 D109's bug at the *set* side, §15 D609, `[S16.4-L1-03]`). The two
    /// previews share one override set, so an inspector field engaged during a
    /// live session replaced the session's content with the field's own patch and
    /// took the typed text off the canvas — and nothing put it back, because
    /// `preview_session` is event-driven rather than per-frame. With the
    /// transaction here, [`Self::set_preview`] can compose the two and
    /// [`Self::clear_gesture_preview`] can rebuild this one alone.
    ///
    /// **`Some` is also the answer to "does a session hold this preview"**, which
    /// is what the flag it replaces meant. Two spellings of one fact is how they
    /// come to disagree.
    ///
    /// The already-shaped layout [`Self::set_session_preview`] takes is
    /// deliberately *not* kept beside it: rebuilding without one is "correct and
    /// merely slower" (§15 D592), and the rebuild happens once per gesture rather
    /// than once per keystroke.
    session_preview: Option<Transaction>,
    /// Whether `overrides` also carries an **in-flight gesture's** patch — a drag
    /// or a field scrub, as against the standing session preview above.
    ///
    /// The two are independent, which is the whole of §15 D609, `[S16.4-L1-03]`: all four
    /// combinations are reachable, and one `bool` could not tell "a session with a
    /// scrub over it" from "a session alone". [`Self::has_gesture_preview`] reads
    /// this and `cancel_gesture` reads that.
    preview_holds_gesture: bool,
    /// The pivot a live edit is showing, and the layer it belongs to.
    ///
    /// **`RenderOverrides` cannot carry it, and by design.** A pivot is baked by
    /// the gesture that reads it rather than composed into a transform (§15 D55),
    /// so the renderer has nothing to do with one and
    /// `RenderOverrides::from_transaction` absorbs `SetPivot` as a no-op. What a
    /// moved pivot changes is *chrome* — the marker, and the point the panel's two
    /// angle fields turn about — which is the same reason guides need
    /// `OndinApp::preview_guide` beside every preview.
    ///
    /// It lives here rather than beside `guide_previews` because
    /// [`Self::preview_pivot`] is the one door all five readers already come
    /// through, and because it is filled from [`Self::set_preview`] rather than
    /// from a fork every call site owes — a debt guides are still paying.
    ///
    /// The inner `Option` is the pivot's own, so previewing a *clear* back to the
    /// box centre is expressible: the outer says whether an edit is live, the
    /// inner what it is showing.
    pivot_preview: Option<(NodeId, Option<Pivot>)>,
    /// Whether the document has edits not yet written to `path`.
    dirty: bool,
    /// The revision a write now in flight will put on disk, if one is
    /// (`library::writer`, §15 D393).
    ///
    /// ⚠️ **This is a third state on the save pill and not a second spelling of
    /// `dirty`.** While a write is in flight the document still differs from its
    /// file — the bytes have not landed — so `dirty` stays `true` and the pill's
    /// dot stays amber. What changes is only the sentence: *Saving…* rather than
    /// *Unsaved*, which are the same fact and very different news.
    ///
    /// **The revision is here rather than a `bool` because a write can be
    /// overtaken.** An edit committed while the serialise is in the worker's
    /// hands means the file will hold something the session has already moved
    /// past, and marking it clean on arrival would be a lie the user has no way
    /// to see. [`Self::finish_save`] compares before it believes.
    saving: Option<u64>,
    /// The verb and moment of the last run commit, while the run is still open —
    /// see [`Self::commit_run`].
    run: Option<(&'static str, Instant)>,
    /// When the document last matched what is on disk. `None` for a document
    /// that has never been read from or written to a file — the top bar's save
    /// pill reports that as "Unsaved", because nothing of it exists on disk.
    saved_at: Option<Instant>,
    /// Bumped by every commit — what a cache of something *derived from* the
    /// document keys on to know it has gone stale.
    ///
    /// **Not `dirty`, which is a different question.** That one asks whether the
    /// document differs from the file and goes back to `false` on a save; this one
    /// only ever goes up, because a save changes nothing about what a render of the
    /// artwork would produce. The Export panel's preview is what it exists for
    /// (§7): rendering a layer is far too expensive to do per frame and exactly
    /// cheap enough to do per edit.
    revision: u64,
    status: Status,
}

impl Default for EditorSession {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorSession {
    /// A session over the starter document.
    pub fn new() -> Self {
        let (doc, ids) = starter_document();
        Self::from_parts(doc, ids, None)
    }

    fn from_parts(doc: Document, ids: IdSource, path: Option<PathBuf>) -> Self {
        let resolved = Resolved::rebuild(&doc);
        let saved_at = path.is_some().then(Instant::now);
        Self {
            doc,
            history: History::new(),
            resolved,
            ids,
            selection: Selection::default(),
            camera: Camera::default(),
            path,
            overrides: RenderOverrides::default(),
            session_preview: None,
            preview_holds_gesture: false,
            pivot_preview: None,
            dirty: false,
            saving: None,
            run: None,
            saved_at,
            revision: 0,
            status: Status::default(),
        }
    }

    /// How many commits this session has seen — see the field.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    // --- rendering inputs -------------------------------------------------

    /// All three render inputs at once. Handed out together so a caller holding
    /// `&mut` on a sibling field (the GPU renderer) can still borrow these.
    pub fn render_inputs(&self) -> (&Document, &Resolved, &RenderOverrides) {
        (&self.doc, &self.resolved, &self.overrides)
    }

    #[cfg(test)]
    pub fn has_preview(&self) -> bool {
        !self.overrides.is_empty()
    }

    /// Whether a **gesture** has a preview installed — a drag or a field scrub in
    /// flight, as against a live text session's standing one.
    ///
    /// `cancel_gesture`'s test for "is there anything to unwind", for the half of it
    /// egui cannot be asked. It used to ask `Context::dragged_id`, and that is
    /// unreliable on the one frame it is consulted: egui clears `dragged` on the
    /// release of **any** button, so a right-click whose press and release land in
    /// the same rendered frame — a fast click, or a frame boundary falling between
    /// the two — takes the drag away before the cancel can see it. Reported as
    /// right-click resetting a value "almost always", and confirmed by
    /// `panels::typography::cancel_gate::a_right_click_short_enough_to_fit_one_frame_takes_the_drag_away`.
    /// ⚠️ **`preview_holds_gesture` and not `!preview_holds_session`**, which is
    /// what this read until §15 D609, `[S16.4-L1-03]`. A scrub engaged *over* a live text
    /// session is both at once, and the old spelling answered `false` for it — so
    /// right-clicking to cancel that scrub fell through to `ctx.dragged_id()`,
    /// the one witness the paragraph above says must never stand alone.
    pub fn has_gesture_preview(&self) -> bool {
        !self.overrides.is_empty() && self.preview_holds_gesture
    }

    /// A read-through view of a node that reflects any live gesture.
    ///
    /// Panels must read through this rather than the document, or their
    /// controls snap back to the committed value on every frame of a drag.
    pub fn display_node(&self, id: NodeId) -> Option<DisplayNode<'_>> {
        Some(DisplayNode {
            node: self.doc.get(id)?,
            over: self.overrides.get(id),
        })
    }

    /// [`Self::display_node`]'s twin over the **committed** document, ignoring any
    /// preview.
    ///
    /// For the one caller that must not see its own preview: a *commit* compares the
    /// value it is about to write against the value already there, and reading that
    /// through the override means comparing an edit against a preview of itself —
    /// which is §15 D109's bug, and D155's. Clearing the preview first is the wrong
    /// way to get this: a preview may belong to a **live text session**
    /// ([`Self::set_session_preview`]) rather than to the gesture in hand, and
    /// clearing that takes uncommitted text off the canvas.
    pub fn committed_node(&self, id: NodeId) -> Option<DisplayNode<'_>> {
        Some(DisplayNode {
            node: self.doc.get(id)?,
            over: None,
        })
    }

    /// World transform including any live gesture.
    pub fn preview_world_transform(&self, id: NodeId) -> Option<Affine> {
        self.overrides
            .world_transform(&self.doc, &self.resolved, id)
    }

    /// The node's **local** transform including any live gesture — its own, not
    /// composed through its ancestors.
    ///
    /// [`Self::preview_world_transform`]'s twin, for the one caller that is
    /// building something to be drawn *as a child of the same parent*: the scene
    /// walk composes a ghost's transform under its parent's world, so handing it
    /// a world transform would apply the parent chain twice.
    pub fn preview_local_transform(&self, id: NodeId) -> Option<Affine> {
        self.doc.get(id)?;
        Some(self.overrides.transform_of(&self.doc, id))
    }

    /// Install the artwork explaining the mode that is open, or take it away.
    ///
    /// **Called unconditionally once a frame**, from `OndinApp::draw_canvas`,
    /// which is the whole design: a `set_preview` replaces the overrides
    /// wholesale, so anything that has to outlive a commit either gets
    /// re-established after the input or gets re-established at every call site
    /// that ever touches a preview. `preview_guide` is what the second one looks
    /// like; this is the first.
    pub fn set_chrome_ghost(&mut self, glued: Option<(NodeId, ondin_render::Ghost)>) {
        self.overrides.set_chrome_ghost(glued);
    }

    /// The canvas ground, including a colour being dragged in the picker.
    pub fn preview_canvas_background(&self) -> Color {
        self.overrides
            .canvas_background()
            .unwrap_or_else(|| self.doc.canvas_background())
    }

    /// The shaped text layout including any live edit.
    pub fn preview_text_layout(&self, id: NodeId) -> Option<&TextLayout> {
        self.overrides
            .get(id)
            .and_then(|o| o.text.as_ref())
            .or_else(|| self.resolved.text_layout(id))
    }

    /// World bounds including any live gesture.
    ///
    /// Falls back to the resolved value whenever nothing in the subtree is
    /// overridden — the common case, and the only one that has to be fast.
    /// Otherwise it recomputes, which is why selection outlines and handles
    /// track a drag exactly instead of lagging a frame behind.
    pub fn preview_world_bounds(&self, id: NodeId) -> Option<Rect> {
        if self.overrides.is_empty() {
            return self.resolved.world_bounds(id);
        }
        let node = self.doc.get(id)?;
        let display = self.display_node(id)?;
        let world = self.preview_world_transform(id)?;

        let own = geometry::world_bounds_of_parts(
            display.kind(),
            display.paint(),
            world,
            self.preview_text_layout(id),
        );
        // Containers union their children; leaves have only their own box.
        node.children()
            .iter()
            .filter_map(|c| self.preview_world_bounds(*c))
            .chain(own)
            .reduce(|a, b| a.union(b))
    }

    /// The node's own box in its **local** space, including any live gesture.
    ///
    /// The selection outline and its handles are drawn from this rather than
    /// from [`Self::preview_world_bounds`], so they follow the shape's own
    /// rectangle under rotation instead of the axis-aligned region it covers
    /// (`ondin_core::local_box`).
    pub fn preview_local_box(&self, id: NodeId) -> Option<Rect> {
        if self.overrides.is_empty() {
            return ondin_core::local_box(&self.doc, &self.resolved, id);
        }
        let display = self.display_node(id)?;
        if let Some(b) = geometry::local_bounds(display.kind(), self.preview_text_layout(id)) {
            return Some(b);
        }
        // A boolean's box is its *result's*, and mid-gesture the result is the one
        // the preview re-derived — so the selection outline and the dimensions badge
        // track the shape being reshaped rather than the operands feeding it.
        if let Some(path) = self
            .overrides
            .boolean_path(id)
            .or_else(|| self.resolved.boolean_path(id))
            && !path.is_empty()
        {
            return Some(ondin_core::kurbo::Shape::bounding_box(path));
        }
        // A container: union its children through their own (possibly
        // previewed) transforms, so dragging a child resizes the group's box.
        let node = self.doc.get(id)?;
        let parent_world = self.preview_world_transform(id)?.inverse();
        node.children()
            .iter()
            .filter_map(|c| {
                let b = self.preview_local_box(*c)?;
                let to_here = parent_world * self.preview_world_transform(*c)?;
                Some(geometry::transform_rect(to_here, b))
            })
            .reduce(|a, b| a.union(b))
    }

    /// Where the node's transforms pivot, in its own local space, resolved
    /// against the box a live gesture is showing.
    ///
    /// The pivot never enters `RenderOverrides` — nothing about it renders, and
    /// the gestures that read it bake it into a transform (§15) — so it is carried
    /// beside them in `pivot_preview` instead. The **box** it may be a fraction of
    /// does go through the overrides, which is why this is not simply
    /// `ondin_core::pivot_world`: a group being resized has to resolve its pivot
    /// against the box on screen, or the marker slides out from under the pointer
    /// for the length of the drag.
    ///
    /// **Both halves are previewed, and they are previewed for different gestures.**
    /// A resize moves the box under a pivot that has not changed; the inspector's
    /// origin fields move the pivot inside a box that has not. The canvas marker
    /// drag is the one that previews *neither* through here — it paints itself from
    /// the live pointer (`draw_pivot`), because it has a pointer and this does not.
    pub fn preview_pivot(&self, id: NodeId) -> Option<Point> {
        self.preview_pivot_in(id, self.preview_local_box(id)?)
    }

    /// [`Self::preview_pivot`] against a box the caller already has.
    ///
    /// 🚨 **This exists because `selection_handles` was walking the same subtree
    /// twice in one call** (§15 D764). That function asks `preview_local_box(id)`
    /// for its own `box_` and then asked `preview_pivot(id)`, which asks
    /// `preview_local_box(id)` again — and `previewed_pivot` answers `Some` for any
    /// existing node, so the second walk always happened. With four production call
    /// sites, that is **eight of the fourteen `query::local_box` asks a frame** that
    /// §15 D741 measured, from one function, for one box.
    ///
    /// ⚠️ **D741 says *"the fourteen asks are fourteen different functions … there
    /// is no single caller to hoist"*, and that is false** — this is the hoist, it
    /// needed no cache and no change to `Resolved`, and it is worth 43% of that
    /// entry's number. The arithmetic said so all along: `[S12.1-L4-03]` measured
    /// 0.6431 ms per selection-box build against `[S4.1-L4-08]`'s 0.3490 ms per
    /// `local_box` call — a factor of 1.84, which is two walks. **Two findings
    /// measured the same work and neither divided.**
    ///
    /// ⚠️ **The pivot is a fraction *of* a box, so passing one in is not an
    /// optimisation shortcut** — it is the honest signature. `preview_pivot` is the
    /// convenience for callers that have no box, and it is written in terms of this
    /// so the two cannot disagree about what `pivot_point` is applied to.
    pub fn preview_pivot_in(&self, id: NodeId, box_: Rect) -> Option<Point> {
        Some(geometry::pivot_point(self.previewed_pivot(id)?, box_))
    }

    /// The node's stored pivot, or the one a live edit is showing in its place —
    /// **`None` for "not moved"**, which is the state [`Self::preview_pivot`]
    /// resolves to the box's centre and cannot report back.
    ///
    /// That distinction is the whole reason this is a second reader: the marker's
    /// badge names a *placement*, so it must not be drawn over a layer whose origin
    /// has been dragged or typed back to the middle. Asking the committed node
    /// instead left the badge on for the length of the edit that removed it.
    ///
    /// The outer `Option` is still "is there such a node", as everywhere else here;
    /// the pivot's own absence is the inner value.
    pub fn previewed_pivot(&self, id: NodeId) -> Option<Option<Pivot>> {
        let node = self.doc.get(id)?;
        Some(match self.pivot_preview {
            Some((previewed, pivot)) if previewed == id => pivot,
            _ => node.pivot(),
        })
    }

    // --- mutation ---------------------------------------------------------

    /// Show `tx` applied, without touching history (invariant 5).
    ///
    /// A transaction that cannot be represented as overrides — or that would
    /// not apply — clears the preview instead of leaving the previous frame's
    /// stale one on screen. Nothing the app previews is structural, so in
    /// practice this always succeeds; see `RenderOverrides::from_transaction`.
    /// **The pivot is picked out of the transaction here** rather than in a fork
    /// each caller owes, because the overrides genuinely cannot hold it (see
    /// `pivot_preview`) and a preview that silently drops part of its transaction
    /// is the shape of §15 D245: a field re-reading a base its own preview is not
    /// in scrubs against a value that never moves.
    ///
    /// Last one wins, which is only reachable through a transaction naming two
    /// layers' pivots — nothing authors one, and if something does it wants the
    /// map `guide_previews` is.
    /// 🚨 **A live text session's preview is composed with, not replaced**
    /// (§15 D609, `[S16.4-L1-03]`). The session's `SetText` is uncommitted content the
    /// document does not have (§9.3), so building the overrides from `tx` alone
    /// took the typed text off the canvas the moment an inspector field engaged —
    /// and left it off, because the clear at the other end of the valve had no
    /// session preview left to exempt and `preview_session` runs on change rather
    /// than per frame. This is D109's exemption at the *set* side, where the two
    /// fixes it records were both on the *clear* side.
    ///
    /// **The session's operations go first**, which is the order the two are
    /// authored in: the session's content is the base the field's patch is being
    /// previewed over, and `RenderOverrides::absorb` is last-one-wins per field.
    /// A field that edits the session's own node — the Transform card's X, which
    /// is what the finding was measured on — therefore wins the transform and
    /// leaves the text alone.
    ///
    /// ⚠️ **A canvas gesture cannot reach this branch**: `Mode::TextInsert` owns
    /// the pointer while a session is live, so the composing callers are the three
    /// inspector valves. It is spelled here rather than at them for D109's own
    /// reason — *"add no third spelling"*.
    pub fn set_preview(&mut self, tx: &Transaction) {
        let composed;
        let tx = match &self.session_preview {
            Some(session) => {
                let mut ops = session.0.clone();
                ops.extend(tx.0.iter().cloned());
                composed = Transaction(ops);
                &composed
            }
            None => tx,
        };
        self.overrides =
            RenderOverrides::from_transaction(&self.doc, &self.resolved, tx).unwrap_or_default();
        self.preview_holds_gesture = true;
        self.pivot_preview = tx.0.iter().rev().find_map(|op| match op {
            Operation::SetPivot { id, pivot } => Some((*id, *pivot)),
            _ => None,
        });
    }

    /// Preview the uncommitted state of a **live text-editing session**.
    ///
    /// The same override, marked as belonging to the session rather than to a
    /// gesture — which is what exempts it from the clear [`Self::try_commit`] does.
    /// A drag's preview is stale the moment anything commits; a session's stands for
    /// content and styling that will not be committed until the session ends (§9.3),
    /// so an unrelated commit must not take the live text off the canvas.
    /// `shaped` is the session's own already-shaped layout for the node the
    /// transaction is about — see [`RenderOverrides::from_transaction_shaped`]
    /// (§15 D592). `None` is correct and merely slower; it is what a caller with
    /// no layout in hand passes.
    pub fn set_session_preview(&mut self, tx: &Transaction, shaped: Option<(NodeId, TextLayout)>) {
        self.overrides =
            RenderOverrides::from_transaction_shaped(&self.doc, &self.resolved, tx, shaped)
                .unwrap_or_default();
        self.pivot_preview = tx.0.iter().rev().find_map(|op| match op {
            Operation::SetPivot { id, pivot } => Some((*id, *pivot)),
            _ => None,
        });
        self.session_preview = Some(tx.clone());
        // Any gesture patch that was composed over the previous session preview is
        // gone with it. The valve re-installs its own on the next engaged frame —
        // it previews unconditionally while engaged — so this is at most one
        // frame's worth, and saying `true` here would leave a phantom gesture for
        // `cancel_gesture` to unwind.
        self.preview_holds_gesture = false;
    }

    /// [`Self::clear_preview`] for a **cancelled gesture**, which must leave a live
    /// text session's preview where it is.
    ///
    /// **The exemption `try_commit` already has, at the other place that clears.**
    /// The two previews share one override set plus a flag, so `clear_preview` drops
    /// both — and a right-click cancelling an inspector scrub took the session's
    /// uncommitted *text* off the canvas with it, leaving the canvas showing the last
    /// commit while every panel went on reading the editor. Reported as a paragraph
    /// field's value "resetting on the text but staying in the field": two readings
    /// of one edit, from two sources, after one of them had been thrown away.
    ///
    /// A cancelled gesture has no result to withdraw from the session — the session's
    /// preview is not the gesture's to drop.
    ///
    /// 🚨 **"Leave it where it is" became "put it back", and the difference is the
    /// composed preview** (§15 D609, `[S16.4-L1-03]`). Once [`Self::set_preview`] composes a
    /// gesture over a live session, the override set holds both — so doing nothing
    /// would leave the cancelled or committed field's patch on the canvas for the
    /// rest of the session. Rebuilding from the session's own transaction is the
    /// same answer as before wherever there is no gesture to remove, and the right
    /// one where there is.
    pub fn clear_gesture_preview(&mut self) {
        match self.session_preview.take() {
            // ⚠️ Rebuilt without the session's already-shaped layout, which
            // `set_session_preview` is handed and this does not keep: correct and
            // merely slower (§15 D592), once per gesture rather than once per key.
            Some(session) => self.set_session_preview(&session, None),
            None => self.clear_preview(),
        }
    }

    /// Preview `tx`, drawing `leaving` outside their parent's clip — the layers
    /// this move would take out of the frame they are in
    /// (`RenderOverrides::escape_clip`).
    ///
    /// The caller decides which those are, from the same rule the *release* uses,
    /// so that a shape drawing in full is a promise about what dropping it does.
    pub fn set_preview_escaping(&mut self, tx: &Transaction, leaving: &[NodeId]) {
        self.set_preview(tx);
        for id in leaving {
            self.overrides.escape_clip(*id);
        }
    }

    /// Preview `tx` with its ghosts drawn outside their parent's clip — the
    /// Alt-drag copy, which leaves a frame exactly as a moved layer does.
    pub fn set_preview_escaping_ghosts(&mut self, tx: &Transaction, leaving: bool) {
        self.set_preview(tx);
        if leaving {
            self.overrides.escape_clip_ghosts();
        }
    }

    /// Drop **every** preview, a live session's included — `finish_text_edit`,
    /// `adopt_document`, `fonts_changed` and the end of a canvas drag.
    /// [`Self::clear_gesture_preview`] is the one that spares the session.
    pub fn clear_preview(&mut self) {
        self.overrides = RenderOverrides::default();
        self.session_preview = None;
        self.preview_holds_gesture = false;
        self.pivot_preview = None;
    }

    /// Commit a transaction, reporting failure through the status line instead
    /// of discarding it. Returns whether it applied.
    pub fn commit(&mut self, tx: Transaction) -> bool {
        match self.try_commit(tx) {
            Ok(()) => true,
            Err(e) => {
                self.fail(format!("Edit failed: {e}"));
                false
            }
        }
    }

    /// Commit `tx` as one of a **run** of `verb` edits: consecutive ones fold into
    /// a single undo step (`History::commit_into_run`).
    ///
    /// **The policy lives here and the mechanism lives in core**, which is the
    /// split that matters. History can say when a merge is *sound*; only the app
    /// knows when two edits are one decision, and the answer is about keys and
    /// gestures: the same verb, nothing else committed in between, no pointer press
    /// between them ([`Self::end_edit_run`]), and close enough in time to read as
    /// one burst.
    ///
    /// The window is feel, not correctness — four taps of an arrow key are one
    /// nudge and a tap a minute later is not, and there is no structural difference
    /// between them to appeal to. Held-key repeat is far inside it either way.
    pub fn commit_run(&mut self, tx: Transaction, verb: &'static str) -> bool {
        if tx.0.is_empty() {
            return true;
        }
        let now = Instant::now();
        let open = self
            .run
            .is_some_and(|(v, at)| v == verb && now.duration_since(at) <= RUN_WINDOW);
        if !open {
            self.history.end_run();
        }
        let applied = match self.commit_inner(tx, true) {
            Ok(()) => true,
            Err(e) => {
                self.fail(format!("Edit failed: {e}"));
                false
            }
        };
        self.run = applied.then_some((verb, now));
        applied
    }

    /// End the current run, so the next [`Self::commit_run`] starts an undo step
    /// of its own.
    ///
    /// Called on any pointer press: a click is a new intent, and it is also how the
    /// subject changes — nudge one point, click another, nudge that one, and the
    /// two bursts have to be two steps even though the verb and the node are the
    /// same. Everything else that ends a run does so by simply committing, since a
    /// plain commit ends one.
    pub fn end_edit_run(&mut self) {
        self.run = None;
        self.history.end_run();
    }

    /// Commit and report the error to the caller. Empty transactions are a
    /// no-op success — tools legitimately produce them (a click that turned out
    /// not to be a drag) and they must not create an undo step.
    pub fn try_commit(&mut self, tx: Transaction) -> Result<(), OpError> {
        self.commit_inner(tx, false)
    }

    /// [`Self::try_commit`], with `run` saying whether this edit may be folded into
    /// the step before it.
    fn commit_inner(&mut self, tx: Transaction, run: bool) -> Result<(), OpError> {
        if tx.0.is_empty() {
            return Ok(());
        }
        // **A completed interaction that changed nothing is not an undo step**
        // (§15 D428).
        // The empty test above is not this one and never was: a click into a
        // value field and straight back out, a resize handle pressed and released
        // without moving, a drop that lands rows where they already are, all
        // produce a *full* transaction whose every operation writes back the
        // value that is already there. Committing it dirtied the document, fired
        // autosave and the crash snapshot, and put a step in the history that
        // undoes to the picture it undid from — so `Ctrl+Z` did nothing visible
        // and had to be pressed twice.
        //
        // **Here rather than at the doors**, because there are of the order of a
        // hundred of them and the round that found this counted eleven already
        // broken across three routes — click, type and drag — with three
        // different per-door guards proposed and none of them able to cover the
        // next door. `Transaction::changes_nothing` is asked of the whole
        // transaction, for the reason written on it.
        //
        // ⚠️ **The preview is still dropped.** A no-op commit is the *end of a
        // gesture* even though it is not an edit, and the clear at the bottom of
        // this function is the only thing several callers rely on to take the
        // ghost off the canvas. Returning before it left the preview standing
        // over the committed document — which is the stale-snapshot failure that
        // comment describes, arriving by a new door.
        if tx.changes_nothing(&self.doc) {
            self.clear_gesture_preview();
            return Ok(());
        }
        if !run {
            // A plain commit is a new decision, so it closes any run — which is
            // what keeps an unrelated edit landing between two nudges from letting
            // them merge across it.
            self.run = None;
        }
        let dirty = match run {
            true => self.history.commit_into_run(&mut self.doc, tx)?,
            false => self.history.commit(&mut self.doc, tx)?,
        };
        // **The one place a caught boolean panic becomes something the user sees.**
        // `evaluate` answers `None`, which is also what a correct empty result
        // answers, so nothing downstream can tell them apart and the shape simply
        // vanished (§15 D239). `boolean::failures` moving across this update is the
        // signal, and this is the span to read it over: a commit is the whole of an
        // edit, so one message stands for however many booleans it re-evaluated at
        // whatever depth.
        //
        // **The transient half, and it says so.** `Status` holds one message, so a
        // caller that reports on its own edit afterwards ("Pasted 3 layers") wins
        // this slot — which is the right way round, and is why the durable half is
        // the layers row's warning colour (`Resolved::boolean_failed`) rather than
        // this line. Undo and redo update outside this function and keep their own
        // "Undo": arriving back at a broken boolean is not news about the undo, and
        // the row still wears the mark.
        let failures = ondin_core::boolean::failures();
        self.resolved.update(&self.doc, &dirty);
        if ondin_core::boolean::failures() != failures {
            self.fail("A boolean could not be computed and draws nothing — undo to bring its operands back");
        }
        self.selection.retain_existing(&self.doc);
        self.dirty = true;
        // **A counter, not a hash of the document.** Anything that caches a
        // *rendering* of the document needs to know when it has gone stale, and
        // the only cheap answer is "something was committed since". Here rather
        // than at the call sites for [`Self::dirty`]'s reason — an undo is a
        // commit too, through the same door, so nothing has to remember.
        self.revision = self.revision.wrapping_add(1);
        // **A commit makes any preview stale by definition**, so it goes here
        // rather than at the call sites that remembered to.
        //
        // `edit_valve` always cleared it, and every control that commits *without*
        // going through the valve — a segmented cell, a paragraph field, an axis
        // slider — did not. That is a bug with a long reach, because the panels read
        // the document back through `display_node`, which applies the overrides: a
        // preview left over from an interrupted drag is a **stale snapshot every
        // later write is then built from**, so each new edit silently reverts to it.
        // Reported twice, as "drag to set a value doesn't work" on the paragraph
        // fields and as "after I set a colour the controls become unresponsive — I
        // can slide a slider and it resets immediately to the value it had".
        //
        // A preview outliving its own gesture is easy to arrange: `edit_valve`
        // previews on `dragged()` and clears on `drag_stopped()`, and a widget that
        // stops existing mid-drag — a tab switched, a section collapsed, a row
        // rebuilt under a different id — never delivers the second.
        //
        // **A text session's preview is the exception and is left alone.** It is
        // not a gesture's: it stands for uncommitted content and styling for the
        // whole session (§9.3), so clearing it on an unrelated commit would drop the
        // live text off the canvas until the next keystroke. Spelled once, in
        // [`Self::clear_gesture_preview`] — a cancelled gesture owes the identical
        // exemption and did not have it, which is how the same bug arrived a third
        // time from the other direction.
        self.clear_gesture_preview();
        Ok(())
    }

    /// Take the last transaction back. **Answers whether the document actually
    /// moved** — `false` for the end of the history and for a failure.
    ///
    /// The `bool` exists for the caller, not for this function: state that
    /// *indexes into* the document lives on `OndinApp` and not here, so the app
    /// has to be told when the ground under it has shifted. `Selection` is the
    /// half that lives on the session and is guarded a line below;
    /// `OndinApp::points` and `OndinApp::pen` are the half that is not, and
    /// [`crate::app::OndinApp::undo`] is where they are dropped (§15 D568).
    ///
    /// **Not "an undo happened"** — pressing `Ctrl+Z` at the bottom of the stack
    /// must not deselect anything, so the answer is about the document rather
    /// than about the keystroke.
    pub fn undo(&mut self) -> bool {
        // An undo is never part of a run — the burst it would merge into is the
        // thing being taken back. `History` says so too; this is the app-side half
        // of the same fact.
        self.run = None;
        match self.history.undo(&mut self.doc) {
            Ok(Some(dirty)) => {
                self.resolved.update(&self.doc, &dirty);
                self.selection.retain_existing(&self.doc);
                self.dirty = true;
                self.info("Undo");
                true
            }
            Ok(None) => {
                self.info("Nothing to undo");
                false
            }
            Err(e) => {
                self.fail(format!("Undo failed: {e}"));
                false
            }
        }
    }

    /// Put back what [`Self::undo`] took, and answer the same question it does.
    pub fn redo(&mut self) -> bool {
        self.run = None;
        match self.history.redo(&mut self.doc) {
            Ok(Some(dirty)) => {
                self.resolved.update(&self.doc, &dirty);
                self.selection.retain_existing(&self.doc);
                self.dirty = true;
                self.info("Redo");
                true
            }
            Ok(None) => {
                self.info("Nothing to redo");
                false
            }
            Err(e) => {
                self.fail(format!("Redo failed: {e}"));
                false
            }
        }
    }

    /// Replace the open document (open / new). Resets history, selection and
    /// save state, and re-seeds the id source so subsequent creates cannot
    /// collide with ids the loaded file already uses.
    pub fn adopt_document(&mut self, doc: Document, path: Option<PathBuf>) {
        self.resolved = Resolved::rebuild(&doc);
        self.doc = doc;
        self.ids = IdSource::new(random_actor());
        ondin_core::reserve_existing_ids(&self.doc, &mut self.ids);
        self.history = History::new();
        self.selection.clear();
        self.clear_preview();
        self.run = None;
        // A just-opened document matches its file, so it counts as saved now.
        self.saved_at = path.is_some().then(Instant::now);
        self.path = path;
        self.dirty = false;
        // A different document entirely, so anything derived from the old one is
        // stale — the strongest reason there is to bump this.
        self.revision = self.revision.wrapping_add(1);
    }

    /// Re-shape every text node and refresh dependent bounds. Called when the
    /// font service registers a family that was previously falling back, so
    /// bounds, hit-testing and selection rectangles stop describing the wrong
    /// face (§5.4a).
    ///
    /// Any in-flight preview is dropped rather than re-shaped: a font landing
    /// mid-gesture is rare, and rebuilding the overrides would need the pending
    /// transaction, which the session does not keep for a *gesture*.
    ///
    /// ⚠️ It does keep a live text session's ([`Self::session_preview`], since
    /// §15 D609, `[S16.4-L1-03]`), so this is now a deliberate `clear_preview` rather than a
    /// forced one: re-shaping the session's text against the newly registered
    /// family is what `preview_session` does on the next keystroke, and the
    /// alternative — rebuilding here — would re-shape the whole node on a frame
    /// that is already re-shaping every text node in the document.
    pub fn fonts_changed(&mut self) {
        self.resolved.invalidate_text(&self.doc);
        self.clear_preview();
    }

    // --- save state -------------------------------------------------------

    /// Whether there are edits not yet written to disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Record that this document is known **not** to match its file, without an
    /// edit having been made.
    ///
    /// ⚠️ **Deliberately one-way**, which is why it takes no argument and cannot
    /// clear the flag: [`Self::mark_saved`] is the only road back to clean, as it
    /// always was. Setting `dirty` is not the whole of it — `saved_at` goes too,
    /// or the top bar's pill would say "Saved · just now" over a document that
    /// differs from its file, which is the one sentence it must never be able to
    /// say (see [`Self::saved_ago`]).
    ///
    /// **Two callers, and they are the same claim from opposite ends.** A
    /// document restored from a crash snapshot (`library::recovery`) has never
    /// been written anywhere — the snapshot is not the file — so it is unsaved by
    /// construction, and the app must not be able to think otherwise. And the
    /// autosave tests, whose rules are all of the form "given a dirty document,
    /// does the interval do X": reaching that state through a real edit would
    /// drag a whole fixture document into a test that is about a timer. This was
    /// `mark_dirty_for_test` and `#[cfg(test)]` until recovery gave it a
    /// production caller.
    pub fn mark_unsaved(&mut self) {
        self.dirty = true;
        self.saved_at = None;
    }

    /// Record that the current document state is what is on disk.
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
        self.saved_at = Some(Instant::now());
        // A synchronous save has just made whatever was in flight irrelevant —
        // it wrote the same path, later, from the same session. `save_file`
        // settles before it writes, so in practice there is nothing here to
        // clear; clearing it anyway is what stops the pill sticking on *Saving…*
        // if that order is ever changed.
        self.saving = None;
    }

    /// A write of `at` has been queued: the pill says *Saving…* until it lands
    /// (`library::writer`, §15 D393).
    pub fn begin_save(&mut self, at: u64) {
        self.saving = Some(at);
    }

    /// Whether a queued write is still in flight — the pill's third state.
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// A queued write is over and did not land.
    ///
    /// ⚠️ **Only `saving` clears, and it must.** The session stays dirty, which is
    /// the truth and is what makes the next interval try again — but without this
    /// the pill sticks on *Saving…* for the rest of the session and
    /// `OndinApp::autosave_tick`'s `is_saving` gate never opens again, so one
    /// failed write turns autosave off silently. That is exactly the state this
    /// whole feature exists to avoid, arrived at from the other side, and it is
    /// the shape of bug a third state invites: a state entered in one place and
    /// left in only one of two.
    pub fn save_failed(&mut self) {
        self.saving = None;
    }

    /// A queued write of revision `at` has reached `path`.
    ///
    /// ⚠️ **Three guards, and each answers a different way of being stale.** The
    /// first is `saving == Some(at)`, in the body below with its own note: it
    /// says this result is about the write currently being waited on. The
    /// revision says the session has not moved on since the clone was taken — an
    /// edit during the serialise means the file is already behind, and calling it
    /// clean would put *Saved · just now* over work that is on disk nowhere. The
    /// path says this is still the same document — `open_path` and `recover`
    /// settle before they adopt, so a mismatch should be unreachable, and the
    /// consequence if it ever is would be marking a *new* document clean because
    /// the *old* one was written.
    ///
    /// Either way `saving` clears: the write is over, whatever it turned out to
    /// be worth. A superseded one leaves the session dirty and the next interval
    /// picks the new revision up.
    ///
    /// Returns whether the session went clean, which is the one thing a caller
    /// wants to key a side effect on — `export_on_save` re-exports after a save
    /// that *counted*, and a superseded write is not one.
    pub fn finish_save(&mut self, path: &std::path::Path, at: u64) -> bool {
        // ⚠️ **Cleared only for the write it is actually about.** A `take()` here
        // would let an older result end the *Saving…* of a newer write still in
        // flight, which is the pill going quiet while the disk is still busy.
        if self.saving != Some(at) {
            return false;
        }
        self.saving = None;
        if self.revision == at && self.path.as_deref() == Some(path) {
            self.dirty = false;
            self.saved_at = Some(Instant::now());
            return true;
        }
        false
    }

    /// How long ago the document last matched its file, or `None` when it never
    /// has (never saved) or no longer does (unsaved edits). The top bar's pill
    /// renders exactly this distinction: `None` is "Unsaved".
    pub fn saved_ago(&self) -> Option<std::time::Duration> {
        if self.dirty {
            return None;
        }
        self.saved_at.map(|t| t.elapsed())
    }

    /// What to call the open document — the name the user typed, not the file.
    ///
    /// ⚠️ **The stem is a slug now, so it cannot be the answer on its own.**
    /// This read `path.file_stem()` until the library landed, which was right
    /// while a filename was whatever somebody typed into a save dialog and is
    /// wrong the moment it is derived: the top bar would say `landing-v4` for a
    /// document called "Landing v4". `library::scan::display_name` is the shared
    /// rule, and it is shared precisely so the library and the title bar cannot
    /// disagree about one document.
    pub fn document_title(&self) -> String {
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match crate::library::scan::display_name(self.doc.meta(), &stem) {
            // Both empty is a document that has never been saved and never been
            // named — the starter document, which is what "Untitled" is for.
            name if name.trim().is_empty() => "Untitled".to_string(),
            name => name,
        }
    }

    // --- status -----------------------------------------------------------

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Status {
            text: text.into(),
            kind: StatusKind::Info,
        };
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        self.status = Status {
            text: text.into(),
            kind: StatusKind::Error,
        };
    }
}

/// The canvas ground a document starts on, and the one the Appearance panel's
/// reset button returns to: the design's near-black `#171718`.
///
/// Lives in core, because it is also what a file written before the ground was
/// saved means (`io::schema`). Re-exported here so the panels can keep asking
/// the session's own module for it.
pub use ondin_core::DEFAULT_CANVAS_BACKGROUND;

/// A per-process-random actor id (§5.2, invariant 3).
///
/// Core deliberately has no RNG policy, so the app supplies one. `RandomState`
/// is seeded by the OS once per process and perturbed per instance, and the
/// clock breaks ties between processes started from the same image — together
/// far more than enough for two sessions never to pick the same actor, which is
/// what keeps ids from colliding across files and (later) across peers.
fn random_actor() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    // Never 0: a zero actor reads like "unset" in the wire form.
    h.finish() | 1
}

/// A fresh document: one artboard with a couple of shapes so the canvas is not
/// empty on first launch.
fn starter_document() -> (Document, IdSource) {
    let mut ids = IdSource::new(random_actor());
    let root = ids.mint();
    let mut doc = Document::new(root);

    let artboard = ids.mint();
    let rect = ids.mint();
    let ellipse = ids.mint();
    let text = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: artboard,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(800.0, 600.0),
            },
            transform: None,
            // `None`, so it is numbered like any other frame — "Frame 1". It was
            // `Some("Frame")`, which was right when a fresh frame was called plainly
            // "Frame" and became the one frame in the app whose name did not match
            // what the tool makes (§15 D127). Still not "Artboard 1": D21's rule is
            // about the *word*, and the number was never the part it objected to.
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: artboard,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(200.0, 120.0),
                corner_radii: RoundedRectRadii::from_single_radius(12.0),
            },
            transform: Some(Affine::translate((80.0, 80.0))),
            name: None,
        },
        Operation::CreateNode {
            id: ellipse,
            parent: artboard,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(160.0, 160.0),
            },
            transform: Some(Affine::translate((360.0, 220.0))),
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: artboard,
            index: 2,
            kind: NodeKind::Text {
                content: "Ondin".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 72.0,
                    weight: 700,
                    line_height: Some(Length::Em(1.2)),
                    ..Default::default()
                }),
                spans: CharSpans::default(),
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((80.0, 420.0))),
            // `None`, so it is numbered like any other text layer — "Text 1" —
            // which is the same correction the artboard above carries and for the
            // same reason (§15 D127). It was `Some("Heading")`, and however well
            // that described 72pt bold type it made this the one layer in a fresh
            // document whose name the tool would never have written.
            name: None,
        },
    ]))
    .expect("seed starter document");

    doc.apply(&Transaction(vec![
        // **The frame's white ground, and it is a fill now like the two below it**
        // (§15 D400) — it used to ride in on `CreateNode` as a field of the kind,
        // which is why it was the one paint in this document not written here.
        Operation::SetFills {
            id: artboard,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 255, 255, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: rect,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(70, 130, 220, 255)),
                visible: true,
            }],
        },
        Operation::SetFills {
            id: ellipse,
            fills: vec![Fill {
                brush: Brush::Solid(Color::from_rgba8(235, 110, 90, 255)),
                visible: true,
            }],
        },
    ]))
    .expect("paint starter shapes");

    (doc, ids)
}

#[cfg(test)]
mod tests {
    //! These exist because the session used to be welded to a `wgpu` device,
    //! which is why none of the bugs below were caught by the test suite.

    use super::*;

    fn first_artboard(s: &EditorSession) -> NodeId {
        s.doc.get(s.doc.root()).unwrap().children()[0]
    }

    fn add_rect(s: &mut EditorSession) -> NodeId {
        let id = s.ids.mint();
        let parent = first_artboard(s);
        let index = s.doc.get(parent).unwrap().children().len();
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        }])));
        id
    }

    #[test]
    fn creating_after_adopting_a_document_does_not_collide() {
        // The open-a-file bug: the id source used to survive `replace_document`
        // while the document under it was swapped, so the next create re-minted
        // an id the file already contained and silently failed.
        let mut s = EditorSession::new();
        for _ in 0..5 {
            add_rect(&mut s);
        }
        let bytes = ondin_core::io::save(&s.doc).unwrap();
        let reopened = ondin_core::io::load(&bytes).unwrap();

        s.adopt_document(reopened, None);
        for _ in 0..10 {
            add_rect(&mut s);
        }
        assert_eq!(s.status().kind, StatusKind::Info, "{:?}", s.status());
    }

    #[test]
    fn a_failed_commit_is_reported_not_swallowed() {
        let mut s = EditorSession::new();
        let ghost = NodeId {
            actor: 0xDEAD,
            seq: 1,
        };
        assert!(!s.commit(Transaction(vec![Operation::SetVisible {
            id: ghost,
            visible: false,
        }])));
        assert_eq!(s.status().kind, StatusKind::Error);
        assert!(!s.is_dirty(), "a failed edit must not dirty the document");
    }

    /// **G16's loss, end to end**: a completed interaction that changed nothing
    /// used to leave a document that was dirty, an undo step that undoes to the
    /// picture it undid from, and — through `is_dirty` — an autosave and a crash
    /// snapshot written for it.
    ///
    /// The transaction here is a *full* one, not an empty one, which is why
    /// `empty_transactions_are_a_no_op` below did not already cover this: the
    /// review found eleven doors emitting exactly this shape across three routes
    /// (click, type and drag), and the empty test at the commit seam let every
    /// one of them through.
    ///
    /// ⚠️ **The preview is still dropped, and that assertion is not decoration.**
    /// Several callers rely on the commit to take a ghost off the canvas, so a
    /// guard that returned before the clear would trade an undo step for a
    /// gesture preview standing over the committed document — the stale-snapshot
    /// failure `commit_inner` is already annotated for, arriving by a new door.
    ///
    /// Flip-checked by disabling the `changes_nothing` guard at `commit_inner`:
    /// predicted to fail on the undo-depth assertion, and it did, `left: 2
    /// right: 1`. ⚠️ **It said `can_undo` first and that assertion was vacuous** —
    /// the setup's own `add_rect` is an undo step, so `!can_undo` was false
    /// before the interesting commit ever ran and the test failed against the
    /// *fixture*. A depth taken before and compared after is what asks the
    /// question; `can_undo` could only ever have asked it on a session with no
    /// history at all, which is not the session anybody edits in.
    #[test]
    fn an_edit_that_writes_back_what_is_already_there_is_not_an_undo_step() {
        let mut s = EditorSession::new();
        let id = add_rect(&mut s);
        // Saved *after* the rect, so `dirty` below is about this commit alone and
        // the undo depth to beat is the one the setup legitimately left.
        s.mark_saved(PathBuf::from("x.ondin"));
        let (depth, revision) = (s.history.undo_depth(), s.revision);

        let already = Transaction(vec![Operation::SetVisible { id, visible: true }]);
        assert!(
            !already.0.is_empty(),
            "this is a full transaction, not an empty one"
        );
        s.set_preview(&already);
        assert!(
            s.has_gesture_preview(),
            "the fixture is in the state under test"
        );

        assert!(s.commit(already), "a no-op commit reports success");
        assert_eq!(
            s.history.undo_depth(),
            depth,
            "and must not create an undo step"
        );
        assert!(
            !s.is_dirty(),
            "nor dirty the document, which is what autosaves"
        );
        assert_eq!(s.revision, revision, "nor stale any cached rendering");
        assert!(
            !s.has_gesture_preview(),
            "but it does end the gesture — the preview must not outlive it"
        );
    }

    #[test]
    fn empty_transactions_are_a_no_op() {
        let mut s = EditorSession::new();
        assert!(s.commit(Transaction(Vec::new())));
        assert!(!s.is_dirty());
        assert!(!s.history.can_undo(), "must not create an undo step");
    }

    #[test]
    fn dirty_tracks_edits_and_saves() {
        let mut s = EditorSession::new();
        assert!(!s.is_dirty(), "a fresh session is clean");

        add_rect(&mut s);
        assert!(s.is_dirty());

        s.mark_saved(PathBuf::from("x.ondin"));
        assert!(!s.is_dirty());

        // Undo is itself a change relative to what is on disk.
        s.undo();
        assert!(s.is_dirty());
    }

    #[test]
    fn saved_ago_is_none_until_there_is_a_file_and_none_again_once_dirty() {
        let mut s = EditorSession::new();
        assert!(
            s.saved_ago().is_none(),
            "a document with no file has never been saved"
        );

        s.mark_saved(PathBuf::from("x.ondin"));
        assert!(s.saved_ago().is_some());

        add_rect(&mut s);
        assert!(
            s.saved_ago().is_none(),
            "unsaved edits mean there is no 'saved N ago' to report"
        );
    }

    #[test]
    fn selection_is_pruned_when_nodes_disappear() {
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        s.selection.set_one(r);
        assert!(s.commit(Transaction(vec![Operation::DeleteNode { id: r }])));
        assert!(s.selection.is_empty(), "deleted node must leave selection");

        // Undo brings it back, but selection stays cleared rather than holding
        // a stale id — re-selecting is the caller's business.
        s.undo();
        assert!(s.doc.contains(r));
    }

    #[test]
    fn preview_never_leaves_a_stale_frame_on_screen() {
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        s.set_preview(&Transaction(vec![Operation::SetOpacity {
            id: r,
            opacity: 0.5,
        }]));
        assert!(s.has_preview());

        // An invalid follow-up must drop the preview, not keep showing the old
        // one as if the gesture were still valid.
        s.set_preview(&Transaction(vec![Operation::SetOpacity {
            id: r,
            opacity: 9.0,
        }]));
        assert!(!s.has_preview());
    }

    #[test]
    fn preview_does_not_touch_the_committed_document_or_history() {
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        let depth = s.history.undo_depth();
        let before = s.doc.clone();

        s.set_preview(&Transaction(vec![Operation::SetOpacity {
            id: r,
            opacity: 0.25,
        }]));
        assert_eq!(s.doc, before, "invariant 5: preview stays out of the model");
        assert_eq!(s.history.undo_depth(), depth);
        assert_eq!(
            s.display_node(r).unwrap().opacity(),
            0.25,
            "the preview is what gets displayed"
        );
        assert_eq!(
            s.doc.get(r).unwrap().opacity(),
            1.0,
            "while the document is untouched"
        );

        s.clear_preview();
        assert_eq!(s.display_node(r).unwrap().opacity(), 1.0);
    }

    #[test]
    fn preview_bounds_track_a_move_without_touching_the_document() {
        // What selection outlines and transform handles read during a drag.
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        let before = s.preview_world_bounds(r).expect("bounds");

        let world = s.resolved.world_transform(r).unwrap();
        s.set_preview(&Transaction(vec![Operation::SetTransform {
            id: r,
            transform: Affine::translate((40.0, 25.0)) * world,
        }]));

        let after = s.preview_world_bounds(r).expect("preview bounds");
        assert!(
            (after.min_x() - before.min_x() - 40.0).abs() < 1e-9,
            "{after:?}"
        );
        assert!(
            (after.min_y() - before.min_y() - 25.0).abs() < 1e-9,
            "{after:?}"
        );
        assert_eq!(
            s.resolved.world_bounds(r),
            Some(before),
            "the resolved layer must not have moved"
        );
    }

    #[test]
    fn a_move_preview_carries_descendants() {
        use ondin_core::build;

        let mut s = EditorSession::new();
        let artboard = first_artboard(&s);
        let before = s.preview_world_bounds(artboard).expect("bounds");
        let child = s.doc.get(artboard).unwrap().children()[0];
        let child_before = s.preview_world_bounds(child).expect("child bounds");

        let tx = build::move_by_world(
            &s.doc,
            &s.resolved,
            &[artboard],
            ondin_core::kurbo::Vec2::new(15.0, 0.0),
        )
        .unwrap();
        s.set_preview(&tx);

        let after = s.preview_world_bounds(artboard).unwrap();
        let child_after = s.preview_world_bounds(child).unwrap();
        assert!((after.min_x() - before.min_x() - 15.0).abs() < 1e-9);
        assert!(
            (child_after.min_x() - child_before.min_x() - 15.0).abs() < 1e-9,
            "children must follow the moved ancestor"
        );
    }

    /// **The origin readout has to follow a live preview, or its field cannot be
    /// scrubbed** — §15 D245's shape, in the one place the overrides genuinely
    /// cannot carry the value.
    ///
    /// A scrubbed field re-reads its base every frame and adds that frame's delta.
    /// `edit_valve` only *previews* while the pointer is down, so a base taken from
    /// the committed node resets to the original on every frame: the number never
    /// moves, the marker never moves, and typing works — which is exactly the report
    /// D245 came in as.
    /// **The two spellings agree, and `preview_pivot` always walked** (§15 D764).
    ///
    /// `preview_pivot_in` exists so a caller holding the box does not re-walk the
    /// subtree for it; the value must be identical or the eight asks it removes
    /// were not the same work. The second assertion is the one that says the
    /// duplication was unconditional rather than occasional: `previewed_pivot`
    /// answers `Some` for any node that exists, so the `?` in `preview_pivot` never
    /// short-circuits ahead of the box walk, and `selection_handles`' four call
    /// sites each paid for two.
    ///
    /// **Flip:** making `preview_pivot_in` ignore its argument and call
    /// `preview_local_box` itself keeps the first assertion green — it is the same
    /// box — and the point of the second is that nothing else here would notice.
    /// The saving is established by reading the call site, which this names.
    #[test]
    fn the_pivot_against_a_box_the_caller_has_is_the_pivot() {
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        let box_ = s.preview_local_box(r).expect("box");
        assert_eq!(
            s.preview_pivot_in(r, box_),
            s.preview_pivot(r),
            "the two spellings must answer alike or D764 removed different work"
        );
        assert!(
            s.previewed_pivot(r).is_some(),
            "an untouched node still answers `Some`, which is why `preview_pivot` \
             reached the box walk every time rather than sometimes"
        );
    }

    #[test]
    fn the_origin_readout_follows_a_live_preview_rather_than_the_committed_node() {
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        let box_ = s.preview_local_box(r).expect("box");
        assert_eq!(
            s.preview_pivot(r),
            Some(box_.center()),
            "an untouched layer pivots on its centre"
        );

        // What one frame of a scrub commits through the valve.
        let placed = ondin_core::geometry::pivot_placed_at(
            &NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
            box_,
            Point::new(box_.x0, box_.y0),
        );
        assert!(placed.is_some(), "the top-left corner is a placement");
        s.set_preview(&Transaction(vec![Operation::SetPivot {
            id: r,
            pivot: placed,
        }]));

        assert_eq!(
            s.preview_pivot(r),
            Some(Point::new(box_.x0, box_.y0)),
            "the field would re-read the centre and scrub against a base that never moves"
        );
        assert_eq!(s.previewed_pivot(r), Some(placed), "the badge reads this");
        assert_eq!(
            s.doc.get(r).unwrap().pivot(),
            None,
            "and the document must not have been touched"
        );

        // Previewing the *clear* is expressible too — the state a 50/50 asks for.
        s.set_preview(&Transaction(vec![Operation::SetPivot {
            id: r,
            pivot: None,
        }]));
        assert_eq!(s.previewed_pivot(r), Some(None));
        s.clear_preview();
        assert_eq!(s.previewed_pivot(r), Some(None), "back to the document");
    }

    #[test]
    fn structural_transactions_are_not_previewed() {
        // Better no preview than one that disagrees with the pending edit.
        let mut s = EditorSession::new();
        let r = add_rect(&mut s);
        s.set_preview(&Transaction(vec![Operation::DeleteNode { id: r }]));
        assert!(!s.has_preview());
    }

    #[test]
    fn two_sessions_pick_different_actors() {
        // Cross-session id collisions are what a fixed actor id caused; a random
        // one is the property multiplayer will also need (§5.2).
        let a = EditorSession::new();
        let b = EditorSession::new();
        assert_ne!(a.ids.actor(), b.ids.actor());
        assert_ne!(a.ids.actor(), 0);
    }

    fn add_guide(s: &mut EditorSession, position: f64) -> ondin_core::Guide {
        let guide = ondin_core::Guide {
            id: ondin_core::GuideId(s.ids.mint()),
            axis: ondin_core::GuideAxis::Horizontal,
            position,
            color: None,
            owner: None,
        };
        assert!(s.commit(Transaction(vec![Operation::AddGuide { guide }])));
        guide
    }

    /// Guides and layers are never selected together, whichever way round the
    /// two arrive. Every mutator has to clear the other half, and this is the
    /// only place that says so — the inspector, Delete and the canvas all read
    /// the pair as "one kind or the other" and would each be subtly wrong if a
    /// mixed selection could exist.
    #[test]
    fn selecting_a_guide_and_selecting_layers_are_exclusive() {
        let mut s = EditorSession::new();
        let rect = add_rect(&mut s);
        let guide = add_guide(&mut s, 40.0);

        s.selection.set_one(rect);
        s.selection.set_guide(guide.id);
        assert_eq!(s.selection.guides(), [guide.id]);
        assert!(s.selection.ids().is_empty(), "the layer must have let go");

        // And back the other way, through each of the four node mutators.
        for select in [
            Selection::set_one,
            Selection::add,
            Selection::toggle,
            |sel: &mut Selection, id| sel.set(vec![id]),
        ] {
            s.selection.set_guide(guide.id);
            select(&mut s.selection, rect);
            assert!(
                s.selection.guides().is_empty(),
                "the guide must have let go"
            );
        }

        // The two emptiness questions are different, and the canvas colour
        // picker's orphan rule turns on which one it asks: with a guide
        // selected there are no *layers*, but the inspector does have a
        // subject, so the picker the empty inspector opened has to close.
        s.selection.set_guide(guide.id);
        assert!(s.selection.is_empty(), "no layers are selected");
        assert!(!s.selection.nothing(), "but something is");
        s.selection.clear();
        assert!(s.selection.nothing());

        // Deleting the guide prunes it from the selection, as deleting a node
        // does — `retain_existing` runs on every commit and undo.
        s.selection.set_guide(guide.id);
        assert!(s.commit(Transaction(vec![Operation::RemoveGuide { id: guide.id }])));
        assert!(s.selection.guides().is_empty());
        // ...and undo does not silently bring the selection back with it.
        s.undo();
        assert!(s.doc.guide(guide.id).is_some());
        assert!(s.selection.guides().is_empty());
    }

    /// Several guides at once, by Shift+click — and the layer half of the
    /// selection stays empty through all of it.
    ///
    /// **The invariant the widening could have broken**: `is_empty`, `len` and
    /// `single` answer about layers, so *two* selected guides must still read as
    /// no layers, not as two of them. Every caller gating node work asks one of
    /// those three, and a guide passing that gate would be handed to `align`,
    /// `nudge` or `outermost` as though it were a node id.
    #[test]
    fn guides_multi_select_without_ever_reading_as_layers() {
        let mut s = EditorSession::new();
        let rect = add_rect(&mut s);
        let (a, b, c) = (
            add_guide(&mut s, 10.0),
            add_guide(&mut s, 20.0),
            add_guide(&mut s, 30.0),
        );

        s.selection.set_guide(a.id);
        s.selection.toggle_guide(b.id);
        assert_eq!(s.selection.guides(), [a.id, b.id]);
        assert!(s.selection.has_guide(a.id) && s.selection.has_guide(b.id));
        assert!(!s.selection.has_guide(c.id));

        // The layer half is untouched, however many guides are held.
        assert!(s.selection.is_empty(), "two guides are still no layers");
        assert_eq!(s.selection.len(), 0);
        assert_eq!(s.selection.single(), None);
        assert!(!s.selection.nothing());

        // Toggling the same one again takes it back out — the way back from a
        // three-guide selection without starting over.
        s.selection.toggle_guide(b.id);
        assert_eq!(s.selection.guides(), [a.id]);

        // `set_guide` replaces the whole set, not just the layers.
        s.selection.toggle_guide(b.id);
        s.selection.toggle_guide(c.id);
        s.selection.set_guide(c.id);
        assert_eq!(s.selection.guides(), [c.id]);

        // A Shift+click on a guide drops a layer selection, and vice versa.
        s.selection.set_one(rect);
        s.selection.toggle_guide(a.id);
        assert!(s.selection.ids().is_empty());
        assert_eq!(s.selection.guides(), [a.id]);

        // Pruning keeps the survivors rather than clearing the set: removing one
        // of two guides must not deselect the other.
        s.selection.toggle_guide(b.id);
        assert!(s.commit(Transaction(vec![Operation::RemoveGuide { id: a.id }])));
        assert_eq!(s.selection.guides(), [b.id]);
    }

    /// *View ▸ Lock guides* has to let go of any guide it is holding — locked means
    /// unselectable, and a guide left selected would be *selected but
    /// unselectable*. It has nothing to say about **layers**.
    ///
    /// The distinction is easy to lose to the exclusivity invariant: because a
    /// guide selection and a layer selection cannot coexist, "there is only ever
    /// one of the two to drop" reads as though clearing everything were harmless.
    /// It is not — with layers selected there are no guides to drop, so clearing
    /// achieved nothing except deselecting the user's work.
    #[test]
    fn clearing_guides_leaves_a_layer_selection_alone() {
        let mut s = EditorSession::new();
        let (rect, other) = (add_rect(&mut s), add_rect(&mut s));
        let guide = add_guide(&mut s, 40.0);

        // Holding guides: they go.
        s.selection.set_guide(guide.id);
        s.selection.clear_guides();
        assert!(s.selection.guides().is_empty());

        // Holding layers: they stay, key included.
        s.selection.set(vec![rect, other]);
        s.selection.set_key(other);
        s.selection.clear_guides();
        assert_eq!(s.selection.ids(), [rect, other], "the layers must survive");
        assert_eq!(s.selection.key(), Some(other), "and so must the key layer");
    }

    // --- the key layer ----------------------------------------------------

    /// The designation is explicit and reversible by the same gesture, which is
    /// the only way back to aligning against the selection's union without
    /// dropping the selection and rebuilding it.
    #[test]
    fn designating_the_key_twice_clears_it() {
        let mut s = EditorSession::new();
        let (a, b) = (add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        assert_eq!(s.selection.key(), None, "no key is the ordinary state");

        s.selection.set_key(b);
        assert_eq!(s.selection.key(), Some(b));
        s.selection.set_key(b);
        assert_eq!(s.selection.key(), None, "the same click undesignates");
    }

    /// A key has to be one *of* the selection. A key outside it would align the
    /// others to a layer nothing shows is involved.
    #[test]
    fn a_layer_outside_the_selection_cannot_be_made_key() {
        let mut s = EditorSession::new();
        let (a, b, outsider) = (add_rect(&mut s), add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(outsider);
        assert_eq!(s.selection.key(), None);
    }

    /// **A key means nothing with nothing to contrast against.** Below two layers
    /// `key()` answers `None` however the selection got there, so align falls back
    /// to the container and the canvas has nothing to mark — one rule, in one
    /// place, rather than three consumers each remembering it.
    #[test]
    fn a_key_is_not_reported_for_a_selection_of_one() {
        let mut s = EditorSession::new();
        let (a, b) = (add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(b);

        s.selection.toggle(a);
        assert_eq!(s.selection.len(), 1);
        assert_eq!(s.selection.key(), None, "one layer, so no key");

        // ...but the designation survives, so growing the selection back does not
        // silently lose it.
        s.selection.toggle(a);
        assert_eq!(s.selection.key(), Some(b));
    }

    /// Deselecting the key gives it up. Keeping it would leave a key outside the
    /// selection — the state `set_key` refuses to create in the first place.
    #[test]
    fn deselecting_the_key_drops_the_designation() {
        let mut s = EditorSession::new();
        let (a, b, c) = (add_rect(&mut s), add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b, c]);
        s.selection.set_key(b);

        s.selection.toggle(b);
        assert_eq!(s.selection.key(), None);
        // And it does not come back when b is re-selected: the designation was
        // given up, not parked.
        s.selection.toggle(b);
        assert_eq!(s.selection.key(), None);
    }

    /// Replacing the selection replaces the key with it — including the marquee,
    /// which goes through `set` and whose ordering is arbitrary anyway.
    #[test]
    fn replacing_the_selection_clears_the_key() {
        let mut s = EditorSession::new();
        let (a, b, c) = (add_rect(&mut s), add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(b);

        s.selection.set(vec![b, c]);
        assert_eq!(s.selection.key(), None, "a new selection is a new question");

        s.selection.set(vec![a, b]);
        s.selection.set_key(b);
        s.selection.set_one(a);
        assert_eq!(s.selection.key(), None);
    }

    /// A deleted key is pruned with the rest of the selection, so an undo/redo
    /// cycle cannot leave a designation pointing at a node that is gone.
    #[test]
    fn a_deleted_key_is_pruned() {
        let mut s = EditorSession::new();
        let (a, b) = (add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(b);

        assert!(s.commit(Transaction(vec![Operation::DeleteNode { id: b }])));
        s.selection.retain_existing(&s.doc.clone());
        assert_eq!(s.selection.key(), None);
    }
}
