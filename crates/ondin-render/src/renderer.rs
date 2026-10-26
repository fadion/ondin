//! The renderer boundary (§6.2): the viewport, and the preview override layer.
//!
//! The drawing boundary itself is [`crate::scene::ScenePainter`]: the
//! document→scene walk is written once and each backend implements the painter.

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
    /// Replacement effect stack (§5.3a), the whole stack as the operation carries it.
    /// Kept apart from `paint` because effects live on containers, which have no `Paint`.
    pub effects: Option<Vec<ondin_core::Keyed<ondin_core::Effect>>>,
    pub opacity: Option<f32>,
    pub visible: Option<bool>,
    /// Re-shaped layout, when the kind change affected text.
    pub text: Option<TextLayout>,
    /// Replacement insets, read only by [`RenderOverrides::relayout`], which turns
    /// them into the transform and kind above. Nothing draws from this directly.
    pub insets: Option<ondin_core::Insets>,
    /// Replacement layout — `Some(None)` takes one away. Read only by the flex relayout.
    pub display: Option<Option<ondin_core::container::Display>>,
    /// The gap and padding bindings an earlier `SetSpacingVars` in the preview set.
    /// A later `SetDisplay` resolves with them, as the commit's
    /// `keep_spacing_bindings` leaves a transaction that sets them alone.
    pub spacing_vars: Option<ondin_core::SpacingVars>,
    /// Replacement layout-item properties, read only by the flex relayout.
    pub item: Option<ondin_core::container::LayoutItem>,
    /// The box a group with a layout is given under this preview —
    /// `Resolved::used_frame`'s twin, written by the flex relayout.
    pub frame: Option<ondin_core::kurbo::Size>,
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
    pub index: usize,
    pub node: GhostNode,
}

/// One node of a ghost subtree: everything the scene walk reads off a `Node`,
/// for artwork there is no `Node` for yet. It carries children so a gesture can
/// preview a copy of a group or a frame.
#[derive(Clone, Debug)]
pub struct GhostNode {
    pub id: NodeId,
    /// LOCAL to the parent ghost, or to `Ghost::parent` at the root.
    pub transform: Affine,
    pub kind: NodeKind,
    pub paint: Paint,
    /// The effect stack this ghost draws with (§5.3a). Not an `Option` as on
    /// [`NodeOverride`]: a ghost is the whole node, so "no effects" is the empty stack.
    pub effects: Vec<ondin_core::Keyed<ondin_core::Effect>>,
    pub text: Option<TextLayout>,
    /// A `Boolean` ghost's combined outline in its own local space, evaluated once
    /// when the subtree is built: the scene walk is handed derived geometry, not
    /// asked to compute it per frame.
    pub path: Option<kurbo::BezPath>,
    pub opacity: f32,
    pub visible: bool,
    /// Whether this ghost clips its children, as `Node::clip` does.
    pub clip: bool,
    /// Whether this ghost masks the ghosts beside it, as `Node::mask` does. Read
    /// only of a child ghost, by the parent's walk.
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
            effects: Vec::new(),
            text,
            // No shape tool makes a boolean or a mask: both designate existing layers.
            path: None,
            opacity: 1.0,
            visible: true,
            clip: false,
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
/// Cost is O(nodes touched) rather than a document clone and re-resolve per frame.
/// Built from the very transaction the gesture will commit ([`Self::from_transaction`]),
/// so the preview and the commit cannot drift.
///
/// Operations that only add artwork become a [`Ghost`]. Delete and reparent are not
/// representable (a patch cannot say "somewhere else in the tree"), so
/// `from_transaction` returns `None` for them. A reorder is a patch on the parent's
/// child order ([`Self::children_of`]), read by the scene walk, the layout view and
/// a boolean's operand order (a `Subtract`'s base) (§15 D877).
#[derive(Default, Debug)]
pub struct RenderOverrides {
    nodes: FxHashMap<NodeId, NodeOverride>,
    /// Parents whose child order this preview changes, each with its whole new
    /// order — see [`Self::children_of`].
    order: FxHashMap<NodeId, Vec<NodeId>>,
    /// Where each flex item this preview drags **will land** — see
    /// [`Self::landings`].
    landing: Vec<(NodeId, Affine, kurbo::Rect)>,
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
    /// A page's ground being dragged in the picker. Not part of the scene walk
    /// (the backdrop is the clear colour), so it rides here only to reach the
    /// caller.
    ground: Option<(NodeId, Color)>,
    /// Artwork drawn to *explain* a mode rather than preview an edit — today the
    /// faded whole picture behind a layer being cropped — glued to the `NodeId`
    /// layer that [`Self::unsnapped`] needs.
    ///
    /// A ghost in a field of its own: [`Self::is_empty`] answers "is a gesture in
    /// flight" (`Session::has_gesture_preview`), and a mode must not read as one.
    /// Re-installed every frame from `OndinApp::draw_canvas`, since `set_preview`
    /// replaces this whole struct.
    chrome: Option<(NodeId, Ghost)>,
    /// Outline view's ink: every outline drawn as a hairline in this colour, no
    /// fill, effect, clip or mask (`scene::OutlinePainter`). A view, not an edit,
    /// so it is re-installed every frame as [`Self::chrome`] is, and not a term
    /// of [`Self::is_empty`].
    outline: Option<Color>,
    /// A text layout the caller has **already shaped** for one node, live only
    /// while [`Self::from_transaction_shaped`] builds.
    ///
    /// A construction argument wearing a field on purpose: [`Self::set_kind`] is
    /// reached from many arms of [`Self::absorb`], and a live text session is a
    /// commit ahead of the document, so `set_kind`'s "re-shape only when the text
    /// changed" guard can never fire for it. Cleared before the value is returned.
    prepared_text: Option<(NodeId, TextLayout)>,
}

impl RenderOverrides {
    /// Whether this preview says nothing — no patched node, no ghost, no ground.
    /// [`Self::chrome`] is deliberately not a term: this asks "is an edit in flight".
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.ghosts.is_empty()
            && self.order.is_empty()
            && self.ground.is_none()
    }

    /// `parent`'s children in the order this preview gives them, where it
    /// reorders them — `None` for the committed order.
    pub fn children_of(&self, parent: NodeId) -> Option<&[NodeId]> {
        self.order.get(&parent).map(Vec::as_slice)
    }

    /// Every flex item this preview **drags**, with where its container will lay
    /// it on release: its world transform there and its box in its own space.
    ///
    /// The insertion indicator's one source: the item is drawn under the pointer, so
    /// the slot [`Self::flex_relayout`] computes for it is otherwise discarded.
    pub fn landings(&self) -> &[(NodeId, Affine, kurbo::Rect)] {
        &self.landing
    }

    /// The grid container `id` as **this preview** lays it — its tracks and its
    /// items' areas ([`ondin_core::container::laid_grid`] over the patched
    /// fields), `None` where it is not a grid.
    ///
    /// The canvas's track lines' one source, so a container being resized re-lays
    /// them against the box the gesture draws rather than the one on disk.
    /// `build::laid_grid` lays through the stored document and differs for a grid
    /// pinned in a parent drawn at another size than it stores.
    pub fn laid_grid(
        &self,
        doc: &Document,
        res: &Resolved,
        id: NodeId,
    ) -> Option<ondin_core::container::LaidGrid> {
        ondin_core::container::laid_grid(&PreviewView { doc, ov: self, res }, id)
    }

    /// The ground this preview asks for on `page`, if it touches that page's
    /// ground at all.
    pub fn ground(&self, page: NodeId) -> Option<Color> {
        self.ground.filter(|(p, _)| *p == page).map(|(_, c)| c)
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
    /// Set exactly when the drop would take the layer out of that frame (the
    /// threshold belongs to `canvas::frame_covering`), so a shape rendering in full
    /// means "release now and it leaves". Z-order is untouched: the parent's clip
    /// layer is closed and reopened around this child. Set from the gesture, never
    /// derived from overrides — a resize also gives a child a transform override
    /// and must stay clipped.
    pub fn escape_clip(&mut self, id: NodeId) {
        self.escaping.insert(id);
    }

    /// Whether `id` is on its way out of its parent's clip.
    pub fn escapes_clip(&self, id: NodeId) -> bool {
        self.escaping.contains(&id)
    }

    /// The ghosts are leaving their parent frame too, like [`Self::escape_clip`].
    /// One switch for all of them: only Alt-drag carries and moves a ghost, and a
    /// shape being created is drawn inside its frame, where clipping is the truth.
    pub fn escape_clip_ghosts(&mut self) {
        self.escaping_ghosts = true;
    }

    /// Whether the ghosts draw outside their parent's clip.
    pub fn ghosts_escape_clip(&self) -> bool {
        self.escaping_ghosts
    }

    /// The ghosts hanging off `parent` — the preview's, and the chrome one — in one
    /// iterator so the z-ordered draw loop and the cull cover both.
    pub fn ghosts_of(&self, parent: NodeId) -> impl Iterator<Item = &Ghost> {
        self.ghosts
            .iter()
            .chain(self.chrome.iter().map(|(_, g)| g))
            .filter(move |g| g.parent == parent)
    }

    /// Install (or clear) the mode's explanatory artwork — see [`Self::chrome`].
    /// `glued` is the layer the ghost is a continuation of, and the ghost itself;
    /// `None` clears it.
    pub fn set_chrome_ghost(&mut self, glued: Option<(NodeId, Ghost)>) {
        self.chrome = glued;
    }

    /// Turn outline view on with `ink` as its colour, or off — see
    /// [`Self::outline`].
    pub fn set_outline(&mut self, ink: Option<Color>) {
        self.outline = ink;
    }

    /// Outline view's ink, `None` when the artwork draws as itself.
    pub fn outline(&self) -> Option<Color> {
        self.outline
    }

    /// Whether `id`'s box is exempt from device-pixel snapping
    /// (`scene::snapped_box_world`).
    ///
    /// Exactly two things are, and together (§15 D272): the chrome ghost and the
    /// layer it is glued to. Snapping rounds two rectangles of different sizes
    /// differently, which slid the crop offcut against its picture by a device
    /// pixel; the layer's rounding is computed deep in the walk, so the two agree
    /// only by both opting out (at the cost of the seam mitigation on that layer).
    /// Derived from [`Self::chrome`] so the pair cannot come apart.
    pub fn unsnapped(&self, id: NodeId) -> bool {
        self.chrome
            .as_ref()
            .is_some_and(|(node, g)| *node == id || g.node.id == id)
    }

    /// The local transform to draw `id` with: a ghost's first (`absorb` writes a
    /// ghost's `SetTransform` into the ghost, so it has no override entry), then the
    /// override, then the *used* local, where container layout placed the node.
    pub fn transform_of(&self, doc: &Document, res: &Resolved, id: NodeId) -> Affine {
        if let Some(g) = self.ghost(id) {
            return g.transform;
        }
        self.get(id)
            .and_then(|o| o.transform)
            .or_else(|| res.used_local(doc, id))
            .unwrap_or(Affine::IDENTITY)
    }

    /// The children `id` has as far as a boolean evaluated over it is concerned: its
    /// committed ones, plus any ghost this preview has parented to it.
    ///
    /// The ghost is a real operand: an Alt-dragged copy of an operand inside a
    /// boolean joins the operation on release, so the preview combines it too.
    ///
    /// Inserted at [`Ghost::index`], ascending, which is the commit's order
    /// (`insert_subtrees`). The fold order must match: `Exclude` over many operands
    /// changes its result measurably when one operand moves to the end of the fold.
    fn operand_children(&self, doc: &Document, id: NodeId) -> Vec<NodeId> {
        if let Some(g) = self.ghost(id) {
            return g.children.iter().map(|c| c.id).collect();
        }
        // A `Subtract` reads its base off the first operand, so the preview's order counts.
        let mut out = match self.children_of(id) {
            Some(order) => order.to_vec(),
            None => doc
                .get(id)
                .map(|n| n.children().to_vec())
                .unwrap_or_default(),
        };
        let mut ghosts: Vec<(usize, NodeId)> =
            self.ghosts_of(id).map(|g| (g.index, g.node.id)).collect();
        ghosts.sort_by_key(|(i, _)| *i);
        for (index, gid) in ghosts {
            // Clamped: an index past the end appends.
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
            .unwrap_or_else(|| doc.shown(id))
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
            world *= self.transform_of(doc, res, *node);
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
    /// node's text (a live text session, whose `TextEdit` just shaped it), so the
    /// preview does not shape it a second time per keystroke.
    ///
    /// `shaped` must be the layout of the kind this transaction produces for that
    /// node, or the canvas draws one thing and every measurement reads another.
    pub fn from_transaction_shaped(
        doc: &Document,
        res: &Resolved,
        tx: &Transaction,
        shaped: Option<(NodeId, TextLayout)>,
    ) -> Option<Self> {
        Self::from_transaction_following(doc, res, tx, &Transaction::default(), shaped)
    }

    /// [`Self::from_transaction_shaped`] with `follows` — a main's edit written
    /// onto its instances' copies (`ondin_core::propagate`, §5.3d's build step 6) —
    /// absorbed after `tx` and **laid out, never taken for the hand's own**.
    ///
    /// `tx` is the gesture: a `SetTransform` in it is where the pointer wants a
    /// layer drawn, so [`Self::flex_relayout`] and [`Self::relayout`] leave it there.
    /// A follow is the main's value carried onto a copy, which the commit draws
    /// where the copy's own container places it.
    pub fn from_transaction_following(
        doc: &Document,
        res: &Resolved,
        tx: &Transaction,
        follows: &Transaction,
        shaped: Option<(NodeId, TextLayout)>,
    ) -> Option<Self> {
        let mut out = Self {
            prepared_text: shaped,
            ..Self::default()
        };
        for op in tx.0.iter().chain(&follows.0) {
            out.absorb(doc, res, op)?;
        }
        // Cleared before anything can observe it — see the field.
        out.prepared_text = None;
        // Before the booleans, which read the transforms these can change — and the
        // flex pass before the insets one, so a pinned child is placed against the
        // box its container's layout has just given it.
        let dragged = out.flex_relayout(doc, res, tx);
        out.relayout(doc, res, tx);
        // The dragged items' landings last, once both passes have placed their containers.
        for (id, placed) in dragged {
            if let Some(landing) = out.landing_of(doc, res, id, &placed) {
                out.landing.push(landing);
            }
        }
        out.reevaluate_booleans(doc, res);
        Some(out)
    }

    /// Re-lay, under this preview, every flex container a node it touches belongs
    /// to — the whole chain up to the layout root (a flex item's size moves its
    /// siblings), and every layout root on the way up, which a plain group hides
    /// from the outer pass.
    ///
    /// `absorb` patches fields and cannot re-run layout, so this runs
    /// `ondin_core::container::lay_out` over a [`PreviewView`] and writes what it
    /// places as ordinary transform, kind and frame overrides. Runs before
    /// [`Self::relayout`] so a pinned child is placed against its container's new box.
    ///
    /// A layer the transaction drags with `SetTransform` is skipped: that is where
    /// the drag wants it. A *resized* item (even with a `SetTransform` from a left
    /// or top handle) and a *turned* one (linear part changed, applied about the box
    /// centre) are re-laid. Items of a layout this preview removes are put back by
    /// their transforms, since no pass roots at a container with no layout.
    ///
    /// It hands back each dragged item's placement rather than composing its
    /// landing: `relayout` may yet move the container.
    fn flex_relayout(
        &mut self,
        doc: &Document,
        res: &Resolved,
        tx: &Transaction,
    ) -> Vec<(NodeId, ondin_core::container::Placed)> {
        // The commit keeps a resized item's stored translation
        // (`build::keep_flex_sizes`), so the layout places it, handle `SetTransform` or not.
        let resized: FxHashSet<NodeId> =
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetGeometry { id, geometry } if geometry.resizes() => Some(*id),
                    _ => None,
                })
                .collect();
        // A turn is not a drag: a `SetTransform` that changes the linear part is
        // re-laid like a resize, and `item_placed` turns the item about its box
        // centre in its slot, as the commit draws. A move keeps the linear part.
        let linear = |a: Affine| {
            let [a, b, c, d, _, _] = a.as_coeffs();
            [a, b, c, d]
        };
        let turned = |id: &NodeId, to: &Affine| {
            doc.get(*id).is_some_and(|n| {
                linear(n.transform())
                    .iter()
                    .zip(linear(*to))
                    .any(|(x, y)| (x - y).abs() > 1e-9)
            })
        };
        let dragged: FxHashSet<NodeId> =
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetTransform { id, transform }
                        if !resized.contains(id) && !turned(id, transform) =>
                    {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect();
        let results = {
            let view = PreviewView { doc, ov: self, res };
            // Every layout root from a touched node up to its chain root, not the chain
            // root alone: the chain climbs through a plain group, where the outer
            // container's pass stops, so a layout nested inside would go un-laid.
            let mut roots: Vec<NodeId> = Vec::new();
            for id in self.nodes.keys() {
                let top = ondin_core::container::chain_root(&view, *id);
                let mut at = Some(*id);
                while let Some(n) = at {
                    if ondin_core::container::is_layout_root(&view, n) && !roots.contains(&n) {
                        roots.push(n);
                    }
                    if n == top {
                        break;
                    }
                    at = ondin_core::container::LayoutView::parent(&view, n);
                }
            }
            let mut results = Vec::new();
            for root in roots {
                for l in ondin_core::container::lay_out(&view, root) {
                    let placed = if l.id == root {
                        let (kind, frame) = ondin_core::container::root_sized(&view, root, l.size);
                        ondin_core::container::Placed {
                            local: None,
                            kind,
                            frame,
                        }
                    } else {
                        ondin_core::container::item_placed(&view, l.id, l.slot, l.size).unwrap_or(
                            ondin_core::container::Placed {
                                local: None,
                                kind: None,
                                frame: None,
                            },
                        )
                    };
                    results.push((l.id, l.id == root, placed));
                }
            }
            results
        };
        // A layout taken away puts its items back: a container whose layout this
        // preview removes roots no pass, so the committed flex placement would show
        // through while the commit draws them by their transforms.
        let laid: FxHashSet<NodeId> = results.iter().map(|(id, _, _)| *id).collect();
        let released: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, o)| matches!(o.display, Some(None)))
            .filter_map(|(id, _)| doc.get(*id))
            .flat_map(|n| n.children().to_vec())
            .filter(|c| !laid.contains(c) && !dragged.contains(c))
            .collect();
        for id in released {
            let Some(node) = doc.get(id) else { continue };
            // Its specified kind, a bound size and bound corners resolved.
            let Some(own) = specified_kind(doc, id) else {
                continue;
            };
            let moved = res.used_local(doc, id) != Some(node.transform());
            let resized = res.used_kind(doc, id) != Some(&own);
            if moved || resized || res.used_frame(id).is_some() {
                self.entry(id).transform = Some(node.transform());
                if resized {
                    self.set_kind(doc, res, id, own);
                }
                self.entry(id).frame = None;
                self.mark_moved(doc, id);
            }
        }
        let mut landings = Vec::new();
        for (id, root, placed) in results {
            if dragged.contains(&id) {
                // Where it will land, for the insertion indicator; handed back rather
                // than composed, as its container may yet be moved.
                if !root {
                    landings.push((id, placed));
                }
                continue;
            }
            self.apply_laid(doc, res, id, root, placed);
        }
        landings
    }

    /// One result of a layout pass written into this preview: an item's slot and
    /// size, a root's size — [`Self::flex_relayout`]'s, and [`Self::relayout`]'s
    /// for a pinned root it re-lays.
    fn apply_laid(
        &mut self,
        doc: &Document,
        res: &Resolved,
        id: NodeId,
        root: bool,
        placed: ondin_core::container::Placed,
    ) {
        let Some(node) = doc.get(id) else { return };
        // A root keeps where its parent puts it; an item goes where its container
        // laid it, stated outright so a committed placement does not show through.
        // "Stored" is what the pass read — this preview's transform where it has one
        // (`PreviewView::local`), not the document's — since `item_placed` answers
        // `local: None` when the slot is exactly that.
        if !root {
            let specified = self
                .get(id)
                .and_then(|o| o.transform)
                .unwrap_or_else(|| node.transform());
            self.entry(id).transform = Some(placed.local.unwrap_or(specified));
        }
        // At its specified size — a bound width or height resolved, never the 0 its
        // literal holds: an item the pass does not resize answers no kind.
        let Some(base) = self.current_kind(doc, id) else {
            return;
        };
        let kind = placed.kind.unwrap_or(base);
        let now = self
            .get(id)
            .and_then(|o| o.kind.clone())
            .or_else(|| res.used_kind(doc, id).cloned());
        if now.as_ref() != Some(&kind) {
            self.set_kind(doc, res, id, kind);
        }
        self.entry(id).frame = placed.frame;
        self.mark_moved(doc, id);
    }

    /// A dragged flex item's landing — its world transform and own box where its
    /// container lays it — for [`Self::landings`]. `None` for a kind with no box.
    fn landing_of(
        &self,
        doc: &Document,
        res: &Resolved,
        id: NodeId,
        placed: &ondin_core::container::Placed,
    ) -> Option<(NodeId, Affine, kurbo::Rect)> {
        let node = doc.get(id)?;
        // `local: None` means the slot *is* the transform the pass read — the drag's
        // own, not the document's.
        let local = placed.local.unwrap_or_else(|| {
            self.get(id)
                .and_then(|o| o.transform)
                .unwrap_or_else(|| node.transform())
        });
        let bx = match placed.frame {
            Some(size) => kurbo::Rect::from_origin_size(kurbo::Point::ZERO, size),
            None => {
                let kind = placed.kind.clone().or_else(|| self.current_kind(doc, id))?;
                ondin_core::geometry::local_bounds(&kind, None)
                    .or_else(|| ondin_core::local_box(doc, res, id))?
            }
        };
        // The container where this preview draws it: a reorder that resizes a hugging
        // container moves it under an outer centring layout.
        let parent_world = self.world_transform(doc, res, node.parent()?)?;
        Some((id, parent_world * local, bx))
    }

    /// Re-place, under this preview, every pinned child whose placement it changes
    /// — the children of a frame it resizes, and any layer whose insets it sets.
    ///
    /// `absorb` cannot re-run layout, so this runs `ondin_core::container::place`
    /// against the preview's frame size and insets and writes ordinary transform
    /// and kind overrides, parents first (a stretched frame hands its children the
    /// stretched size).
    ///
    /// A layer the transaction places itself (`SetTransform` or `SetGeometry`) is
    /// skipped, or the drag would be drawn away from the pointer. Flex has its own
    /// pass, [`Self::flex_relayout`], run first. A pinned **layout root** is laid
    /// out again at the size its insets give it, and its items placed from that. A
    /// child with no insets is left where the commit drew it, unless this preview
    /// unpinned it.
    fn relayout(&mut self, doc: &Document, res: &Resolved, tx: &Transaction) {
        // Placed by the transaction: a transform, or a patch that changes the box. A
        // corner radius or side count reshapes a kind still at its *stored* size, so
        // a pinned layer given one is re-placed below.
        let placed: FxHashSet<NodeId> =
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetTransform { id, .. } => Some(*id),
                    Operation::SetGeometry { id, geometry } if geometry.resizes() => Some(*id),
                    _ => None,
                })
                .collect();
        let mut queue: std::collections::VecDeque<NodeId> = std::collections::VecDeque::new();
        for (id, over) in &self.nodes {
            // Its insets changed, or its kind did without its box (the case above).
            if over.insets.is_some() || over.kind.is_some() {
                queue.push_back(*id);
            }
            // A frame re-sized, or a group with a layout given a new box by the
            // flex pass that ran first: its pinned children are re-placed.
            if (matches!(over.kind, Some(NodeKind::Artboard { .. })) || over.frame.is_some())
                && let Some(n) = doc.get(*id)
            {
                queue.extend(n.children().iter().copied());
            }
        }
        let mut seen: FxHashSet<NodeId> = FxHashSet::default();
        while let Some(id) = queue.pop_front() {
            if placed.contains(&id) || !seen.insert(id) {
                continue;
            }
            let Some(node) = doc.get(id) else { continue };
            let Some(parent) = node.parent() else {
                continue;
            };
            let frame = match self.get(parent).and_then(|o| o.kind.as_ref()) {
                Some(k) => k.clone(),
                None => match res.used_kind(doc, parent) {
                    Some(k) => k.clone(),
                    None => continue,
                },
            };
            // A frame's box is its size; a group with a layout's is the box the
            // flex relayout just gave it, or the committed one.
            let size = match frame {
                NodeKind::Artboard { size } => size,
                NodeKind::Group => match self
                    .get(parent)
                    .and_then(|o| o.frame)
                    .or_else(|| res.used_frame(parent))
                {
                    Some(s) => s,
                    None => continue,
                },
                _ => continue,
            };
            let insets = self
                .get(id)
                .and_then(|o| o.insets)
                .unwrap_or(*node.insets());
            // The kind this preview has given it, else the document's; either way at
            // its specified size, which `place` stretches from.
            let Some(mut base) = self.current_kind(doc, id) else {
                continue;
            };
            // A layer with no insets is not this pass's business unless this preview
            // unpinned it: untouched, it is drawn where the commit drew it, not
            // re-placed at its stored transform and kind (`place` answers `None`).
            if !insets.is_authored() && self.get(id).is_none_or(|o| o.insets.is_none()) {
                continue;
            }
            // A pinned layout root is laid out again, `Resolved`'s order (a root is
            // laid before it is placed). Its own size is the base `place` stretches
            // from, as `resolve::root_used`'s is.
            let laid: Vec<(NodeId, bool, ondin_core::container::Placed)> = {
                let view = PreviewView { doc, ov: self, res };
                if insets.is_authored() && ondin_core::container::is_layout_root(&view, id) {
                    ondin_core::container::lay_out(&view, id)
                        .into_iter()
                        .map(|l| {
                            let placed = if l.id == id {
                                let (kind, frame) =
                                    ondin_core::container::root_sized(&view, id, l.size);
                                ondin_core::container::Placed {
                                    local: None,
                                    kind,
                                    frame,
                                }
                            } else {
                                ondin_core::container::item_placed(&view, l.id, l.slot, l.size)
                                    .unwrap_or(ondin_core::container::Placed {
                                        local: None,
                                        kind: None,
                                        frame: None,
                                    })
                            };
                            (l.id, l.id == id, placed)
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            };
            if let Some((_, _, own)) = laid.iter().find(|(l, root, _)| *root && *l == id)
                && let Some(kind) = &own.kind
            {
                base = kind.clone();
            }
            for (l, root, placed) in laid {
                if !root {
                    self.apply_laid(doc, res, l, false, placed);
                }
            }
            let (local, kind) =
                match ondin_core::container::place(&insets, size, node.transform(), &base) {
                    Some((local, kind)) => (local, kind.unwrap_or_else(|| base.clone())),
                    // Unpinned by this preview: back to where the document puts it.
                    None => (node.transform(), base.clone()),
                };
            // Against what would be drawn now, so an absorbed kind at the stored size
            // is always replaced by the placed one.
            let now = self
                .get(id)
                .and_then(|o| o.kind.clone())
                .or_else(|| res.used_kind(doc, id).cloned());
            let resized = now.as_ref() != Some(&kind);
            self.entry(id).transform = Some(local);
            if resized {
                self.set_kind(doc, res, id, kind.clone());
            }
            self.mark_moved(doc, id);
            // A frame this re-sized re-places its own pinned children.
            if resized && matches!(kind, NodeKind::Artboard { .. }) {
                queue.extend(node.children().iter().copied());
            }
        }
    }

    /// Re-derive the outline of every boolean above something this preview moves.
    ///
    /// `Resolved`'s cache refreshes only on commit, so without this a boolean lags
    /// its own operands until release. Runs once per preview build, deepest first,
    /// so a nested boolean is evaluated before the parent that reads it.
    ///
    /// Three kinds of seed: a moved node reshapes every boolean above it; a ghost
    /// reshapes the boolean it is parented *to* (its seed is its parent itself); and
    /// a reordered parent, since a `Subtract` reads its base off the first operand.
    fn reevaluate_booleans(&mut self, doc: &Document, res: &Resolved) {
        let mut affected: Vec<(usize, NodeId)> = Vec::new();
        let seeds = self
            .moved
            .iter()
            .filter_map(|m| doc.get(*m).and_then(|n| n.parent()))
            .chain(self.ghosts.iter().map(|g| g.parent))
            .chain(self.order.keys().copied());
        // Depth from the root, taken per node, not the distance walked from a seed:
        // the latter is not comparable between seeds. Evaluated outermost-first, the
        // `nested:` closure would silently fall through to `Resolved`'s committed
        // cache, a plausible path from before the gesture.
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
                    local_of: &|c| self.transform_of(doc, res, c),
                    // The overridden kind, not the committed one: a resize is partly
                    // new geometry.
                    kind_of: &|c| self.drawn_kind(doc, res, c),
                    // This preview's order plus its ghosts: it cannot reparent or
                    // delete, but it can reorder and add. See [`Self::operand_children`].
                    children_of: &|c| self.operand_children(doc, c),
                    visible_of: &|c| self.operand_visible(doc, c),
                    // The pending result, then a ghost's own, else the committed cache.
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
        // A bound family or type number written its literal draws its variable, as
        // the commit leaves it (`build::keep_text_bindings`). Not the content: an
        // empty one written is a text emptied, and detaches.
        if let Some(n) = doc.get(id)
            && let (
                NodeKind::Text {
                    style, paragraph, ..
                },
                NodeKind::Text {
                    style: literal,
                    paragraph: literal_para,
                    ..
                },
            ) = (&kind, n.kind())
        {
            let mut kept = n.text_vars();
            kept[ondin_core::variable::TEXT_CONTENT] = None;
            for (k, v) in kept.iter_mut().enumerate() {
                let written = (&**style, paragraph);
                if !ondin_core::variable::text_slot_eq(k, written, (&**literal, literal_para)) {
                    *v = None;
                }
            }
            if kept.iter().any(Option::is_some) {
                kind = doc.text_drawn_in(doc.variables(), id, &kind, kept);
            }
        }
        self.set_kind(doc, res, id, kind);
        self.moved.insert(id);
        Some(())
    }

    fn absorb(&mut self, doc: &Document, res: &Resolved, op: &Operation) -> Option<()> {
        match op {
            // Ghosts first, here and in every other setter: `insert_subtrees` follows
            // its `InsertSubtree` with a `SetTransform` about a node the document has
            // never seen, which `entry` would key to an absent node and the walk drop.
            Operation::SetTransform { id, transform } => match self.ghost_mut(*id) {
                Some(g) => g.transform = *transform,
                None => {
                    self.entry(*id).transform = Some(*transform);
                    self.mark_moved(doc, *id);
                }
            },
            Operation::SetGeometry { id, geometry } => {
                let base = self.current_kind(doc, *id)?;
                let mut patched = geometry.applied_to(&base)?;
                // Any geometry write on a rect with bound corners draws the ones the
                // commit keeps at their variables (`build::kept_corner_vars`).
                if let NodeKind::Rect { corner_radii, .. } = &mut patched
                    && doc
                        .get(*id)
                        .is_some_and(|n| n.corner_vars().iter().any(Option::is_some))
                {
                    let w = [
                        corner_radii.top_left,
                        corner_radii.top_right,
                        corner_radii.bottom_right,
                        corner_radii.bottom_left,
                    ];
                    let kept = ondin_core::build::kept_corner_vars(doc, *id, w);
                    *corner_radii = doc.corners_drawn_in(doc.variables(), *id, *corner_radii, kept);
                }
                // A bound width or height is drawn where the commit puts it
                // (`build::kept_size_vars`): an axis written what it draws keeps its
                // variable's number, one written past it the written size.
                if let Some(written) = patched.box_size()
                    && doc
                        .get(*id)
                        .is_some_and(|n| n.size_vars().iter().any(Option::is_some))
                {
                    let kept = ondin_core::build::kept_size_vars(doc, Some(res), *id, written);
                    patched = doc.size_drawn_in(doc.variables(), *id, &patched, kept);
                }
                self.set_kind(doc, res, *id, patched);
                // Only the node's own painted area moves, not its children's frame.
                self.moved.insert(*id);
            }
            // Recorded, not drawn: `relayout` turns the insets into a transform and a
            // kind once every op is in. A ghost carries none (`build::keep_insets`).
            Operation::SetInsets { id, insets } => {
                if self.ghost(*id).is_none() {
                    self.entry(*id).insets = Some(*insets);
                    self.mark_moved(doc, *id);
                }
            }
            // Recorded, not drawn, `SetInsets`' way: the flex relayout turns it into
            // placements. The commit's own validity checks run here too (`SetOpacity`'s
            // rule), asked against the document's value as `op_set_display` asks it.
            Operation::SetDisplay { id, display } => {
                let was = doc.get(*id).and_then(|n| n.display());
                if display
                    .as_ref()
                    .is_some_and(|d| !d.is_finite() || !d.is_valid_over(was))
                {
                    return None;
                }
                // Stored resolved, as `PreviewView` answers an override's layout as it
                // is. Resolved with the bindings the commit will keep
                // (`build::kept_spacing_vars`), unless an earlier `SetSpacingVars` in
                // the preview set them, which `keep_spacing_bindings` leaves alone.
                if self.ghost(*id).is_none() {
                    let pending = self.get(*id).and_then(|o| o.spacing_vars);
                    let drawn = display.as_ref().map(|d| {
                        let vars = pending
                            .unwrap_or_else(|| ondin_core::build::kept_spacing_vars(doc, *id, d));
                        doc.spacing_drawn_in(doc.variables(), *id, d, vars)
                    });
                    self.entry(*id).display = Some(drawn);
                    self.mark_moved(doc, *id);
                }
            }
            // A gap's or padding's binding previews as the numbers it will lay out at,
            // over the layout earlier ops left (else the document's); a slot `vars`
            // leaves unbound keeps what that holds.
            Operation::SetSpacingVars { id, vars } => {
                let pending = self.get(*id).and_then(|o| o.display.clone()).flatten();
                let base = pending
                    .or_else(|| doc.resolved_display(*id).map(std::borrow::Cow::into_owned))?;
                if self.ghost(*id).is_none() {
                    let drawn = doc.spacing_drawn_in(doc.variables(), *id, &base, *vars);
                    self.entry(*id).display = Some(Some(drawn));
                    self.entry(*id).spacing_vars = Some(*vars);
                    self.mark_moved(doc, *id);
                }
            }
            Operation::SetLayoutItem { id, item } => {
                if let Some(was) = doc.get(*id).map(|n| *n.item())
                    && (!item.is_finite() || !item.is_valid_over(&was))
                {
                    return None;
                }
                if self.ghost(*id).is_none() {
                    // Its bound limits where the commit will leave them.
                    self.entry(*id).item = Some(ondin_core::build::kept_limits(doc, *id, *item));
                    self.mark_moved(doc, *id);
                }
            }
            // The text ops patch a field of the node's `Text` kind and re-measure, so
            // they share `patch_text`; rebuilding the kind by hand is how a new field
            // ends up preserved by some and reset by others.
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
            // Both halves of `Document::op_set_text_style`: the given list where the op
            // carries one, the re-statement where it does not (§15 D163). Not for the
            // pixels but for the model, which the Type panel reads back through
            // `display_node`.
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
                // Checked here too, so a preview never shows an edit the commit rejects.
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
            // A visibility's binding previews as the bound variable's value in the
            // mode in force; unbound, the literal earlier ops left, else the document's.
            Operation::SetVisibleVar { id, var } => {
                if self.ghost(*id).is_none() {
                    let shown = match var {
                        Some(v) => match doc.resolve_var(*id, *v) {
                            Some(ondin_core::Value::Boolean(b)) => b,
                            _ => return None,
                        },
                        None => self
                            .get(*id)
                            .and_then(|o| o.visible)
                            .or_else(|| doc.get(*id).map(|n| n.visible()))?,
                    };
                    self.entry(*id).visible = Some(shown);
                }
            }
            // An opacity's binding previews as it will draw: `SetVisibleVar`'s rule.
            Operation::SetOpacityVar { id, var } => {
                let literal = match self.ghost(*id) {
                    Some(g) => g.opacity,
                    None => self
                        .get(*id)
                        .and_then(|o| o.opacity)
                        .or_else(|| doc.get(*id).map(|n| n.opacity()))?,
                };
                let drawn = doc.opacity_drawn_in(doc.variables(), *id, literal, *var);
                match self.ghost_mut(*id) {
                    Some(g) => g.opacity = drawn,
                    None => self.entry(*id).opacity = Some(drawn),
                }
            }
            Operation::SetFills { id, fills } => {
                let mut paint = self.current_paint(doc, *id)?;
                paint.fills = fills.clone();
                self.set_paint(*id, paint);
            }
            Operation::SetStrokes { id, strokes } => {
                let mut paint = self.current_paint(doc, *id)?;
                // As the commit will keep it (`build::kept_stroke_widths`): a bound
                // width the write leaves bound stays bound, any other detached.
                paint.strokes = build::kept_stroke_widths(doc, *id, strokes.clone());
                self.set_paint(*id, paint);
                // A stroke's reach is part of the ink the cull tests (`SetEffects`' reason).
                self.mark_moved(doc, *id);
            }
            // Every number in an effect popover is scrubbable, so it needs a live
            // preview; a shadow reaches outside its node, so the node is marked moved
            // (its cached bounds no longer cover its ink). Kept as the commit keeps it
            // (`build::kept_effect_vars`): a bound number the write leaves bound stays
            // bound, any other is detached.
            Operation::SetEffects { id, effects } => match self.ghost_mut(*id) {
                Some(g) => g.effects = effects.clone(),
                None => {
                    self.entry(*id).effects =
                        Some(build::kept_effect_vars(doc, *id, effects.clone()));
                    self.mark_moved(doc, *id);
                }
            },
            // A shape tool dragging out a brand-new leaf: a childless ghost is exact.
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
                // The ancestor half of the rule: a frame's ghost under a mask is a
                // create `apply` will refuse.
                if matches!(kind, NodeKind::Artboard { .. }) && !doc.frame_may_sit_under(*parent)
                {
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
            // A whole subtree being added: the copy an Alt-drag carries. The same
            // checks as `CreateNode`. `index` must be honoured: the copy lands just
            // above its original, and the walk interleaves ghosts among the real
            // children by it ([`crate::scene`]).
            Operation::InsertSubtree {
                nodes,
                parent,
                index,
            } => {
                let parent_kind = doc.get(*parent)?.kind().clone();
                if nodes.iter().any(|n| doc.contains(n.id())) {
                    return None;
                }
                let node = ghost_subtree(doc, nodes)?;
                if !build::can_parent(&parent_kind, &node.kind) {
                    return None;
                }
                // And the ancestor half, above the paste and inside it.
                if frame_barred(nodes, doc.frame_may_sit_under(*parent)) {
                    return None;
                }
                self.ghosts.push(Ghost {
                    parent: *parent,
                    index: *index,
                    node,
                });
            }
            Operation::SetPageGround { id, ground } => {
                self.ground = Some((*id, *ground));
            }
            // A draft flag and the page order are chrome.
            Operation::SetPageDraft { .. } | Operation::MovePage { .. } => {}
            // A page coming or going is structural, as the vault is.
            Operation::AddPage { .. } | Operation::RemovePage { .. } => return None,
            // Nothing the renderer draws, absorbed as a no-op rather than refused so
            // the rest of the transaction still previews. Guides and layout grids are
            // egui chrome the app previews from its own gesture state, and so is the
            // pivot (it is baked by the gesture that reads it). The app owes that
            // other half: a preview that drops part of its transaction leaves a field
            // unscrubbable (§15 D245).
            Operation::SetName { .. }
            | Operation::SetLocked { .. }
            | Operation::SetProportionsLocked { .. }
            | Operation::SetPivot { .. }
            | Operation::SetExports { .. }
            | Operation::SetLayoutGrids { .. }
            // Being a main or linked draws nothing (`changes_ink` agrees), nor does a
            // swap (the children its commit rewrites do) or a set's, variant's or
            // component's properties.
            | Operation::SetComponent { .. }
            | Operation::SetLink { .. }
            | Operation::SetSwap { .. }
            | Operation::SetVariantSet { .. }
            | Operation::SetVariant { .. }
            | Operation::SetProperties { .. }
            | Operation::SetDescription { .. }
            | Operation::AddGuide { .. }
            | Operation::RemoveGuide { .. }
            | Operation::SetGuidePosition { .. }
            | Operation::SetGuideColor { .. }
            | Operation::SetGuideScope { .. } => {}
            // The one no-op that does change ink: `RenderOverrides` has no image table
            // to patch, and refusing would also blank the `CreateNode` beside it. A
            // previewed image shows only once committed. So `changes_ink`'s `false`
            // arm is a subset of `absorb`'s no-ops, pinned by `overrides.rs ·
            // every_no_op_of_absorb_is_a_no_op_and_two_of_them_still_change_ink`.
            Operation::AddImage { .. } | Operation::RemoveImage { .. } => {}
            // A cell's new value previews on everything that reads it. The table has
            // no override, so each reading node is patched with what it draws against
            // the table with this one cell changed (`Variables::with_cell`, the
            // document's `_in` doors), built on its pending paint and kind. One cell
            // per transaction: the paint written is resolved, so a second cell finds
            // literals and moves nothing. A cell or mode not in the table refuses.
            Operation::SetVariableCell { id: var, mode, cell } => {
                let t = doc.variables().with_cell(*var, *mode, cell.clone())?;
                let bound: Vec<NodeId> = doc.bound_nodes().collect();
                for n in bound {
                    if self.ghost(n).is_some() {
                        continue;
                    }
                    let Some(paint) = self.current_paint(doc, n) else {
                        continue;
                    };
                    // A bound stroke width too, which moves the ink, so the cull hears it.
                    let widths = paint.strokes.iter().any(|s| s.width_var.is_some());
                    let reads = paint.fills.iter().any(|f| f.brush.is_bound())
                        || paint.strokes.iter().any(|s| s.brush.is_bound())
                        || widths;
                    if widths {
                        self.mark_moved(doc, n);
                    }
                    if reads {
                        let drawn = doc.resolved_paint_in(&t, n, &paint).into_owned();
                        self.set_paint(n, drawn);
                    }
                    if let Some(NodeKind::Rect { size, corner_radii }) = self.current_kind(doc, n)
                        && let Some(radii) = doc.resolve_corners_in(&t, n, corner_radii)
                    {
                        self.set_kind(
                            doc,
                            res,
                            n,
                            NodeKind::Rect {
                                size,
                                corner_radii: radii,
                            },
                        );
                        self.moved.insert(n);
                    }
                    // A bound width or height the same way, after the corners above,
                    // whose kind it starts from; the layout pass lays the chain again.
                    if let Some(node) = doc.get(n)
                        && node.size_vars().iter().any(Option::is_some)
                        && let Some(base) = self.current_kind(doc, n)
                    {
                        let drawn = doc.size_drawn_in(&t, n, &base, node.size_vars());
                        self.set_kind(doc, res, n, drawn);
                        self.mark_moved(doc, n);
                    }
                    // A bound content or family the same way.
                    if let Some(node) = doc.get(n)
                        && node.text_vars().iter().any(Option::is_some)
                        && let Some(base) = self.current_kind(doc, n)
                    {
                        let drawn = doc.text_drawn_in(&t, n, &base, node.text_vars());
                        self.set_kind(doc, res, n, drawn);
                        self.mark_moved(doc, n);
                    }
                    // A bound visibility the same way.
                    if doc.get(n).is_some_and(|node| node.visible_var().is_some()) {
                        self.entry(n).visible = Some(doc.shown_in(&t, n));
                    }
                    // And an effect's bound numbers; the ink moves, so the cull hears it.
                    if let Some(node) = doc.get(n)
                        && node.effects().iter().any(|e| e.vars.iter().any(Option::is_some))
                    {
                        let pending = self.get(n).and_then(|o| o.effects.clone());
                        let base = pending.unwrap_or_else(|| node.effects().to_vec());
                        let drawn = doc.resolved_effects_in(&t, n, &base).into_owned();
                        self.entry(n).effects = Some(drawn);
                        self.mark_moved(doc, n);
                    }
                    // And a bound opacity.
                    if let Some(node) = doc.get(n)
                        && node.opacity_var().is_some()
                    {
                        let drawn =
                            doc.opacity_drawn_in(&t, n, node.opacity(), node.opacity_var());
                        self.entry(n).opacity = Some(drawn);
                    }
                    // A bound gap or padding the same way; the layout pass lays the
                    // chain again.
                    if let Some(node) = doc.get(n)
                        && node.spacing_vars().iter().any(Option::is_some)
                    {
                        let pending = self.get(n).and_then(|o| o.display.clone()).flatten();
                        if let Some(base) = pending.or_else(|| node.display().cloned()) {
                            let drawn = doc.spacing_drawn_in(&t, n, &base, node.spacing_vars());
                            self.entry(n).display = Some(Some(drawn));
                            self.mark_moved(doc, n);
                        }
                    }
                }
            }
            // The rest of the table has no override: the walk reads it off the
            // document, so these draw as committed. None is dragged, and absorbing
            // rather than refusing keeps the rest of the transaction previewing.
            Operation::AddVariable { .. }
            | Operation::RemoveVariable { .. }
            | Operation::SetVariableName { .. }
            | Operation::AddMode { .. }
            | Operation::RemoveMode { .. }
            | Operation::SetModeName { .. }
            | Operation::SetVariable { .. }
            | Operation::SetKitModes { .. } => {}
            // A style draws through the lists it is copied into; the table and a
            // layer's binding draw nothing of themselves. The app's style previews
            // write those lists directly, so these pass through.
            Operation::AddStyle { .. }
            | Operation::RemoveStyle { .. }
            | Operation::SetStyleName { .. }
            | Operation::SetStyleOrigin { .. }
            | Operation::SetStyleValue { .. }
            | Operation::SetFillStyle { .. }
            | Operation::SetStrokeStyle { .. }
            | Operation::SetEffectsStyle { .. }
            | Operation::SetTypeStyle { .. }
            | Operation::SetLayoutStyle { .. } => {}
            // A corner's binding previews as the radii it will draw: the bound corners
            // are resolved through the document (the mode in force is not previewed)
            // over the literal kind earlier ops left, `vars` being the whole binding.
            Operation::SetCornerVars { id, vars } => {
                let NodeKind::Rect {
                    size,
                    mut corner_radii,
                } = self.current_kind(doc, *id)?
                else {
                    return None;
                };
                let slots = [
                    &mut corner_radii.top_left,
                    &mut corner_radii.top_right,
                    &mut corner_radii.bottom_right,
                    &mut corner_radii.bottom_left,
                ];
                for (slot, var) in slots.into_iter().zip(vars) {
                    if let Some(var) = var {
                        let Some(ondin_core::Value::Number(n)) = doc.resolve_var(*id, *var) else {
                            return None;
                        };
                        *slot = n;
                    }
                }
                self.set_kind(doc, res, *id, NodeKind::Rect { size, corner_radii });
                self.moved.insert(*id);
            }
            // A width's or height's binding previews as the size it will draw, over
            // the kind earlier ops left; an axis `vars` leaves unbound keeps that.
            Operation::SetSizeVars { id, vars } => {
                let base = self.current_kind(doc, *id)?;
                base.box_size()?;
                let drawn = doc.size_drawn_in(doc.variables(), *id, &base, *vars);
                self.set_kind(doc, res, *id, drawn);
                self.moved.insert(*id);
            }
            // A content's or family's binding previews as the string it will draw, as
            // `SetSizeVars`; the bind verb's `SetText` and `SetTextStyle` come first
            // and leave the fields empty.
            Operation::SetTextVars { id, vars } => {
                let base = self.current_kind(doc, *id)?;
                if !matches!(base, NodeKind::Text { .. }) {
                    return None;
                }
                let drawn = doc.text_drawn_in(doc.variables(), *id, &base, *vars);
                self.set_kind(doc, res, *id, drawn);
                self.moved.insert(*id);
            }
            // A frame's mode changes what a whole subtree resolves to, read through
            // the document and `Resolved`, which no override patches.
            Operation::SetMode { .. } => return None,
            // A mask changes which *other* nodes the walk clips, and an override
            // patches one node. Nothing sets these mid-gesture.
            Operation::SetClip { .. }
            | Operation::SetMask { .. }
            | Operation::SetMaskMode { .. }
            // A fill rule is not structural, but `NodeOverride` has no field for it
            // and nothing scrubs it; if something does, add a field.
            | Operation::SetFillRule { .. } => return None,
            // A reorder is a patch on the parent, read through `children_of`. The
            // index is `Document::apply`'s — into the list with the node taken out —
            // and one it would refuse is refused here.
            Operation::Reorder { id, index } => {
                let parent = doc.get(*id)?.parent()?;
                let mut order = self
                    .order
                    .get(&parent)
                    .cloned()
                    .unwrap_or_else(|| doc.get(parent).map(|p| p.children().to_vec()).unwrap_or_default());
                order.retain(|c| c != id);
                if *index > order.len() {
                    return None;
                }
                order.insert(*index, *id);
                self.order.insert(parent, order);
                // Touched, so `flex_relayout` finds the chain to re-lay.
                self.entry(*id);
            }
            // Structural: no patch can say "somewhere else" or "not there".
            Operation::DeleteNode { .. } | Operation::Reparent { .. } => return None,
            // A vault draws nothing but is a root no gesture builds. Refused rather
            // than filed with the no-ops, whose count is pinned against `changes_ink`'s.
            Operation::OpenVault { .. }
            | Operation::CloseVault { .. }
            | Operation::SetOrigin { .. } => return None,
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
    ///
    /// The document's kind under the overrides, not the used one: this is what an
    /// op is *applied to*, and building on a laid-out size would write it back as
    /// typed. Drawing asks [`Self::drawn_kind`].
    fn current_kind(&self, doc: &Document, id: NodeId) -> Option<NodeKind> {
        if let Some(g) = self.ghost(id) {
            return Some(g.kind.clone());
        }
        // At its drawn size where a width or height is bound: the stored literal
        // there is 0. Unbound, this is the stored kind.
        self.get(id)
            .and_then(|o| o.kind.clone())
            .or_else(|| specified_kind(doc, id))
    }

    /// The kind to **measure and draw** `id` with: a ghost's, then the override's,
    /// then the used kind — [`Self::current_kind`]'s twin for the reading side;
    /// they differ only in that last fallback.
    fn drawn_kind(&self, doc: &Document, res: &Resolved, id: NodeId) -> Option<NodeKind> {
        if let Some(g) = self.ghost(id) {
            return Some(g.kind.clone());
        }
        self.get(id)
            .and_then(|o| o.kind.clone())
            .or_else(|| res.used_kind(doc, id).cloned())
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
        // Re-shape only when the text changed, so a resize drag does not re-run
        // parley. The guard cannot fire for a live session, which commits once at
        // `finish_text_edit` (§9.3); that caller hands its layout over in `prepared_text`.
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

/// A preview as the flex engine sees it (`ondin_core::container::LayoutView`):
/// the **specified** value the transaction leaves (kind, layout, item, insets,
/// visibility) over the document's tree, bindings resolved. `Resolved`'s
/// `UsedView` is the committed twin; one engine runs over both.
///
/// Ghosts are not in the tree it describes: a copy Alt-dragged into a flex
/// container is laid out on release, and until then its siblings make no room.
///
/// Its one used answer is the box a pinned root is stretched against
/// ([`LayoutView::parent_box`](ondin_core::container::LayoutView::parent_box)):
/// the parent's size as this preview draws it, else as `res` says it is drawn.
struct PreviewView<'a> {
    doc: &'a Document,
    ov: &'a RenderOverrides,
    res: &'a Resolved,
}

impl ondin_core::container::LayoutView for PreviewView<'_> {
    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.doc.get(id).and_then(|n| n.parent())
    }
    fn children(&self, id: NodeId) -> Vec<NodeId> {
        if let Some(order) = self.ov.children_of(id) {
            return order.to_vec();
        }
        self.doc
            .get(id)
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }
    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.ov.current_kind(self.doc, id)
    }
    /// An override's layout is stored **resolved** — its bound gaps and padding
    /// already the numbers they draw (`SetDisplay`'s and `SetSpacingVars`' absorb) —
    /// so it is answered as it is; the document's is resolved here.
    fn display(&self, id: NodeId) -> Option<std::borrow::Cow<'_, ondin_core::container::Display>> {
        match self.ov.get(id).and_then(|o| o.display.as_ref()) {
            Some(d) => d.as_ref().map(std::borrow::Cow::Borrowed),
            None => self.doc.resolved_display(id),
        }
    }
    /// Bound limits resolved, as `Document::resolved_item` lays them out.
    fn item(&self, id: NodeId) -> ondin_core::container::LayoutItem {
        match self.ov.get(id).and_then(|o| o.item) {
            Some(item) => self.doc.limits_drawn_in(self.doc.variables(), id, item),
            None => self.doc.resolved_item(id),
        }
    }
    fn insets(&self, id: NodeId) -> ondin_core::Insets {
        self.ov
            .get(id)
            .and_then(|o| o.insets)
            .or_else(|| self.doc.get(id).map(|n| *n.insets()))
            .unwrap_or_default()
    }
    fn visible(&self, id: NodeId) -> bool {
        self.ov.operand_visible(self.doc, id)
    }
    fn mask(&self, id: NodeId) -> bool {
        self.doc.get(id).is_some_and(|n| n.mask())
    }
    fn local(&self, id: NodeId) -> Affine {
        self.ov
            .get(id)
            .and_then(|o| o.transform)
            .or_else(|| self.doc.get(id).map(|n| n.transform()))
            .unwrap_or_default()
    }
    fn parent_box(&self, id: NodeId) -> Option<kurbo::Size> {
        let parent = self.doc.get(id)?.parent()?;
        let over = self.ov.get(parent);
        match over.and_then(|o| o.kind.as_ref()) {
            Some(NodeKind::Artboard { size }) => Some(*size),
            _ => match self.res.used_kind(self.doc, parent)? {
                NodeKind::Artboard { size } => Some(*size),
                NodeKind::Group => over.and_then(|o| o.frame).or(self.res.used_frame(parent)),
                _ => None,
            },
        }
    }
    /// The committed pass's measurement where this preview leaves the node's kind
    /// alone, so a per-frame re-lay does not re-shape every text item. A node this
    /// preview gives a kind of its own is measured afresh.
    fn measured(&self, id: NodeId, kind: &NodeKind) -> Option<kurbo::Rect> {
        if self.ov.get(id).and_then(|o| o.kind.as_ref()).is_none()
            && let Some(b) = self.res.remembered_text_box(id, kind)
        {
            return b;
        }
        ondin_core::geometry::local_bounds(kind, None)
    }
    fn content_widths(&self, id: NodeId, kind: &NodeKind) -> Option<(f64, f64)> {
        if self.ov.get(id).and_then(|o| o.kind.as_ref()).is_none()
            && let Some(w) = self.res.remembered_text_widths(id)
        {
            return w;
        }
        ondin_core::node::TextRef::of(kind).map(ondin_core::text::content_widths)
    }
    fn first_baseline(&self, id: NodeId, kind: &NodeKind) -> Option<f64> {
        if self.ov.get(id).and_then(|o| o.kind.as_ref()).is_none()
            && let Some(b) = self.res.remembered_text_baseline(id, kind)
        {
            return b;
        }
        let layout = ondin_core::text::layout(ondin_core::node::TextRef::of(kind)?);
        layout.baselines.first().map(|b| b - layout.origin.y)
    }
}

/// Whether an `InsertSubtree`'s `nodes` would put a frame under a boolean or a
/// mask — `Document::apply`'s walk, asked of a subtree not yet in the document.
/// `above_ok` is the destination's own answer,
/// [`ondin_core::Document::frame_may_sit_under`].
///
/// Walked up from each frame, since the list is not yet a validated tree
/// ([`ghost_subtree`] refuses a malformed one).
fn frame_barred(nodes: &[ondin_core::Node], above_ok: bool) -> bool {
    let by_id: FxHashMap<NodeId, &ondin_core::Node> = nodes.iter().map(|n| (n.id(), n)).collect();
    let bars = |n: &ondin_core::Node| matches!(n.kind(), NodeKind::Boolean { .. }) || n.mask();
    nodes
        .iter()
        .filter(|n| matches!(n.kind(), NodeKind::Artboard { .. }))
        .any(|frame| {
            let mut up = frame.parent();
            // Bounded by the list's length, so a cycle in a malformed list ends.
            for _ in 0..nodes.len() {
                match up.and_then(|p| by_id.get(&p)) {
                    Some(n) if bars(n) => return true,
                    Some(n) => up = n.parent(),
                    None => return !above_ok,
                }
            }
            false
        })
}

/// Turn a captured subtree — the node list an `InsertSubtree` carries — into a
/// ghost of it.
///
/// `None` if the list is not a single well-formed tree: no root, more than one
/// root, or a child id the list does not contain — the conditions `Document::apply`
/// rejects the op for.
fn ghost_subtree(doc: &Document, nodes: &[ondin_core::Node]) -> Option<GhostNode> {
    let by_id: FxHashMap<NodeId, &ondin_core::Node> = nodes.iter().map(|n| (n.id(), n)).collect();
    // The root is the one node whose parent is outside the list.
    let mut roots = nodes
        .iter()
        .filter(|n| n.parent().is_none_or(|p| !by_id.contains_key(&p)));
    let root = roots.next()?;
    if roots.next().is_some() {
        return None;
    }
    build_ghost(doc, root, &by_id, &mut FxHashMap::default())
}

/// A captured node's kind at its drawn size: a bound width, height, content or
/// family resolved through `doc` in the default mode, [`ghost_shown`]'s reading
/// (their literals are 0 or empty).
fn ghost_kind(doc: &Document, n: &ondin_core::Node) -> NodeKind {
    let sized = doc.size_drawn_in(doc.variables(), n.id(), n.kind(), n.size_vars());
    let kind = doc.text_drawn_in(doc.variables(), n.id(), &sized, n.text_vars());
    with_corners_drawn(doc, n, kind)
}

/// `id`'s kind as specified, with every binding of its own resolved (size, text,
/// corners) and not layout's. `Document::resolved_kind` leaves the corners to
/// `Resolved`, which a preview's re-laid kind never passes through.
fn specified_kind(doc: &Document, id: NodeId) -> Option<NodeKind> {
    let kind = doc.resolved_kind(id)?.into_owned();
    Some(with_corners_drawn(doc, doc.get(id)?, kind))
}

/// `kind` with `n`'s bound corners read in the mode in force, for a rect.
fn with_corners_drawn(doc: &Document, n: &ondin_core::Node, kind: NodeKind) -> NodeKind {
    match kind {
        NodeKind::Rect { size, corner_radii } if n.corner_vars().iter().any(Option::is_some) => {
            NodeKind::Rect {
                size,
                corner_radii: doc.corners_drawn_in(
                    doc.variables(),
                    n.id(),
                    corner_radii,
                    n.corner_vars(),
                ),
            }
        }
        k => k,
    }
}

/// Whether a captured node shows: its binding resolved through `doc` in the
/// **default mode** (the ghost's id is in no frame yet, as `paint_ghost` resolves a
/// bound paint), else its literal.
fn ghost_shown(doc: &Document, n: &ondin_core::Node) -> bool {
    match n.visible_var() {
        Some(v) => match doc.resolve_var(n.id(), v) {
            Some(ondin_core::Value::Boolean(b)) => b,
            _ => n.visible(),
        },
        None => n.visible(),
    }
}

/// Builds the ghost of `node` and its subtree. `derived` accumulates the boolean
/// outlines built so far, filled children-first, so a nested boolean is read from it.
fn build_ghost(
    doc: &Document,
    node: &ondin_core::Node,
    by_id: &FxHashMap<NodeId, &ondin_core::Node>,
    derived: &mut FxHashMap<NodeId, kurbo::BezPath>,
) -> Option<GhostNode> {
    let mut children = Vec::with_capacity(node.children().len());
    for id in node.children() {
        children.push(build_ghost(doc, by_id.get(id)?, by_id, derived)?);
    }
    // Over the captured `Node`s, not the `GhostNode`s just built: a second walk over
    // the ghosts would be a second reading of what an operand contributes.
    let path = match node.kind() {
        NodeKind::Boolean { op } => {
            let result = ondin_core::boolean::evaluate_node(
                node.id(),
                *op,
                &ondin_core::boolean::Operands {
                    local_of: &|c| by_id.get(&c).map(|n| n.transform()).unwrap_or_default(),
                    kind_of: &|c| by_id.get(&c).map(|n| ghost_kind(doc, n)),
                    children_of: &|c| {
                        by_id
                            .get(&c)
                            .map(|n| n.children().to_vec())
                            .unwrap_or_default()
                    },
                    visible_of: &|c| by_id.get(&c).is_some_and(|n| ghost_shown(doc, n)),
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
    let kind = ghost_kind(doc, node);
    Some(GhostNode {
        id: node.id(),
        transform: node.transform(),
        // Shaped once here, not per frame, from the kind it draws (a bound
        // content's string); `Resolved` has never seen this node.
        text: shaped(&kind),
        kind,
        // Resolved at capture in the default mode `paint_ghost` reads a ghost in,
        // so `scene::ghost_ink`, which has no document, measures the drawn stroke width.
        paint: doc.resolved_paint(node.id(), node.paint()).into_owned(),
        // Resolved at capture like the paint: nothing downstream resolves effects.
        effects: doc.resolved_effects(node.id(), node.effects()).into_owned(),
        path,
        // A bound opacity resolved in the default mode, as `ghost_shown`: the id is
        // in no document, so `drawn_opacity` would answer 1 for every ghost.
        opacity: doc.opacity_drawn_in(
            doc.variables(),
            node.id(),
            node.opacity(),
            node.opacity_var(),
        ),
        visible: ghost_shown(doc, node),
        clip: node.clip(),
        mask: node.mask(),
        mask_mode: node.mask_mode(),
        children,
    })
}

fn shaped(kind: &NodeKind) -> Option<TextLayout> {
    ondin_core::TextRef::of(kind).map(text::layout)
}
