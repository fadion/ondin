//! Resolved — the derived layer (§5.9, invariant 4).
//!
//! World transforms, stroke-expanded world bounds, shaped text layouts, and an
//! R-tree spatial index over the indexable leaves. NEVER serialized; always
//! reconstructible from the `Document` via `rebuild`. `update` incrementally
//! applies a `DirtySet`: text is re-shaped, then transforms and bounds are
//! recomputed, only for affected subtrees (plus bound propagation up to the
//! root); the index is then refreshed from the current bounds. `update` must be
//! equivalent to `rebuild` — a randomized differential test enforces this.
//!
//! The text cache is the reason this layer exists for text at all: shaping is
//! by far the most expensive thing in the pipeline, and bounds, hit-testing and
//! scene building all need it. Shaping depends on which fonts are registered
//! with `crate::text`, so when the app registers a newly downloaded or
//! system-loaded family it must call [`Resolved::invalidate_text`].

use crate::boolean;
use crate::document::Document;
use crate::effect;
use crate::geometry;
use crate::id::NodeId;
use crate::node::{Node, NodeKind};
use crate::op::DirtySet;
use crate::text::{self, TextLayout};
use kurbo::{Affine, BezPath, Rect};
use rstar::{AABB, RTree, RTreeObject};
use rustc_hash::{FxHashMap, FxHashSet};

/// R-tree leaf: the world bounds of one indexable node.
#[derive(Clone, PartialEq, Debug)]
pub struct BoundsEntry {
    pub id: NodeId,
    min: [f64; 2],
    max: [f64; 2],
}

impl RTreeObject for BoundsEntry {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.min, self.max)
    }
}

/// `b` cut down to `m`, or `None` if the two do not meet.
///
/// Not `Rect::is_empty`, which calls a zero-height box empty: a horizontal line
/// has exactly that box, and dropping its contribution would shrink the box its
/// container is culled against.
fn clipped_to(b: Rect, m: Rect) -> Option<Rect> {
    let i = b.intersect(m);
    (i.x1 >= i.x0 && i.y1 >= i.y0).then_some(i)
}

/// The box a mask **clips with**, read out of already-measured child boxes
/// (§15 D460).
///
/// **Not the same thing as the mask's own box, and that gap is the whole of
/// `[S4.1-L2-01]`.** [`Resolved::mask_path`]'s `Group | Root` arm skips a mask
/// *inside* a mask group and unions the rest **un-narrowed** — a deliberate
/// choice, since the exact answer needs a path intersection and *"a clip that is
/// too generous"* is the safe direction. The bounds passes did the opposite: they
/// applied the inner mask, so `world_bounds(M)` was the inner mask's box while
/// `mask_path(M)` was the whole union, and nothing reconciled them.
///
/// Measured in release on a group `M` marked *Use as mask*, holding a 40×40 rect
/// also marked *Use as mask* under a 400×400 one:
///
/// ```text
/// mask_path(M).bbox = 0,0 .. 400,400   <- what actually clips, and what hit_test asks
/// world_bounds(M)   = 0,0 ..  40,40    <- what clipped the box of every sibling above it
/// ```
///
/// The container `G` therefore reported a 40×40 box for artwork drawn across
/// 400×400, and since `scene::paint_node` prunes a `Group`'s whole subtree on
/// `ink_bounds`, **a layer that was on screen and clickable was simply not
/// painted** as soon as the viewport stopped touching the 40×40 corner. Driven
/// through the real walk with a recording painter: 1 fill with the page in view,
/// **0** scrolled onto the artwork, 1 for the same group with no inner mask.
/// Every other consumer of the container's box was wrong by the same 10× —
/// *zoom to selection*, the selection outline, the marquee, the inspector's W/H,
/// and `ondin_export::plan`'s exported region.
///
/// **Written once and called from both passes**, which is the point: `boxes` is
/// whichever map the caller is filling, so the incremental
/// [`Resolved::recompute_bounds`] and the full `resolve_subtree` cannot drift
/// apart the way this rule and `mask_path` did. It reads only child boxes that
/// are already fresh — in the full pass because the child's whole subtree has
/// been resolved, in the incremental one because a node not in the dirty set
/// keeps a box that is still valid.
///
/// **Arm for arm with `mask_path`**, which is what makes it right rather than
/// merely different: the `Group | Root` arm filters mask children out and unions
/// the rest, an invisible node answers `None`, and every other kind is its own
/// measured box. It costs a walk of the mask's subtree, once per mask.
fn mask_extent(doc: &Document, boxes: &FxHashMap<NodeId, Rect>, id: NodeId) -> Option<Rect> {
    let node = doc.get(id)?;
    if !node.visible() {
        return None;
    }
    match node.kind() {
        NodeKind::Group | NodeKind::Root => node
            .children()
            .iter()
            .filter(|c| !doc.get(**c).is_some_and(|n| n.mask()))
            .filter_map(|c| mask_extent(doc, boxes, *c))
            .reduce(|a, b| a.union(b)),
        _ => boxes.get(&id).copied(),
    }
}

fn entry(id: NodeId, r: Rect) -> BoundsEntry {
    BoundsEntry {
        id,
        min: [r.min_x(), r.min_y()],
        max: [r.max_x(), r.max_y()],
    }
}

/// A node's **used** geometry, where it differs from what the document specifies
/// (§15 D868) — CSS's *used* value against its *specified* one.
///
/// **Sparse, and that is the whole contract.** A node has an entry only when
/// container layout placed or sized it somewhere other than the document says, so
/// a document with no layout in it holds nothing here and every read falls back to
/// [`Node::transform`] and [`Node::kind`]. The two fields are separate `Option`s
/// because the two are separate facts: a flex item that moves along its row keeps
/// its size, and one that grows keeps nothing but its kind.
///
/// Read through [`local_in`] and [`kind_in`] inside this module and through
/// [`Resolved::used_local`] and [`Resolved::used_kind`] outside it — never off the
/// map directly, which is what keeps "no entry" meaning "as specified" everywhere.
#[derive(Clone, Debug, PartialEq)]
struct Used {
    /// The local transform layout places the node with.
    local: Option<Affine>,
    /// The kind layout sizes the node with. **Always the same variant as the
    /// document's kind**: layout resizes a rectangle, it never makes one into
    /// anything else — which is what lets every `match` on a node's *kind* to ask
    /// what sort of node it is keep reading the document.
    kind: Option<NodeKind>,
    /// The box of a container with a layout and **no size of its own** — a group
    /// with `display` (§15 D869), whose size is whatever its layout makes it. Its
    /// origin is the group's local origin. `None` for everything else, a frame
    /// included: a frame's size is its kind's.
    frame: Option<kurbo::Size>,
}

/// The committed document as the flex engine sees it (`container::LayoutView`):
/// every answer a **specified** value, read straight off the node. Crate-visible
/// for `build`'s conversions, which ask the same questions of the same tree.
pub(crate) struct DocView<'a>(pub(crate) &'a Document);

impl crate::container::LayoutView for DocView<'_> {
    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.0.get(id).and_then(|n| n.parent())
    }
    fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.0
            .get(id)
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }
    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.0.get(id).map(|n| n.kind().clone())
    }
    fn display(&self, id: NodeId) -> Option<crate::container::Display> {
        self.0.get(id).and_then(|n| n.display().copied())
    }
    fn item(&self, id: NodeId) -> crate::container::FlexItem {
        self.0.get(id).map(|n| *n.item()).unwrap_or_default()
    }
    fn insets(&self, id: NodeId) -> crate::container::Insets {
        self.0.get(id).map(|n| *n.insets()).unwrap_or_default()
    }
    fn visible(&self, id: NodeId) -> bool {
        self.0.get(id).is_some_and(|n| n.visible())
    }
    fn mask(&self, id: NodeId) -> bool {
        self.0.get(id).is_some_and(|n| n.mask())
    }
    fn local(&self, id: NodeId) -> Affine {
        self.0.get(id).map(|n| n.transform()).unwrap_or_default()
    }
}

/// The box a pinned child is placed against: its parent frame's used size, or
/// the laid-out box of a group with a layout. `None` for a parent with no edges.
fn frame_box(used: &FxHashMap<NodeId, Used>, parent: &Node) -> Option<kurbo::Size> {
    match kind_in(used, parent) {
        NodeKind::Artboard { size } => Some(*size),
        NodeKind::Group => used.get(&parent.id()).and_then(|u| u.frame),
        _ => None,
    }
}

/// The used geometry of `id`, or `None` where it is the specified geometry.
///
/// **Flex first** (§15 D875): a node the pass laid out — an item placed from
/// `laid`, or a layout root sized by its own layout — answers what the layout
/// made of it, a group's box included.
///
/// **Then absolute insets** (§15 D871, D867's build step 2): a child of a frame,
/// or of a group with a layout, with any inset set is placed by
/// [`crate::container::place`] against its container's **used** box — read out
/// of `used`, which the caller fills parents first, so a frame another frame has
/// stretched hands its children the stretched size. Everything else answers
/// `None` and is drawn where the document says.
///
/// Until step 2 this answered `None` for every node, on purpose: step 1 routed
/// every consumer through [`Used`] behind that identity pass so that the refactor
/// was proved by the goldens staying byte-identical, and a document with no insets
/// in it still takes exactly that path.
///
/// Under `cfg(test)` a probe can answer instead (`probe::set`), which is how this
/// module's tests drive arbitrary used geometry through the plumbing; a probe
/// answering `None` falls through to the real layout.
fn used_geometry(
    doc: &Document,
    used: &FxHashMap<NodeId, Used>,
    laid: &FxHashMap<NodeId, crate::container::Laid>,
    id: NodeId,
) -> Option<Used> {
    #[cfg(test)]
    if let Some(u) = probe::used(doc, id) {
        return Some(u);
    }
    let node = doc.get(id)?;
    // **Laid out by a flex container** (step 3): an in-flow item takes its slot
    // and size from the pass its chain root ran; a layout root takes only its size
    // from its own pass (it hugs), and is placed as any other child of its parent.
    if let Some(l) = laid.get(&id) {
        let view = DocView(doc);
        let item =
            crate::container::parent_lays_out(&view, id) && crate::container::in_flow(&view, id);
        if item {
            return crate::container::item_placed(&view, id, l.slot, l.size).map(|p| Used {
                local: p.local,
                kind: p.kind,
                frame: p.frame,
            });
        }
        return root_used(doc, used, node, l.size);
    }
    let parent = doc.get(node.parent()?)?;
    let frame = frame_box(used, parent)?;
    let (local, kind) =
        crate::container::place(node.insets(), frame, node.transform(), node.kind())?;
    Some(Used {
        local: Some(local),
        kind,
        frame: None,
    })
}

/// A layout root's used geometry: the size its own pass gave it — a frame asked
/// to hug grows its kind, a group with a layout takes the box — placed as any
/// other child of its parent (by its transform, or by insets in a frame).
fn root_used(
    doc: &Document,
    used: &FxHashMap<NodeId, Used>,
    node: &Node,
    size: kurbo::Size,
) -> Option<Used> {
    let (kind, frame) = crate::container::root_sized(&DocView(doc), node.id(), size);
    // Insets place a frame root inside a frame the step-2 way; a group root takes
    // no insets yet (its box is new in this step, §15 D869).
    let pinned = node
        .parent()
        .and_then(|p| doc.get(p))
        .and_then(|p| frame_box(used, p))
        .and_then(|f| {
            let k = kind.clone().unwrap_or_else(|| node.kind().clone());
            crate::container::place(node.insets(), f, node.transform(), &k)
        });
    match pinned {
        Some((local, k)) => Some(Used {
            local: Some(local),
            kind: k.or(kind),
            frame,
        }),
        None => (kind.is_some() || frame.is_some()).then_some(Used {
            local: None,
            kind,
            frame,
        }),
    }
}

/// `id`'s used geometry, running its layout pass first when it roots one — so a
/// chain's items find their results in `laid` when the parents-first walk reaches
/// them.
fn place_node(
    doc: &Document,
    used: &FxHashMap<NodeId, Used>,
    laid: &mut FxHashMap<NodeId, crate::container::Laid>,
    id: NodeId,
) -> Option<Used> {
    if crate::container::is_layout_root(&DocView(doc), id) {
        for l in crate::container::lay_out(&DocView(doc), id) {
            laid.insert(l.id, l);
        }
    }
    used_geometry(doc, used, laid, id)
}

/// `node`'s used local transform: the entry's where layout moved it, the
/// document's otherwise.
fn local_in(used: &FxHashMap<NodeId, Used>, node: &Node) -> Affine {
    used.get(&node.id())
        .and_then(|u| u.local)
        .unwrap_or_else(|| node.transform())
}

/// `node`'s used kind: the entry's where layout resized it, the document's
/// otherwise.
fn kind_in<'a>(used: &'a FxHashMap<NodeId, Used>, node: &'a Node) -> &'a NodeKind {
    used.get(&node.id())
        .and_then(|u| u.kind.as_ref())
        .unwrap_or_else(|| node.kind())
}

/// Nodes that appear in the spatial index — the drawable/selectable leaves.
/// Groups and Root are containers and are excluded.
///
/// `pub(crate)` for one reader and it is a test: `node`'s kind-predicate matrix
/// asserts the set this divides `NodeKind` into, beside the five others (§15
/// D668). Its membership is **identical to `NodeKind::takes_paint`'s** today, for
/// two unrelated reasons, which is the drift the matrix exists to make visible.
pub(crate) fn is_indexed(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Path { .. }
            | NodeKind::Text { .. }
            | NodeKind::Artboard { .. }
            // A container, but a drawable and clickable one: it is the only thing
            // in a boolean subtree the canvas paints, so it is the only thing there
            // a click can land on. Left out at first, and the symptom was a boolean
            // that could be selected from the layers tree and not from the canvas —
            // `hit_test` asks the index for candidates, and it was never a candidate.
            | NodeKind::Boolean { .. }
    )
}

pub struct Resolved {
    world: FxHashMap<NodeId, Affine>,
    world_bounds: FxHashMap<NodeId, Rect>,
    /// The world AABB of everything a node's **ink** covers, effects included —
    /// [`Self::world_bounds`] grown by [`crate::effect::stack_escape`] at every
    /// level of the subtree.
    ///
    /// **A second map rather than a wider `world_bounds`, and the split is the
    /// point.** `world_bounds` is the box of the thing you are *editing*: the
    /// selection outline, the inspector's W and H, snapping, the rulers and the
    /// marquee all read it, and a shadow must not move any of them — softening a
    /// blur would grow the reported height, and a snap would catch on a shadow's
    /// edge. Culling and export need the opposite answer, because a drop shadow
    /// really is drawn out there and `ondin_export::plan` really does decide the
    /// exported region from a box.
    ///
    /// Equal to `world_bounds` for the overwhelming majority of nodes, which is
    /// why it is a map and not a field on every entry: nothing carries an effect
    /// until someone adds one.
    ink_bounds: FxHashMap<NodeId, Rect>,
    /// [`Self::ink_bounds`] **before this node's own effects escape** — what the
    /// subtree draws, with every descendant's effects in it and none of this
    /// node's.
    ///
    /// **A third box, and it is a third question rather than a convenience.**
    /// `world_bounds` is what you are editing and `ink_bounds` is what is drawn
    /// (§15 D334); this is what an effect *layer* has to be big enough to hold
    /// before its own padding is added — which is the only one of the three that a
    /// caller with a **different** effect stack in hand can use. The renderer has
    /// exactly that during a preview: the panel is scrubbing a blur, so the live
    /// stack is in `RenderOverrides` and the cache knows only the committed one.
    /// Sizing the buffer from `ink_bounds` clipped the very effect being dragged
    /// and un-clipped it on release (§15 D341).
    ///
    /// Only the `Group`/`Root` arm of `scene::effect_bounds` reads it: every other
    /// kind's ink is its own box, and the walk has the *live* geometry there.
    inner_ink: FxHashMap<NodeId, Rect>,
    /// Shaped layout per `Text` node — the cache described in §5.9. Keyed by
    /// node id and invalidated through the `DirtySet` like everything else.
    text: FxHashMap<NodeId, TextLayout>,
    /// Combined outline per `Boolean` node, in that node's own local space.
    ///
    /// **The second derived-geometry cache, and it exists for the same reason the
    /// first does.** A boolean's outline is a function of its whole subtree, so
    /// there is nowhere in the document to keep it and no way for
    /// `geometry::local_path` to produce it; bounds, hit-testing, the scene walk
    /// and both exporters all read it from here. Unlike the text cache it is
    /// evaluated **children-first**, because a boolean may contain a boolean.
    boolean: FxHashMap<NodeId, BezPath>,
    /// Booleans whose arithmetic was **abandoned** rather than merely empty — an
    /// upstream panic caught by `boolean::evaluate`'s guard (§15 D239).
    ///
    /// **The defect this was built for is fixed** (flo_curves 0.8.1, 2026-08-31)
    /// and the guard is deliberately kept, so an entry here now means an *unknown*
    /// upstream panic rather than the known one. The set, and everything that reads
    /// it, is unchanged: what a caught unwind means changed, not what happens next.
    ///
    /// A separate set instead of an absence in `boolean` above, because the two
    /// states this tells apart are both spelled "no entry there": an intersection
    /// of shapes that do not overlap is empty and *correct*, and drawing nothing
    /// for it is the right answer. Only the other one is worth saying out loud.
    failed: FxHashSet<NodeId>,
    index: RTree<BoundsEntry>,
    /// Used geometry, sparse (§15 D868) — see [`Used`].
    ///
    /// **The seventh map, and §5.9 calls a seventh map a decision rather than an
    /// optimisation.** D868 is that decision: unlike the local-box cache D778
    /// declined, which would have held what any caller could already derive, a
    /// used box is derivable nowhere else. What it costs is the thing §5.9 warns
    /// of — `update` must keep it equal to `rebuild` — and
    /// `assert_resolved_matches_rebuild` compares it through [`Self::used_local`]
    /// and [`Self::used_kind`] for every node.
    ///
    /// **Filled first**, before text is shaped and before any world transform is
    /// composed, because both read it: a text node is shaped at its used kind, and
    /// a world transform composes used locals.
    used: FxHashMap<NodeId, Used>,
}

impl Resolved {
    /// Full reconstruction from the document.
    pub fn rebuild(doc: &Document) -> Self {
        // Used geometry before anything else, since the walk below shapes text at
        // its used kind and composes world transforms from used locals.
        // Pre-order, so a frame's entry is in the map before its children are
        // placed against it.
        let mut used: FxHashMap<NodeId, Used> = FxHashMap::default();
        let mut laid = FxHashMap::default();
        for id in crate::subtree_nodes(doc, &[doc.root()]) {
            if let Some(u) = place_node(doc, &used, &mut laid, id) {
                used.insert(id, u);
            }
        }
        let mut world = FxHashMap::default();
        let mut world_bounds = FxHashMap::default();
        let mut ink_bounds = FxHashMap::default();
        let mut inner_ink = FxHashMap::default();
        let mut text = FxHashMap::default();
        resolve_subtree(
            doc,
            doc.root(),
            Affine::IDENTITY,
            &used,
            &mut Caches {
                world: &mut world,
                world_bounds: &mut world_bounds,
                ink_bounds: &mut ink_bounds,
                inner_ink: &mut inner_ink,
                text: &mut text,
            },
        );
        let index = build_index(doc, &world_bounds);
        let mut resolved = Self {
            world,
            world_bounds,
            ink_bounds,
            inner_ink,
            text,
            boolean: FxHashMap::default(),
            failed: FxHashSet::default(),
            index,
            used,
        };
        // **Booleans in a second pass, through `update`.** The walk above cannot do
        // them: a boolean's outline needs its children's outlines, and a nested one
        // needs the cache it is itself filling. Rather than thread the cache
        // through the recursion and write the children-first ordering a second
        // time — the exact shape of drift this file's comments keep warning about —
        // the ids are handed to `update`, which already owns that ordering. A
        // document with no booleans makes this an early return.
        let booleans: FxHashSet<NodeId> = crate::subtree_nodes(doc, &[doc.root()])
            .into_iter()
            .filter(|id| {
                matches!(
                    doc.get(*id).map(|n| n.kind()),
                    Some(NodeKind::Boolean { .. })
                )
            })
            .collect();
        resolved.update(doc, &DirtySet(booleans));
        resolved
    }

    /// Incrementally apply a `DirtySet`. Equivalent in result to `rebuild`.
    pub fn update(&mut self, doc: &Document, dirty: &DirtySet) {
        if dirty.0.is_empty() {
            return;
        }

        // Drop nodes that no longer exist.
        for id in dirty.0.iter().filter(|id| !doc.contains(**id)) {
            self.world.remove(id);
            self.world_bounds.remove(id);
            self.ink_bounds.remove(id);
            self.inner_ink.remove(id);
            self.text.remove(id);
            self.boolean.remove(id);
            self.failed.remove(id);
            self.used.remove(id);
        }

        let existing: Vec<NodeId> = dirty
            .0
            .iter()
            .copied()
            .filter(|id| doc.contains(*id))
            .collect();
        if existing.is_empty() {
            // Only deletions happened; still refresh bounds up-tree from parents,
            // which the op layer added to the dirty set, so this branch is rare.
            self.reindex(doc);
            return;
        }

        // Recompute set: every dirty node plus its descendants (their world
        // transforms depend on the dirty ancestor).
        //
        // **Keyed by depth as it goes** (§15 D594): the walk descends, so a
        // child's depth is its parent's plus one and the sort below needs no
        // second traversal.
        let mut affected: FxHashMap<NodeId, usize> = FxHashMap::default();
        for id in &existing {
            // **From the root of its flex chain, not from itself** (step 3): an
            // item's change moves its siblings and can resize every hugging
            // container above it, and the pass that says where they all go runs
            // at the chain's root. For a node in no chain this is the node.
            let from = crate::container::chain_root(&DocView(doc), *id);
            collect_subtree(doc, from, depth(doc, from), &mut affected);
        }
        let mut ordered: Vec<NodeId> = affected.keys().copied().collect();

        // Parents before children — for the used-geometry pass below, where a
        // child is placed against its frame's *used* size (§15 D871), and for the
        // world transforms after it.
        //
        // 🚨 **`sort_by_cached_key`, and the word is worth 47×** (§15 D594,
        // `[S4.1-L4-03]`). `sort_by_key` is documented to call its key function on
        // **every comparison**, and `depth` walks the parent chain to the root —
        // so this was Θ(n log n) walks of O(d) each, per commit. `ordered` is
        // collected from an `FxHashSet`, so it arrives scrambled and the adaptive
        // sort gets no help from a nearly-ordered input.
        //
        // Measured in release with the node count held near constant and depth
        // varied — `update` against the full `rebuild` it exists to avoid:
        //
        // ```text
        // depth × per-level   nodes | update | rebuild | this line
        //   200 ×   1           402 |  5.819 |  0.122  | 5.431  (93% of update)
        //    20 ×  10           222 |  0.357 |  0.075  | 0.357  (100%)
        //     4 ×  50           206 |  0.098 |  0.066  | 0.045  (46%)
        //     1 × 200           203 |  0.063 |  0.060  | 0.010  (16%)     (ms)
        // ```
        //
        // At depth 20 — ordinary for imported SVG — the incremental path was
        // **4.8×** the full reconstruction; at depth 200, **47×**. `rebuild` needs
        // no sort at all, its recursion putting parents before children
        // structurally, which is why it wins outright the moment depth costs
        // anything.
        //
        // ⚠️ **The quantity is depth, and `[A4-MAP]`'s cost model has no depth
        // axis** — it denominates this path in node count, and 402 nodes at depth
        // 200 cost more here than 4,000 flat ones.
        //
        // The key is a lookup rather than a walk because `collect_subtree`
        // recorded it on the way down; `sort_by_cached_key` then asks once per
        // element rather than once per comparison. `sort_by_key` with the *walk*
        // measured 42× the full `rebuild` on a 200-deep chain in debug, the walk
        // with `sort_by_cached_key` 3.2×, and this 1.2×.
        ordered.sort_by_cached_key(|id| affected.get(id).copied().unwrap_or(0));

        // Used geometry before text and before transforms (§15 D868), since both
        // read it. Over `affected` rather than `existing`: whether layout moves a
        // node is a question about its container, so a dirty ancestor is exactly
        // the thing that can change the answer. Each node whose entry actually
        // *changed* is remembered, because a text node is shaped at its used kind
        // and has to be re-shaped when that moves even though its own kind did not
        // — which is the one door past D590's rule below, and it is only as wide
        // as the entries that changed. **Siblings are in `affected`** because the
        // collection above starts from each dirty node's `chain_root` (§15 D868's
        // widening, paid by step 3, D875): an item moves when its neighbour grows,
        // and the whole chain is re-laid.
        let mut relaid: Vec<NodeId> = Vec::new();
        let mut laid = FxHashMap::default();
        for id in &ordered {
            let fresh = place_node(doc, &self.used, &mut laid, *id);
            let changed = match fresh {
                Some(u) => self.used.insert(*id, u.clone()).as_ref() != Some(&u),
                None => self.used.remove(id).is_some(),
            };
            if changed {
                relaid.push(*id);
            }
        }

        // Text first: bounds depend on the shaped size, so re-shape before
        // anything measures.
        //
        // 🚨 **Over `existing`, not over `affected`** (§15 D590, `[A4-L4-03]`).
        // The subtree expansion above is for *world transforms*, which really do
        // depend on a dirty ancestor; a text layout does not depend on one at all.
        // `text::layout` reads the node's **used** kind and nothing else —
        // content, style, both span lists, sizing and the rail are all the node's
        // own, and the used kind differs from them only by a wrap width layout
        // handed down (§15 D868) — so a node whose layout needs redoing is a node
        // whose own kind changed, which the `op_*` handlers dirty, or whose used
        // kind moved, which the `relaid` pass above caught. `op_insert_subtree`
        // and `op_delete_node` both enumerate every node they touch rather than
        // only the subtree root, which is what makes this safe for the two
        // structural doors.
        //
        // What it cost: select a group of twenty 4,000-character text blocks and
        // hold an arrow key. Every nudge commits, the whole subtree lands in
        // `affected`, and all twenty were re-shaped for a **translate** — about
        // 23 ms per nudge at a measured 1.17 ms per 4k-character shape, against a
        // Windows key-repeat of ~30/s. The same on opacity, visibility, paint,
        // lock and reorder, and on every undo and redo of one. Not one glyph can
        // move in any of them.
        //
        // ⚠️ **Nothing that existed could see this.** `update`'s correctness is
        // owned by a randomized differential against `rebuild`, and the layouts
        // produced are *identical* either way — the waste is invisible to any
        // assertion about the result. `text::shapes` is what makes it assertable.
        for id in &existing {
            self.reshape_text(doc, *id);
        }
        // And the text whose used kind moved under it, above. Not already shaped
        // above, or it would be shaped twice for one commit.
        for id in relaid.iter().filter(|id| !existing.contains(id)) {
            self.reshape_text(doc, *id);
        }

        // Transforms: parents before children, which `ordered` already is — it
        // was sorted above, before the used-geometry pass that needs the same
        // order.
        for id in &ordered {
            let node = doc.get(*id).expect("in document");
            let local = local_in(&self.used, node);
            let world = match node.parent() {
                Some(p) => self.world.get(&p).copied().unwrap_or(Affine::IDENTITY) * local,
                None => local,
            };
            self.world.insert(*id, world);
        }

        // Bounds: children before parents — and the booleans in the same pass and
        // the same direction, since a boolean's box is measured from the outline
        // and a nested one's outline is an operand of its parent's.
        ordered.reverse();
        for id in &ordered {
            self.reevaluate_boolean(doc, *id);
            self.recompute_bounds(doc, *id);
        }

        // Propagate bound changes up to the root from each dirty node's parent
        // (ancestors above the affected subtrees are not in `affected`).
        let mut seen: FxHashSet<NodeId> = FxHashSet::default();
        for id in &existing {
            let mut cursor = doc.get(*id).and_then(|n| n.parent());
            while let Some(c) = cursor {
                if !seen.insert(c) {
                    break; // this chain already recomputed
                }
                // **A boolean ancestor is re-evaluated, not just re-measured.**
                // Moving one operand changes the *shape* of every boolean above it,
                // which is the whole difference between this and a group — a group
                // ancestor only needs its box unioned again.
                self.reevaluate_boolean(doc, c);
                self.recompute_bounds(doc, c);
                cursor = doc.get(c).and_then(|n| n.parent());
            }
        }

        self.reindex(doc);
    }

    pub fn world_transform(&self, id: NodeId) -> Option<Affine> {
        self.world.get(&id).copied()
    }

    /// The local transform `id` is **placed** with — layout's where it moved the
    /// node, the document's everywhere else (§15 D868). `None` for a node the
    /// document does not hold.
    ///
    /// **Ask this to draw, measure or hit-test — and to edit** (§15 D874). The two
    /// are equal until container layout places something, and where they part a
    /// tool computes from where the node *is drawn*, which is what the user sees and
    /// drags, and writes the placement it wants as an ordinary `SetTransform`;
    /// `crate::build::keep_insets` turns that into new insets for a pinned layer, at
    /// the one place commits enter. Step 1 read *"read [`Node::transform`] to edit"*
    /// here, and step 2 reversed it: a pinned layer's stored transform is not where
    /// it is, so a tool starting from it would compose an edit against a place the
    /// layer has left.
    pub fn used_local(&self, doc: &Document, id: NodeId) -> Option<Affine> {
        doc.get(id).map(|n| local_in(&self.used, n))
    }

    /// The kind `id` is **sized** with — layout's where it resized the node, the
    /// document's everywhere else (§15 D868). `None` for a node the document does
    /// not hold.
    ///
    /// Always the same variant as [`Node::kind`] (see `Used::kind`), so a question
    /// about *what sort* of node this is can ask either; a question about its
    /// geometry asks this — to draw and to edit, under [`Self::used_local`]'s rule
    /// (§15 D874). ⚠️ **What an operation is *applied to* is still the document's
    /// kind**: a `GeometryPatch` that is not a whole size — a corner radius, a side
    /// count — patches the stored kind, so it carries the stored size with it.
    pub fn used_kind<'a>(&'a self, doc: &'a Document, id: NodeId) -> Option<&'a NodeKind> {
        doc.get(id).map(|n| kind_in(&self.used, n))
    }

    /// [`Self::used_local`] for a node already in hand, so infallible.
    ///
    /// **For writers that pass a `&Node` down a chain of helpers with no document
    /// in reach** — the SVG writer's are the case this was written for — where the
    /// alternative was a new parameter on every one of them.
    ///
    /// ⚠️ **`node` must be this document's own**, borrowed out of the `Document`
    /// this `Resolved` was built from. The map is keyed by id, so a *copy* — a
    /// captured subtree, a clipboard node, a ghost — would be answered with
    /// whatever layout did to the original, which is exactly wrong for a copy
    /// that has not been placed anywhere yet.
    pub fn used_local_of(&self, node: &Node) -> Affine {
        local_in(&self.used, node)
    }

    /// [`Self::used_kind`] for a node already in hand, under
    /// [`Self::used_local_of`]'s condition: `node` must be this document's own.
    pub fn used_kind_of<'a>(&'a self, node: &'a Node) -> &'a NodeKind {
        kind_in(&self.used, node)
    }

    /// The box a group with a layout was given, in its own space from its origin
    /// (§15 D869) — `None` for any other node, a frame included, whose size is
    /// its kind's. What the selection, the W/H and a resize read for such a group.
    pub fn used_frame(&self, id: NodeId) -> Option<kurbo::Size> {
        self.used.get(&id).and_then(|u| u.frame)
    }

    /// The stroke-expanded world AABB of `id`.
    ///
    /// **Not reduced by any clip, a mask included.** A layer inside a clipping
    /// frame keeps the box its ink would cover if the frame let it, and a masked
    /// layer keeps the box its ink would cover with the mask taken away — one
    /// rule, and the older half of it has been the answer since frames clipped.
    ///
    /// Two things follow. It is *conservative*: a clip only ever removes ink, so
    /// a box that ignores clips is never too small, which is what culling and the
    /// spatial index need of it. And it is the box of the thing you are *editing*
    /// — dragging a photograph about under a mask is the whole gesture, and a
    /// selection outline that shrank to the visible part would leave nothing to
    /// aim at.
    ///
    /// What it costs is that a marquee or a snap can find ink that is clipped
    /// away. That is a real gap, it is the same gap the frame clip has always
    /// had, and whatever closes it should close both.
    pub fn world_bounds(&self, id: NodeId) -> Option<Rect> {
        self.world_bounds.get(&id).copied()
    }

    /// The world AABB of everything `id`'s ink covers, its effects and its
    /// descendants' effects included. See [`Self::ink_bounds`]'s field for why
    /// this is a separate question from [`Self::world_bounds`].
    ///
    /// **Ask this for culling, for export, and for damage.** Ask `world_bounds`
    /// for anything the user aims at or reads a number from. When a node carries
    /// no effect anywhere beneath it the two are equal, so choosing wrongly is
    /// invisible until the first shadow — which is exactly why the choice should
    /// be made deliberately rather than by whichever name came to mind.
    pub fn ink_bounds(&self, id: NodeId) -> Option<Rect> {
        self.ink_bounds.get(&id).copied()
    }

    /// See the [`Self::inner_ink`] field: the subtree's ink **before** this node's
    /// own effects escape.
    pub fn inner_ink(&self, id: NodeId) -> Option<Rect> {
        self.inner_ink.get(&id).copied()
    }

    /// The cached shaped layout of a `Text` node — glyph runs and box size.
    /// `None` for every other kind.
    pub fn text_layout(&self, id: NodeId) -> Option<&TextLayout> {
        self.text.get(&id)
    }

    /// The combined outline of a `Boolean` node, in its own local space. `None`
    /// for every other kind, and for a boolean whose operands cancel out.
    pub fn boolean_path(&self, id: NodeId) -> Option<&BezPath> {
        self.boolean.get(&id)
    }

    /// Whether `id` is a boolean whose arithmetic was **abandoned** — it draws
    /// nothing, and that is a defect rather than an answer (§15 D239).
    ///
    /// The distinction [`Self::boolean_path`] cannot make: it answers `None` for a
    /// boolean that came out legitimately empty and for one that gave up, and the
    /// first is a shape the user asked for. Anything that wants to *tell* them —
    /// the layers row's warning colour, the status line — has to ask this instead,
    /// or it marks correct work as broken.
    pub fn boolean_failed(&self, id: NodeId) -> bool {
        self.failed.contains(&id)
    }

    /// What the subtree at `id` contributes as a boolean operand, in its
    /// **parent's** space — a shape's own outline, a group's contents unioned, a
    /// boolean's cached result, nothing at all for text.
    ///
    /// The committed half of [`boolean::Operands`], spelled once. `flatten` needs
    /// exactly this question answered of an arbitrary selection, and writing the
    /// five lookups out at the call site would have been a third copy of the view
    /// that [`Self::reevaluate_boolean`] and `RenderOverrides` already keep in step.
    pub fn operand_path(&self, doc: &Document, id: NodeId) -> Option<BezPath> {
        self.with_committed_view(doc, |view| boolean::operand_of(id, view))
    }

    /// Run `f` against the lookups that read the committed tree and this cache.
    ///
    /// A callback rather than a returned value because [`boolean::Operands`] holds
    /// `&dyn Fn`s: the closures are locals, so the view can be lent out but not
    /// handed back.
    fn with_committed_view<R>(
        &self,
        doc: &Document,
        f: impl FnOnce(&boolean::Operands<'_>) -> R,
    ) -> R {
        f(&boolean::Operands {
            local_of: &|c| {
                doc.get(c)
                    .map(|n| local_in(&self.used, n))
                    .unwrap_or_default()
            },
            kind_of: &|c| doc.get(c).map(|n| kind_in(&self.used, n).clone()),
            children_of: &|c| {
                doc.get(c)
                    .map(|n| n.children().to_vec())
                    .unwrap_or_default()
            },
            visible_of: &|c| doc.get(c).is_some_and(|n| n.visible()),
            nested: &|c| self.boolean.get(&c).cloned(),
        })
    }

    /// The outline to draw `id` with, in its local space: a boolean's from the
    /// cache, everything else's from its kind.
    ///
    /// **It has two callers: [`Self::mask_path`] and the canvas's
    /// `stroke_outlines`** — which is the third instalment of a note this comment
    /// carried until 2026-08-21, when it read "it has no callers … delete it, or
    /// give it the one caller that would justify it", and then said "one caller"
    /// until §15 D563. The second one is the caller §9.4 always claimed this had:
    /// *"`Resolved::local_path` is the canvas's"*, which was false as written for
    /// as long as the hover outline asked the bare function and drew a boolean as
    /// its operands.
    ///
    /// The rest of the note still stands and is worth keeping, because it says what
    /// this is *not*: the claim it originally carried — "the one every consumer
    /// should ask" — was false the whole time. The two consumers that look like
    /// they want it do not. The scene walk wants an `Option<&BezPath>` rather than a
    /// clone and reads `boolean_path` beside the kind (`scene::Painted::path`), and
    /// the SVG writer has its own `outline`, which also has to reach for a text
    /// node's glyph outlines and so could never have been this.
    ///
    /// What made a mask fit where those did not is that it needs a *clone* anyway
    /// (the path is pushed as a clip and outlives the borrow) and it handles text
    /// and groups in its own arms above, leaving exactly the two this answers.
    pub fn local_path(&self, doc: &Document, id: NodeId) -> Option<BezPath> {
        let node = doc.get(id)?;
        match kind_in(&self.used, node) {
            NodeKind::Boolean { .. } => self.boolean.get(&id).cloned(),
            kind => geometry::local_path(kind),
        }
    }

    /// The outline a **mask** clips with, in its **parent's** space — the space
    /// its siblings are drawn in, so the result can be handed straight to
    /// `ScenePainter::push_layer` under the parent's world transform, or written
    /// into a `<clipPath>` beside the parent's `<g>`.
    ///
    /// The kinds are decided the same way [`crate::boolean::operand_of`] decides
    /// them, with **one addition that is the whole point of the feature**: a
    /// `Text` node contributes its glyph outlines. Text contributes nothing to a
    /// boolean, and §15 D191 named "a picture masked by text" as one of the two
    /// cases the fill model cannot cover — so the arm a boolean leaves out is the
    /// one a mask is largely for. It costs nothing new: `text::outline` has
    /// existed since outside-aligned type strokes did.
    ///
    /// **`None` masks nothing.** A hidden mask, an empty text node, a boolean
    /// whose operands cancel, a group with nothing in it — every one of them
    /// answers `None`, and every caller reads that as *no clip*, so the layers
    /// above draw unclipped. The other reading, "clip everything away", would
    /// make hiding a mask look like deleting the artwork.
    ///
    /// Filled **non-zero**, which is not a choice: it is the rule the same
    /// outline is filled with when the layer paints normally, so what a mask
    /// keeps is exactly the ink it would have laid down.
    pub fn mask_path(&self, doc: &Document, id: NodeId) -> Option<BezPath> {
        let node = doc.get(id)?;
        if !node.visible() {
            return None;
        }
        let own = match node.kind() {
            NodeKind::Text { .. } => self
                .text
                .get(&id)
                .map(text::outline)
                .filter(|p| !p.elements().is_empty()),
            // A **union**, not a concatenation, and the reason is narrower than
            // `operand_of`'s — measured, because the cheap version passes a
            // careless test. This clip is filled **non-zero**, under which two
            // *same-wound* members appended do merge; what breaks is two members
            // wound **against** each other, whose overlap sums to zero and comes
            // out as a hole exactly where the shapes cross. A user's own paths
            // carry whatever winding they were drawn in, so that is the ordinary
            // case rather than a contrived one, and
            // `a_group_masks_with_the_union_of_its_contents` is built out of it
            // for that reason. This is the one arm that costs real arithmetic,
            // and it is bounded by what is inside the group.
            //
            // **A mask *inside* a mask group is skipped rather than honoured**,
            // and the two halves of that are worth separating. Skipped: it paints
            // nothing, so unioning its outline in would make the group keep ink
            // it does not draw. Not honoured: its own siblings are unioned in
            // whole, so a group that masks internally offers slightly more
            // outline than it shows. Both readings are conservative — a clip that
            // is too generous — and the exact answer needs a path *intersection*,
            // which is a second boolean per mask per frame for a case nobody has
            // asked for yet.
            NodeKind::Group | NodeKind::Root => {
                let inner: Vec<BezPath> = node
                    .children()
                    .iter()
                    .filter(|c| !doc.get(**c).is_some_and(|n| n.mask()))
                    .filter_map(|c| self.mask_path(doc, *c))
                    .collect();
                boolean::evaluate(crate::node::BoolOp::Union, &inner)
            }
            // **Every other kind through [`Self::local_path`]**, which is the one
            // that already knows a boolean's outline comes from the cache and
            // everything else's from its kind. Spelling those two arms again here
            // would be a second copy of that split, and this is the caller that
            // function's doc comment asked for.
            _ => self.local_path(doc, id),
        }?;
        Some(local_in(&self.used, node) * own)
    }

    /// Re-shape every `Text` node and refresh everything that depends on it.
    ///
    /// Shaping resolves against the fonts registered with `crate::text`, so a
    /// layout computed before a family was available used the fallback face.
    /// The app calls this after registering newly loaded fonts (§5.4a: "treat
    /// font arrival as dirtying all Text nodes"), otherwise bounds, selection
    /// rectangles and hit-testing stay measured against the wrong face.
    pub fn invalidate_text(&mut self, doc: &Document) {
        let text_nodes: FxHashSet<NodeId> = self
            .text
            .keys()
            .copied()
            .filter(|id| doc.contains(*id))
            .collect();
        if text_nodes.is_empty() {
            return;
        }
        self.update(doc, &DirtySet(text_nodes));
    }

    /// Recompute the cached layout for `id` if it is a `Text` node; clear any
    /// stale entry if it is not (a kind can change identity across undo/redo).
    fn reshape_text(&mut self, doc: &Document, id: NodeId) {
        // At the **used** kind (§15 D868): layout sizes a text box by handing it a
        // wrap width, and the glyphs are shaped at that width, not the typed one.
        let laid_out = doc
            .get(id)
            .map(|n| kind_in(&self.used, n))
            .and_then(crate::TextRef::of)
            .map(text::layout);
        match laid_out {
            Some(laid_out) => {
                self.text.insert(id, laid_out);
            }
            None => {
                self.text.remove(&id);
            }
        }
    }

    /// Ids of indexed nodes whose world bounds intersect `area` (unordered).
    /// Used by culling and as hit-test candidates.
    pub(crate) fn candidates_intersecting(&self, area: Rect) -> Vec<NodeId> {
        let env = AABB::from_corners([area.min_x(), area.min_y()], [area.max_x(), area.max_y()]);
        self.index
            .locate_in_envelope_intersecting(env)
            .map(|e| e.id)
            .collect()
    }

    /// Recompute the combined outline for `id` if it is a `Boolean`; clear any
    /// stale entry if it is not (a kind can change identity across undo/redo).
    ///
    /// **Must run children-first**, and does: both callers reach a node only after
    /// its subtree, so a boolean nested inside a boolean reads a cache entry that
    /// is already current rather than one frame stale.
    fn reevaluate_boolean(&mut self, doc: &Document, id: NodeId) {
        let Some(node) = doc.get(id) else {
            self.boolean.remove(&id);
            self.failed.remove(&id);
            return;
        };
        let NodeKind::Boolean { op } = *node.kind() else {
            self.boolean.remove(&id);
            self.failed.remove(&id);
            return;
        };
        // The committed document's own transforms and this cache's own entries —
        // the recursion itself lives in `boolean.rs`, shared with the preview
        // layer, which asks the same question of a gesture's pending transforms.
        //
        // **Bracketed by `boolean::failures`, which is the only way to tell the two
        // kinds of `None` apart** — an empty result and an abandoned one. Round the
        // whole `evaluate_node` rather than round the outermost `evaluate`, because
        // a group operand is unioned by an inner call that catches its own unwind:
        // the failure that matters to the user is *this boolean draws nothing*, at
        // whatever depth it happened, and that is exactly the span this measures.
        // Recomputed on every pass rather than only inserted, so a boolean that is
        // edited back into a shape flo_curves can handle stops being marked.
        let before = boolean::failures();
        let result = self.with_committed_view(doc, |view| boolean::evaluate_node(id, op, view));
        if boolean::failures() == before {
            self.failed.remove(&id);
        } else {
            self.failed.insert(id);
        }
        match result {
            Some(path) => {
                self.boolean.insert(id, path);
            }
            None => {
                self.boolean.remove(&id);
            }
        }
    }

    fn recompute_bounds(&mut self, doc: &Document, id: NodeId) {
        let node = doc.get(id).expect("in document");
        let world = self.world.get(&id).copied().unwrap_or(Affine::IDENTITY);
        let laid_box = self.used.get(&id).and_then(|u| u.frame);
        let bounds = match node.kind() {
            // **A group with a layout is a box** (§15 D869): its bounds are the box
            // its layout made, padding and all, not the union of what it holds —
            // the selection outline and the W/H are the container's.
            NodeKind::Group if laid_box.is_some() => laid_box.and_then(|s| {
                geometry::measurable(geometry::transform_rect(
                    world,
                    Rect::from_origin_size(kurbo::Point::ZERO, s),
                ))
            }),
            // **The one place a mask reaches a box.** A masked layer keeps its own
            // bounds — see the note on [`Self::world_bounds`] — but what it lends
            // its *container* is only what the mask lets through, so zooming to a
            // mask group frames the picture you can see rather than the whole
            // photograph behind it.
            //
            // Order-safe in both passes: the mask is a sibling, so by the time a
            // container is measured every child's box is already fresh (the
            // incremental pass measures children before parents, and the full one
            // walks the list in order, where a mask precedes everything it masks).
            NodeKind::Group | NodeKind::Root => {
                let mut mask_box: Option<Rect> = None;
                node.children()
                    .iter()
                    .filter_map(|c| {
                        let is_mask = doc.get(*c).is_some_and(|n| n.mask());
                        let b = self.world_bounds.get(c).copied();
                        if is_mask {
                            // [`mask_extent`], not the child's own box: a mask
                            // group narrows its own box with its own inner mask
                            // and clips with the un-narrowed union (§15 D460).
                            mask_box = mask_extent(doc, &self.world_bounds, *c);
                            // **And it contributes nothing** (§15 D494). A mask
                            // is a clip, not an area: it is never painted, so a
                            // mask drawn larger than the artwork it passes was
                            // lending its container a box the group does not
                            // show. Returning `b` here made the group the mask.
                            return None;
                        }
                        match mask_box {
                            Some(m) => b.and_then(|b| clipped_to(b, m)),
                            None => b,
                        }
                    })
                    .reduce(|acc, b| acc.union(b))
            }
            // Measured from the derived outline, not from the children: a boolean's
            // box is what its *result* covers, which is smaller than the union of
            // its operands for every operation except Union.
            NodeKind::Boolean { .. } => match self.boolean.get(&id) {
                Some(path) => geometry::world_bounds_of_path(
                    path,
                    node.paint(),
                    kind_in(&self.used, node),
                    world,
                ),
                // **A boolean that gave up still has a box, because it still draws
                // something** — the placeholder (§15 D298), over the operands' union.
                // Without this it has no bounds at all, which is three separate
                // failures rather than one: it is not in the spatial index, so a
                // click cannot find it; it has nothing for a marquee or *zoom to
                // selection* to measure; and the walk's own cull would have nothing
                // to test the drawing against.
                //
                // **Only when it failed.** A boolean whose operands correctly cancel
                // out draws nothing and *should* have no box — giving it one would
                // put an invisible click target over the artwork behind it.
                // ⚠️ **Through [`geometry::measurable`], because this is the one
                // arm of this function that reaches [`geometry::transform_rect`]
                // raw** (§15 D667, `[S4.1-L1-07]`). Every other route into
                // `world_bounds` already filters: `world_bounds_of_path` and
                // `world_bounds_of_parts` end in `measurable` (§15 D495), and a
                // container's union is of boxes that passed it. The placeholder
                // did not, and `query::local_box` composes child transforms
                // itself — so a `scale(1e308)` two levels down gives an ordered,
                // infinite rect that is stored, unioned into every ancestor up to
                // the root, and handed to the R-tree.
                None if self.failed.contains(&id) => {
                    crate::query::boolean_placeholder(doc, self, id)
                        .map(|b| geometry::transform_rect(world, b))
                        .and_then(geometry::measurable)
                }
                None => None,
            },
            _ => geometry::world_bounds_of_parts(
                kind_in(&self.used, node),
                node.paint(),
                world,
                self.text.get(&id),
            ),
        };
        match bounds {
            Some(b) => {
                self.world_bounds.insert(id, b);
            }
            None => {
                self.world_bounds.remove(&id);
            }
        }

        // **Ink is measured from the children's ink, not from the box just
        // stored**, or a shadow one level down would be forgotten by every
        // ancestor. A container's own escape then applies on top of that union,
        // which is what makes a blur on a group cover the group's whole contents
        // rather than each child separately.
        //
        // Order-safe for the same reason the box above is: both passes measure
        // children before parents, and the up-tree propagation in `update` walks
        // through here as well.
        let ink = match node.kind() {
            NodeKind::Group | NodeKind::Root => self.children_ink(doc, node),
            // **A frame that does not clip lets its children's ink out, so its own
            // ink has to say so** (§15 D876). It was its box alone, on the reading
            // that its children *"are either clipped to it or walked on their own
            // terms"* — true of the walk, which descends into a frame whatever its
            // ink, and false of every ancestor that culls a whole subtree on the
            // union of its children's ink. That was only the root until frames
            // could sit in groups; a group is culled on this union, and its card's
            // overflow vanished whenever the group's box was off screen. Too large
            // is the one direction these bounds may err, so a clipping frame keeps
            // its box: nothing of its children is painted outside it.
            NodeKind::Artboard { .. } if !node.clip() => {
                union_of(bounds, self.children_ink(doc, node))
            }
            // Everything else takes its own box, and a clipping frame's children
            // are clipped to it.
            _ => bounds,
        };
        match ink {
            Some(b) => {
                self.inner_ink.insert(id, b);
            }
            None => {
                self.inner_ink.remove(&id);
            }
        }
        // Through `measurable`, for the box's reason one screen up (§15 D685): a
        // `stack_escape` that overflows answers the unbounded rect, and a node
        // whose ink is not a number has no ink rather than infinite ink — which is
        // the absence §15 D495 chose over an infinite box.
        let ink = ink
            .map(|b| effect::escaped(b, effect::stack_escape(node.effects()), world))
            .and_then(geometry::measurable);
        match ink {
            Some(b) => {
                self.ink_bounds.insert(id, b);
            }
            None => {
                self.ink_bounds.remove(&id);
            }
        }
    }

    /// The union of `node`'s children's ink, each clipped by the mask governing
    /// it and no mask contributing — what a container's ink starts from, before
    /// its own effects escape over it.
    fn children_ink(&self, doc: &Document, node: &crate::Node) -> Option<Rect> {
        let mut mask_ink: Option<Rect> = None;
        node.children()
            .iter()
            .filter_map(|c| {
                let is_mask = doc.get(*c).is_some_and(|n| n.mask());
                let b = self.ink_bounds.get(c).copied();
                if is_mask {
                    // The ink twin of the box in `recompute_bounds`, and it needs
                    // [`mask_extent`] for the same reason (§15 D460) — more so,
                    // since this is the box `scene::paint_node` culls a whole
                    // subtree on.
                    mask_ink = mask_extent(doc, &self.ink_bounds, *c);
                    // Nothing, for the box's reason (§15 D494). Safe in the
                    // direction this union may not err: what is drawn is every
                    // other child's ink *intersected* with this, so dropping it can
                    // only shrink the union to ink that is actually painted.
                    return None;
                }
                match mask_ink {
                    // **Clipped by the mask's *ink*, where the box uses the mask's
                    // box.** A blurred mask passes ink beyond its own outline, so
                    // its geometry box is the one thing here that could be too
                    // small — and too small is the one error these bounds may not
                    // make.
                    Some(m) => b.and_then(|b| clipped_to(b, m)),
                    None => b,
                }
            })
            .reduce(|acc, b| acc.union(b))
    }

    fn reindex(&mut self, doc: &Document) {
        self.index = build_index(doc, &self.world_bounds);
    }
}

/// Two optional rects unioned, either one standing alone when the other is
/// absent.
fn union_of(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        (a, b) => a.or(b),
    }
}

/// The five maps [`resolve_subtree`] fills, which always travel together.
///
/// A struct rather than five `&mut` parameters: they are one thing — everything a
/// pass down the tree caches — and the alternative was silencing
/// `too_many_arguments`, which is the same admission with the reason left out.
///
/// **The used-geometry map is not one of them**, because the walk reads it and
/// does not fill it: [`Resolved::rebuild`] computes it whole first, and it travels
/// beside this as a shared borrow.
struct Caches<'a> {
    world: &'a mut FxHashMap<NodeId, Affine>,
    world_bounds: &'a mut FxHashMap<NodeId, Rect>,
    ink_bounds: &'a mut FxHashMap<NodeId, Rect>,
    inner_ink: &'a mut FxHashMap<NodeId, Rect>,
    text: &'a mut FxHashMap<NodeId, TextLayout>,
}

/// Recursively compute world transforms and bounds for the subtree at `id`,
/// returning this node's world bounds and ink bounds (if any) so parents can
/// union them.
///
/// Every transform and kind it reads is the **used** one (§15 D868), out of
/// `used`, which is [`Resolved::update`]'s reading too — the two are asserted
/// equal, so one reading the document and the other the map would be a
/// divergence the first laid-out node exposed.
fn resolve_subtree(
    doc: &Document,
    id: NodeId,
    parent_world: Affine,
    used: &FxHashMap<NodeId, Used>,
    c: &mut Caches<'_>,
) -> (Option<Rect>, Option<Rect>) {
    let node = doc.get(id).expect("valid tree");
    let kind = kind_in(used, node);
    let w = parent_world * local_in(used, node);
    c.world.insert(id, w);

    // Shape before measuring: the text box size comes out of the layout.
    if let Some(parts) = crate::TextRef::of(kind) {
        c.text.insert(id, text::layout(parts));
    }

    let mut child_union: Option<Rect> = None;
    let mut child_ink_union: Option<Rect> = None;
    // The mask governing the children walked so far, `None` before the first one
    // and replaced at each — which is the backwards scan of
    // [`Document::governing_mask`] read forwards, and has to agree with it.
    let mut mask_box: Option<Rect> = None;
    // Its ink twin. Kept apart rather than reused, for the reason
    // `recompute_bounds` gives: a blurred mask lets ink past its own outline, so
    // the box that clips ink has to be the wider of the two.
    let mut mask_ink: Option<Rect> = None;
    for child in node.children() {
        let is_mask = doc.get(*child).is_some_and(|n| n.mask());
        let (cb, ci) = resolve_subtree(doc, *child, w, used, c);
        if is_mask {
            // Not `cb`/`ci` — a mask group narrows its own boxes with its own
            // inner mask and clips with the un-narrowed union (§15 D460). The
            // child's whole subtree is in the caches by now, which is what makes
            // reading it here sound.
            mask_box = mask_extent(doc, c.world_bounds, *child);
            mask_ink = mask_extent(doc, c.ink_bounds, *child);
        }
        // A mask contributes nothing at all — `recompute_bounds`' arm, and §15
        // D494's reason: it is a clip rather than an area, and a mask larger
        // than what it passes was making its container the size of the mask.
        let contribution = match mask_box {
            _ if is_mask => None,
            Some(m) => cb.and_then(|b| clipped_to(b, m)),
            None => cb,
        };
        if let Some(cb) = contribution {
            child_union = Some(match child_union {
                Some(u) => u.union(cb),
                None => cb,
            });
        }
        let ink_contribution = match mask_ink {
            _ if is_mask => None,
            Some(m) => ci.and_then(|b| clipped_to(b, m)),
            None => ci,
        };
        if let Some(ci) = ink_contribution {
            child_ink_union = Some(match child_ink_union {
                Some(u) => u.union(ci),
                None => ci,
            });
        }
    }

    // `recompute_bounds`' twin: a group with a layout is the box its layout made.
    let laid_box = used.get(&id).and_then(|u| u.frame);
    let bounds = match node.kind() {
        NodeKind::Group if laid_box.is_some() => laid_box.and_then(|s| {
            geometry::measurable(geometry::transform_rect(
                w,
                Rect::from_origin_size(kurbo::Point::ZERO, s),
            ))
        }),
        NodeKind::Group | NodeKind::Root => child_union,
        _ => geometry::world_bounds_of_parts(kind, node.paint(), w, c.text.get(&id)),
    };
    if let Some(b) = bounds {
        c.world_bounds.insert(id, b);
    }

    // See the matching passage in `recompute_bounds`: the union is of the
    // children's *ink*, and this node's own escape applies over the result.
    let inner = match node.kind() {
        NodeKind::Group | NodeKind::Root => child_ink_union,
        // `recompute_bounds`' arm, and its reason (§15 D876): a frame that does not
        // clip lends its container its children's ink.
        NodeKind::Artboard { .. } if !node.clip() => union_of(bounds, child_ink_union),
        _ => bounds,
    };
    if let Some(b) = inner {
        c.inner_ink.insert(id, b);
    }
    // `recompute_bounds`' twin, and it has to be (§15 D685) — `update` and
    // `rebuild` are asserted equal by a randomized differential test, so a guard on
    // one is a divergence.
    let ink = inner
        .map(|b| effect::escaped(b, effect::stack_escape(node.effects()), w))
        .and_then(geometry::measurable);
    if let Some(b) = ink {
        c.ink_bounds.insert(id, b);
    }
    (bounds, ink)
}

/// The R-tree over every indexed node that has a box the tree can hold.
///
/// ⚠️ **[`geometry::measurable`] again, and here it is a guard on the *container*
/// rather than on the arithmetic** (§15 D667, `[S4.1-L1-07]`). `rstar`'s
/// `AABB::from_corners` takes its corners component-wise, so an **inverted** rect
/// — which is what [`geometry::transform_rect`] answers for a `NaN` transform,
/// since `f64::min` returns the operand that is not `NaN` — normalises to the
/// *maximal* box. The node is then a candidate for every spatial query for the
/// life of the session: measured at `nodes_in_view` over a 100×100 view at
/// −5000,−5000 containing nothing at all, which returned exactly that node and
/// correctly excluded the artboard beside it.
///
/// **This is deliberately a second statement of a rule `recompute_bounds` already
/// keeps**, and the only place in the file where that is the point: the writer
/// guards what it stores, and this guards what the index accepts however the map
/// came to hold it. `world_bounds` is derived from geometry as well as from
/// transforms, and the doors onto a non-finite one are not enumerable — the one
/// that was open when this was written was three call frames away, in
/// `query::local_box`.
fn build_index(doc: &Document, world_bounds: &FxHashMap<NodeId, Rect>) -> RTree<BoundsEntry> {
    let entries: Vec<BoundsEntry> = world_bounds
        .iter()
        .filter(|(id, _)| doc.get(**id).is_some_and(|n| is_indexed(n.kind())))
        .filter_map(|(id, r)| geometry::measurable(*r).map(|r| entry(*id, r)))
        .collect();
    RTree::bulk_load(entries)
}

/// Every node in `id`'s subtree, **with its depth**, into `out` (§15 D594).
///
/// The depth comes free here: the walk descends, so a child's is its parent's
/// plus one. Working it out afterwards means [`depth`] walking back to the root
/// once per node, which is the same tree traversed twice — and once per *sort
/// comparison* if the caller is careless about which sort it reaches for, which
/// is what `[S4.1-L4-03]` measured at 47× the full `rebuild`.
///
/// **A node already in `out` short-circuits the recursion**, which is what keeps
/// two dirty ids on one chain from walking their shared descendants twice.
///
/// ⚠️ **Its depth is *rewritten*, not left alone, and the rewrite is a no-op for
/// a reason worth stating.** `HashMap::insert` replaces the value and returns the
/// old one, so the second walk to arrive does overwrite — with the identical
/// number, because `at` is the node's true depth on every path that reaches it:
/// each walk starts at `depth(doc, root)` and adds one per level. Said out loud
/// because the obvious reading of the line above is that the map is left
/// untouched, and it is not.
fn collect_subtree(doc: &Document, id: NodeId, at: usize, out: &mut FxHashMap<NodeId, usize>) {
    if out.insert(id, at).is_some() {
        return;
    }
    if let Some(node) = doc.get(id) {
        for child in node.children() {
            collect_subtree(doc, *child, at + 1, out);
        }
    }
}

fn depth(doc: &Document, id: NodeId) -> usize {
    let mut d = 0;
    let mut cursor = doc.get(id).and_then(|n| n.parent());
    while let Some(p) = cursor {
        d += 1;
        cursor = doc.get(p).and_then(|n| n.parent());
    }
    d
}

/// A stand-in layout for this module's tests: whatever the installed closure
/// answers, `used_geometry` answers.
///
/// **Why it exists**: the identity pass answers `None` for every node, so nothing
/// in production can put an entry in the used-geometry map — and a refactor whose
/// new path is never taken is proved by nothing but the goldens staying the same,
/// which is exactly what a consumer still reading the document would also do.
/// This makes the map non-empty on purpose, so the tests below can ask whether
/// each consumer inside `Resolved` and `query` actually follows it.
///
/// Thread-local, like the test harness's threads, so two tests installing probes
/// cannot see each other's.
#[cfg(test)]
mod probe {
    use super::Used;
    use crate::document::Document;
    use crate::id::NodeId;
    use std::cell::RefCell;

    type Layout = Box<dyn Fn(&Document, NodeId) -> Option<Used>>;

    thread_local! {
        static PROBE: RefCell<Option<Layout>> = const { RefCell::new(None) };
    }

    /// Install `f` as this thread's layout until the returned guard drops.
    pub(super) fn set(f: impl Fn(&Document, NodeId) -> Option<Used> + 'static) -> Guard {
        PROBE.with(|p| *p.borrow_mut() = Some(Box::new(f)));
        Guard
    }

    pub(super) fn used(doc: &Document, id: NodeId) -> Option<Used> {
        PROBE.with(|p| p.borrow().as_ref().and_then(|f| f(doc, id)))
    }

    /// Uninstalls the probe on drop, so a failing assertion cannot leave it
    /// answering for the next test the harness runs on this thread.
    pub(super) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            PROBE.with(|p| *p.borrow_mut() = None);
        }
    }
}

/// The two guards §15 D667 added, tested from inside the module because both
/// subjects are private (`build_index` and `recompute_bounds`) and the state the
/// second one needs cannot be reached through the public API.
#[cfg(test)]
mod measurable_bounds_tests {
    use super::*;
    use crate::id::IdSource;
    use crate::op::{Operation, Transaction};
    use kurbo::{RoundedRectRadii, Size};

    fn rect_kind(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
        }
    }

    fn create(
        doc: &mut Document,
        id: NodeId,
        parent: NodeId,
        index: usize,
        kind: NodeKind,
        t: Affine,
    ) {
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(t),
            name: None,
        }]))
        .unwrap();
    }

    /// `build_index` refuses a box the R-tree would normalise to the whole plane
    /// — `[S4.1-L1-07]`'s own measurement, run against the container rather than
    /// against the arithmetic.
    ///
    /// The map is written by hand rather than produced by a document, and that is
    /// the point of the test: it stands in for whatever future door puts a
    /// non-finite box in `world_bounds`, which is the case `build_index`'s guard
    /// exists for and the one no fixture can enumerate. Both spellings are here,
    /// because either test alone passes half of them — the inverted rect is what
    /// `transform_rect` answers for a `NaN` transform, the ordered infinite one is
    /// what it answers for a finite affine whose product overflows, and `rstar`
    /// turns *both* into the maximal envelope.
    ///
    /// ⚠️ **The good sibling is the control and it is load-bearing**: without it a
    /// `build_index` that dropped every entry would pass. It is the artboard's
    /// role in the finding's own probe.
    ///
    /// **Flip:** drop `.filter_map(… measurable …)` back to the `.map(…)` it
    /// replaced and this fails on the first assertion — *"inverted: a view
    /// containing nothing at all got 1 candidate(s)"*. The site was predicted
    /// correctly and the **count** was not: the prediction said 2, and it is 1,
    /// because the good sibling's own box is nowhere near that view either. That
    /// is the control doing its job in the direction nobody plans for — it is not
    /// there to be found, it is there to prove the index is not simply empty.
    /// The loop's second case (the ordered infinite rect) fails the same way and
    /// is a separate case of the guard rather than a second reading of one.
    #[test]
    fn the_index_refuses_a_box_it_cannot_hold() {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let good = ids.mint();
        let bad = ids.mint();
        create(
            &mut doc,
            good,
            root,
            0,
            rect_kind(10.0, 10.0),
            Affine::IDENTITY,
        );
        create(
            &mut doc,
            bad,
            root,
            1,
            rect_kind(10.0, 10.0),
            Affine::IDENTITY,
        );

        // Nowhere near either node, and nowhere near the origin.
        let far = Rect::new(-5000.0, -5000.0, -4900.0, -4900.0);
        let inverted = Rect::new(
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        let overflowed = Rect::new(0.0, 0.0, f64::INFINITY, f64::INFINITY);

        for (name, r) in [("inverted", inverted), ("ordered infinite", overflowed)] {
            let mut bounds = FxHashMap::default();
            bounds.insert(good, Rect::new(0.0, 0.0, 10.0, 10.0));
            bounds.insert(bad, r);
            let index = build_index(&doc, &bounds);
            let hits: Vec<NodeId> = index
                .locate_in_envelope_intersecting(AABB::from_corners(
                    [far.x0, far.y0],
                    [far.x1, far.y1],
                ))
                .map(|e| e.id)
                .collect();
            assert!(
                hits.is_empty(),
                "{name}: a view containing nothing at all got {} candidate(s)",
                hits.len()
            );
            // The control: the well-formed sibling is still indexed.
            let near = AABB::from_corners([0.0f64, 0.0], [10.0, 10.0]);
            let found: Vec<NodeId> = index
                .locate_in_envelope_intersecting(near)
                .map(|e| e.id)
                .collect();
            assert_eq!(found, vec![good], "{name}: the good sibling left the index");
        }
    }

    /// The placeholder arm of `recompute_bounds` stores nothing rather than an
    /// infinite box — the live door `[S4.1-L1-07]` was open through after §15
    /// D495 closed the other four.
    ///
    /// 🚨 **The failure is forced through the front door, and the first version of
    /// this test was not.** It wrote `res.failed` and `res.boolean` by hand under a
    /// paragraph claiming the arm was otherwise unreachable, on S4.1's pass note
    /// that nothing can make flo_curves unwind since 0.8.1. That note is about the
    /// *defect*; `boolean::poison_next` (§15 D239) arms a **synthetic** unwind
    /// inside `guarded`, which is the one seam both entry points share, so
    /// `evaluate_node` reaches it and `reevaluate_boolean`'s `failures()` bracket
    /// marks the node exactly as a real failure would. The hand-written version
    /// asserted the arm's arithmetic; this asserts that the arm is reached.
    ///
    /// `boolean_failed` is checked before the bounds, because a poisoned call that
    /// landed on some *other* `guarded` call would leave this test green and about
    /// nothing.
    ///
    /// The overflow is authored the way a document reaches it — `scale(1e308)` on
    /// an operand, every coefficient finite, so `op_set_transform`'s own guard
    /// accepts it and `query::local_box` composes it into an infinite local box.
    ///
    /// **Flip:** remove `.and_then(geometry::measurable)` and the bounds assertion
    /// fails with `Some(Rect { x0: 0.0, y0: 0.0, x1: inf, y1: inf })`. Site and
    /// value both predicted correctly, and re-run against this fixture after it was
    /// rewritten to poison rather than to write `failed` by hand. ⚠️ **The
    /// container assertion bites independently**, checked rather than reasoned by
    /// replacing the one above it with a print: the group came back
    /// `0,0 .. inf,inf` against the sibling's `0,0 .. 10,10`. That is the half
    /// worth keeping — one leaf nothing can measure takes its container's extent,
    /// and the union carries it to the root, which is what `plan::raster_size`
    /// reads.
    ///
    /// The stderr line about a *"synthetic boolean failure"* is `guarded`'s default
    /// panic hook and is expected; the guard deliberately suppresses only its own
    /// explanatory message for a poisoned call.
    ///
    /// 🚨 **Found by `arch-scribe` reading D667's brief against the code, and it
    /// was right**: this test's first version wrote `res.failed` by hand and said
    /// the arm could not be reached, which the paragraph above now records.
    #[test]
    fn a_failed_boolean_whose_operands_overflow_gets_no_box_at_all() {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let boolean = ids.mint();
        let huge = ids.mint();
        let small = ids.mint();
        let sibling = ids.mint();
        create(&mut doc, group, root, 0, NodeKind::Group, Affine::IDENTITY);
        create(
            &mut doc,
            boolean,
            group,
            0,
            NodeKind::Boolean {
                op: crate::node::BoolOp::Union,
            },
            Affine::IDENTITY,
        );
        create(
            &mut doc,
            huge,
            boolean,
            0,
            rect_kind(100.0, 100.0),
            Affine::scale(1e308),
        );
        create(
            &mut doc,
            small,
            boolean,
            1,
            rect_kind(10.0, 10.0),
            Affine::IDENTITY,
        );
        // The container's own measurable extent, so that the second assertion is
        // about the boolean's contribution rather than about the group having no
        // children left with a box.
        create(
            &mut doc,
            sibling,
            group,
            1,
            rect_kind(10.0, 10.0),
            Affine::IDENTITY,
        );

        // One synthetic unwind, taken by the first `guarded` call the rebuild
        // makes — which is this boolean's, since nothing in the fixture masks.
        boolean::poison_next(1);
        let res = Resolved::rebuild(&doc);
        assert!(
            res.boolean_failed(boolean),
            "the poisoned call landed somewhere else, so the rest of this test is \
             about nothing"
        );

        assert_eq!(
            res.world_bounds.get(&boolean).copied(),
            None,
            "a box nothing can measure was stored anyway"
        );
        assert_eq!(
            res.world_bounds.get(&group).copied(),
            Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
            "the container took its extent from the operand it cannot measure"
        );
    }

    /// **A blur under a very large scale does not give the document infinite ink**
    /// (§15 D676).
    ///
    /// 🚨 **A second door onto the same failure, and it is not the one
    /// `[S4.1-L1-07]` names** — found by `arch-scribe` reading D667's brief against
    /// `effect::escaped` and confirmed here. That function maps the escape radius
    /// as an ellipse, `((a·rx)² + (c·ry)²).sqrt()`, and a **square** overflows at
    /// about `1.3e154` where the product itself is finite to `1.8e308`. So every
    /// input is legal and every guard passes: `build::effect_is_finite` accepts the
    /// radius, `affine_is_finite` accepts `scale(1e155)`, `geometry::measurable`
    /// accepts the world box at `1e157` — and `h` comes back infinite, which makes
    /// the ink box `(−inf, −inf, inf, inf)`.
    ///
    /// That box is not `world_bounds`, so §15 D495 and D667 never see it. It is
    /// what `scene::paint_node` culls whole subtrees on, what
    /// `ondin_export::plan::raster_size` sizes a raster from, and what the SVG
    /// writer measures.
    ///
    /// ⚠️ **`f64::hypot` is the repair rather than a guard on the result**, because
    /// there is nothing wrong with the answer — only with the way it was reached.
    /// `hypot` is the same value computed without an intermediate that can
    /// overflow, so the box stays finite and stays *right*. The `measurable` filter
    /// beside it is the D667 shape kept for the boxes that really have no answer.
    ///
    /// **Flip:** put back `((a * rad.x).powi(2) + (c * rad.y).powi(2)).sqrt()` and
    /// the first assertion fails with an infinite box; the control below it stays
    /// green, which is what says the change is about overflow and not about the
    /// arithmetic.
    #[test]
    fn a_blur_under_a_huge_scale_keeps_the_documents_ink_finite() {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let node = ids.mint();
        create(
            &mut doc,
            node,
            root,
            0,
            rect_kind(100.0, 100.0),
            Affine::scale(1e155),
        );
        doc.apply(&Transaction(vec![Operation::SetEffects {
            id: node,
            effects: vec![crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius: 10.0 },
            )],
        }]))
        .expect("a blur is a legal effect");

        let res = Resolved::rebuild(&doc);
        let ink = res.ink_bounds(node).expect("the node has ink");
        assert!(
            ink.x0.is_finite() && ink.y0.is_finite() && ink.x1.is_finite() && ink.y1.is_finite(),
            "the escape overflowed and took the whole plane with it: {ink:?}"
        );
        assert_eq!(
            res.ink_bounds(root),
            Some(ink),
            "and the root inherits exactly that box"
        );

        // The control: the same blur at an ordinary scale reaches the distance it
        // is supposed to, so the repair is not a clamp.
        let mut ids = IdSource::new(9);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let plain = ids.mint();
        create(
            &mut doc,
            plain,
            root,
            0,
            rect_kind(100.0, 100.0),
            Affine::IDENTITY,
        );
        doc.apply(&Transaction(vec![Operation::SetEffects {
            id: plain,
            effects: vec![crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius: 10.0 },
            )],
        }]))
        .expect("a blur is a legal effect");
        let res = Resolved::rebuild(&doc);
        let reach = crate::effect::stack_escape(doc.get(plain).expect("the node").effects());
        let ink = res.ink_bounds(plain).expect("the node has ink");
        assert_eq!(
            (ink.x0, ink.x1),
            (-reach.x0, 100.0 + reach.x1),
            "at scale 1 the ink is the box grown by the escape, both sides"
        );
    }

    /// **A blur radius whose *square* overflows does not report that it reaches
    /// nowhere** (§15 D685).
    ///
    /// 🚨 **The sibling `arch-scribe` found while sweeping for D676's shape, and it
    /// fails in the opposite direction, which is worse.** `stack_escape` summed
    /// `deviation(r).powi(2)`, so a radius past about `2.7e154` — finite, and
    /// accepted by `build::effect_is_finite` at both its doors — made `sigma2`
    /// infinite and handed `escaped` an `Insets::uniform(inf)`. That function takes
    /// the insets apart as `x1 - x0`, so the offset came out **NaN**, and
    /// `f64::min` returns the operand that is not NaN: the box came back
    /// **unchanged**. An unbounded blur reporting the extent of a bare rectangle.
    ///
    /// ⚠️ **D676 answered `inf` and this answered the *original box*, and only the
    /// second is dangerous.** An infinite ink box makes everything draw and every
    /// raster enormous — loud. A box that has silently not grown makes
    /// `scene::paint_node` cull artwork that is on screen, which is the one error
    /// these bounds may not make (`mask_extent`'s doc says so at length).
    ///
    /// **Two flips, and the pair is the argument for both repairs.** Restoring
    /// `deviation(radius).powi(2)` in `stack_escape` fails at
    /// `expect("the node has ink")` — ⚠️ **not** at the assertion, which is the
    /// prediction corrected: `escaped`'s finiteness guard catches the infinite
    /// escape and `measurable` drops the box, so the *floor* holds and the node
    /// simply has no ink. Removing that guard **as well** is what reproduces the
    /// original defect, and it prints it exactly: *"the blur reported reaching
    /// nowhere: ink `0,0 .. 100,100` against box `0,0 .. 100,100`"*. So the two are
    /// a fix and a floor rather than one rule written twice, and only running both
    /// flips says which is which.
    #[test]
    fn a_blur_whose_square_overflows_still_reports_the_ink_it_covers() {
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let node = ids.mint();
        create(
            &mut doc,
            node,
            root,
            0,
            rect_kind(100.0, 100.0),
            Affine::IDENTITY,
        );
        // `deviation` halves it, so the square of this is about `1.8e308` — the
        // last decade before the old spelling overflowed.
        let radius = 4e154;
        doc.apply(&Transaction(vec![Operation::SetEffects {
            id: node,
            effects: vec![crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius },
            )],
        }]))
        .expect("a blur is a legal effect");

        let res = Resolved::rebuild(&doc);
        let ink = res.ink_bounds(node).expect("the node has ink");
        let box_ = res.world_bounds(node).expect("and a box");
        assert!(
            ink.x0 < box_.x0 && ink.x1 > box_.x1,
            "the blur reported reaching nowhere: ink {ink:?} against box {box_:?}"
        );
        assert!(
            ink.x0.is_finite() && ink.x1.is_finite(),
            "and it is a number: {ink:?}"
        );

        // The control: a radius whose escape genuinely overflows `f64` has no
        // measurable ink at all, rather than the box it started from.
        doc.apply(&Transaction(vec![Operation::SetEffects {
            id: node,
            effects: vec![crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius: f64::MAX },
            )],
        }]))
        .expect("still a legal effect");
        let res = Resolved::rebuild(&doc);
        assert_eq!(
            res.ink_bounds(node),
            None,
            "an escape past f64 is an absence, not the original box"
        );
    }
}

/// Used geometry driven through the plumbing on purpose (§15 D868, D867's build
/// step 1).
///
/// The identity pass never puts an entry in the used-geometry map, so every
/// consumer that still read the document would pass every other test in the
/// workspace. These install a `probe` layout that places and sizes three nodes
/// somewhere other than the document says, and ask each consumer inside
/// `Resolved` and `query` whether it followed.
#[cfg(test)]
mod used_geometry_tests {
    use super::*;
    use crate::TextSizing;
    use crate::id::IdSource;
    use crate::op::{GeometryPatch, Operation, Transaction};
    use kurbo::{Point, RoundedRectRadii, Shape, Size};

    fn rect_kind(w: f64, h: f64) -> NodeKind {
        NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
        }
    }

    fn text_kind(sizing: TextSizing) -> NodeKind {
        NodeKind::Text {
            content: "several short words that can wrap".into(),
            style: Box::new(crate::TextStyle::default()),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    fn create(doc: &mut Document, id: NodeId, parent: NodeId, kind: NodeKind, t: Affine) {
        let index = doc.get(parent).unwrap().children().len();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(t),
            name: None,
        }]))
        .unwrap();
    }

    struct Fixture {
        doc: Document,
        frame: NodeId,
        rect: NodeId,
        group: NodeId,
        inner: NodeId,
        text: NodeId,
    }

    /// A 300×200 frame holding a 100×50 rect at (10, 10), a group at (0, 120)
    /// with a 20×20 rect in it, and an auto-width text node at (10, 80).
    fn fixture() -> Fixture {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [frame, rect, group, inner, text] = [(); 5].map(|_| ids.mint());
        create(
            &mut doc,
            frame,
            root,
            NodeKind::Artboard {
                size: Size::new(300.0, 200.0),
            },
            Affine::IDENTITY,
        );
        let at = |x: f64, y: f64| Affine::translate((x, y));
        create(
            &mut doc,
            rect,
            frame,
            rect_kind(100.0, 50.0),
            at(10.0, 10.0),
        );
        create(&mut doc, group, frame, NodeKind::Group, at(0.0, 120.0));
        create(
            &mut doc,
            inner,
            group,
            rect_kind(20.0, 20.0),
            Affine::IDENTITY,
        );
        create(
            &mut doc,
            text,
            frame,
            text_kind(TextSizing::Auto),
            at(10.0, 80.0),
        );
        Fixture {
            doc,
            frame,
            rect,
            group,
            inner,
            text,
        }
    }

    /// The stand-in layout: while the frame is at least 100 wide, the rect is
    /// placed at (5, 5) and stretched to the frame's width less 10, the grouped
    /// rect is slid 30 along inside its group, and the text wraps at a tenth of
    /// the frame's width (a fifth once the frame is faded). Below 100 wide nothing
    /// is laid out — so shrinking the frame is how a test makes entries *leave*
    /// the map.
    fn install(f: &Fixture) -> probe::Guard {
        let (frame, rect, inner, text) = (f.frame, f.rect, f.inner, f.text);
        probe::set(move |doc, id| {
            let NodeKind::Artboard { size } = doc.get(frame)?.kind() else {
                return None;
            };
            let width = size.width;
            if width < 100.0 {
                return None;
            }
            if id == rect {
                Some(Used {
                    local: Some(Affine::translate((5.0, 5.0))),
                    kind: Some(rect_kind(width - 10.0, 50.0)),
                    frame: None,
                })
            } else if id == inner {
                Some(Used {
                    local: Some(Affine::translate((30.0, 0.0))),
                    kind: None,
                    frame: None,
                })
            } else if id == text {
                // A faded frame halves the wrap: an input that dirties the frame
                // and nothing below it, which a resize does not (it dirties the
                // whole subtree).
                let share = if doc.get(frame)?.opacity() < 1.0 {
                    5.0
                } else {
                    10.0
                };
                Some(Used {
                    local: None,
                    kind: Some(text_kind(TextSizing::AutoHeight(width / share))),
                    frame: None,
                })
            } else {
                None
            }
        })
    }

    /// Every measure `Resolved` and `query` take of a node follows its used
    /// geometry, not the document's.
    ///
    /// Each assertion is placed where the two answers differ by a long way — the
    /// specified rect covers x 10..110 and the used one 5..295 — so no assertion
    /// here can be passed by a consumer reading the document.
    ///
    /// **Flips**, each run and each failing where predicted: `hit_test` handing
    /// `contains_local` the document's kind fails *"a click at x 250 lands on the
    /// stretched rect"*; `local_box` composing `child.transform()` fails the
    /// group's box, `(0,0)-(20,20)` against `(30,0)-(50,20)`; `resolve_subtree`
    /// shaping at `node.kind()` fails the wrapped-text assertion. ⚠️ **Shaping at
    /// `node.kind()` in `reshape_text` instead does not bite here** — this test
    /// only rebuilds, and `rebuild` shapes through `resolve_subtree` — and was
    /// predicted to; it is the next test's, failing there at the resize.
    #[test]
    fn every_measure_in_resolved_and_query_follows_the_used_geometry() {
        let f = fixture();
        let _probe = install(&f);
        let res = Resolved::rebuild(&f.doc);
        let doc = &f.doc;

        assert_eq!(
            res.used_local(doc, f.rect),
            Some(Affine::translate((5.0, 5.0)))
        );
        assert_eq!(res.used_kind(doc, f.rect), Some(&rect_kind(290.0, 50.0)));
        assert_eq!(
            res.used_local(doc, f.frame),
            Some(Affine::IDENTITY),
            "a node with no entry answers what the document says"
        );

        assert_eq!(
            res.world_bounds(f.rect),
            Some(Rect::new(5.0, 5.0, 295.0, 55.0))
        );
        assert_eq!(res.ink_bounds(f.rect), res.world_bounds(f.rect));
        assert_eq!(
            res.world_bounds(f.inner),
            Some(Rect::new(30.0, 120.0, 50.0, 140.0)),
            "a used local composes into the world transform of the node below it"
        );

        assert!(
            crate::hit_test(&res, doc, Point::new(250.0, 30.0), 0.0).contains(&f.rect),
            "a click at x 250 lands on the stretched rect"
        );
        assert!(
            crate::hit_test(&res, doc, Point::new(7.0, 7.0), 0.0).contains(&f.rect),
            "and at (7, 7), which is outside the rect the document holds"
        );
        assert_eq!(
            crate::outline_at(&res, doc, Point::new(295.0, 30.0), 1.0),
            Some(f.rect),
            "the outline a rail can be picked from is the used one"
        );

        assert_eq!(
            crate::local_box(doc, &res, f.rect),
            Some(Rect::new(0.0, 0.0, 290.0, 50.0))
        );
        assert_eq!(
            crate::local_box(doc, &res, f.group),
            Some(Rect::new(30.0, 0.0, 50.0, 20.0)),
            "a group's box unions its children through their used locals"
        );
        assert_eq!(
            res.local_path(doc, f.rect).map(|p| p.bounding_box()),
            Some(Rect::new(0.0, 0.0, 290.0, 50.0))
        );
        assert_eq!(
            res.mask_path(doc, f.rect).map(|p| p.bounding_box()),
            Some(Rect::new(5.0, 5.0, 295.0, 55.0)),
            "a mask clips with its used outline, placed by its used local"
        );

        let specified = text::layout(crate::TextRef::of(doc.get(f.text).unwrap().kind()).unwrap());
        let used = res.text_layout(f.text).unwrap();
        assert_eq!(
            specified.baselines.len(),
            1,
            "the fixture's own text is one line at auto width"
        );
        assert!(
            used.baselines.len() > 1,
            "text is shaped at its used kind, wrapping at 30: {} line(s)",
            used.baselines.len()
        );
    }

    /// Every map `Resolved` keeps, used geometry included, compared node by node.
    fn assert_equal_to_rebuild(doc: &Document, live: &Resolved, when: &str) {
        let fresh = Resolved::rebuild(doc);
        for id in crate::subtree_nodes(doc, &[doc.root()]) {
            assert_eq!(
                live.used_local(doc, id),
                fresh.used_local(doc, id),
                "{when}: used local of {id:?}"
            );
            assert_eq!(
                live.used_kind(doc, id),
                fresh.used_kind(doc, id),
                "{when}: used kind of {id:?}"
            );
            assert_eq!(
                live.world_transform(id),
                fresh.world_transform(id),
                "{when}: world of {id:?}"
            );
            assert_eq!(
                live.world_bounds(id),
                fresh.world_bounds(id),
                "{when}: bounds of {id:?}"
            );
            assert_eq!(
                live.ink_bounds(id),
                fresh.ink_bounds(id),
                "{when}: ink of {id:?}"
            );
            assert_eq!(
                live.text_layout(id),
                fresh.text_layout(id),
                "{when}: text of {id:?}"
            );
        }
    }

    fn commit(doc: &mut Document, res: &mut Resolved, op: Operation) {
        let out = doc.apply(&Transaction(vec![op])).unwrap();
        res.update(doc, &out.dirty);
    }

    /// `update` keeps the used geometry equal to `rebuild` when it moves, when it
    /// leaves, and when nothing it depends on changed — and re-shapes text exactly
    /// when its used kind moved.
    ///
    /// **The frame is the only node any commit here names**, and that is the
    /// point: every used entry that changes belongs to a node the commit did not
    /// name. ⚠️ A resize dirties the frame's whole subtree (`op_set_geometry`), so
    /// only the fade and the move leave the children out of `existing` — the fade
    /// is the case the re-shape of `relaid` text exists for, and the move is the
    /// case D590's saving has to survive.
    ///
    /// **Flips**, each run: composing world transforms from `node.transform()` in
    /// `update` fails *"frame widened to 400: world of"* the rect; shaping at
    /// `node.kind()` in `reshape_text` fails the same comparison on the text's
    /// bounds, the resize having dirtied it; dropping the re-shape of `relaid`
    /// text fails the fade's count, 0 against 1; and recomputing used geometry
    /// over `existing` instead of `affected` fails that same count — not the
    /// resize, as a first reading predicts, because the resize dirties the rect
    /// itself, and it is the fade that leaves a node whose entry moves out of
    /// `existing`.
    #[test]
    fn update_keeps_the_used_geometry_equal_to_rebuild() {
        let mut f = fixture();
        let _probe = install(&f);
        let mut res = Resolved::rebuild(&f.doc);
        let size = |w: f64| Operation::SetGeometry {
            id: f.frame,
            geometry: GeometryPatch::Size(Size::new(w, 200.0)),
        };

        commit(&mut f.doc, &mut res, size(400.0));
        assert_equal_to_rebuild(&f.doc, &res, "frame widened to 400");
        assert_eq!(res.used_kind(&f.doc, f.rect), Some(&rect_kind(390.0, 50.0)));

        // Opacity dirties the frame alone, so the text reaches the re-shape only
        // through the door for text whose *used* kind moved.
        let before = text::shapes();
        commit(
            &mut f.doc,
            &mut res,
            Operation::SetOpacity {
                id: f.frame,
                opacity: 0.5,
            },
        );
        assert_eq!(
            text::shapes() - before,
            1,
            "the text's used kind moved under an undirtied node, so it is re-shaped — once"
        );
        assert_equal_to_rebuild(&f.doc, &res, "frame faded");

        let before = text::shapes();
        commit(
            &mut f.doc,
            &mut res,
            Operation::SetTransform {
                id: f.frame,
                transform: Affine::translate((40.0, 0.0)),
            },
        );
        assert_eq!(
            text::shapes(),
            before,
            "a move changes no used kind, so D590's saving still holds: nothing re-shaped"
        );
        assert_equal_to_rebuild(&f.doc, &res, "frame moved");

        commit(&mut f.doc, &mut res, size(50.0));
        assert_equal_to_rebuild(&f.doc, &res, "frame narrowed below the probe's floor");
        assert_eq!(
            res.world_bounds(f.rect),
            Some(Rect::new(50.0, 10.0, 150.0, 60.0)),
            "the entry left, and the rect is back where the document puts it"
        );
    }
}
