//! The renderer boundary (§6.2): the viewport, and the preview override layer.
//!
//! The drawing boundary itself is [`crate::scene::ScenePainter`]: the
//! document→scene walk is written once and each backend implements the painter.
//! An earlier design also named a `trait Renderer` plus a `RenderTarget`
//! object; both sat here unimplemented while `ScenePainter` did the work, so
//! they were removed rather than left as a decoy (§15, D2).

use ondin_core::kurbo::Affine;
use ondin_core::peniko::Color;
use ondin_core::text::TextLayout;
use ondin_core::{
    Document, NodeId, NodeKind, Operation, Paint, Resolved, Transaction, build, text,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// The visible region. `zoom` is derived from `view` vs `pixel_size` — it is
/// not stored, so there is no redundant field to keep in sync (§6.2).
pub struct Viewport {
    pub view: ondin_core::kurbo::Rect,
    pub pixel_size: (u32, u32),
}

impl Viewport {
    /// Pixels per document unit along x (uniform scale assumed).
    pub fn zoom(&self) -> f64 {
        self.pixel_size.0 as f64 / self.view.width()
    }
}

/// What a pending transaction would change about one node, without changing it.
#[derive(Default, Clone, Debug)]
pub struct NodeOverride {
    /// Replacement LOCAL transform.
    pub transform: Option<Affine>,
    /// Replacement kind — geometry edits and text content/style changes.
    pub kind: Option<NodeKind>,
    pub paint: Option<Paint>,
    /// Replacement effect stack (§5.3a).
    ///
    /// **A field of its own rather than something folded into `paint`**, because
    /// the model keeps them apart for a reason the preview inherits: effects live
    /// on containers, which have no `Paint` for this to have ridden along inside.
    ///
    /// The whole stack, like the operation that produces it — so scrubbing one
    /// shadow's blur previews through the same door as reordering two of them,
    /// and there is one description of the edit rather than one per field
    /// (§15 D245 is the bug where a field has no door at all).
    pub effects: Option<Vec<ondin_core::Effect>>,
    pub opacity: Option<f32>,
    pub visible: Option<bool>,
    /// Re-shaped layout, when the kind change affected text.
    pub text: Option<TextLayout>,
}

/// Artwork that does not exist in the document yet, drawn among `parent`'s children at
/// [`Self::index`] — a shape being dragged out with a create tool, or the copy an
/// Alt-drag is carrying.
#[derive(Clone, Debug)]
pub struct Ghost {
    /// The document node the ghost hangs off. Only the root of the subtree has
    /// one; below it, parentage is the nesting of [`GhostNode::children`].
    pub parent: NodeId,
    /// Where among `parent`'s children it lands — the index its `InsertSubtree`
    /// names, so the preview stacks it where the commit will.
    ///
    /// **This used to be ignored, and ghosts simply drew last.** That was right for
    /// as long as every caller appended: an Alt-drag copy is the thing under the
    /// pointer and it did land on top. Once a copy started landing *immediately above
    /// its original* rather than above everything, "last" became a different z-order
    /// from the commit's, which is exactly the divergence a preview may not have.
    pub index: usize,
    pub node: GhostNode,
}

/// One node of a ghost subtree: everything the scene walk reads off a `Node`,
/// for artwork there is no `Node` for yet.
///
/// A ghost used to be a single childless leaf, which is all a shape tool needs
/// and is why it was one — but it also meant no gesture could preview a copy of
/// a *group* or a *frame*, and that single limitation is what forced Alt-drag to
/// move the original and leave the copy behind (§15, D40). Carrying children
/// costs one recursive walk and removes the whole class of restriction.
#[derive(Clone, Debug)]
pub struct GhostNode {
    pub id: NodeId,
    /// LOCAL to the parent ghost, or to `Ghost::parent` at the root.
    pub transform: Affine,
    pub kind: NodeKind,
    pub paint: Paint,
    /// The effect stack this ghost draws with (§5.3a).
    ///
    /// Not an `Option` as it is on [`NodeOverride`]: a ghost is the whole node
    /// rather than a patch over one, so "no effects" is the empty stack and there
    /// is no third state meaning "ask the document". An Alt-drag copy therefore
    /// carries its original's shadow from the first frame, which is what the copy
    /// will have once it commits.
    pub effects: Vec<ondin_core::Effect>,
    pub text: Option<TextLayout>,
    /// A `Boolean` ghost's combined outline, in its own local space — the ghost's
    /// share of what `Resolved` keeps for a committed boolean.
    ///
    /// **Evaluated once, when the subtree is built**, beside the shaped text below
    /// and for the identical reason: the scene walk must be *handed* derived geometry
    /// rather than compute it, and path arithmetic per frame is worse than shaping
    /// per frame. Alt-dragging a copy of a boolean group used to preview as its
    /// *operands* — the shapes the operation is supposed to consume, drawn because
    /// the ghost walk had no result to draw and no reason to stop.
    pub path: Option<kurbo::BezPath>,
    pub opacity: f32,
    pub visible: bool,
    /// Whether this ghost clips its children, as `Node::clip` does.
    pub clip: bool,
    /// Whether this ghost masks the ghosts beside it, as `Node::mask` does.
    ///
    /// Only ever read of a **child** ghost, by the parent's walk — a run is a fact
    /// about a list of siblings, and the root of a ghost forest has none.
    pub mask: bool,
    /// How it masks, as `Node::mask_mode` does. Read only when `mask` is set.
    pub mask_mode: ondin_core::MaskMode,
    pub children: Vec<GhostNode>,
}

impl GhostNode {
    /// A childless ghost with a layer's default properties — a shape tool
    /// dragging out a new node.
    fn leaf(id: NodeId, transform: Affine, kind: NodeKind, text: Option<TextLayout>) -> Self {
        Self {
            id,
            transform,
            kind,
            paint: Paint::default(),
            // A shape dragged out with a tool has no effects for the same reason
            // it has no fills of its own yet: nothing has been authored on it.
            effects: Vec::new(),
            text,
            // No shape tool makes a boolean: it takes two layers that already exist,
            // so there is no childless boolean for this to be `Some` for.
            path: None,
            opacity: 1.0,
            visible: true,
            clip: false,
            // No shape tool makes a mask either: it is a designation put on a
            // layer that already exists, so a leaf being dragged out is never one.
            mask: false,
            mask_mode: ondin_core::MaskMode::default(),
            children: Vec::new(),
        }
    }

    /// This ghost or the one under it with `id`.
    fn find_mut(&mut self, id: NodeId) -> Option<&mut Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    fn find(&self, id: NodeId) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

/// Uncommitted gesture state, drawn without touching the `Document`
/// (invariant 5).
///
/// This is what makes a drag cheap. The alternative the app used to run —
/// clone the document, apply the pending transaction, re-resolve — is correct
/// but costs O(nodes) per frame *including re-shaping every text node*, where
/// the gesture itself touches a handful. Overrides cost O(nodes touched).
///
/// Built from the very transaction the gesture will eventually commit
/// ([`Self::from_transaction`]), so what is previewed and what is committed
/// cannot drift: there is only one description of the edit.
///
/// The structural operations that only *add* artwork become a [`Ghost`]: the
/// single-node `CreateNode` a shape tool emits, and the `InsertSubtree` an
/// Alt-drag carries. The ones that move or remove existing nodes — delete,
/// reparent, reorder — are deliberately **not** representable, because a patch
/// cannot say "this node is somewhere else in the tree now"; `from_transaction`
/// returns `None` for those rather than silently previewing something different
/// from what will be committed.
#[derive(Default, Debug)]
pub struct RenderOverrides {
    nodes: FxHashMap<NodeId, NodeOverride>,
    ghosts: Vec<Ghost>,
    /// Nodes whose world transform may have moved, including descendants. The
    /// scene walk must not cull these against `Resolved`'s stale bounds.
    moved: FxHashSet<NodeId>,
    /// Nodes on their way *out* of the frame they are in: drawn in their own place
    /// in the tree, but outside their parent's clip. See [`Self::escape_clip`].
    escaping: FxHashSet<NodeId>,
    /// Whether the ghosts are leaving too — see [`Self::escape_clip_ghosts`].
    escaping_ghosts: bool,
    /// Outlines for the booleans this gesture reshapes, keyed by node — the
    /// preview's twin of `Resolved`'s cache. See [`Self::reevaluate_booleans`].
    boolean: FxHashMap<NodeId, kurbo::BezPath>,
    /// A ground being dragged in the picker. Not part of the scene walk — the
    /// backdrop is the clear colour, handed to the renderer separately — so it
    /// rides here only to reach the caller that passes it.
    canvas_background: Option<Color>,
    /// Artwork drawn to *explain* a mode rather than to preview an edit — today
    /// the faded whole picture behind a layer being cropped (§15 D268).
    ///
    /// **A ghost, because it is the same problem** — ink with no `Node` behind it,
    /// wanted at a particular depth in one parent's z-order — and a separate field,
    /// because it is not a preview and must not be counted as one. That is the
    /// whole reason it is not simply pushed onto `ghosts`: [`Self::is_empty`] is
    /// how the app asks "is a gesture in flight" (`Session::has_gesture_preview`),
    /// and a mode that answered yes for as long as it was open would have
    /// `cancel_gesture` unwinding a drag nobody started on every right-click.
    ///
    /// **Re-installed every frame, from one place** (`OndinApp::draw_canvas`), so
    /// it survives a commit without the sprawl `preview_guide` pays for: a
    /// `set_preview` replaces the whole of this struct, and a second call every
    /// call site owes is a call site that will one day not owe it.
    ///
    /// **The `NodeId` is the layer it is glued to**, and it is here rather than
    /// implied because [`Self::unsnapped`] needs it — see there.
    chrome: Option<(NodeId, Ghost)>,
    /// A text layout the caller has **already shaped**, for one node, live only
    /// while [`Self::from_transaction_shaped`] is building (§15 D592,
    /// `[A4-L4-01]`).
    ///
    /// 🚨 **A construction argument wearing a field, and that is deliberate.**
    /// [`Self::set_kind`] is reached from eleven arms of [`Self::absorb`] through
    /// [`Self::patch_text`]; threading a parameter to all of them to say something
    /// about *one* node would put the same `None` at ten call sites. It is set
    /// before the absorb loop, read by `set_kind`, and cleared before the value is
    /// returned, so no `RenderOverrides` a caller can observe ever holds one.
    ///
    /// **What it is for.** A live text session is a commit ahead of the document
    /// by construction — one `SetText` at `finish_text_edit`, §9.3 — so
    /// `set_kind`'s *"re-shape only when the text actually changed"* guard can
    /// never fire for it, and every keystroke shaped the whole node a **second**
    /// time. Measured in release on one node, mean of 20 inserts: at 16,000
    /// characters, 4.76 ms in `TextEdit::reshape` and 5.44 ms again here; at
    /// 64,000, 19.0 and 21.8. `TextEdit` has the answer already — `text_layout()`
    /// is the `extract` half without the shape, 0.63 ms at 16k — and this is how
    /// it reaches the preview.
    prepared_text: Option<(NodeId, TextLayout)>,
}

impl RenderOverrides {
    /// Whether this preview says nothing — no patched node, no ghost, no ground.
    ///
    /// **[`Self::chrome`] is deliberately not a term.** This is the question "is
    /// an edit in flight", asked by the fast paths that skip re-deriving bounds
    /// and by `Session::has_gesture_preview`, and a mode's explanatory artwork is
    /// an answer to a different one. The scene walk asks
    /// [`Self::ghosts_of`] instead, which is why nothing draws less for this.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ghosts.is_empty() && self.canvas_background.is_none()
    }

    /// The ground this preview asks for, if it touches the ground at all.
    pub fn canvas_background(&self) -> Option<Color> {
        self.canvas_background
    }

    pub fn get(&self, id: NodeId) -> Option<&NodeOverride> {
        self.nodes.get(&id)
    }

    /// Whether `id` sits in a subtree the gesture is moving, so its cached
    /// world bounds cannot be trusted for culling.
    pub fn moved(&self, id: NodeId) -> bool {
        self.moved.contains(&id)
    }

    /// Draw `id` outside its parent's clip, in its own place in the tree.
    ///
    /// **This is what makes dragging a layer out of a frame look like dragging it
    /// out of a frame.** A clipping artboard limits its children to its own box,
    /// and a child being dragged past that edge is still a child until the gesture
    /// commits — so it drew cut off at the frame's boundary, with only the selection
    /// box outside to say where it really was, and it snapped into view at the
    /// instant of release.
    ///
    /// **Set exactly when the drop would take the layer out of that frame**, which
    /// is what makes the drawing worth trusting: a shape rendering in full is the
    /// tool saying "release now and this leaves the frame", and a shape still
    /// clipped is it saying the opposite. The threshold is therefore not this
    /// layer's to choose — it belongs to `canvas::frame_covering`, which owns what
    /// the release does (more than half the layer's area inside a frame keeps it).
    ///
    /// **Z-order is untouched.** An escaping layer is drawn where its siblings
    /// expect it, not on top: which layer covers which is information the user is
    /// steering by, and a dragged shape that jumped to the front would lose it. So
    /// the parent's clip layer is closed and reopened *around* this child rather
    /// than the child being deferred to the end of the walk.
    ///
    /// Set from the *gesture*, never derived from the overrides. "Has a transform
    /// override" is not the same question: a resize gives a child one too, and a
    /// child growing past its frame's edge is meant to stay clipped — deriving it
    /// would trade this pop-on-release for the mirror image of it.
    pub fn escape_clip(&mut self, id: NodeId) {
        self.escaping.insert(id);
    }

    /// Whether `id` is on its way out of its parent's clip.
    pub fn escapes_clip(&self, id: NodeId) -> bool {
        self.escaping.contains(&id)
    }

    /// The ghosts are leaving their parent frame too, like [`Self::escape_clip`].
    ///
    /// **The Alt-drag copy needs this as much as the layer does**: the ghost hangs
    /// off the original's parent, so a copy carried out of a clipping frame was cut
    /// off at exactly the same edge. One switch for all of them, because the only
    /// gesture that carries a ghost *and* moves it is Alt-drag — a shape being
    /// created is being drawn inside its frame, where clipping is the truth.
    pub fn escape_clip_ghosts(&mut self) {
        self.escaping_ghosts = true;
    }

    /// Whether the ghosts draw outside their parent's clip.
    pub fn ghosts_escape_clip(&self) -> bool {
        self.escaping_ghosts
    }

    /// The ghosts hanging off `parent` — the preview's, and the chrome one.
    ///
    /// One iterator rather than two readers, so everything that already reasons
    /// about ghosts covers the chrome one for free: the z-ordered draw loop, and
    /// the cull that must not return before reaching a ghost drawn clear of every
    /// committed bound.
    pub fn ghosts_of(&self, parent: NodeId) -> impl Iterator<Item = &Ghost> {
        self.ghosts
            .iter()
            .chain(self.chrome.iter().map(|(_, g)| g))
            .filter(move |g| g.parent == parent)
    }

    /// Install (or clear) the mode's explanatory artwork — see [`Self::chrome`].
    /// `glued` is the layer the ghost is a continuation of, and the ghost itself.
    ///
    /// Takes an `Option` rather than having a companion "clear", because the
    /// caller runs unconditionally once a frame and the honest spelling of "no
    /// mode is open" is the same call with nothing in it.
    pub fn set_chrome_ghost(&mut self, glued: Option<(NodeId, Ghost)>) {
        self.chrome = glued;
    }

    /// Whether `id`'s box is exempt from device-pixel snapping
    /// (`scene::snapped_box_world`).
    ///
    /// **Exactly two things are, and they are exempt *together*** (§15 D272): the
    /// chrome ghost, and the layer it is glued to. Snapping rounds a box's edges to
    /// the grid and then rebuilds its scale from the rounded span, so two
    /// rectangles of different sizes round *differently* — and the crop offcut is a
    /// continuation of the layer beneath it, drawn as a separate and much larger
    /// rectangle. Rounding them apart slid the offcut against the picture it
    /// belongs to by a device pixel at its far edge, changing as the crop was
    /// dragged. Reported as exactly that.
    ///
    /// **Neither rounds, rather than the ghost being made to round like the
    /// layer.** Making them *equal* is the requirement; the layer's rounding is
    /// computed deep in the walk from the device base, so the only place the two
    /// can be made to agree is by both opting out. What that costs is the seam
    /// mitigation `SNAP_BOXES_TO_DEVICE_PIXELS` buys, on one layer, for as long as
    /// somebody is looking at its crop — which is the trade that constant's own
    /// doc describes in the other direction: snapping makes edges step rather than
    /// glide, and a stepping edge is the thing being complained about here.
    ///
    /// Derived from [`Self::chrome`] rather than kept in a set of its own, so the
    /// pair cannot come apart: there is one place that says which two boxes have
    /// to agree, and it is the same place that creates the second one.
    pub fn unsnapped(&self, id: NodeId) -> bool {
        self.chrome
            .as_ref()
            .is_some_and(|(node, g)| *node == id || g.node.id == id)
    }

    /// The local transform to draw `id` with.
    ///
    /// **Ghosts first**, as every other reader of a ghost's fields does: `absorb`
    /// writes a `SetTransform` aimed at a ghost into the ghost itself, so an override
    /// entry is precisely what a ghost id does *not* have — and this used to answer
    /// `Affine::IDENTITY` for one, having found neither an override nor a document
    /// node. Nothing drew wrong from that (the ghost walk reads `GhostNode::transform`
    /// directly), but a boolean evaluating over a ghost operand asks *here*.
    pub fn transform_of(&self, doc: &Document, id: NodeId) -> Affine {
        if let Some(g) = self.ghost(id) {
            return g.transform;
        }
        self.get(id)
            .and_then(|o| o.transform)
            .or_else(|| doc.get(id).map(|n| n.transform()))
            .unwrap_or(Affine::IDENTITY)
    }

    /// The children `id` has as far as a boolean evaluated over it is concerned: its
    /// committed ones, plus any ghost this preview has parented to it.
    ///
    /// **The ghost is a real operand.** Alt-dragging a copy of an operand *inside* a
    /// boolean parents the copy to the boolean (`canvas::ClonedSubtree` lands a copy
    /// beside its source), and on release it joins the operation — so the preview has
    /// to combine it too, or the shape sits at its pre-drag outline and snaps on
    /// release, which is the very report `reevaluate_booleans` exists to answer, one
    /// gesture over.
    ///
    /// **Inserted at [`Ghost::index`], which is what the commit will do.** It used to
    /// append, and that was exact twice over — ghosts drew after their parent's real
    /// children, and `insert_subtrees` appended on commit — until a copy started
    /// landing immediately above its original rather than above everything.
    ///
    /// ⚠️ **The argument that excused appending after that was measured false on
    /// 2026-08-31** (§15 D239's amendment). It ran: `boolean::evaluate` is a left fold
    /// in which only the *first* operand is order-sensitive, Subtract's tail being a
    /// union and the other three commuting, so a copy that can never take slot 0 is
    /// safe wherever else it lands. **The commuting is true of the mathematics and not
    /// of this implementation.** Moving one operand of a twenty-circle `Exclude` to the
    /// end of the fold changes the result by about **3% of its area** — 2,482 units of
    /// 85,850, at an unchanged element count — because the accumulated geometry differs,
    /// and the error grows with the operand count (0.009 at five operands, 1,654 at
    /// twelve). `Union` and `Subtract` move by float noise; `Exclude` does not.
    ///
    /// So the preview was drawing a measurably different shape from the commit for an
    /// Alt-drag into a large `Exclude`, and the fold order is now simply the commit's.
    /// *Ascending, because that is the order `insert_subtrees` applies them in and two
    /// ghosts landing in one parent must not swap.*
    fn operand_children(&self, doc: &Document, id: NodeId) -> Vec<NodeId> {
        if let Some(g) = self.ghost(id) {
            return g.children.iter().map(|c| c.id).collect();
        }
        let mut out = doc
            .get(id)
            .map(|n| n.children().to_vec())
            .unwrap_or_default();
        let mut ghosts: Vec<(usize, NodeId)> =
            self.ghosts_of(id).map(|g| (g.index, g.node.id)).collect();
        ghosts.sort_by_key(|(i, _)| *i);
        for (index, gid) in ghosts {
            // Clamped rather than trusted: an index past the end is what appending
            // always did, and is the only sane reading of "insert at 9 of 3".
            out.insert(index.min(out.len()), gid);
        }
        out
    }

    /// Whether `id` is visible, ghosts included.
    fn operand_visible(&self, doc: &Document, id: NodeId) -> bool {
        if let Some(g) = self.ghost(id) {
            return g.visible;
        }
        self.get(id)
            .and_then(|o| o.visible)
            .unwrap_or_else(|| doc.get(id).is_some_and(|n| n.visible()))
    }

    /// The world transform `id` would have, composed through any overridden
    /// ancestors. Falls back to the resolved value when nothing on the chain is
    /// overridden, which is both faster and bit-identical.
    pub fn world_transform(&self, doc: &Document, res: &Resolved, id: NodeId) -> Option<Affine> {
        if self.is_empty() {
            return res.world_transform(id);
        }
        let mut chain = Vec::new();
        let mut cursor = Some(id);
        while let Some(c) = cursor {
            chain.push(c);
            cursor = doc.get(c)?.parent();
        }
        let mut world = Affine::IDENTITY;
        for node in chain.iter().rev() {
            world *= self.transform_of(doc, *node);
        }
        Some(world)
    }

    /// Project a pending transaction onto the derived layer.
    ///
    /// `None` means the transaction cannot be previewed this way and the caller
    /// should not show a preview at all — better a missing preview than one
    /// that disagrees with the edit it claims to show.
    pub fn from_transaction(doc: &Document, res: &Resolved, tx: &Transaction) -> Option<Self> {
        Self::from_transaction_shaped(doc, res, tx, None)
    }

    /// [`Self::from_transaction`] where the caller has **already shaped** one
    /// node's text and would otherwise be paying for it twice (§15 D592,
    /// `[A4-L4-01]`).
    ///
    /// The one caller is a live text session: `TextEdit::reshape` has just run
    /// parley over the edited string, and building the preview from the resulting
    /// `SetText` ran it again from byte zero — 5.44 ms against 4.76 for a
    /// 16,000-character node, per keystroke, none of which could produce a
    /// different answer. `TextEdit::text_layout()` is the `extract` half of the
    /// same work without the shape.
    ///
    /// ⚠️ **`shaped` must be the layout of the kind this transaction produces for
    /// that node, or the canvas draws one thing and every measurement reads
    /// another.** That is why the argument is a whole `TextLayout` keyed to a
    /// `NodeId` rather than a flag: a caller cannot pass it by accident, and the
    /// session's `text_layout()` is derived from exactly the parts its `SetText`
    /// carries.
    pub fn from_transaction_shaped(
        doc: &Document,
        res: &Resolved,
        tx: &Transaction,
        shaped: Option<(NodeId, TextLayout)>,
    ) -> Option<Self> {
        let mut out = Self {
            prepared_text: shaped,
            ..Self::default()
        };
        for op in &tx.0 {
            out.absorb(doc, res, op)?;
        }
        // Cleared before anything can observe it — see the field.
        out.prepared_text = None;
        out.reevaluate_booleans(doc, res);
        Some(out)
    }

    /// Re-derive the outline of every boolean above something this preview moves.
    ///
    /// **Without this a boolean is a frame behind its own operands.** `Resolved`'s
    /// cache is only refreshed on commit, so dragging an operand showed the shape
    /// the boolean had *before* the drag, snapping into place on release — reported
    /// as "the boolean is left behind and re-evaluates when I drop".
    ///
    /// Once per preview build, not once per frame: this is the same reason the scene
    /// walk is handed a shaped text layout rather than shaping one. Deepest-first, so
    /// a boolean nested inside a boolean is evaluated before the parent that reads it.
    ///
    /// **Two kinds of seed, because there are two ways a preview changes a boolean's
    /// shape.** A moved node reshapes every boolean *above* it, and a ghost reshapes
    /// the boolean it has been parented *to* — so a ghost's seed is its parent itself
    /// rather than its parent's parent. Missing the second meant Alt-dragging a copy of
    /// an operand out of a boolean previewed the pre-drag outline: the copy joins the
    /// operation on release, so the shape jumped at the instant of the drop.
    fn reevaluate_booleans(&mut self, doc: &Document, res: &Resolved) {
        let mut affected: Vec<(usize, NodeId)> = Vec::new();
        let seeds = self
            .moved
            .iter()
            .filter_map(|m| doc.get(*m).and_then(|n| n.parent()))
            .chain(self.ghosts.iter().map(|g| g.parent));
        // **Depth from the root, taken per node** — not the distance walked from
        // the seed (§15 D461). `[S11.2-L1-02]`: this counted *upward from the
        // seed*, so a boolean nearer the root carried the **larger** number and
        // `Reverse` evaluated the outermost one first — the opposite of the rule
        // the comment below states. Measured in release on
        // `Subtract(Union(r1, r2), r3)`, dragging `r2` 260 units: the inner
        // union's preview followed the pointer to `x1 = 360` while the outer
        // subtract sat at its **pre-drag** `x1 = 160` for the whole gesture and
        // snapped into place on release — the exact report this function exists to
        // answer, one level of nesting up.
        //
        // ⚠️ **Silent, because the fallback is plausible.** With the outer
        // evaluated first, `self.boolean` has no entry for the inner one yet, so
        // the `nested:` closure falls through to `Resolved`'s **committed** cache
        // — a real path of the right shape, from before the gesture. §6.2's rule
        // is that a missing preview is recoverable and a lying one is not.
        //
        // ⚠️ **And reversing the old key alone would not have been the fix.** A
        // distance from *a* seed is not comparable between seeds: a multi-selection
        // drag of two operands at different tree depths ordered their booleans by
        // an accident of which seed reached each first. A depth from the root is
        // one scale for the whole document, so it composes.
        fn depth_from_root(doc: &Document, mut id: NodeId) -> usize {
            let mut d = 0usize;
            while let Some(p) = doc.get(id).and_then(|n| n.parent()) {
                d += 1;
                id = p;
            }
            d
        }
        for seed in seeds {
            let mut cursor = Some(seed);
            while let Some(c) = cursor {
                if matches!(doc.get(c).map(|n| n.kind()), Some(NodeKind::Boolean { .. }))
                    && !affected.iter().any(|(_, id)| *id == c)
                {
                    affected.push((depth_from_root(doc, c), c));
                }
                cursor = doc.get(c).and_then(|n| n.parent());
            }
        }
        if affected.is_empty() {
            return;
        }
        // Deepest first: a nested boolean's result is an operand of its parent's.
        affected.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
        for (_, id) in affected {
            let Some(op) = doc.get(id).and_then(|n| match n.kind() {
                NodeKind::Boolean { op } => Some(*op),
                _ => None,
            }) else {
                continue;
            };
            let path = ondin_core::boolean::evaluate_node(
                id,
                op,
                &ondin_core::boolean::Operands {
                    local_of: &|c| self.transform_of(doc, c),
                    // **The overridden kind, not the committed one.** A resize
                    // expresses itself partly as new *geometry*, so reading the
                    // document here moved the operands without resizing them: the
                    // shape shuffled about under the pointer and snapped to the
                    // right size on release.
                    kind_of: &|c| self.current_kind(doc, c),
                    // The committed children plus this preview's ghosts — a preview
                    // cannot reparent, reorder or delete (`absorb` refuses all three),
                    // but it can *add*, and an added operand counts. See
                    // [`Self::operand_children`].
                    children_of: &|c| self.operand_children(doc, c),
                    visible_of: &|c| self.operand_visible(doc, c),
                    // The pending result if this preview has one, then a ghost's own —
                    // evaluated when its subtree was built — else the committed cache,
                    // which is what an untouched nested boolean still has.
                    nested: &|c| {
                        self.boolean
                            .get(&c)
                            .cloned()
                            .or_else(|| self.ghost(c).and_then(|g| g.path.clone()))
                            .or_else(|| res.boolean_path(c).cloned())
                    },
                },
            );
            match path {
                Some(path) => {
                    self.boolean.insert(id, path);
                }
                // An operation whose result has become empty mid-drag: recorded as
                // such, so the walk draws nothing rather than the last good shape.
                None => {
                    self.boolean.insert(id, kurbo::BezPath::new());
                }
            }
        }
    }

    /// The outline to draw a `Boolean` with during this preview, if the gesture
    /// changed it.
    pub fn boolean_path(&self, id: NodeId) -> Option<&kurbo::BezPath> {
        self.boolean.get(&id)
    }

    /// Patch one field of a text node's kind as a render override, re-measuring
    /// it. `None` — so the whole preview is declined — when the target is not a
    /// text node, because there is nothing honest to show for it.
    fn patch_text(
        &mut self,
        doc: &Document,
        res: &Resolved,
        id: NodeId,
        patch: impl FnOnce(&mut NodeKind),
    ) -> Option<()> {
        let mut kind = self.current_kind(doc, id)?;
        if !matches!(kind, NodeKind::Text { .. }) {
            return None;
        }
        patch(&mut kind);
        self.set_kind(doc, res, id, kind);
        self.moved.insert(id);
        Some(())
    }

    fn absorb(&mut self, doc: &Document, res: &Resolved, op: &Operation) -> Option<()> {
        match op {
            // Ghosts first, here and in every other setter: `insert_subtrees`
            // follows its `InsertSubtree` with a `SetTransform` that places the
            // new root, so a transaction that adds artwork and then positions it
            // arrives as two ops about a node the document has never seen. Left
            // to `entry`, that transform would become an override keyed to an
            // absent node and be silently dropped by the walk.
            Operation::SetTransform { id, transform } => match self.ghost_mut(*id) {
                Some(g) => g.transform = *transform,
                None => {
                    self.entry(*id).transform = Some(*transform);
                    self.mark_moved(doc, *id);
                }
            },
            Operation::SetGeometry { id, geometry } => {
                let base = self.current_kind(doc, *id)?;
                let patched = geometry.applied_to(&base)?;
                self.set_kind(doc, res, *id, patched);
                // A geometry change can move the node's own painted area but
                // not its children's frame of reference, so only it is dirty.
                self.moved.insert(*id);
            }
            // The six text ops are one shape — patch a field of the node's
            // `Text` kind and re-measure — so they share one helper rather than
            // six copies of the destructure-and-rebuild. Rebuilding by hand is
            // how a field added to the kind ends up preserved by five of the six
            // and reset by the sixth.
            Operation::SetText {
                id,
                content,
                spans,
                para_spans,
            } => self.patch_text(doc, res, *id, |k| {
                if let NodeKind::Text {
                    content: c,
                    spans: sp,
                    para_spans: psp,
                    ..
                } = k
                {
                    *c = content.clone();
                    *sp = spans.clone();
                    *psp = para_spans.clone();
                }
            })?,
            // Both halves of what `Document::op_set_text_style` does: the given list
            // where the op carries one (the inverse's case), the re-statement where it
            // does not (§15 D163). **Not for the pixels** — the re-statement only
            // drops spans that have become echoes of the new defaults, so it cannot
            // change what is drawn — but for the *model*, which the Type panel reads
            // back through `display_node`. A preview whose kind disagrees with the
            // commit's is the shape of bug D109 was.
            Operation::SetTextStyle { id, style, spans } => {
                self.patch_text(doc, res, *id, |k| {
                    if let NodeKind::Text {
                        style: s,
                        spans: sp,
                        ..
                    } = k
                    {
                        **s = style.clone();
                        *sp = match spans {
                            Some(given) => given.clone(),
                            None => sp.restated(s),
                        };
                    }
                })?
            }
            Operation::SetTextSpans { id, spans } => self.patch_text(doc, res, *id, |k| {
                if let NodeKind::Text { spans: sp, .. } = k {
                    *sp = spans.clone();
                }
            })?,
            Operation::SetParagraphStyle {
                id,
                paragraph,
                spans,
            } => self.patch_text(doc, res, *id, |k| {
                if let NodeKind::Text {
                    paragraph: p,
                    para_spans: sp,
                    ..
                } = k
                {
                    *p = paragraph.clone();
                    *sp = match spans {
                        Some(given) => given.clone(),
                        None => sp.restated(p),
                    };
                }
            })?,
            Operation::SetParagraphSpans { id, spans } => self.patch_text(doc, res, *id, |k| {
                if let NodeKind::Text { para_spans: sp, .. } = k {
                    *sp = spans.clone();
                }
            })?,
            Operation::SetBlockStyle { id, block } => self.patch_text(doc, res, *id, |k| {
                if let NodeKind::Text { block: b, .. } = k {
                    *b = *block;
                }
            })?,
            Operation::SetOpacity { id, opacity } => {
                // Value constraints are checked here too, so a preview can
                // never show an edit that the commit will reject.
                if !build::valid_opacity(*opacity) {
                    return None;
                }
                match self.ghost_mut(*id) {
                    Some(g) => g.opacity = *opacity,
                    None => self.entry(*id).opacity = Some(*opacity),
                }
            }
            Operation::SetVisible { id, visible } => match self.ghost_mut(*id) {
                Some(g) => g.visible = *visible,
                None => self.entry(*id).visible = Some(*visible),
            },
            Operation::SetFills { id, fills } => {
                let mut paint = self.current_paint(doc, *id)?;
                paint.fills = fills.clone();
                self.set_paint(*id, paint);
            }
            Operation::SetStrokes { id, strokes } => {
                let mut paint = self.current_paint(doc, *id)?;
                paint.strokes = strokes.clone();
                self.set_paint(*id, paint);
            }
            // **Not in the invisible list above, even though the ops beside it
            // there are also "one field on one node".** Every number in an
            // effect popover is scrubbable, so each one needs a live preview or
            // it is the unscrubbable field of §15 D245 — and a shadow reaches
            // outside the node it hangs on, so the node is marked moved as well:
            // its cached bounds no longer cover where its ink lands.
            Operation::SetEffects { id, effects } => match self.ghost_mut(*id) {
                Some(g) => g.effects = effects.clone(),
                None => {
                    self.entry(*id).effects = Some(effects.clone());
                    self.mark_moved(doc, *id);
                }
            },
            // A shape tool dragging out a brand-new leaf. It has no children
            // yet, so a childless ghost is exact.
            Operation::CreateNode {
                id,
                parent,
                index,
                kind,
                transform,
                ..
            } => {
                let parent_kind = doc.get(*parent)?.kind();
                if doc.contains(*id) || !build::can_parent(parent_kind, kind) {
                    return None;
                }
                self.ghosts.push(Ghost {
                    parent: *parent,
                    index: *index,
                    node: GhostNode::leaf(
                        *id,
                        transform.unwrap_or(Affine::IDENTITY),
                        kind.clone(),
                        shaped(kind),
                    ),
                });
            }
            // A whole subtree being added: the copy an Alt-drag carries.
            //
            // Addition is previewable in a way that moving is not — nothing in
            // the committed tree changes, so there is no node whose absence the
            // walk would have to fake. The same checks as `CreateNode`, because
            // the same failures are possible: ids that already exist, and a root
            // its destination will not accept.
            //
            // `index` **is** honoured, and has to be: an Alt-drag copy lands
            // immediately above its original rather than on top of the parent, so a
            // ghost that drew last would preview a different z-order from the one the
            // release produces. The walk interleaves ghosts among the real children by
            // this index ([`crate::scene`]).
            Operation::InsertSubtree {
                nodes,
                parent,
                index,
            } => {
                let parent_kind = doc.get(*parent)?.kind().clone();
                if nodes.iter().any(|n| doc.contains(n.id())) {
                    return None;
                }
                let node = ghost_subtree(nodes)?;
                if !build::can_parent(&parent_kind, &node.kind) {
                    return None;
                }
                self.ghosts.push(Ghost {
                    parent: *parent,
                    index: *index,
                    node,
                });
            }
            Operation::SetCanvasBackground { background } => {
                self.canvas_background = Some(*background);
            }
            // Renaming and locking change nothing visible. Clipping does, but
            // it is a checkbox rather than a gesture: it commits immediately,
            // so there is nothing to preview and no override to carry it.
            //
            // Guides change nothing the *renderer* draws: they are egui chrome
            // over the canvas, not scene content, so the app previews a guide
            // being dragged from its own gesture state. Absorbed as a no-op
            // rather than refused, so a transaction that moves a guide does not
            // blank the preview of whatever else is in it.
            //
            // The proportion lock is a no-op for the same reason and it matters
            // more here: it changes what a *future* handle drag does and nothing
            // about what is on screen now, so refusing it would blank the
            // preview of everything committed alongside it in exchange for
            // fidelity there was never anything to gain.
            //
            // The pivot is the same case again, and by design: it is *baked* by
            // the gesture that reads it rather than composed into the world
            // transform (§15), so the renderer has nothing to do with it and the
            // canvas draws its marker as chrome. What a pivot edit previews is
            // that marker — from the live pointer for the canvas drag, and from
            // `EditorSession::pivot_preview` for the inspector's origin fields,
            // which have no pointer on the canvas to read.
            //
            // **A no-op here is therefore not the whole story**, and the app owes
            // the other half: a preview that silently drops part of its
            // transaction is what makes a field unscrubbable (§15 D245).
            Operation::SetName { .. }
            | Operation::SetLocked { .. }
            | Operation::SetProportionsLocked { .. }
            | Operation::SetPivot { .. }
            // Export settings are read by the export path and by nothing that
            // draws, so there is genuinely nothing to preview — this is the
            // plain case the list was written for, not one of the two the
            // comments above have to argue for.
            | Operation::SetExports { .. }
            // A layout grid is chrome over a frame and never in the scene, so
            // the same plain case as the exports above: the app draws it, this
            // does not, and there is nothing here to preview.
            | Operation::SetLayoutGrids { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. } => {}
            // **A no-op here, and the only one that is not also invisible.** An
            // image op changes what is drawn (`Operation::changes_ink`), so it
            // does not belong with the list above on the merits — it is here
            // because `RenderOverrides` has no image table to patch, and the two
            // honest answers are this or refusing the whole preview. Refusing is
            // worse: a transaction that places an image is `AddImage` *and* a
            // `CreateNode`, so declining would blank the preview of the shape as
            // well as the picture in it.
            //
            // What it costs is that a previewed image is not visible until the
            // commit lands. Nothing exercises that yet — placing is a click, not
            // a drag — and when something does, the fix is a preview table on the
            // overrides, the shape `canvas_background` already has here.
            //
            // ⚠️ **This arm is why `absorb`'s no-ops are thirteen and not
            // eleven, and the design said eleven for months** (§15 D659,
            // `[S19.2-L2-04]`). §5.3b read *"those two lists are **the same eleven
            // operations**"* and §5.7 *"if the two lists ever disagree, one of
            // them is wrong"* — and a review pass certified the agreement by
            // hand-counting the arm above and stopping short of this one.
            // `changes_ink`'s eleven are a **subset**. That matters
            // beyond tidiness: `EditorSession::has_gesture_preview` is
            // `!overrides.is_empty() && …`, so every transaction absorbed here is
            // invisible to `cancel_gesture`'s gate, and `[A1-L2-02]`'s repair list
            // has to be derived from *this* function rather than from the design.
            // `overrides.rs · every_no_op_of_absorb_is_a_no_op_and_two_of_them_still_change_ink`
            // is the sibling of `op.rs`'s eleven-item fixture.
            Operation::AddImage { .. } | Operation::RemoveImage { .. } => {}
            // **Structural in the sense that matters here**: a mask changes which
            // *other* nodes the walk clips, and an override is a patch on one
            // node. Nothing sets it mid-gesture — it is a click on the identity
            // row — so refusing costs a preview nobody asks for.
            Operation::SetClip { .. }
            | Operation::SetMask { .. }
            | Operation::SetMaskMode { .. }
            // **Refused for the same reason and not the same cause.** A fill rule is
            // not structural — it patches one node and nothing else — but there is no
            // `NodeOverride` field for it, and adding one would buy a live preview of
            // a control that is a click. `build::boolean` also emits it when it makes
            // an `Exclude`, and a boolean is committed rather than dragged out. If
            // something ever scrubs this, the fix is a field here rather than a
            // change of category.
            | Operation::SetFillRule { .. } => return None,
            // Genuinely structural: these move or remove nodes the walk is
            // about to read out of the committed tree, and no patch can say
            // "somewhere else" or "not there".
            Operation::DeleteNode { .. }
            | Operation::Reparent { .. }
            | Operation::Reorder { .. } => return None,
        }
        Some(())
    }

    fn entry(&mut self, id: NodeId) -> &mut NodeOverride {
        self.nodes.entry(id).or_default()
    }

    /// The ghost with `id`, at any depth in any of the forests.
    fn ghost(&self, id: NodeId) -> Option<&GhostNode> {
        self.ghosts.iter().find_map(|g| g.node.find(id))
    }

    fn ghost_mut(&mut self, id: NodeId) -> Option<&mut GhostNode> {
        self.ghosts.iter_mut().find_map(|g| g.node.find_mut(id))
    }

    /// The kind as the transaction has left it so far — later ops in the same
    /// transaction build on earlier ones, as they would when applied.
    fn current_kind(&self, doc: &Document, id: NodeId) -> Option<NodeKind> {
        if let Some(g) = self.ghost(id) {
            return Some(g.kind.clone());
        }
        self.get(id)
            .and_then(|o| o.kind.clone())
            .or_else(|| doc.get(id).map(|n| n.kind().clone()))
    }

    fn current_paint(&self, doc: &Document, id: NodeId) -> Option<Paint> {
        if let Some(g) = self.ghost(id) {
            return Some(g.paint.clone());
        }
        self.get(id)
            .and_then(|o| o.paint.clone())
            .or_else(|| doc.get(id).map(|n| n.paint().clone()))
    }

    fn set_kind(&mut self, doc: &Document, res: &Resolved, id: NodeId, kind: NodeKind) {
        // Re-shape only when the text actually changed; reuse the cached layout
        // otherwise so a resize drag does not re-run parley.
        //
        // ⚠️ **The cached-layout guard cannot fire for a live session** (§15 D592).
        // `doc.get(id)` is the last *commit* and a session commits once, at
        // `finish_text_edit` (§9.3), so the two kinds are unequal for the whole
        // session by construction and every keystroke took the `shaped` arm. The
        // caller that already holds the answer hands it over in `prepared_text`;
        // see the field for the measurements.
        let text = match &kind {
            NodeKind::Text { .. } => match self.prepared_text.as_ref() {
                Some((for_id, laid)) if *for_id == id => Some(laid.clone()),
                _ => match doc.get(id).map(|n| n.kind()) {
                    Some(existing) if *existing == kind => res.text_layout(id).cloned(),
                    _ => shaped(&kind),
                },
            },
            _ => None,
        };
        if let Some(g) = self.ghost_mut(id) {
            g.kind = kind;
            g.text = text;
            return;
        }
        let entry = self.entry(id);
        entry.kind = Some(kind);
        entry.text = text;
    }

    fn set_paint(&mut self, id: NodeId, paint: Paint) {
        if let Some(g) = self.ghost_mut(id) {
            g.paint = paint;
            return;
        }
        self.entry(id).paint = Some(paint);
    }

    /// Mark `id` and everything under it as possibly moved.
    fn mark_moved(&mut self, doc: &Document, id: NodeId) {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            if !self.moved.insert(current) {
                continue;
            }
            if let Some(node) = doc.get(current) {
                stack.extend(node.children().iter().copied());
            }
        }
    }
}

/// Turn a captured subtree — the node list an `InsertSubtree` carries — into a
/// ghost of it.
///
/// `None` if the list is not a single well-formed tree: no root, more than one
/// root, or a child id the list does not contain. Those are the same conditions
/// `Document::apply` rejects the op for, so a subtree that cannot be ghosted is
/// one that was never going to be inserted either.
fn ghost_subtree(nodes: &[ondin_core::Node]) -> Option<GhostNode> {
    let by_id: FxHashMap<NodeId, &ondin_core::Node> = nodes.iter().map(|n| (n.id(), n)).collect();
    // The root is the one node whose parent is outside the list — the insert op
    // is what will give it a real one.
    let mut roots = nodes
        .iter()
        .filter(|n| n.parent().is_none_or(|p| !by_id.contains_key(&p)));
    let root = roots.next()?;
    if roots.next().is_some() {
        return None;
    }
    build_ghost(root, &by_id, &mut FxHashMap::default())
}

/// `derived` accumulates the boolean outlines built so far, so a boolean nested
/// inside a boolean is read from it rather than evaluated twice. Filled
/// children-first, which this recursion is by construction — the children are built
/// before the node that reads them.
fn build_ghost(
    node: &ondin_core::Node,
    by_id: &FxHashMap<NodeId, &ondin_core::Node>,
    derived: &mut FxHashMap<NodeId, kurbo::BezPath>,
) -> Option<GhostNode> {
    let mut children = Vec::with_capacity(node.children().len());
    for id in node.children() {
        children.push(build_ghost(by_id.get(id)?, by_id, derived)?);
    }
    // **The captured `Node`s, not the `GhostNode`s just built.** A capture is a real
    // subtree with its parent and child links intact, so `boolean`'s own recursion
    // walks it directly — which is the point of that recursion taking lookups rather
    // than a `Document`. A second walk over the ghosts would be a second reading of
    // what an operand contributes, and the two would drift.
    let path = match node.kind() {
        NodeKind::Boolean { op } => {
            let result = ondin_core::boolean::evaluate_node(
                node.id(),
                *op,
                &ondin_core::boolean::Operands {
                    local_of: &|c| by_id.get(&c).map(|n| n.transform()).unwrap_or_default(),
                    kind_of: &|c| by_id.get(&c).map(|n| n.kind().clone()),
                    children_of: &|c| {
                        by_id
                            .get(&c)
                            .map(|n| n.children().to_vec())
                            .unwrap_or_default()
                    },
                    visible_of: &|c| by_id.get(&c).is_some_and(|n| n.visible()),
                    nested: &|c| derived.get(&c).cloned(),
                },
            );
            if let Some(path) = &result {
                derived.insert(node.id(), path.clone());
            }
            result
        }
        _ => None,
    };
    Some(GhostNode {
        id: node.id(),
        transform: node.transform(),
        kind: node.kind().clone(),
        paint: node.paint().clone(),
        effects: node.effects().to_vec(),
        // A captured subtree carries its own text, but not the *shaped* layout —
        // `Resolved` holds those and has never seen this node. Shape it once
        // here rather than per frame: a ghost's text is fixed for the gesture.
        text: shaped(node.kind()),
        path,
        opacity: node.opacity(),
        visible: node.visible(),
        clip: node.clip(),
        mask: node.mask(),
        mask_mode: node.mask_mode(),
        children,
    })
}

fn shaped(kind: &NodeKind) -> Option<TextLayout> {
    ondin_core::TextRef::of(kind).map(text::layout)
}
