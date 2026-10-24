//! `EditorSession` — everything an editing session is, with no GPU in it (§9.2).
//!
//! Owns the document, history, resolved layer, id source, selection, camera and
//! save state, and is the only place they are mutated. Free of `wgpu`/`egui`
//! types so it can be unit-tested without a graphics device, `ondin serve` (§8.2)
//! can run it without a window, and MCP handlers (§8.3) can submit transactions
//! through [`EditorSession::commit`], the path the UI uses.
//!
//! Rendering lives in `canvas.rs`, which borrows `render_inputs()`.

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
    Pivot, Resolved, Transaction, geometry, keyed_by_position,
};
use ondin_render::{NodeOverride, RenderOverrides};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a run of same-verb edits stays open between two of them
/// ([`EditorSession::commit_run`]).
const RUN_WINDOW: Duration = Duration::from_millis(600);

/// What the status line is reporting. Errors are kept distinct so the chrome
/// can colour them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    /// When it was written: the floating status bar shows a message for a few
    /// seconds from here. Writing the same sentence again restarts the clock.
    pub at: Instant,
    /// The frame it was written in (`EditorSession::next_status_pass`).
    pass: u64,
}

impl Default for Status {
    /// Empty: the status line reports what just happened, and at startup nothing has.
    fn default() -> Self {
        Self {
            text: String::new(),
            kind: StatusKind::Info,
            at: Instant::now(),
            pass: 0,
        }
    }
}

/// What is selected: some nodes, or some guides.
///
/// A newtype so multi-select rules (dedupe, prune on delete, "the single
/// selection") live in one place, and so the exclusivity between the two kinds of
/// subject is structural: a guide and a layer are never selected together, since
/// nothing acts on both. Every mutator therefore clears the other half.
///
/// **Both halves are lists**, but [`Self::is_empty`], [`Self::len`] and
/// [`Self::single`] answer about **layers** and read a guide selection as empty
/// however many guides it holds; otherwise node code would start treating guides
/// as layers.
#[derive(Default, Clone, Debug)]
pub struct Selection {
    ids: Vec<NodeId>,
    /// The **key layer**: the one member a multi-selection is measured *against*
    /// rather than merely included in. Align moves everything to its edge, and
    /// `build::boolean` makes it the base operand.
    ///
    /// **Explicit, not "the last one clicked"**: deriving it from `ids` order would
    /// stop align aligning to the selection's union. `None` means "no key".
    key: Option<NodeId>,
    guides: Vec<GuideId>,
    /// The orientation a **multi-selection's** frame has been left in by a turn or
    /// a lean, with the ids it was measured for (§15 D326). A set has no stored
    /// frame in the document, so the box would otherwise snap upright on release.
    ///
    /// **Guarded twice, because neither guard covers both losses**: every method
    /// that changes `ids` calls [`Selection::forget_frame`] (reselecting the same
    /// layers is a new selection the ids cannot see), and the orientation carries
    /// its ids so a mutator that forgets the call cannot leave it on another set.
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
    /// Whether `id` is one of the selected guides. **There is deliberately no
    /// `guide() -> Option<GuideId>`**: it would invite a caller that means this.
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
    /// Drop the guide half and leave the layers alone (*View ▸ Lock guides*).
    pub fn clear_guides(&mut self) {
        self.guides.clear();
    }
    /// Shift+click semantics for guides: [`Self::toggle`]'s counterpart.
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
    /// A selected guide reads as empty here, and so do [`Self::len`] and
    /// [`Self::single`]; for "nothing is selected", ask [`Self::nothing`].
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    /// Whether nothing at all is selected — no layers **and** no guides.
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
    /// **`None` below two layers, whatever is stored**; the stored value survives
    /// shrinking the selection to one and growing it back.
    pub fn key(&self) -> Option<NodeId> {
        self.key.filter(|_| self.ids.len() >= 2)
    }
    /// Designate `id` the key layer, or clear the designation if it already is one.
    /// Ignores an `id` that is not selected.
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
                // A key outside the selection is one `set_key` refuses to create.
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
    /// lean has left one on *exactly* these ids. The id comparison is the
    /// invalidation.
    pub fn frame(&self) -> Affine {
        match &self.frame {
            Some((ids, f)) if *ids == self.ids => *f,
            _ => Affine::IDENTITY,
        }
    }

    /// Leave `f` as this selection's frame orientation; callers compose
    /// (`set_frame(turn * selection.frame())`) so a lean after a turn keeps both.
    pub fn set_frame(&mut self, f: Affine) {
        self.frame = Some((self.ids.clone(), f));
    }

    /// Give up any stored frame orientation. A new mutator of `ids` owes a call.
    fn forget_frame(&mut self) {
        self.frame = None;
    }

    /// Drop ids that are no longer in the document (after a delete or an undo) —
    /// or that are in its vault, which is never selectable (§15 D1012).
    pub fn retain_existing(&mut self, doc: &Document) {
        let before = self.ids.len();
        let gone = |id: NodeId| !doc.contains(id) || doc.in_vault(id);
        self.ids.retain(|id| !gone(*id));
        self.key.take_if(|id| gone(*id));
        self.guides.retain(|id| doc.guide(*id).is_some());
        // Only when it dropped something: this runs after every commit, including
        // the one that just set the orientation.
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
/// Panels and overlays go through this instead of `Document`: reading the
/// document during a drag makes a control snap back to the committed value every
/// frame, which breaks accumulating gestures like a `DragValue`.
pub struct DisplayNode<'a> {
    node: &'a Node,
    over: Option<&'a NodeOverride>,
    /// The **used** kind — where container layout sized the node. `None` for
    /// [`EditorSession::committed_node`], which reports the value a commit is about
    /// to overwrite.
    used: Option<&'a NodeKind>,
    /// The layout with bound gaps and paddings resolved; `None` where nothing is
    /// bound or for [`EditorSession::committed_node`].
    drawn: Option<ondin_core::container::Display>,
    /// Whether the layer is shown, a bound visibility resolved.
    shown: bool,
    /// The opacity it draws at, a bound one resolved.
    opacity: f32,
}

impl<'a> DisplayNode<'a> {
    pub fn name(&self) -> &'a str {
        self.node.name()
    }
    pub fn locked(&self) -> bool {
        self.node.locked()
    }
    /// No override behind this one: clipping commits at once, so there is no
    /// pending version to read through.
    pub fn clip(&self) -> bool {
        self.node.clip()
    }
    /// No override behind this one either: the chain button commits on click.
    pub fn proportions_locked(&self) -> bool {
        self.node.proportions_locked()
    }
    /// The override's kind, then the used one, then the document's.
    ///
    /// **Shown, not typed**: edits compute from used geometry, and
    /// `build::keep_insets` turns a placement for a pinned layer into insets. A
    /// text's *mode* is the exception — stretching changes how it is drawn
    /// (`AutoHeight(w)`, `Fixed`) — so read it from the node
    /// (`tools::gesture_text_sizing`); a bake keeps it (`build::baked_ops`).
    pub fn kind(&self) -> &'a NodeKind {
        self.over
            .and_then(|o| o.kind.as_ref())
            .or(self.used)
            .unwrap_or_else(|| self.node.kind())
    }
    pub fn paint(&self) -> &'a Paint {
        self.over
            .and_then(|o| o.paint.as_ref())
            .unwrap_or_else(|| self.node.paint())
    }
    /// The opacity the layer draws at — a live preview's, else a bound one
    /// resolved, never the 0 a binding's literal holds.
    pub fn opacity(&self) -> f32 {
        self.over.and_then(|o| o.opacity).unwrap_or(self.opacity)
    }
    /// Whether the layer is shown — a bound visibility resolved, a live preview's
    /// included.
    pub fn visible(&self) -> bool {
        self.over.and_then(|o| o.visible).unwrap_or(self.shown)
    }
    /// The layer's insets inside its frame, a live inset scrub's included — **with
    /// an override behind it**, so a dragged field accumulates rather than snapping
    /// back each frame.
    pub fn insets(&self) -> ondin_core::Insets {
        self.over
            .and_then(|o| o.insets)
            .unwrap_or_else(|| *self.node.insets())
    }
    /// The layer's layout as a container, a live Container-card scrub's included —
    /// [`Self::insets`]' rule.
    ///
    /// **As laid out**: a bound gap or padding reads its variable's number, never
    /// the 0 its literal holds — the override's layout is stored resolved by the
    /// preview, the document's is resolved in `drawn`.
    pub fn display(&self) -> Option<ondin_core::container::Display> {
        self.over
            .and_then(|o| o.display.clone())
            .unwrap_or_else(|| self.drawn.clone().or_else(|| self.node.display().cloned()))
    }
    /// The layer's layout-item properties, a live Item-card scrub's included —
    /// [`Self::insets`]' rule.
    pub fn item(&self) -> ondin_core::container::LayoutItem {
        self.over
            .and_then(|o| o.item)
            .unwrap_or_else(|| *self.node.item())
    }
    /// The layer's effect stack (§5.3a).
    ///
    /// **With an override behind it, unlike `clip` and `proportions_locked`**:
    /// every number in an effect popover is scrubbable and needs a live preview,
    /// which is why `RenderOverrides` carries `SetEffects`.
    pub fn effects(&self) -> &'a [ondin_core::Keyed<ondin_core::Effect>] {
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
    /// Costs O(nodes the gesture touches). While a main component's layer is
    /// edited, [`Self::preview_follows`] reads each copy's old value off a one-node
    /// scratch (`Document::peek`); otherwise it is one scan of the links
    /// (`propagate::touches_copied`).
    overrides: RenderOverrides,
    /// The live text-editing session's own transaction, kept while the session's
    /// content is what `overrides` stands for — see [`Self::set_session_preview`]
    /// and the clear in [`Self::try_commit`].
    ///
    /// **Kept so a gesture laid *over* it can be wound back to it rather than
    /// cleared**: the previews share one override set and `preview_session` is
    /// event-driven, so nothing would restore the typed text after an inspector
    /// field replaced it. The already-shaped layout is deliberately not kept:
    /// rebuilding without one is correct and happens once per gesture.
    session_preview: Option<Transaction>,
    /// Whether `overrides` also carries an **in-flight gesture's** patch, as
    /// against the standing session preview above. All four combinations are
    /// reachable, so one `bool` could not tell "a session with a scrub over it"
    /// from "a session alone".
    preview_holds_gesture: bool,
    /// The pivot a live edit is showing, and the layer it belongs to.
    ///
    /// **`RenderOverrides` cannot carry it, by design**: a pivot is baked by the
    /// gesture that reads it, so `RenderOverrides::from_transaction` absorbs
    /// `SetPivot` as a no-op. The inner `Option` is the pivot's own, so previewing a
    /// *clear* back to the box centre is expressible; the outer says whether an
    /// edit is live.
    pivot_preview: Option<(NodeId, Option<Pivot>)>,
    /// Whether the document has edits not yet written to `path`.
    dirty: bool,
    /// The revision a write now in flight will put on disk, if one is
    /// (`library::writer`).
    ///
    /// While a write is in flight `dirty` stays `true` and only the save dot's
    /// sentence changes (*Saving…*). **A revision rather than a `bool` because a
    /// write can be overtaken**: marking the session clean on arrival after a later
    /// edit would be a lie; [`Self::finish_save`] compares first.
    saving: Option<u64>,
    /// The verb and moment of the last run commit, while the run is still open —
    /// see [`Self::commit_run`].
    run: Option<(&'static str, Instant)>,
    /// The style the browser's *Edit* opened: while set, a write of its slot on a
    /// layer bound to it is an edit of the style itself (`style::edit_through`, at
    /// the commit door), so the inspector's cards edit every user at once.
    pub style_edit: Option<ondin_core::style::StyleId>,
    /// When the document last matched what is on disk. `None` for a document never
    /// read from or written to a file.
    saved_at: Option<Instant>,
    /// Bumped by every change to the document — a commit, an undo, a redo, a
    /// document adopted — and what a cache of something *derived from* the
    /// document keys on. **Not `dirty`**: this only goes up, because a save changes
    /// nothing a render would produce.
    revision: u64,
    /// How many commits this session has seen — `revision` without undo and redo.
    /// [`FlexReceipt`] and a cut's move (`OndinApp::cut_move`, through
    /// [`Self::commits`]) read it: an undo and a redo with no commit between are
    /// the same history.
    commits: u64,
    /// What the last commit's resize did to flex items' growth — see
    /// [`FlexReceipt`] and [`Self::flex_receipt`], which is the only reader.
    flex_receipt: Option<FlexReceipt>,
    /// The styles and the variables the last commit's passes detached, by name —
    /// what [`said_detached`] worded onto the status — so a door whose own report
    /// overwrites that status can carry them ([`Self::last_detached`]).
    last_detached: (Vec<String>, Vec<String>),
    status: Status,
    /// Which frame a status write belongs to ([`Self::next_status_pass`]): writes in
    /// one frame are one handler's, and join after an error.
    status_pass: u64,
}

/// **The flex-item properties a resize changed on the user's behalf** — the
/// Item card's receipt.
///
/// A resize of an in-flow item writes `flex-grow: 0`, `flex-shrink: 0` and, on a
/// stretched cross axis, `align-self` (`build::keep_flex_sizes`), none of which the
/// user typed.
///
/// **Its Undo is the whole step, never the flips alone**, so the receipt is true
/// only while that step is the top of the history: it records the commit count
/// (not the revision, which undo and redo move) and the undo depth, and
/// [`EditorSession::flex_receipt`] answers `None` once either moves.
#[derive(Clone, Debug, PartialEq)]
pub struct FlexReceipt {
    /// Each item the commit changed, with its properties before and after.
    pub held: Vec<(
        NodeId,
        ondin_core::container::LayoutItem,
        ondin_core::container::LayoutItem,
    )>,
    commit: u64,
    depth: usize,
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
            style_edit: None,
            saved_at,
            revision: 0,
            commits: 0,
            flex_receipt: None,
            last_detached: Default::default(),
            status: Status::default(),
            status_pass: 0,
        }
    }

    /// How many times this session's document has changed — see the field.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many commits this session has seen, undo and redo not counted — see
    /// the field.
    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// The last resize's [`FlexReceipt`], while its step is still the top of the
    /// history — `None` once anything has been committed or undone since.
    pub fn flex_receipt(&self) -> Option<&FlexReceipt> {
        self.flex_receipt
            .as_ref()
            .filter(|r| r.commit == self.commits && r.depth == self.history.undo_depth())
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
    /// `cancel_gesture`'s test for "is there anything to unwind": egui cannot be
    /// asked, since it clears `dragged` on the release of **any** button, so a
    /// right-click whose press and release share a frame takes the drag away first
    /// (`panels::typography::cancel_gate::a_right_click_short_enough_to_fit_one_frame_takes_the_drag_away`).
    /// A scrub over a live text session is both, so this reads
    /// `preview_holds_gesture`, not the negation of a session flag.
    pub fn has_gesture_preview(&self) -> bool {
        !self.overrides.is_empty() && self.preview_holds_gesture
    }

    /// A read-through view of a node that reflects any live gesture.
    ///
    /// Panels must read through this rather than the document, or their
    /// controls snap back to the committed value on every frame of a drag.
    pub fn display_node(&self, id: NodeId) -> Option<DisplayNode<'_>> {
        let node = self.doc.get(id)?;
        Some(DisplayNode {
            node,
            over: self.overrides.get(id),
            used: Some(self.resolved.used_kind_of(node)),
            drawn: match self.doc.resolved_display(id) {
                Some(std::borrow::Cow::Owned(d)) => Some(d),
                _ => None,
            },
            shown: self.doc.shown(id),
            opacity: self.doc.drawn_opacity(id),
        })
    }

    /// [`Self::display_node`]'s twin over the **committed** document, ignoring any
    /// preview.
    ///
    /// For the one caller that must not see its own preview: a *commit* compares the
    /// value it is about to write against the one already there, and reading that
    /// through the override compares an edit against a preview of itself. Clearing
    /// the preview first is wrong: it may belong to a **live text session**
    /// ([`Self::set_session_preview`]), and clearing it takes uncommitted text off
    /// the canvas.
    pub fn committed_node(&self, id: NodeId) -> Option<DisplayNode<'_>> {
        let node = self.doc.get(id)?;
        Some(DisplayNode {
            node,
            over: None,
            used: None,
            drawn: None,
            // What shows, not the literal: a binding is not a preview.
            shown: self.doc.shown(id),
            opacity: self.doc.drawn_opacity(id),
        })
    }

    /// The box a group with a layout is drawn as, a live gesture's
    /// included — the preview's own if it re-laid the group, else the committed
    /// one. `None` for anything that is not such a group.
    pub fn preview_frame(&self, id: NodeId) -> Option<ondin_core::kurbo::Size> {
        self.overrides
            .get(id)
            .and_then(|o| o.frame)
            .or_else(|| self.resolved.used_frame(id))
    }

    /// World transform including any live gesture.
    pub fn preview_world_transform(&self, id: NodeId) -> Option<Affine> {
        self.overrides
            .world_transform(&self.doc, &self.resolved, id)
    }

    /// The node's **local** transform including any live gesture — its own, not
    /// composed through its ancestors.
    ///
    /// [`Self::preview_world_transform`]'s twin, for the scene walk, which composes
    /// a ghost's transform under its parent's world: a world transform would apply
    /// the parent chain twice.
    pub fn preview_local_transform(&self, id: NodeId) -> Option<Affine> {
        self.doc.get(id)?;
        Some(self.overrides.transform_of(&self.doc, &self.resolved, id))
    }

    /// Install the artwork explaining the mode that is open, or take it away.
    ///
    /// **Called unconditionally once a frame**, from `OndinApp::draw_canvas`: a
    /// `set_preview` replaces the overrides wholesale, so anything that must outlive
    /// a commit is re-established after the input rather than at every call site
    /// that touches a preview.
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
    /// Falls back to the resolved value whenever nothing is overridden — the
    /// common case; otherwise it recomputes so outlines and handles track a drag.
    pub fn preview_world_bounds(&self, id: NodeId) -> Option<Rect> {
        if self.overrides.is_empty() {
            return self.resolved.world_bounds(id);
        }
        let node = self.doc.get(id)?;
        let display = self.display_node(id)?;
        let world = self.preview_world_transform(id)?;
        // A group with a layout is the box its layout made, not its children's union.
        if let Some(frame) = self.preview_frame(id) {
            return Some(geometry::transform_rect(
                world,
                Rect::from_origin_size(Point::ZERO, frame),
            ));
        }

        // The paint as drawn, a bound stroke width its number.
        let own = geometry::world_bounds_of_parts(
            display.kind(),
            &self.doc.resolved_paint(id, display.paint()),
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
    /// The selection outline and handles are drawn from this, so they follow the
    /// shape's own rectangle under rotation (`ondin_core::local_box`).
    pub fn preview_local_box(&self, id: NodeId) -> Option<Rect> {
        if self.overrides.is_empty() {
            return ondin_core::local_box(&self.doc, &self.resolved, id);
        }
        let display = self.display_node(id)?;
        if let Some(b) = geometry::local_bounds(display.kind(), self.preview_text_layout(id)) {
            return Some(b);
        }
        if let Some(frame) = self.preview_frame(id) {
            return Some(Rect::from_origin_size(Point::ZERO, frame));
        }
        // A boolean's box is its *result's*, as the preview re-derived it.
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
    /// The pivot is carried in `pivot_preview` but the **box** it may be a fraction
    /// of goes through the overrides, so this is not simply
    /// `ondin_core::pivot_world`. Both halves are previewed, for different
    /// gestures: a resize moves the box, the inspector's origin fields move the
    /// pivot. The canvas marker drag paints from the live pointer (`draw_pivot`).
    pub fn preview_pivot(&self, id: NodeId) -> Option<Point> {
        self.preview_pivot_in(id, self.preview_local_box(id)?)
    }

    /// [`Self::preview_pivot`] against a box the caller already has.
    ///
    /// Exists so `selection_handles`, which already holds the box, does not walk
    /// the subtree a second time (§15 D764); `preview_pivot` is written in terms of
    /// this so the two cannot disagree.
    pub fn preview_pivot_in(&self, id: NodeId, box_: Rect) -> Option<Point> {
        Some(geometry::pivot_point(self.previewed_pivot(id)?, box_))
    }

    /// The node's stored pivot, or the one a live edit is showing in its place —
    /// **`None` for "not moved"**, which [`Self::preview_pivot`] resolves to the
    /// box's centre and cannot report back; the marker's badge names a *placement*
    /// and must not show over a layer reset to the middle. The outer `Option` is
    /// "is there such a node".
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
    /// A transaction that cannot be represented as overrides, or would not apply,
    /// clears the preview. **The pivot is picked out of the transaction here**
    /// because the overrides cannot hold it.
    ///
    /// **A live text session's preview is composed with, not replaced**: building
    /// from `tx` alone would take the typed text off the canvas for good, since
    /// `preview_session` runs on change. The session's operations go first
    /// (`RenderOverrides::absorb` is last-one-wins per field). Only the inspector
    /// valves compose; `Mode::TextInsert` owns the pointer during a session.
    pub fn set_preview(&mut self, tx: &Transaction) {
        // **A resized flex item holds its size in the preview as the commit will
        // make it**: the growth, `align-self` and size keyword `keep_flex_sizes`
        // adds, or the preview's re-layout stretches the item back every frame.
        let holds = ondin_core::build::flex_holds(&self.doc, &self.resolved, tx);
        let mut ops = match &self.session_preview {
            Some(session) => session.0.clone(),
            None => Vec::new(),
        };
        ops.extend(tx.0.iter().cloned());
        ops.extend(holds);
        let composed = Transaction(ops);
        let tx = &composed;
        let follows = self.preview_follows(tx);
        self.overrides = RenderOverrides::from_transaction_following(
            &self.doc,
            &self.resolved,
            tx,
            &follows,
            None,
        )
        .unwrap_or_default();
        self.preview_holds_gesture = true;
        // From the gesture's own operations, never the follows: a `SetPivot` on a
        // main's layer follows onto its copies, and the last of those would name a
        // copy, leaving the main's origin fields scrubbing against another node.
        self.pivot_preview = tx.0.iter().rev().find_map(|op| match op {
            Operation::SetPivot { id, pivot } => Some((*id, *pivot)),
            _ => None,
        });
    }

    /// What a previewed edit owes the copies of whatever mains it touches: the
    /// commit door's field pass (`propagate::propagate`) run **as the door rewrites
    /// the edit** (`keep_insets`, `keep_flex_sizes`), for
    /// `RenderOverrides::from_transaction_following` to lay out. Field pass only —
    /// the structural pass would blank the preview (§6.2). Empty for an edit that
    /// touches no main (`propagate::touches_copied`).
    ///
    /// **A pinned copy the follows give a transform and no insets has its own insets
    /// restated**: `RenderOverrides::relayout` re-places only a layer with an insets
    /// or kind override
    /// (`a_copy_pinned_by_its_own_inset_previews_unmoved_by_its_mains_move`).
    fn preview_follows(&self, tx: &Transaction) -> Transaction {
        if !ondin_core::propagate::touches_copied(&self.doc, tx) {
            return Transaction::default();
        }
        let (doc, res) = (&self.doc, &self.resolved);
        let door = ondin_core::build::keep_flex_sizes(
            doc,
            res,
            ondin_core::build::keep_insets(doc, res, tx.clone()),
        );
        let mut follows = ondin_core::propagate::propagate(doc, &door);
        let restated: Vec<Operation> = follows
            .iter()
            .filter_map(|op| match op {
                Operation::SetTransform { id, .. }
                    if !follows
                        .iter()
                        .any(|o| matches!(o, Operation::SetInsets { id: i, .. } if i == id)) =>
                {
                    doc.get(*id).filter(|n| n.insets().is_authored()).map(|n| {
                        Operation::SetInsets {
                            id: *id,
                            insets: *n.insets(),
                        }
                    })
                }
                _ => None,
            })
            .collect();
        follows.extend(restated);
        Transaction(follows)
    }

    /// Preview the uncommitted state of a **live text-editing session**.
    ///
    /// The same override, marked as the session's, which exempts it from the clear
    /// [`Self::try_commit`] does: a session's stands for content not committed
    /// until it ends (§9.3). `shaped` is the session's already-shaped layout for
    /// the node (see [`RenderOverrides::from_transaction_shaped`]); `None` is
    /// correct and merely slower.
    pub fn set_session_preview(&mut self, tx: &Transaction, shaped: Option<(NodeId, TextLayout)>) {
        // The session's own transaction is what is kept below, so a rebuild
        // (`clear_gesture_preview`) runs the follows pass afresh.
        let follows = self.preview_follows(tx);
        self.overrides = RenderOverrides::from_transaction_following(
            &self.doc,
            &self.resolved,
            tx,
            &follows,
            shaped,
        )
        .unwrap_or_default();
        self.pivot_preview = tx.0.iter().rev().find_map(|op| match op {
            Operation::SetPivot { id, pivot } => Some((*id, *pivot)),
            _ => None,
        });
        self.session_preview = Some(tx.clone());
        // A gesture patch composed over the previous session preview is gone with
        // it; the valve re-installs its own next frame, and `true` here would leave
        // a phantom gesture for `cancel_gesture` to unwind.
        self.preview_holds_gesture = false;
    }

    /// [`Self::clear_preview`] for a **cancelled gesture**, which must spare a live
    /// text session's uncommitted *text*. Because [`Self::set_preview`] composes a
    /// gesture over the session, this rebuilds from the session's own transaction.
    pub fn clear_gesture_preview(&mut self) {
        match self.session_preview.take() {
            // Rebuilt without the session's already-shaped layout, which this does
            // not keep: correct, merely slower, once per gesture.
            Some(session) => self.set_session_preview(&session, None),
            None => self.clear_preview(),
        }
    }

    /// Preview `tx`, drawing `leaving` outside their parent's clip
    /// (`RenderOverrides::escape_clip`). The caller picks them by the rule the
    /// *release* uses, so a shape drawing in full promises what dropping it does.
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
    /// **The policy lives here and the mechanism in core**: only the app knows when
    /// two edits are one decision — the same verb, nothing committed between, no
    /// pointer press between ([`Self::end_edit_run`]), and close in time. The window
    /// is feel, not correctness.
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

    /// End the current run, so the next [`Self::commit_run`] starts an undo step of
    /// its own. Called on any pointer press.
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
        // Cleared first, so a commit that empties, changes nothing or fails
        // never hands the caller the one before it's detaches.
        self.last_detached = Default::default();
        if tx.0.is_empty() {
            return Ok(());
        }
        // **While the browser's *Edit* is open, an edit of the style's users is an
        // edit of the style** — first, so every pass below reads the style's edit and
        // `style::settle` writes it back onto every user.
        let tx = match self.style_edit {
            Some(s) => ondin_core::style::edit_through(&self.doc, tx, s),
            None => tx,
        };
        // **Every placement a tool writes is where it wants the layer drawn**, and
        // for a layer pinned by insets that has to become new insets — here, once,
        // rather than in every tool. Before the no-op test below, because the
        // conversion also recognises a pinned layer written back to where it is
        // drawn as changing nothing.
        let tx = ondin_core::build::keep_insets(&self.doc, &self.resolved, tx);
        // The items this edit sets itself, so the receipt names only what
        // `keep_flex_sizes` added.
        let asked: Vec<NodeId> =
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetLayoutItem { id, .. } => Some(*id),
                    _ => None,
                })
                .collect();
        // A resized flex item keeps the size it was dragged to, and an in-flow item
        // its stored translation.
        let tx = ondin_core::build::keep_flex_sizes(&self.doc, &self.resolved, tx);
        // Where the binding passes below start appending their detaches, so the
        // floating status can name the variables they cut.
        let asked_ops = tx.0.len();
        // A radius written over a bound corner detaches it — before `propagate`, so
        // a copy bound as its main was follows the detach as well as the radius.
        let tx = ondin_core::build::keep_corner_bindings(&self.doc, tx);
        // A gap or a padding written over a bound slot, the same way.
        let tx = ondin_core::build::keep_spacing_bindings(&self.doc, tx);
        // A visibility written over a bound layer.
        let tx = ondin_core::build::keep_visible_bindings(&self.doc, tx);
        // An opacity written over a bound layer.
        let tx = ondin_core::build::keep_opacity_bindings(&self.doc, tx);
        // A stroke's width written over its binding is rewritten in the list, not
        // appended, so the widths the pass cuts are read around it for the status.
        let widths_bound = stroke_widths_bound(&tx);
        let tx = ondin_core::build::keep_stroke_width_bindings(&self.doc, tx);
        let widths_cut = stroke_widths_cut(&self.doc, &tx, &widths_bound);
        // An effect's numbers, read around the pass the same way.
        let numbers_bound = effect_numbers_bound(&tx);
        let tx = ondin_core::build::keep_effect_bindings(&self.doc, tx);
        let numbers_cut = effect_numbers_cut(&self.doc, &tx, &numbers_bound);
        // A width or a height written over a bound axis, or made computed by the
        // edit — after `keep_flex_sizes`, whose holds are among the layout writes it
        // reads.
        let tx = ondin_core::build::keep_size_bindings(&self.doc, Some(&self.resolved), tx);
        // A text's content or family written over its binding — before `propagate`,
        // whose `style::settle` writes a newly applied text style's family after this
        // has detached the binding it replaces.
        let tx = ondin_core::build::keep_text_bindings(&self.doc, tx);
        let mut tx = tx;
        // What the components owe the edit — a swap's rewrite, a main's structure
        // and fields reaching its instances, the links and variant names settled —
        // in the one order the core tests pin (`propagate::owed`'s doc).
        let user = tx.0.len();
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        let mut vars_cut = variables_detached(&self.doc, &tx, asked_ops, user);
        for name in widths_cut.into_iter().chain(numbers_cut) {
            if !vars_cut.contains(&name) {
                vars_cut.push(name);
            }
        }
        vars_cut.sort();
        let styles_cut = styles_detached(&self.doc, &tx, user);
        let cut = (styles_cut.clone(), vars_cut.clone());
        let detached = said_detached(styles_cut, vars_cut);
        let held = self.growth_held(&tx, &asked);
        // Either pass can empty a transaction outright (`keep_insets` on a pinned
        // layer written back where it is, `keep_flex_sizes` on a translation alone
        // on an in-flow item), which `changes_nothing` below does not catch.
        if tx.0.is_empty() {
            self.clear_gesture_preview();
            return Ok(());
        }
        // **A completed interaction that changed nothing is not an undo step**
        // (§15 D428): a click into a field and back out, or a handle released
        // without moving, writes back what is there and would dirty the document
        // and add a step that undoes to the same picture. Checked here rather than
        // at the doors. **The preview is still dropped**: a no-op commit ends the
        // gesture, and the clear below takes the ghost off the canvas.
        if tx.changes_nothing(&self.doc) {
            self.clear_gesture_preview();
            return Ok(());
        }
        if !run {
            // A plain commit is a new decision, so it closes any run.
            self.run = None;
        }
        let dirty = match run {
            true => self.history.commit_into_run(&mut self.doc, tx)?,
            false => self.history.commit(&mut self.doc, tx)?,
        };
        // **The one place a caught boolean panic becomes something the user sees.**
        // `evaluate` answers `None` for a panic and for a correct empty result alike,
        // so only `boolean::failures` moving across this update tells them apart.
        // This is the transient half: a caller reporting on its own edit afterwards
        // wins the status slot; the durable half is the layers row's warning colour
        // (`Resolved::boolean_failed`).
        let failures = ondin_core::boolean::failures();
        self.resolved.update(&self.doc, &dirty);
        if ondin_core::boolean::failures() != failures {
            self.fail("A boolean could not be computed and draws nothing — undo to bring its operands back");
        }
        self.selection.retain_existing(&self.doc);
        self.dirty = true;
        // **A counter, not a hash of the document**: anything caching a *rendering*
        // needs only "something was committed since". Undo and redo do not come
        // through this door and bump it themselves; every reader keyed on it
        // (`FrameIndex`, the Export preview, the recovery gate, `finish_save`)
        // depends on that.
        self.revision = self.revision.wrapping_add(1);
        self.commits = self.commits.wrapping_add(1);
        // Recorded after the bump, so the receipt names this commit; a run's
        // merged commit has no single step to undo and leaves none.
        self.flex_receipt = (!held.is_empty() && !run).then(|| FlexReceipt {
            held,
            commit: self.commits,
            depth: self.history.undo_depth(),
        });
        // **A commit makes any preview stale by definition**: a leftover one is read
        // back by `display_node` as a **stale snapshot every later write is built
        // from**, and a widget that vanishes mid-drag never delivers the
        // `drag_stopped()` that clears it. A text session's preview is the exception
        // (§9.3), spelled once in [`Self::clear_gesture_preview`].
        self.clear_gesture_preview();
        // **A door that detached a style or a variable says so** — the canvas,
        // *Paste properties*, MCP — before any caller's own report, which
        // overwrites it.
        if let Some(said) = detached {
            self.info(said);
        }
        self.last_detached = cut;
        Ok(())
    }

    /// The items whose growth `tx` stops that the edit did not set itself —
    /// [`FlexReceipt::held`], read off the transaction `keep_flex_sizes` handed
    /// back, before it is applied. Only `flex-grow`, `flex-shrink`, `flex-basis`,
    /// `align-self` and a grid item's `justify-self` count: a size keyword going
    /// back to `auto` is the resize itself, shown in the Transform card's W and H.
    /// `justify-self` matters for a grid item resized across alone.
    fn growth_held(
        &self,
        tx: &Transaction,
        asked: &[NodeId],
    ) -> Vec<(
        NodeId,
        ondin_core::container::LayoutItem,
        ondin_core::container::LayoutItem,
    )> {
        tx.0.iter()
            .filter_map(|op| match op {
                Operation::SetLayoutItem { id, item } if !asked.contains(id) => {
                    let was = *self.doc.get(*id)?.item();
                    (was.grow != item.grow
                        || was.shrink != item.shrink
                        || was.basis != item.basis
                        || was.align_self != item.align_self
                        || was.justify_self != item.justify_self)
                        .then_some((*id, was, *item))
                }
                _ => None,
            })
            .collect()
    }

    /// Take the last transaction back. **Answers whether the document actually
    /// moved** — `false` for the end of the history and for a failure.
    ///
    /// The `bool` is for the caller: state that *indexes into* the document
    /// (`OndinApp::points`, `OndinApp::pen`) lives on `OndinApp` and is dropped in
    /// [`crate::app::OndinApp::undo`] when the ground shifts. **Not "an undo
    /// happened"**: `Ctrl+Z` at the bottom of the stack must not deselect anything.
    pub fn undo(&mut self) -> bool {
        // An undo is never part of a run; `History` says so too.
        self.run = None;
        match self.history.undo(&mut self.doc) {
            Ok(Some(dirty)) => {
                self.resolved.update(&self.doc, &dirty);
                self.selection.retain_existing(&self.doc);
                self.dirty = true;
                // `finish_save` above all must see it, or it marks the session
                // clean over a save queued before the undo.
                self.revision = self.revision.wrapping_add(1);
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
                // `commits` stays put, so a redo brings the step's receipt back.
                self.revision = self.revision.wrapping_add(1);
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
        self.style_edit = None;
        // A just-opened document matches its file, so it counts as saved now.
        self.saved_at = path.is_some().then(Instant::now);
        self.path = path;
        self.dirty = false;
        // A different document, so anything derived from the old one is stale.
        self.revision = self.revision.wrapping_add(1);
    }

    /// Re-shape every text node and refresh dependent bounds. Called when the
    /// font service registers a family that was previously falling back, so
    /// bounds, hit-testing and selection rectangles stop describing the wrong
    /// face (§5.4a).
    ///
    /// Any in-flight preview is dropped rather than re-shaped: the session does not
    /// keep a *gesture's* transaction. A live text session's
    /// ([`Self::session_preview`]) is dropped too, deliberately: `preview_session`
    /// re-shapes it on the next keystroke, and rebuilding here would re-shape the
    /// node on a frame already re-shaping every text node.
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
    /// **Deliberately one-way**: it cannot clear the flag, since
    /// [`Self::mark_saved`] is the only road back to clean. `saved_at` goes too, or
    /// the save dot would go green over a document that differs from its file (see
    /// [`Self::saved_ago`]).
    ///
    /// Callers: a document restored from a crash snapshot (`library::recovery`) is
    /// unsaved by construction, and the autosave tests reach "a dirty document"
    /// without a fixture edit.
    pub fn mark_unsaved(&mut self) {
        self.dirty = true;
        self.saved_at = None;
    }

    /// Record that the current document state is what is on disk.
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
        self.saved_at = Some(Instant::now());
        // `save_file` settles before it writes, so there is nothing to clear in
        // practice; clearing anyway stops the tooltip sticking on *Saving…* if that
        // order changes.
        self.saving = None;
    }

    /// A write of `at` has been queued: the save dot's tooltip says *Saving…* until
    /// it lands (`library::writer`).
    pub fn begin_save(&mut self, at: u64) {
        self.saving = Some(at);
    }

    /// Whether a queued write is still in flight — the save dot tooltip's third state.
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// A queued write is over and did not land.
    ///
    /// **Only `saving` clears, and it must.** The session stays dirty, so the next
    /// interval tries again; without the clear the tooltip sticks on *Saving…* and
    /// `OndinApp::autosave_tick`'s `is_saving` gate never opens again, silently
    /// turning autosave off.
    pub fn save_failed(&mut self) {
        self.saving = None;
    }

    /// A queued write of revision `at` has reached `path`.
    ///
    /// **Three guards, each answering a different way of being stale.**
    /// `saving == Some(at)` says this result is about the write being waited on. The
    /// revision says the session has not moved on since the clone was taken, or
    /// calling it clean would put *Saved · just now* over work that is on disk
    /// nowhere. The path says this is still the same document (`open_path` and
    /// `recover` settle before they adopt, so a mismatch should be unreachable).
    ///
    /// Either way `saving` clears; a superseded write leaves the session dirty for
    /// the next interval. Returns whether the session went clean, which
    /// `export_on_save` keys on.
    pub fn finish_save(&mut self, path: &std::path::Path, at: u64) -> bool {
        // **Cleared only for the write it is about**: a `take()` here would let an
        // older result end the *Saving…* of a newer write still in flight.
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
    /// has (never saved) or no longer does (unsaved edits). The top bar's save dot
    /// renders exactly this distinction: `None` is red.
    pub fn saved_ago(&self) -> Option<std::time::Duration> {
        if self.dirty {
            return None;
        }
        self.saved_at.map(|t| t.elapsed())
    }

    /// What to call the open document — the name the user typed, not the file.
    ///
    /// **The stem is a slug, so it cannot be the answer on its own** (`landing-v4`
    /// for "Landing v4"). `library::scan::display_name` is the rule shared with the
    /// library so the two cannot disagree.
    pub fn document_title(&self) -> String {
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match crate::library::scan::display_name(self.doc.meta(), &stem) {
            // Never saved and never named: the starter document.
            name if name.trim().is_empty() => "Untitled".to_string(),
            name => name,
        }
    }

    // --- status -----------------------------------------------------------

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// What the last commit's passes detached, as `(styles, variables)` by
    /// name — empty after a commit that detached nothing or did not land. For a
    /// door whose own report replaces the status the commit wrote, so the detach is
    /// said in it rather than lost under it.
    pub fn last_detached(&self) -> &(Vec<String>, Vec<String>) {
        &self.last_detached
    }

    /// The slot itself, so a test can age a message past its hold.
    #[cfg(test)]
    pub fn status_mut(&mut self) -> &mut Status {
        &mut self.status
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.say(text.into(), StatusKind::Info);
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        self.say(text.into(), StatusKind::Error);
    }

    /// Start a new frame's status writes; the app calls it once a frame.
    pub fn next_status_pass(&mut self) {
        self.status_pass += 1;
    }

    /// Write the status line. **An error written this frame is never replaced**: a
    /// later message joins it, so a refusal is not hidden by the report that follows
    /// it in the same handler. A sentence already there is not said twice. A write in
    /// a later frame replaces whatever is showing, as a new action's report should.
    fn say(&mut self, text: String, kind: StatusKind) {
        let old = &self.status;
        // An empty write is a clear, and clears.
        let join = old.pass == self.status_pass
            && old.kind == StatusKind::Error
            && !old.text.is_empty()
            && !text.is_empty();
        let (text, kind) = match join {
            true if old.text.split(STATUS_JOIN).any(|t| t == text) => {
                (old.text.clone(), StatusKind::Error)
            }
            true => (
                format!("{}{STATUS_JOIN}{text}", old.text),
                StatusKind::Error,
            ),
            false => (text, kind),
        };
        self.status = Status {
            text,
            kind,
            at: Instant::now(),
            pass: self.status_pass,
        };
    }
}

/// What separates two messages joined on the status line.
pub const STATUS_JOIN: &str = " · ";

/// The canvas ground a document starts on, and the one the Appearance panel's
/// reset button returns to: the design's near-black `#171718`.
///
/// Lives in core because it is also what a file written before the ground was
/// saved means (`io::schema`); re-exported for the panels.
pub use ondin_core::DEFAULT_CANVAS_BACKGROUND;

/// The styles the commit detached where the edit wrote over them, by name, for
/// [`said_detached`]. Empty when the edit itself detached something — the
/// capsule's *Detach* and the `⋯` menu's are asked for, and say nothing.
/// `user` is how many of `tx`'s operations the edit wrote before the commit's
/// own passes.
fn styles_detached(doc: &Document, tx: &Transaction, user: usize) -> Vec<String> {
    use ondin_core::style::binds_slot;
    let asked = tx.0[..user.min(tx.0.len())]
        .iter()
        .any(|op| matches!(binds_slot(op), Some((_, _, None))));
    if asked {
        return Vec::new();
    }
    // A swap's rewrite of the copy it swaps is the swap the user picked, not a
    // detach: a copy following an unstyled main now has no style to keep.
    let swapped: Vec<NodeId> = tx.0[..user.min(tx.0.len())]
        .iter()
        .filter_map(|op| match op {
            Operation::SetSwap { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    let swapped = ondin_core::subtree_nodes(doc, &swapped);
    let mut names: Vec<String> = tx.0[user.min(tx.0.len())..]
        .iter()
        .filter_map(|op| match binds_slot(op) {
            Some((id, slot, None)) if !swapped.contains(&id) => doc.get(id)?.styles().get(slot),
            _ => None,
        })
        .filter_map(|s| doc.styles().get(s).map(|s| s.name.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The variables the commit's binding passes cut where the edit wrote over a
/// bound field — a gap, a padding, a radius, a visibility, a size, a text's
/// content or family, an opacity — read off the `Set*Vars` the passes append
/// between `from` and `to` (`build::keep_corner_bindings` and its siblings each
/// push rather than rewrite), against the bindings the document holds. Empty when
/// the edit itself set a binding of those kinds, as [`styles_detached`]' asked-for
/// detach is.
fn variables_detached(doc: &Document, tx: &Transaction, from: usize, to: usize) -> Vec<String> {
    use ondin_core::VarId;
    // Each slot's binding before the op and after it.
    type Slots = Vec<(Option<VarId>, Option<VarId>)>;
    // A binding op's node and the slots it writes, `None` for an op of
    // another kind.
    let slots = |op: &Operation| -> Option<(NodeId, Slots)> {
        let node = |id: &NodeId| doc.get(*id);
        Some(match op {
            Operation::SetCornerVars { id, vars } => {
                let had = node(id)?.corner_vars();
                (*id, had.iter().copied().zip(vars.iter().copied()).collect())
            }
            Operation::SetSpacingVars { id, vars } => {
                let had = node(id)?.spacing_vars();
                (*id, had.iter().copied().zip(vars.iter().copied()).collect())
            }
            Operation::SetVisibleVar { id, var } => (*id, vec![(node(id)?.visible_var(), *var)]),
            Operation::SetOpacityVar { id, var } => (*id, vec![(node(id)?.opacity_var(), *var)]),
            Operation::SetSizeVars { id, vars } => {
                let had = node(id)?.size_vars();
                (*id, had.iter().copied().zip(vars.iter().copied()).collect())
            }
            Operation::SetTextVars { id, vars } => {
                let had = node(id)?.text_vars();
                (*id, had.iter().copied().zip(vars.iter().copied()).collect())
            }
            _ => return None,
        })
    };
    let (from, to) = (from.min(tx.0.len()), to.min(tx.0.len()));
    if tx.0[..from].iter().any(|op| slots(op).is_some()) {
        return Vec::new();
    }
    let table = doc.variables();
    let mut names: Vec<String> = tx.0[from..to.max(from)]
        .iter()
        .filter_map(slots)
        .flat_map(|(_, s)| s)
        .filter_map(|(had, now)| match (had, now) {
            (Some(v), None) => table.get(v).map(|x| x.name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every stroke `tx` writes with its width bound, as (layer, stroke, variable)
/// — read before `build::keep_stroke_width_bindings`, which rewrites a cut
/// binding inside the list rather than appending an op.
fn stroke_widths_bound(tx: &Transaction) -> Vec<(NodeId, ondin_core::ItemId, ondin_core::VarId)> {
    tx.0.iter()
        .filter_map(|op| match op {
            Operation::SetStrokes { id, strokes } => Some((*id, strokes)),
            _ => None,
        })
        .flat_map(|(id, strokes)| {
            strokes
                .iter()
                .filter_map(move |s| s.width_var.map(|v| (id, s.id, v)))
        })
        .collect()
}

/// The variables `bound`'s strokes lost in `tx`, the pass run — each a width
/// the edit wrote over its binding, by name, for [`said_detached`]. A stroke the
/// edit itself unbinds never enters `bound`, so it is not said.
fn stroke_widths_cut(
    doc: &Document,
    tx: &Transaction,
    bound: &[(NodeId, ondin_core::ItemId, ondin_core::VarId)],
) -> Vec<String> {
    let table = doc.variables();
    let mut names: Vec<String> = bound
        .iter()
        .filter(|(id, item, _)| {
            tx.0.iter().any(|op| match op {
                Operation::SetStrokes { id: n, strokes } if n == id => strokes
                    .iter()
                    .any(|s| s.id == *item && s.width_var.is_none()),
                _ => false,
            })
        })
        .filter_map(|(_, _, v)| table.get(*v).map(|x| x.name.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every effect number `tx` writes bound, as (layer, effect, slot, variable) —
/// [`stroke_widths_bound`]' twin for `build::keep_effect_bindings`.
fn effect_numbers_bound(
    tx: &Transaction,
) -> Vec<(NodeId, ondin_core::ItemId, usize, ondin_core::VarId)> {
    tx.0.iter()
        .filter_map(|op| match op {
            Operation::SetEffects { id, effects } => Some((*id, effects)),
            _ => None,
        })
        .flat_map(|(id, effects)| {
            effects
                .iter()
                .flat_map(move |e| (0..4).filter_map(move |k| e.vars[k].map(|v| (id, e.id, k, v))))
        })
        .collect()
}

/// The variables `bound`'s effect numbers lost in `tx`, the pass run —
/// [`stroke_widths_cut`]'s twin.
fn effect_numbers_cut(
    doc: &Document,
    tx: &Transaction,
    bound: &[(NodeId, ondin_core::ItemId, usize, ondin_core::VarId)],
) -> Vec<String> {
    let table = doc.variables();
    let mut names: Vec<String> = bound
        .iter()
        .filter(|(id, item, slot, _)| {
            tx.0.iter().any(|op| match op {
                Operation::SetEffects { id: n, effects } if n == id => effects
                    .iter()
                    .any(|e| e.id == *item && e.vars[*slot].is_none()),
                _ => false,
            })
        })
        .filter_map(|(_, _, _, v)| table.get(*v).map(|x| x.name.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// What the floating status says when the commit detached something the edit
/// wrote over: *Detached surface/raised*, *Detached space/gap*, or a count —
/// *3 styles*, *2 variables*, *3 styles and variables* — `None` when nothing was.
pub(crate) fn said_detached(styles: Vec<String>, vars: Vec<String>) -> Option<String> {
    match (styles.as_slice(), vars.as_slice()) {
        ([], []) => None,
        ([one], []) | ([], [one]) => Some(format!("Detached {one}")),
        (many, []) => Some(format!("Detached {} styles", many.len())),
        ([], many) => Some(format!("Detached {} variables", many.len())),
        (s, v) => Some(format!(
            "Detached {} styles and variables",
            s.len() + v.len()
        )),
    }
}

/// A per-process-random actor id (§5.2, invariant 3).
///
/// Core has no RNG policy, so the app supplies one: `RandomState` is OS-seeded
/// per process and the clock breaks ties, enough for two sessions never to pick
/// the same actor, which keeps ids from colliding across files and peers.
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
            // `None`, so it is numbered like any other frame — "Frame 1".
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
            // `None`, so it is numbered like any other text layer — "Text 1".
            name: None,
        },
    ]))
    .expect("seed starter document");

    doc.apply(&Transaction(vec![
        // The frame's white ground is a fill like the two below it. New nodes, so
        // lists built whole (`ondin_core::keyed_by_position`).
        Operation::SetFills {
            id: artboard,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 255, 255, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: rect,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(70, 130, 220, 255)).into(),
                visible: true,
            }]),
        },
        Operation::SetFills {
            id: ellipse,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(235, 110, 90, 255)).into(),
                visible: true,
            }]),
        },
    ]))
    .expect("paint starter shapes");

    (doc, ids)
}

#[cfg(test)]
mod tests {
    //! Session tests, run without a graphics device.

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

    /// A message after a refusal in the same frame joins it instead of hiding it, once
    /// per sentence; a later frame's message replaces it, and an info is replaced.
    /// flip: `say` without the join → fails at the first joined text.
    #[test]
    fn a_refusal_on_screen_is_joined_not_replaced() {
        let mut s = EditorSession::new();
        s.fail("photo.heic is not a format Ondin reads");
        s.info("Placed 2 images");
        assert_eq!(
            s.status().text,
            "photo.heic is not a format Ondin reads · Placed 2 images"
        );
        assert_eq!(
            s.status().kind,
            StatusKind::Error,
            "and it stays coloured as one"
        );
        s.info("Placed 2 images");
        assert_eq!(
            s.status().text,
            "photo.heic is not a format Ondin reads · Placed 2 images",
            "a sentence already there is not said twice"
        );

        s.next_status_pass();
        s.info("Copied");
        assert_eq!(
            s.status().text,
            "Copied",
            "the next action's report replaces it"
        );
        s.info("Pasted");
        assert_eq!(
            s.status().text,
            "Pasted",
            "an info is replaced, even in one frame"
        );
        assert_eq!(s.status().kind, StatusKind::Info);
    }

    /// A door's edit over a bound field names what it cut (W alone, then both), and
    /// the chip's own detach (`detach_size_ops`) says nothing.
    ///
    /// Coverage finding: dropping `variables_detached`' asked-for test stays green,
    /// because `keep_size_bindings`' `sets_vars` already leaves a node whose binding
    /// the edit sets, so the chip's detach appends nothing to read. Only an edit
    /// setting one layer's binding and writing over another's would decide it.
    #[test]
    fn a_detach_the_commit_makes_is_said_and_one_asked_for_is_not() {
        use ondin_core::{Cell, Mode, VarKind, Variable};
        let mut s = EditorSession::new();
        let id = add_rect(&mut s);
        let (mode, w, h) = (s.ids.mint_mode(), s.ids.mint_var(), s.ids.mint_var());
        let var = |id, name: &str| Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(10.0))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        };
        assert!(s.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            var(w, "size/w"),
            var(h, "size/h"),
        ])));
        let bind = |s: &mut EditorSession| {
            let ops = ondin_core::build::bind_size_ops(&s.doc, id, [Some(w), Some(h)]);
            assert!(s.commit(Transaction(ops)));
            s.info("");
        };
        let resize = |s: &mut EditorSession, size: Size| {
            assert!(s.commit(Transaction(vec![Operation::SetGeometry {
                id,
                geometry: ondin_core::GeometryPatch::Size(size),
            }])));
        };
        bind(&mut s);
        resize(&mut s, Size::new(30.0, 10.0));
        assert_eq!(s.doc.get(id).unwrap().size_vars(), [None, Some(h)]);
        assert_eq!(s.status().text, "Detached size/w", "W alone");

        bind(&mut s);
        resize(&mut s, Size::new(40.0, 40.0));
        assert_eq!(s.doc.get(id).unwrap().size_vars(), [None, None]);
        assert_eq!(s.status().text, "Detached 2 variables", "both");

        bind(&mut s);
        let ops = ondin_core::build::detach_size_ops(&s.doc, id, [true, false]);
        assert!(s.commit(Transaction(ops)));
        assert_eq!(s.doc.get(id).unwrap().size_vars(), [None, Some(h)]);
        assert_eq!(s.status().text, "", "the chip's detach says nothing");
    }

    #[test]
    fn creating_after_adopting_a_document_does_not_collide() {
        // The id source must be re-seeded with the adopted document's ids.
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

    /// A *full* transaction writing back what is already there is no undo step, no
    /// dirt and no revision bump (so `empty_transactions_are_a_no_op` does not cover
    /// it), and it still ends the gesture: the preview must not outlive it.
    ///
    /// flip: `commit_inner`'s `changes_nothing` guard disabled → fails at the
    /// undo-depth assertion. The depth is compared against one taken before, since
    /// `can_undo` is already true from the fixture's own `add_rect`.
    #[test]
    fn an_edit_that_writes_back_what_is_already_there_is_not_an_undo_step() {
        let mut s = EditorSession::new();
        let id = add_rect(&mut s);
        // Saved *after* the rect, so `dirty` below is about this commit alone.
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

        // Undo brings it back, but selection stays cleared.
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

        // An invalid follow-up must drop the preview.
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

    /// That two boxes agree to 1e-9 — a previewed one and the one the commit gave.
    fn same_box(what: &str, shown: Rect, landed: Rect) {
        assert!(
            (shown.min_x() - landed.min_x()).abs() < 1e-9
                && (shown.min_y() - landed.min_y()).abs() < 1e-9
                && (shown.width() - landed.width()).abs() < 1e-9
                && (shown.height() - landed.height()).abs() < 1e-9,
            "{what}: previewed {shown:?}, committed {landed:?}"
        );
    }

    /// The instance root a copy sits directly under, in `main_and_instance`'s.
    fn instance_of(s: &EditorSession, copy: NodeId) -> NodeId {
        s.doc.get(copy).unwrap().parent().unwrap()
    }

    /// A main frame laid out by `display` holding two rects, the first with
    /// `insets`, and an instance of it beside. Returns the session, the main's
    /// first rect and its copy in the instance.
    fn main_and_instance(
        display: Option<ondin_core::container::Display>,
        insets: ondin_core::Insets,
    ) -> (EditorSession, NodeId, NodeId) {
        let mut s = EditorSession::new();
        let root = s.doc.root();
        let [m, a, b] = [(); 3].map(|_| s.ids.mint());
        let rect = |id, index, w| Operation::CreateNode {
            id,
            parent: m,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: None,
        };
        assert!(s.commit(Transaction(vec![
            Operation::CreateNode {
                id: m,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 200.0),
                },
                transform: Some(Affine::translate((1000.0, 0.0))),
                name: Some("Card".into()),
            },
            rect(a, 0, 40.0),
            rect(b, 1, 60.0),
            Operation::SetDisplay { id: m, display },
            Operation::SetInsets { id: a, insets },
            Operation::SetComponent {
                id: m,
                component: true,
            },
        ])));
        let (tx, made) = ondin_core::insert_subtrees(
            &s.doc,
            &mut s.ids,
            &[ondin_core::Placement {
                nodes: s.doc.capture_subtree(m).unwrap(),
                parent: root,
                index: None,
            }],
            Default::default(),
        );
        assert!(s.commit(tx));
        let copy = s.doc.get(made[0]).unwrap().children()[0];
        assert_eq!(
            s.doc.get(copy).unwrap().link(),
            Some(a),
            "the fixture: linked"
        );
        (s, a, copy)
    }

    /// A main's edit reaches its instance's copy while it is made, and the previewed
    /// box equals the committed one (§5.3d's *preview*): a move in a plain frame, a
    /// move of a layer pinned by insets (`keep_insets`), and a resize of a flex item
    /// (`keep_flex_sizes`; stored values are covered by
    /// `a_tool_write_on_a_mains_flex_item_follows_onto_its_copy_exactly`).
    ///
    /// flip: `set_preview` without its `propagate` call → fails at *"the copy
    /// follows the preview"* on the first edit. flip: the pass run before the flex
    /// holds are added → fails the third, the copy previewed stretched to 200 where
    /// the commit holds it at 45.
    #[test]
    fn a_mains_edit_reaches_its_copy_in_the_preview_as_the_commit_will_place_it() {
        use ondin_core::container::{Display, Flex};
        let pinned = ondin_core::Insets {
            left: Some(ondin_core::LengthPct::Px(20.0)),
            top: Some(ondin_core::LengthPct::Px(20.0)),
            ..Default::default()
        };
        let cases: [(&str, Option<Display>, ondin_core::Insets, bool); 3] = [
            ("a move", None, ondin_core::Insets::default(), false),
            ("a pinned move", None, pinned, false),
            (
                "a flex resize",
                Some(Display::Flex(Flex::default())),
                ondin_core::Insets::default(),
                true,
            ),
        ];
        for (what, display, insets, resize) in cases {
            let (mut s, a, copy) = main_and_instance(display, insets);
            let before = s.preview_world_bounds(copy).expect("bounds");
            let tx = Transaction(vec![match resize {
                true => Operation::SetGeometry {
                    id: a,
                    geometry: ondin_core::GeometryPatch::Size(Size::new(90.0, 45.0)),
                },
                false => Operation::SetTransform {
                    id: a,
                    transform: Affine::translate((55.0, 35.0)),
                },
            }]);
            s.set_preview(&tx);
            let shown = s.preview_world_bounds(copy).expect("preview bounds");
            assert_ne!(shown, before, "{what}: the copy follows the preview");
            assert!(s.commit(tx));
            let landed = s.resolved.world_bounds(copy).expect("committed bounds");
            assert!(
                (shown.min_x() - landed.min_x()).abs() < 1e-9
                    && (shown.min_y() - landed.min_y()).abs() < 1e-9
                    && (shown.width() - landed.width()).abs() < 1e-9
                    && (shown.height() - landed.height()).abs() < 1e-9,
                "{what}: previewed {shown:?}, committed {landed:?}"
            );
        }
    }

    /// A tool's transform write on a main's in-flow flex item follows onto its copy
    /// bit for bit. `rotate_node` and a left-handle `resize_layer` compose on the
    /// item's **slot** and the door rewrites the transform
    /// (`kept_flow_translations`: stored translation kept, new linear part taken),
    /// so a copy that took the tool's value would read as an override silently. The
    /// instance is resized to 437.5 × 213 first so the copy is laid unlike the
    /// main's item; the **height** does the work, since the slot origin is (0, 0)
    /// in both. A left-handle resize on the turned item writes a transform the door
    /// drops whole, so there the size is what follows.
    ///
    /// flip: `commit_inner`'s `propagate` handed the transaction before `keep_insets`
    /// and `keep_flex_sizes` → fails at *"a rotate: the transform followed
    /// exactly"*.
    #[test]
    fn a_tool_write_on_a_mains_flex_item_follows_onto_its_copy_exactly() {
        use crate::preview::Handle;
        use ondin_core::container::{Display, Flex};
        let (mut s, a, copy) = main_and_instance(
            Some(Display::Flex(Flex::default())),
            ondin_core::Insets::default(),
        );
        let inst = instance_of(&s, copy);
        assert!(s.commit(Transaction(vec![Operation::SetGeometry {
            id: inst,
            geometry: ondin_core::GeometryPatch::Size(Size::new(437.5, 213.0)),
        }])));
        let stored = s.doc.get(a).unwrap().transform().translation();
        assert_ne!(
            s.resolved.used_local(&s.doc, a).map(|t| t.translation()),
            Some(stored),
            "the fixture: the item's slot is not its stored translation, so the \
             door has something to keep"
        );
        type Edit = fn(&EditorSession, NodeId) -> Transaction;
        let edits: [(&str, Edit); 3] = [
            ("a rotate", |s, a| {
                let pivot = s.resolved.world_bounds(a).unwrap().center();
                crate::tools::rotate_node(&s.doc, &s.resolved, a, pivot, 0.37)
            }),
            ("a left-handle resize", |s, a| {
                let b = s.resolved.world_bounds(a).unwrap();
                crate::tools::resize_layer(
                    &s.doc,
                    &s.resolved,
                    a,
                    Handle::Left,
                    Point::new(b.min_x() - 13.7, b.center().y),
                    crate::tools::Resize::geometry(false, false),
                )
            }),
            ("a second rotate", |s, a| {
                let pivot = s.resolved.world_bounds(a).unwrap().center();
                crate::tools::rotate_node(&s.doc, &s.resolved, a, pivot, -0.11)
            }),
        ];
        for (what, edit) in edits {
            let was = s.doc.get(a).unwrap().clone();
            let tx = edit(&s, a);
            assert!(
                tx.0.iter()
                    .any(|op| matches!(op, Operation::SetTransform { id, .. } if *id == a)),
                "{what}: the fixture — the tool wrote a transform for the door to keep"
            );
            assert!(s.commit(tx), "{what}: committed");
            let (main, cp) = (s.doc.get(a).unwrap(), s.doc.get(copy).unwrap());
            // A resize's write is the slot shifted to hold the far edge, with the
            // linear part as it was: kept at the stored translation it changes
            // nothing and the door drops it, so the size is what moved.
            assert!(
                main.transform().as_coeffs() != was.transform().as_coeffs()
                    || main.kind() != was.kind(),
                "{what}: the fixture — the main changed"
            );
            assert_eq!(
                main.transform().translation(),
                stored,
                "{what}: the door kept the stored translation"
            );
            assert_eq!(
                cp.transform().as_coeffs(),
                main.transform().as_coeffs(),
                "{what}: the transform followed exactly"
            );
            assert_eq!(cp.kind(), main.kind(), "{what}: and the geometry");
            assert_eq!(cp.item(), main.item(), "{what}: and the layout item");
            assert_eq!(
                ondin_core::reset::overrides(&s.doc, copy),
                Vec::new(),
                "{what}: so nothing reads as an override"
            );
        }
    }

    /// Where the copy's own container places it, the preview places it too. Cases,
    /// one test each so a flip says which it reaches:
    /// - a **right-pinned copy in an instance wider than its main**, the main's
    ///   layer moved: `keep_insets` makes the move insets, which the copy is pinned
    ///   by against its own width;
    /// - an **instance root pinned right in a host frame**, its main resized: the
    ///   root's follow is its size alone and the commit keeps its right edge;
    /// - a **main's in-flow flex item moved**: the commit drops the translation
    ///   (`kept_flow_translations`), so the copy stays in its slot;
    /// - a `SetPivot` on a main's layer: the pivot preview names the main's layer,
    ///   not the copy the last follow names.
    ///
    /// flip: `from_transaction_following` handing the follows to the layout passes
    /// as the hand's → fails this one and the pinned root's, not the flex one.
    /// flip: `preview_follows` over the raw edit → fails this one alone. The flex
    /// case fails only with both flips (the door drops the move's follow and the
    /// layout passes take no follow for a drag; either is enough). flip: the pivot
    /// read from the follows → fails *"the main's pivot"*.
    #[test]
    fn a_pinned_copy_in_a_wider_instance_previews_where_its_insets_place_it() {
        use ondin_core::{Insets, LengthPct};
        let right = Insets {
            right: Some(LengthPct::Px(20.0)),
            top: Some(LengthPct::Px(20.0)),
            ..Default::default()
        };
        let (mut s, a, copy) = main_and_instance(None, right);
        let inst = instance_of(&s, copy);
        assert!(s.commit(Transaction(vec![Operation::SetGeometry {
            id: inst,
            geometry: ondin_core::GeometryPatch::Size(Size::new(400.0, 200.0)),
        }])));
        let tx = Transaction(vec![Operation::SetTransform {
            id: a,
            transform: Affine::translate((55.0, 35.0)),
        }]);
        s.set_preview(&tx);
        let shown = s.preview_world_bounds(copy).expect("preview bounds");
        assert!(s.commit(tx));
        same_box(
            "a pinned copy, wider",
            shown,
            s.resolved.world_bounds(copy).unwrap(),
        );
    }

    /// `a_pinned_copy_in_a_wider_instance_previews_where_its_insets_place_it`'s
    /// second case: an instance root pinned right in a host frame, its main resized.
    #[test]
    fn a_pinned_instance_root_previews_its_mains_resize_from_its_far_edge() {
        use ondin_core::{Insets, LengthPct};
        let (mut s, a, copy) = main_and_instance(None, Insets::default());
        let (inst, main) = (
            instance_of(&s, copy),
            s.doc.get(a).unwrap().parent().unwrap(),
        );
        let host = s.ids.mint();
        let doc_root = s.doc.root();
        assert!(s.commit(Transaction(vec![
            Operation::CreateNode {
                id: host,
                parent: doc_root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(800.0, 400.0),
                },
                transform: Some(Affine::translate((0.0, 500.0))),
                name: None,
            },
            Operation::Reparent {
                id: inst,
                new_parent: host,
                index: 0,
            },
            Operation::SetInsets {
                id: inst,
                insets: Insets {
                    right: Some(LengthPct::Px(30.0)),
                    top: Some(LengthPct::Px(30.0)),
                    ..Default::default()
                },
            },
        ])));
        let before = s.preview_world_bounds(inst).unwrap();
        let tx = Transaction(vec![Operation::SetGeometry {
            id: main,
            geometry: ondin_core::GeometryPatch::Size(Size::new(360.0, 200.0)),
        }]);
        s.set_preview(&tx);
        let shown = s.preview_world_bounds(inst).expect("preview bounds");
        assert_ne!(shown, before, "the fixture: the root follows the resize");
        assert!(s.commit(tx));
        same_box(
            "a pinned root",
            shown,
            s.resolved.world_bounds(inst).unwrap(),
        );
    }

    /// `a_pinned_copy_in_a_wider_instance_previews_where_its_insets_place_it`'s
    /// third case: a main's in-flow flex item moved.
    #[test]
    fn a_flex_copy_stays_in_its_slot_while_its_mains_item_is_dragged() {
        use ondin_core::container::{Display, Flex};
        let (mut s, a, copy) = main_and_instance(
            Some(Display::Flex(Flex::default())),
            ondin_core::Insets::default(),
        );
        let before = s.preview_world_bounds(copy).unwrap();
        let tx = Transaction(vec![Operation::SetTransform {
            id: a,
            transform: Affine::translate((150.0, 90.0)),
        }]);
        s.set_preview(&tx);
        let shown = s.preview_world_bounds(copy).expect("preview bounds");
        same_box("a flex copy stays in its slot", shown, before);
        assert!(s.commit(tx));
        same_box("a flex copy", shown, s.resolved.world_bounds(copy).unwrap());
    }

    /// `a_pinned_copy_in_a_wider_instance_previews_where_its_insets_place_it`'s
    /// fourth case: a `SetPivot` on a main's layer.
    #[test]
    fn the_pivot_preview_names_the_mains_layer_not_its_copy() {
        let (mut s, a, _) = main_and_instance(None, ondin_core::Insets::default());
        s.set_preview(&Transaction(vec![Operation::SetPivot {
            id: a,
            pivot: Some(ondin_core::Pivot::Normalized(ondin_core::kurbo::Vec2::new(
                0.0, 0.0,
            ))),
        }]));
        assert_eq!(
            s.pivot_preview.map(|(id, _)| id),
            Some(a),
            "the main's pivot"
        );
    }

    /// A pinned copy with **its own value for the inset a move changes**: its
    /// transform follows the main's but the commit draws it by its own insets,
    /// unmoved. Without `preview_follows` restating the insets it previews at the
    /// main's placement.
    #[test]
    fn a_copy_pinned_by_its_own_inset_previews_unmoved_by_its_mains_move() {
        use ondin_core::{Insets, LengthPct};
        let left = Insets {
            left: Some(LengthPct::Px(20.0)),
            top: Some(LengthPct::Px(20.0)),
            ..Default::default()
        };
        let (mut s, a, copy) = main_and_instance(None, left);
        // The copy's own left, written as an inset alone so its transform stays
        // its main's.
        assert!(s.commit(Transaction(vec![Operation::SetInsets {
            id: copy,
            insets: Insets {
                left: Some(LengthPct::Px(70.0)),
                ..left
            },
        }])));
        assert_eq!(
            s.doc.get(copy).unwrap().transform(),
            s.doc.get(a).unwrap().transform(),
            "the fixture: the copy's transform is its main's"
        );
        let tx = Transaction(vec![Operation::SetTransform {
            id: a,
            transform: Affine::translate((55.0, 20.0)),
        }]);
        s.set_preview(&tx);
        let shown = s.preview_world_bounds(copy).expect("preview bounds");
        assert!(s.commit(tx));
        same_box(
            "a copy pinned by its own inset",
            shown,
            s.resolved.world_bounds(copy).unwrap(),
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

    /// `preview_pivot_in` answers what `preview_pivot` does, and `previewed_pivot`
    /// answers `Some` for any existing node, so `preview_pivot` always walks the box.
    ///
    /// Coverage finding: making `preview_pivot_in` ignore its argument keeps the
    /// first assertion green (it is the same box); the saving is established by
    /// reading the call site in `selection_handles`.
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

    /// The origin readout follows a live preview: a scrubbed field re-reads its base
    /// every frame and `edit_valve` only *previews* while the pointer is down, so a
    /// base taken from the committed node would reset each frame and the number
    /// would never move.
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

        // Previewing the *clear* is expressible too.
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
        // A fixed actor id would collide ids across sessions (§5.2).
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

    /// Guides and layers are never selected together, whichever way round the two
    /// arrive: every mutator clears the other half.
    #[test]
    fn selecting_a_guide_and_selecting_layers_are_exclusive() {
        let mut s = EditorSession::new();
        let rect = add_rect(&mut s);
        let guide = add_guide(&mut s, 40.0);

        s.selection.set_one(rect);
        s.selection.set_guide(guide.id);
        assert_eq!(s.selection.guides(), [guide.id]);
        assert!(s.selection.ids().is_empty(), "the layer must have let go");

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

        // The two emptiness questions differ, and the canvas colour picker's orphan
        // rule turns on which it asks: with a guide selected there are no *layers*,
        // but the inspector has a subject.
        s.selection.set_guide(guide.id);
        assert!(s.selection.is_empty(), "no layers are selected");
        assert!(!s.selection.nothing(), "but something is");
        s.selection.clear();
        assert!(s.selection.nothing());

        // Deleting the guide prunes it, as deleting a node does.
        s.selection.set_guide(guide.id);
        assert!(s.commit(Transaction(vec![Operation::RemoveGuide { id: guide.id }])));
        assert!(s.selection.guides().is_empty());
        // ...and undo does not silently bring the selection back with it.
        s.undo();
        assert!(s.doc.guide(guide.id).is_some());
        assert!(s.selection.guides().is_empty());
    }

    /// Several guides at once, by Shift+click, with the layer half empty throughout:
    /// `is_empty`, `len` and `single` answer about layers, so *two* selected guides
    /// must read as no layers, or a guide would reach `align`, `nudge` or
    /// `outermost` as a node id.
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

        assert!(s.selection.is_empty(), "two guides are still no layers");
        assert_eq!(s.selection.len(), 0);
        assert_eq!(s.selection.single(), None);
        assert!(!s.selection.nothing());

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

        // Pruning keeps the survivors.
        s.selection.toggle_guide(b.id);
        assert!(s.commit(Transaction(vec![Operation::RemoveGuide { id: a.id }])));
        assert_eq!(s.selection.guides(), [b.id]);
    }

    /// *View ▸ Lock guides* lets go of any held guide (locked means unselectable)
    /// but leaves **layers** alone; clearing everything would only deselect the
    /// user's work.
    #[test]
    fn clearing_guides_leaves_a_layer_selection_alone() {
        let mut s = EditorSession::new();
        let (rect, other) = (add_rect(&mut s), add_rect(&mut s));
        let guide = add_guide(&mut s, 40.0);

        s.selection.set_guide(guide.id);
        s.selection.clear_guides();
        assert!(s.selection.guides().is_empty());

        s.selection.set(vec![rect, other]);
        s.selection.set_key(other);
        s.selection.clear_guides();
        assert_eq!(s.selection.ids(), [rect, other], "the layers must survive");
        assert_eq!(s.selection.key(), Some(other), "and so must the key layer");
    }

    // --- the key layer ----------------------------------------------------

    /// The designation is explicit and reversible by the same gesture.
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

    /// A key has to be one *of* the selection.
    #[test]
    fn a_layer_outside_the_selection_cannot_be_made_key() {
        let mut s = EditorSession::new();
        let (a, b, outsider) = (add_rect(&mut s), add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(outsider);
        assert_eq!(s.selection.key(), None);
    }

    /// Below two layers `key()` answers `None` however the selection got there, but
    /// the designation survives growing the selection back.
    #[test]
    fn a_key_is_not_reported_for_a_selection_of_one() {
        let mut s = EditorSession::new();
        let (a, b) = (add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b]);
        s.selection.set_key(b);

        s.selection.toggle(a);
        assert_eq!(s.selection.len(), 1);
        assert_eq!(s.selection.key(), None, "one layer, so no key");

        s.selection.toggle(a);
        assert_eq!(s.selection.key(), Some(b));
    }

    /// Deselecting the key gives it up; it does not come back on re-selection.
    #[test]
    fn deselecting_the_key_drops_the_designation() {
        let mut s = EditorSession::new();
        let (a, b, c) = (add_rect(&mut s), add_rect(&mut s), add_rect(&mut s));
        s.selection.set(vec![a, b, c]);
        s.selection.set_key(b);

        s.selection.toggle(b);
        assert_eq!(s.selection.key(), None);
        s.selection.toggle(b);
        assert_eq!(s.selection.key(), None);
    }

    /// Replacing the selection replaces the key with it, the marquee's `set` included.
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

    /// **A swap's rewrite of a styled copy is not a detach the status names**:
    /// the copy follows an unstyled main now; a commit's own `None` write past
    /// the edit on a layer the edit did not swap still is.
    ///
    /// flip: `styles_detached`' swapped-subtree filter removed → fails *"the
    /// swap names nothing"*.
    #[test]
    fn a_swap_to_an_unstyled_main_names_no_detached_style() {
        let mut s = EditorSession::new();
        let (a, b) = (add_rect(&mut s), add_rect(&mut s));
        let style = s.ids.mint_style();
        assert!(s.commit(Transaction(vec![
            Operation::AddStyle {
                style: ondin_core::style::Style {
                    id: style,
                    name: "Brand".into(),
                    value: ondin_core::style::StyleValue::Fills(vec![]),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFills {
                id: a,
                fills: vec![]
            },
            Operation::SetFills {
                id: b,
                fills: vec![]
            },
            Operation::SetFillStyle {
                id: a,
                style: Some(style),
            },
            Operation::SetFillStyle {
                id: b,
                style: Some(style),
            },
        ])));
        let cut = |id| Operation::SetFillStyle { id, style: None };
        let swap = Operation::SetSwap { id: a, swap: None };
        let tx = Transaction(vec![swap.clone(), cut(a)]);
        assert!(
            styles_detached(&s.doc, &tx, 1).is_empty(),
            "the swap names nothing"
        );
        let tx = Transaction(vec![swap, cut(b)]);
        assert_eq!(styles_detached(&s.doc, &tx, 1), vec!["Brand".to_string()]);
    }
}
