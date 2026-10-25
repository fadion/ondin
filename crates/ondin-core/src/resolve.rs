//! Resolved — the derived layer (§5.9, invariant 4).
//!
//! World transforms, stroke-expanded world bounds, shaped text layouts, and an
//! R-tree spatial index over the indexable leaves. Never serialized; always
//! reconstructible from the `Document` via `rebuild`. `update` applies a
//! `DirtySet` incrementally and must be equivalent to `rebuild` — a randomized
//! differential test enforces this.
//!
//! The text cache exists because shaping is the most expensive step in the
//! pipeline and bounds, hit-testing and scene building all need it. Shaping
//! depends on the fonts registered with `crate::text`, so the app must call
//! [`Resolved::invalidate_text`] when it registers a family.

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

/// The box a mask **clips with**: [`mask_parts`] folded over already-measured
/// child boxes.
///
/// Not the mask's own box: [`Resolved::mask_path`] does not narrow by an inner
/// mask, so the bounds must not either, or a container's box shrinks below its
/// artwork and `scene::paint_node` prunes it. One function serves both
/// [`Resolved::recompute_bounds`] and `resolve_subtree` so they cannot drift.
fn mask_extent(doc: &Document, boxes: &FxHashMap<NodeId, Rect>, id: NodeId) -> Option<Rect> {
    match mask_parts(doc, id)? {
        MaskParts::Members(cs) => cs
            .into_iter()
            .filter_map(|c| mask_extent(doc, boxes, c))
            .reduce(|a, b| a.union(b)),
        MaskParts::Own => boxes.get(&id).copied(),
    }
}

/// What a mask clips with, one level down.
pub(crate) enum MaskParts {
    /// The union of these children, each read the same way.
    Members(Vec<NodeId>),
    /// The node's own outline or box.
    Own,
}

/// The one statement of what a mask clips with, which [`Resolved::mask_path`],
/// [`mask_extent`] and `query::mask_extent_local` each fold into their own answer.
/// `None` for a hidden node. A group or the root clips with the union of its
/// children that are not masks themselves (an inner mask is not honoured, so the
/// clip is a little generous rather than an intersection per frame); any other
/// kind clips with itself.
pub(crate) fn mask_parts(doc: &Document, id: NodeId) -> Option<MaskParts> {
    let node = doc.get(id)?;
    if !doc.shown(id) {
        return None;
    }
    Some(match node.kind() {
        NodeKind::Group | NodeKind::Root => MaskParts::Members(
            node.children()
                .iter()
                .copied()
                .filter(|c| !doc.get(*c).is_some_and(|n| n.mask()))
                .collect(),
        ),
        _ => MaskParts::Own,
    })
}

fn entry(id: NodeId, r: Rect) -> BoundsEntry {
    BoundsEntry {
        id,
        min: [r.min_x(), r.min_y()],
        max: [r.max_x(), r.max_y()],
    }
}

/// A node's **used** geometry, where it differs from what the document specifies
/// (CSS's used value against its specified one).
///
/// Sparse: a node has an entry only when container layout placed or sized it
/// differently, or its corners or size are bound to variables (`with_bound_corners`,
/// `with_bound_size`); every other read falls back to [`Node::transform`] and
/// [`Node::kind`]. `local` and `kind` are separate `Option`s because a flex item
/// that moves keeps its size, and one that grows keeps nothing but its kind.
/// Read through [`local_in`] and [`kind_in`] here and [`Resolved::used_local`] and
/// [`Resolved::used_kind`] outside, so "no entry" always means "as specified".
#[derive(Clone, Debug, PartialEq)]
struct Used {
    /// The local transform layout places the node with.
    local: Option<Affine>,
    /// The kind layout sizes the node with. Always the same variant as the
    /// document's kind, so a `match` on a node's kind can keep reading the document.
    kind: Option<NodeKind>,
    /// The box of a container with a layout and no size of its own (a group with
    /// `display`), with its origin at the group's local origin. `None` otherwise,
    /// a frame included: a frame's size is its kind's.
    frame: Option<kurbo::Size>,
}

/// The committed document as the flex engine sees it (`container::LayoutView`):
/// every answer a specified value read off the node, with bindings resolved (bound
/// gaps, padding, visibility, width and height answer what they draw, never the
/// literal behind them). Crate-visible for `build`'s conversions.
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
    /// At its drawn size: a bound item is laid out at its variable's number.
    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.0.resolved_kind(id).map(std::borrow::Cow::into_owned)
    }
    fn display(&self, id: NodeId) -> Option<std::borrow::Cow<'_, crate::container::Display>> {
        self.0.resolved_display(id)
    }
    /// Bound limits resolved: laid out at their variables' numbers.
    fn item(&self, id: NodeId) -> crate::container::LayoutItem {
        self.0.resolved_item(id)
    }
    fn insets(&self, id: NodeId) -> crate::container::Insets {
        self.0.get(id).map(|n| *n.insets()).unwrap_or_default()
    }
    /// Shown, bound visibility resolved: a hidden item is out of its layout's flow.
    fn visible(&self, id: NodeId) -> bool {
        self.0.shown(id)
    }
    fn mask(&self, id: NodeId) -> bool {
        self.0.get(id).is_some_and(|n| n.mask())
    }
    fn local(&self, id: NodeId) -> Affine {
        self.0.get(id).map(|n| n.transform()).unwrap_or_default()
    }
}

/// [`DocView`] with the pinned-child box read from the used geometry filled so far
/// ([`frame_box`]) and text measured through the [`TextMemo`]. [`place_node`] lays
/// a root out through it; every other answer is the document's.
struct UsedView<'a> {
    doc: &'a Document,
    used: &'a FxHashMap<NodeId, Used>,
    memo: &'a std::cell::RefCell<FxHashMap<NodeId, TextMemo>>,
}

impl crate::container::LayoutView for UsedView<'_> {
    fn parent(&self, id: NodeId) -> Option<NodeId> {
        DocView(self.doc).parent(id)
    }
    fn children(&self, id: NodeId) -> Vec<NodeId> {
        DocView(self.doc).children(id)
    }
    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        DocView(self.doc).kind(id)
    }
    fn display(&self, id: NodeId) -> Option<std::borrow::Cow<'_, crate::container::Display>> {
        self.doc.resolved_display(id)
    }
    fn item(&self, id: NodeId) -> crate::container::LayoutItem {
        DocView(self.doc).item(id)
    }
    fn insets(&self, id: NodeId) -> crate::container::Insets {
        DocView(self.doc).insets(id)
    }
    fn visible(&self, id: NodeId) -> bool {
        DocView(self.doc).visible(id)
    }
    fn mask(&self, id: NodeId) -> bool {
        DocView(self.doc).mask(id)
    }
    fn local(&self, id: NodeId) -> Affine {
        DocView(self.doc).local(id)
    }
    fn parent_box(&self, id: NodeId) -> Option<kurbo::Size> {
        frame_box(self.used, self.doc.get(self.doc.get(id)?.parent()?)?)
    }
    fn measured(&self, id: NodeId, kind: &NodeKind) -> Option<Rect> {
        let Some(key) = sizing_key(kind) else {
            return geometry::local_bounds(kind, None);
        };
        if let Some(memo) = self.memo.borrow().get(&id)
            && let Some((_, b)) = memo.boxes.iter().find(|(k, _)| *k == key)
        {
            return *b;
        }
        let b = geometry::local_bounds(kind, None);
        self.memo
            .borrow_mut()
            .entry(id)
            .or_default()
            .boxes
            .push((key, b));
        b
    }
    fn content_widths(&self, id: NodeId, kind: &NodeKind) -> Option<(f64, f64)> {
        if let Some(w) = self.memo.borrow().get(&id).and_then(|m| m.widths) {
            return w;
        }
        let w = crate::node::TextRef::of(kind).map(text::content_widths);
        self.memo.borrow_mut().entry(id).or_default().widths = Some(w);
        w
    }
    fn first_baseline(&self, id: NodeId, kind: &NodeKind) -> Option<f64> {
        let key = sizing_key(kind)?;
        if let Some(memo) = self.memo.borrow().get(&id)
            && let Some((_, b)) = memo.baselines.iter().find(|(k, _)| *k == key)
        {
            return *b;
        }
        let layout = text::layout(crate::node::TextRef::of(kind)?);
        let b = layout.baselines.first().map(|b| b - layout.origin.y);
        self.memo
            .borrow_mut()
            .entry(id)
            .or_default()
            .baselines
            .push((key, b));
        b
    }
}

/// The committed document as a finished [`Resolved`] draws it, for a builder that
/// re-runs a layout pass: [`DocView`] with the pinned-root box read from the used
/// geometry and text measured through the memo. A grid drop
/// (`build::grid_drop_many`) lays through it so it picks cells in the grid the
/// commit lays.
pub(crate) struct CommittedView<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) res: &'a Resolved,
}

impl crate::container::LayoutView for CommittedView<'_> {
    fn parent(&self, id: NodeId) -> Option<NodeId> {
        DocView(self.doc).parent(id)
    }
    fn children(&self, id: NodeId) -> Vec<NodeId> {
        DocView(self.doc).children(id)
    }
    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        DocView(self.doc).kind(id)
    }
    fn display(&self, id: NodeId) -> Option<std::borrow::Cow<'_, crate::container::Display>> {
        self.doc.resolved_display(id)
    }
    fn item(&self, id: NodeId) -> crate::container::LayoutItem {
        DocView(self.doc).item(id)
    }
    fn insets(&self, id: NodeId) -> crate::container::Insets {
        DocView(self.doc).insets(id)
    }
    fn visible(&self, id: NodeId) -> bool {
        DocView(self.doc).visible(id)
    }
    fn mask(&self, id: NodeId) -> bool {
        DocView(self.doc).mask(id)
    }
    fn local(&self, id: NodeId) -> Affine {
        DocView(self.doc).local(id)
    }
    fn parent_box(&self, id: NodeId) -> Option<kurbo::Size> {
        frame_box(&self.res.used, self.doc.get(self.doc.get(id)?.parent()?)?)
    }
    fn measured(&self, id: NodeId, kind: &NodeKind) -> Option<Rect> {
        match self.res.remembered_text_box(id, kind) {
            Some(b) => b,
            None => geometry::local_bounds(kind, None),
        }
    }
    fn content_widths(&self, id: NodeId, kind: &NodeKind) -> Option<(f64, f64)> {
        match self.res.remembered_text_widths(id) {
            Some(w) => w,
            None => crate::node::TextRef::of(kind).map(text::content_widths),
        }
    }
    fn first_baseline(&self, id: NodeId, kind: &NodeKind) -> Option<f64> {
        if let Some(b) = self.res.remembered_text_baseline(id, kind) {
            return b;
        }
        let layout = text::layout(crate::node::TextRef::of(kind)?);
        layout.baselines.first().map(|b| b - layout.origin.y)
    }
}

/// One text node's measurements as the layout pass asked for them (its box at each
/// sizing, its content widths), **kept across commits**.
///
/// A layout pass measures every text item at several widths, and runs for every
/// layout root under a commit's chain root, so without the memo a nudge of a
/// container re-shapes all its text. A text's box depends on nothing outside its
/// own node and every change dirties it, so a node's entries are dropped when it is
/// dirty and kept otherwise. Fonts arriving (`Resolved::invalidate_text`) are the
/// one outside input and dirty every text node.
#[derive(Clone, Default)]
struct TextMemo {
    boxes: Vec<(SizingKey, Option<Rect>)>,
    widths: Option<Option<(f64, f64)>>,
    /// The first baseline at each sizing asked.
    baselines: Vec<(SizingKey, Option<f64>)>,
}

/// A text sizing as a memo key: the variant and its numbers' bits.
type SizingKey = (u8, u64, u64);

fn sizing_key(kind: &NodeKind) -> Option<SizingKey> {
    use crate::node::TextSizing;
    match kind {
        NodeKind::Text { sizing, .. } => Some(match sizing {
            TextSizing::Auto => (0, 0, 0),
            TextSizing::AutoHeight(w) => (1, w.to_bits(), 0),
            TextSizing::Fixed(s) => (2, s.width.to_bits(), s.height.to_bits()),
        }),
        _ => None,
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
/// Flex first: a node the pass laid out (an item placed from `laid`, or a layout
/// root sized by its own layout) answers what the layout made of it, a group's box
/// included. Then absolute insets: a child of a frame, or of a group with a layout,
/// with any inset set is placed by [`crate::container::place`] against its
/// container's used box, read out of `used`, which the caller fills parents first.
/// Everything else answers `None` and is drawn where the document says.
///
/// Under `cfg(test)` a probe can answer instead (`probe::set`) to drive arbitrary
/// used geometry through the plumbing; a probe answering `None` falls through.
fn used_geometry(
    view: &UsedView<'_>,
    laid: &FxHashMap<NodeId, crate::container::Laid>,
    id: NodeId,
) -> Option<Used> {
    let (doc, used) = (view.doc, view.used);
    #[cfg(test)]
    if let Some(u) = probe::used(doc, id) {
        return Some(u);
    }
    let node = doc.get(id)?;
    // An in-flow item takes its slot and size from the pass its chain root ran; a
    // layout root takes only its size from its own pass, and is placed as any other child.
    if let Some(l) = laid.get(&id) {
        let item =
            crate::container::parent_lays_out(view, id) && crate::container::in_flow(view, id);
        if item {
            return crate::container::item_placed(view, id, l.slot, l.size).map(|p| Used {
                local: p.local,
                kind: p.kind,
                frame: p.frame,
            });
        }
        return root_used(doc, used, node, l.size);
    }
    let parent = doc.get(node.parent()?)?;
    let frame = frame_box(used, parent)?;
    let own = doc.resolved_kind(id)?;
    let (local, kind) = crate::container::place(node.insets(), frame, node.transform(), &own)?;
    Some(Used {
        local: Some(local),
        kind,
        frame: None,
    })
}

/// A layout root's used geometry: the size its own pass gave it (a hugging frame
/// grows its kind, a group with a layout takes the box), placed as any other child.
fn root_used(
    doc: &Document,
    used: &FxHashMap<NodeId, Used>,
    node: &Node,
    size: kurbo::Size,
) -> Option<Used> {
    let (kind, frame) = crate::container::root_sized(&DocView(doc), node.id(), size);
    // Insets place a frame root inside a frame; a group root takes no insets.
    let pinned = node
        .parent()
        .and_then(|p| doc.get(p))
        .and_then(|p| frame_box(used, p))
        .and_then(|f| {
            let k = kind.clone().unwrap_or_else(|| {
                doc.resolved_kind(node.id())
                    .map_or_else(|| node.kind().clone(), std::borrow::Cow::into_owned)
            });
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

/// `id`'s used geometry, running its layout pass first when it roots one, so a
/// chain's items find their results in `laid`.
fn place_node(
    view: &UsedView<'_>,
    laid: &mut FxHashMap<NodeId, crate::container::Laid>,
    id: NodeId,
) -> Option<Used> {
    if crate::container::is_layout_root(view, id) {
        for l in crate::container::lay_out(view, id) {
            laid.insert(l.id, l);
        }
    }
    let used = with_bound_size(view.doc, id, used_geometry(view, laid, id));
    with_bound_corners(view.doc, id, used)
}

/// `used` with a layer's bound width and height (and a bound text's content and
/// family) in its kind where nothing else sized it: the variable's value in the
/// mode in force, not the literal. Layout and insets already size from it via
/// `Document::resolved_kind`; this covers a bound layer neither laid nor pinned.
fn with_bound_size(doc: &Document, id: NodeId, used: Option<Used>) -> Option<Used> {
    let Some(node) = doc.get(id) else {
        return used;
    };
    let bound = node.size_vars().iter().any(Option::is_some)
        || node.text_vars().iter().any(Option::is_some);
    if !bound || used.as_ref().is_some_and(|u| u.kind.is_some()) {
        return used;
    }
    let kind = doc.resolved_kind(id)?.into_owned();
    let mut u = used.unwrap_or(Used {
        local: None,
        kind: None,
        frame: None,
    });
    u.kind = Some(kind);
    Some(u)
}

/// `used` with a rectangle's bound corners resolved into its kind, where every
/// geometry reader already looks. Layout's size is kept; only the radii change.
fn with_bound_corners(doc: &Document, id: NodeId, used: Option<Used>) -> Option<Used> {
    let Some(node) = doc.get(id) else {
        return used;
    };
    if node.corner_vars().iter().all(Option::is_none) {
        return used;
    }
    let kind = used
        .as_ref()
        .and_then(|u| u.kind.as_ref())
        .unwrap_or_else(|| node.kind());
    let NodeKind::Rect { size, corner_radii } = kind else {
        return used;
    };
    let Some(drawn) = doc.resolve_corners(id, *corner_radii) else {
        return used;
    };
    let size = *size;
    let mut u = used.unwrap_or(Used {
        local: None,
        kind: None,
        frame: None,
    });
    u.kind = Some(NodeKind::Rect {
        size,
        corner_radii: drawn,
    });
    Some(u)
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
/// `pub(crate)` for `node`'s kind-predicate matrix test. Its membership matches
/// `NodeKind::takes_paint`'s today for two unrelated reasons; the matrix makes
/// drift visible.
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
            // A container, but the only thing in a boolean subtree the canvas
            // paints, so the only thing there a click can land on.
            | NodeKind::Boolean { .. }
    )
}

/// The document's derived state, kept equal to [`Self::rebuild`] by
/// [`Self::update`]. `Clone` so an edit not yet committed is laid out by updating
/// a copy, which reshapes only the text it dirties.
#[derive(Clone)]
pub struct Resolved {
    world: FxHashMap<NodeId, Affine>,
    world_bounds: FxHashMap<NodeId, Rect>,
    /// The world AABB of everything a node's **ink** covers, effects included:
    /// [`Self::world_bounds`] grown by [`crate::effect::stack_escape`] at every
    /// level of the subtree.
    ///
    /// A second map because `world_bounds` is the box of the thing being edited,
    /// which a shadow must not move, while culling and export need the box that is
    /// really drawn.
    ink_bounds: FxHashMap<NodeId, Rect>,
    /// [`Self::ink_bounds`] before this node's own effects escape.
    ///
    /// What an effect layer must hold before its own padding is added; the only
    /// box usable by a caller with a different effect stack in hand (the renderer
    /// during a preview). Only the `Group`/`Root` arm of `scene::effect_bounds`
    /// reads it.
    inner_ink: FxHashMap<NodeId, Rect>,
    /// Shaped layout per `Text` node, invalidated through the `DirtySet`.
    text: FxHashMap<NodeId, TextLayout>,
    /// Combined outline per `Boolean` node, in that node's own local space.
    ///
    /// A boolean's outline is a function of its whole subtree, so the document
    /// cannot hold it. Evaluated children-first, because a boolean may contain one.
    boolean: FxHashMap<NodeId, BezPath>,
    /// Booleans whose arithmetic was **abandoned** (an upstream panic caught by
    /// `boolean::evaluate`'s guard) rather than merely empty. A separate set
    /// because both read as "no entry" in `boolean`.
    failed: FxHashSet<NodeId>,
    index: RTree<BoundsEntry>,
    /// Used geometry, sparse — see [`Used`].
    ///
    /// `update` must keep it equal to `rebuild`
    /// (`assert_resolved_matches_rebuild`). Filled first, before text is shaped and
    /// world transforms are composed, because both read it.
    used: FxHashMap<NodeId, Used>,
    /// The layout pass's text measurements, kept across commits — see
    /// [`TextMemo`]. Taken out into a `RefCell` for the length of a pass and put
    /// back, so `Resolved` holds no interior mutability.
    text_memo: FxHashMap<NodeId, TextMemo>,
}

impl Resolved {
    /// Full reconstruction from the document.
    pub fn rebuild(doc: &Document) -> Self {
        // Used geometry first, in pre-order so a frame's entry is in the map before
        // its children are placed against it.
        let mut used: FxHashMap<NodeId, Used> = FxHashMap::default();
        let mut laid = FxHashMap::default();
        let memo = std::cell::RefCell::new(FxHashMap::default());
        for id in crate::subtree_nodes(doc, &[doc.root()]) {
            let view = UsedView {
                doc,
                used: &used,
                memo: &memo,
            };
            if let Some(u) = place_node(&view, &mut laid, id) {
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
            text_memo: memo.into_inner(),
        };
        // Booleans in a second pass, through `update`, which already owns the
        // children-first ordering; the walk above cannot do them because a nested
        // boolean needs the cache it is itself filling.
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

        // Drop nodes that no longer exist, and the vault's, which are never
        // resolved: `rebuild` never meets the vault, and resolving a dirtied
        // vendored main here would put a hidden main in the index.
        let unresolved = |id: NodeId| !doc.contains(id) || doc.in_vault(id);
        for id in dirty.0.iter().filter(|id| unresolved(**id)) {
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
            .filter(|id| !unresolved(*id))
            .collect();
        if existing.is_empty() {
            // Only deletions happened; still refresh bounds up-tree from parents,
            // which the op layer added to the dirty set, so this branch is rare.
            self.reindex(doc);
            return;
        }

        // Recompute set: every dirty node plus its descendants (their world
        // transforms depend on the dirty ancestor), keyed by depth as the walk
        // descends so the sort below needs no second traversal.
        let mut affected: FxHashMap<NodeId, usize> = FxHashMap::default();
        for id in &existing {
            // From the root of the node's flex chain: an item's change moves its
            // siblings and can resize every hugging container above it, and the
            // pass that places them runs at the chain's root.
            let from = crate::container::chain_root(&DocView(doc), *id);
            collect_subtree(doc, from, depth(doc, from), &mut affected);
        }
        let mut ordered: Vec<NodeId> = affected.keys().copied().collect();

        // Parents before children, for the used-geometry pass (a child is placed
        // against its frame's used size) and the world transforms after it.
        //
        // `sort_by_cached_key`, not `sort_by_key`: the latter calls its key on every
        // comparison, and `depth` walks the parent chain, so deep trees made the
        // incremental path far slower than `rebuild`. The key here is a lookup
        // recorded by `collect_subtree`.
        ordered.sort_by_cached_key(|id| affected.get(id).copied().unwrap_or(0));

        // Used geometry before text and transforms, over `affected` rather than
        // `existing` because a dirty ancestor can change where layout puts a node.
        // Nodes whose entry changed are remembered: a text node is shaped at its
        // used kind and must be re-shaped when that moves though its own did not.
        let mut relaid: Vec<NodeId> = Vec::new();
        let mut laid = FxHashMap::default();
        // The text memo without every dirty node's entries: a node's measurements
        // depend on its own kind alone, and every change to one dirties it, deleted
        // nodes included.
        let mut memo = std::mem::take(&mut self.text_memo);
        for id in &dirty.0 {
            memo.remove(id);
        }
        // Except a bound text's: its content and family are its string variables' in
        // the mode in force, which a frame's `SetMode` or a reparent changes without
        // dirtying it. Few texts are bound, so their entries go on every update.
        memo.retain(|id, _| {
            doc.get(*id)
                .is_none_or(|n| n.text_vars().iter().all(Option::is_none))
        });
        let memo = std::cell::RefCell::new(memo);
        for id in &ordered {
            let fresh = {
                let view = UsedView {
                    doc,
                    used: &self.used,
                    memo: &memo,
                };
                place_node(&view, &mut laid, *id)
            };
            let changed = match fresh {
                Some(u) => self.used.insert(*id, u.clone()).as_ref() != Some(&u),
                None => self.used.remove(id).is_some(),
            };
            if changed {
                relaid.push(*id);
            }
        }
        self.text_memo = memo.into_inner();

        // Text first: bounds depend on the shaped size. Over `existing`, not
        // `affected`: a layout depends only on the node's used kind, so it is redone
        // only when its own kind changed or its used kind moved (`relaid`). The
        // waste of more is invisible to the differential test; `text::shapes`
        // makes it assertable.
        for id in &existing {
            self.reshape_text(doc, *id);
        }
        // And the text whose used kind moved, if not already shaped above.
        for id in relaid.iter().filter(|id| !existing.contains(id)) {
            self.reshape_text(doc, *id);
        }

        // Transforms: parents before children, which `ordered` already is.
        for id in &ordered {
            let node = doc.get(*id).expect("in document");
            let local = local_in(&self.used, node);
            let world = match node.parent() {
                Some(p) => self.world.get(&p).copied().unwrap_or(Affine::IDENTITY) * local,
                None => local,
            };
            self.world.insert(*id, world);
        }

        // Bounds: children before parents, booleans in the same pass, since a
        // boolean's box is measured from its outline and a nested one is an operand.
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
                // A boolean ancestor is re-evaluated, not just re-measured: moving
                // an operand changes its shape, where a group only re-unions boxes.
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

    /// The local transform `id` is **placed** with: layout's where it moved the
    /// node, the document's everywhere else. `None` for a node the document does
    /// not hold.
    ///
    /// Ask this to draw, measure, hit-test and edit: a tool computes from where the
    /// node is drawn and writes an ordinary `SetTransform`, which
    /// `crate::build::keep_insets` turns into new insets for a pinned layer.
    pub fn used_local(&self, doc: &Document, id: NodeId) -> Option<Affine> {
        doc.get(id).map(|n| local_in(&self.used, n))
    }

    /// The kind `id` is **sized** with: layout's where it resized the node, the
    /// document's everywhere else. `None` for a node the document does not hold.
    ///
    /// Always the same variant as [`Node::kind`]; geometry questions ask this, under
    /// [`Self::used_local`]'s rule. What an operation is applied to is still the
    /// document's kind: a `GeometryPatch` that is not a whole size (a corner radius,
    /// a side count) patches the stored kind and carries the stored size with it.
    pub fn used_kind<'a>(&'a self, doc: &'a Document, id: NodeId) -> Option<&'a NodeKind> {
        doc.get(id).map(|n| kind_in(&self.used, n))
    }

    /// [`Self::used_local`] for a node already in hand, so infallible.
    ///
    /// For writers that pass a `&Node` down helpers with no document in reach (the
    /// SVG writer). `node` must be this document's own: the map is keyed by id, so
    /// a copy (clipboard node, ghost) would be answered with the original's layout.
    pub fn used_local_of(&self, node: &Node) -> Affine {
        local_in(&self.used, node)
    }

    /// [`Self::used_kind`] for a node already in hand, under
    /// [`Self::used_local_of`]'s condition: `node` must be this document's own.
    pub fn used_kind_of<'a>(&'a self, node: &'a Node) -> &'a NodeKind {
        kind_in(&self.used, node)
    }

    /// The box the layout pass last measured for text node `id` drawn as `kind`,
    /// if it is remembered: `Some(answer)` for a hit, `None` for a miss. `kind` must
    /// be `id`'s committed kind at some sizing, since the memo is keyed by sizing
    /// alone.
    ///
    /// For the renderer's preview, so its layout passes do not re-shape every text
    /// item each frame; it asks only for nodes whose kind the preview leaves alone.
    pub fn remembered_text_box(&self, id: NodeId, kind: &NodeKind) -> Option<Option<Rect>> {
        let key = sizing_key(kind)?;
        self.text_memo
            .get(&id)?
            .boxes
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, b)| *b)
    }

    /// [`Self::remembered_text_box`] for the content widths.
    pub fn remembered_text_widths(&self, id: NodeId) -> Option<Option<(f64, f64)>> {
        self.text_memo.get(&id)?.widths
    }

    /// [`Self::remembered_text_box`] for the first baseline.
    pub fn remembered_text_baseline(&self, id: NodeId, kind: &NodeKind) -> Option<Option<f64>> {
        let key = sizing_key(kind)?;
        self.text_memo
            .get(&id)?
            .baselines
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, b)| *b)
    }

    /// The box a group with a layout was given, in its own space from its origin;
    /// `None` for any other node, a frame included. What the selection, the W/H and
    /// a resize read for such a group.
    pub fn used_frame(&self, id: NodeId) -> Option<kurbo::Size> {
        self.used.get(&id).and_then(|u| u.frame)
    }

    /// The stroke-expanded world AABB of `id`.
    ///
    /// **Not reduced by any clip, a mask included.** It is conservative (a clip only
    /// removes ink, so the box is never too small for culling and the index) and it
    /// is the box of the thing being edited: a selection outline that shrank to the
    /// visible part of a masked photograph would leave nothing to aim at. A marquee
    /// or snap can therefore find clipped-away ink, as with frame clips.
    pub fn world_bounds(&self, id: NodeId) -> Option<Rect> {
        self.world_bounds.get(&id).copied()
    }

    /// The world AABB of everything `id`'s ink covers, its effects and its
    /// descendants' effects included. See [`Self::ink_bounds`]'s field for why
    /// this is a separate question from [`Self::world_bounds`].
    ///
    /// Ask this for culling, export and damage; ask `world_bounds` for anything the
    /// user aims at or reads a number from. The two are equal without effects, so a
    /// wrong choice stays invisible until the first shadow.
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
    /// nothing, and that is a defect rather than an answer.
    ///
    /// [`Self::boolean_path`] answers `None` both for a legitimately empty boolean
    /// and for one that gave up; anything that tells them apart (the layers row's
    /// warning colour, the status line) asks this.
    pub fn boolean_failed(&self, id: NodeId) -> bool {
        self.failed.contains(&id)
    }

    /// What the subtree at `id` contributes as a boolean operand, in its
    /// **parent's** space — a shape's own outline, a group's contents unioned, a
    /// boolean's cached result, nothing at all for text.
    ///
    /// The committed half of [`boolean::Operands`], spelled once for `flatten`
    /// rather than copied a third time beside [`Self::reevaluate_boolean`] and
    /// `RenderOverrides`.
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
            visible_of: &|c| doc.shown(c),
            nested: &|c| self.boolean.get(&c).cloned(),
        })
    }

    /// The outline to draw `id` with, in its local space: a boolean's from the
    /// cache, everything else's from its kind.
    ///
    /// Its callers are [`Self::mask_path`] and the canvas's `stroke_outlines`. The
    /// scene walk wants an `Option<&BezPath>` rather than a clone
    /// (`scene::Painted::path`), and the SVG writer has its own `outline`, which
    /// also reaches for text glyph outlines.
    pub fn local_path(&self, doc: &Document, id: NodeId) -> Option<BezPath> {
        let node = doc.get(id)?;
        match kind_in(&self.used, node) {
            NodeKind::Boolean { .. } => self.boolean.get(&id).cloned(),
            kind => geometry::local_path(kind),
        }
    }

    /// The outline a **mask** clips with, in its **parent's** space, ready for
    /// `ScenePainter::push_layer` under the parent's world transform or a
    /// `<clipPath>` beside the parent's `<g>`.
    ///
    /// Kinds are decided as [`crate::boolean::operand_of`] does, plus a `Text` node
    /// contributing its glyph outlines. `None` masks nothing (hidden mask, empty
    /// text, cancelled boolean, empty group): callers read it as no clip, so hiding
    /// a mask does not look like deleting the artwork. Filled non-zero, as painted.
    pub fn mask_path(&self, doc: &Document, id: NodeId) -> Option<BezPath> {
        let node = doc.get(id)?;
        let own = match mask_parts(doc, id)? {
            // A union, not a concatenation: under non-zero two members wound against
            // each other would cancel to a hole (`a_group_masks_with_the_union_of_its_contents`).
            MaskParts::Members(cs) => {
                let inner: Vec<BezPath> = cs
                    .into_iter()
                    .filter_map(|c| self.mask_path(doc, c))
                    .collect();
                boolean::evaluate(crate::node::BoolOp::Union, &inner)
            }
            MaskParts::Own => match node.kind() {
                NodeKind::Text { .. } => self
                    .text
                    .get(&id)
                    .map(text::outline)
                    .filter(|p| !p.elements().is_empty()),
                _ => self.local_path(doc, id),
            },
        }?;
        Some(local_in(&self.used, node) * own)
    }

    /// Re-shape every `Text` node and refresh everything that depends on it.
    ///
    /// A layout computed before a family was registered with `crate::text` used the
    /// fallback face; the app calls this after registering fonts, or bounds and
    /// hit-testing stay measured against the wrong face.
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
        // At the used kind: layout hands a text box a wrap width, and the glyphs
        // are shaped at that width, not the typed one.
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
    /// Must run children-first, as both callers do, so a nested boolean reads a
    /// current cache entry.
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
        // Bracketed by `boolean::failures`, the only way to tell an empty result
        // from an abandoned one. It wraps the whole `evaluate_node`, because a group
        // operand is unioned by an inner call that catches its own unwind.
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
            // A group with a layout is a box: its bounds are the box its layout
            // made, padding and all, not the union of what it holds.
            NodeKind::Group if laid_box.is_some() => laid_box.and_then(|s| {
                geometry::measurable(geometry::transform_rect(
                    world,
                    Rect::from_origin_size(kurbo::Point::ZERO, s),
                ))
            }),
            // The one place a mask reaches a box: a masked layer keeps its own
            // bounds, but lends its container only what the mask lets through.
            // Order-safe in both passes: every child's box is fresh when its
            // container is measured, and a mask precedes everything it masks.
            NodeKind::Group | NodeKind::Root => {
                let mut mask_box: Option<Rect> = None;
                node.children()
                    .iter()
                    .filter_map(|c| {
                        let is_mask = doc.get(*c).is_some_and(|n| n.mask());
                        let b = self.world_bounds.get(c).copied();
                        if is_mask {
                            // `mask_extent`, not the child's own box. The mask
                            // contributes nothing itself: it is a clip, never
                            // painted, and returning `b` would size the group
                            // to the mask.
                            mask_box = mask_extent(doc, &self.world_bounds, *c);
                            return None;
                        }
                        match mask_box {
                            Some(m) => b.and_then(|b| clipped_to(b, m)),
                            None => b,
                        }
                    })
                    .reduce(|acc, b| acc.union(b))
            }
            // Measured from the derived outline, not the children: a result is
            // smaller than the union of its operands for every operation but Union.
            NodeKind::Boolean { .. } => match self.boolean.get(&id) {
                Some(path) => geometry::world_bounds_of_path(
                    path,
                    &doc.resolved_paint(id, node.paint()),
                    kind_in(&self.used, node),
                    world,
                ),
                // A boolean that gave up still draws the placeholder, so it needs a
                // box for the index, marquee, zoom and cull. Only when it failed: a
                // boolean whose operands cancel draws nothing and must have no box,
                // or it would be an invisible click target.
                // Through [`geometry::measurable`], because this is the one arm that
                // reaches [`geometry::transform_rect`] raw, and `query::local_box`
                // composes child transforms itself, so a huge scale would store an
                // infinite rect and union it into every ancestor.
                None if self.failed.contains(&id) => {
                    crate::query::boolean_placeholder(doc, self, id)
                        .map(|b| geometry::transform_rect(world, b))
                        .and_then(geometry::measurable)
                }
                None => None,
            },
            // The paint as drawn, a bound stroke width its number: the stroke's
            // reach is part of the box.
            _ => geometry::world_bounds_of_parts(
                kind_in(&self.used, node),
                &doc.resolved_paint(id, node.paint()),
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

        // Ink is measured from the children's ink, not the box just stored, or a
        // shadow one level down would be forgotten by every ancestor; a container's
        // own escape then applies over that union, so a blur on a group covers all
        // its contents. Order-safe for the same reason as the box above.
        let ink = match node.kind() {
            NodeKind::Group | NodeKind::Root => self.children_ink(doc, node),
            // A frame that does not clip lets its children's ink out, and an
            // ancestor culls a whole subtree on the union of its children's ink.
            // A clipping frame keeps its box: nothing of its children paints outside.
            NodeKind::Artboard { .. } if !node.clip() => {
                union_of(bounds, self.children_ink(doc, node))
            }
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
        // Through `measurable`: a `stack_escape` that overflows answers the
        // unbounded rect, and a node whose ink is not a number has no ink rather
        // than infinite ink.
        let ink = ink
            // As drawn, a bound effect number its variable's.
            .map(|b| {
                let fx = doc.resolved_effects(id, node.effects());
                effect::escaped(b, effect::stack_escape(&fx), world)
            })
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
                    // The ink twin of the box in `recompute_bounds`, which
                    // `scene::paint_node` culls a whole subtree on. The mask
                    // contributes nothing: what is drawn is every other child's ink
                    // intersected with it.
                    mask_ink = mask_extent(doc, &self.ink_bounds, *c);
                    return None;
                }
                match mask_ink {
                    // Clipped by the mask's ink, where the box uses its box: a
                    // blurred mask passes ink beyond its outline, and too small is
                    // the one error these bounds may not make.
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

/// The five maps [`resolve_subtree`] fills. The used-geometry map is not one of
/// them: the walk reads it, since [`Resolved::rebuild`] computes it whole first.
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
/// Every transform and kind it reads is the used one, out of `used`, as
/// [`Resolved::update`] reads it; the two are asserted equal.
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
    // The mask governing the children walked so far; the backwards scan of
    // [`Document::governing_mask`] read forwards, and it has to agree with it.
    let mut mask_box: Option<Rect> = None;
    // Its ink twin, kept apart because a blurred mask lets ink past its outline.
    let mut mask_ink: Option<Rect> = None;
    for child in node.children() {
        let is_mask = doc.get(*child).is_some_and(|n| n.mask());
        let (cb, ci) = resolve_subtree(doc, *child, w, used, c);
        if is_mask {
            // Not `cb`/`ci`: see `mask_extent`. The child's whole subtree is in the
            // caches by now.
            mask_box = mask_extent(doc, c.world_bounds, *child);
            mask_ink = mask_extent(doc, c.ink_bounds, *child);
        }
        // A mask contributes nothing, as in `recompute_bounds`.
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
        // As drawn, a bound stroke width resolved.
        _ => geometry::world_bounds_of_parts(
            kind,
            &doc.resolved_paint(id, node.paint()),
            w,
            c.text.get(&id),
        ),
    };
    if let Some(b) = bounds {
        c.world_bounds.insert(id, b);
    }

    // As in `recompute_bounds`: the union is of the children's ink, and this
    // node's own escape applies over the result.
    let inner = match node.kind() {
        NodeKind::Group | NodeKind::Root => child_ink_union,
        // A frame that does not clip lends its container its children's ink.
        NodeKind::Artboard { .. } if !node.clip() => union_of(bounds, child_ink_union),
        _ => bounds,
    };
    if let Some(b) = inner {
        c.inner_ink.insert(id, b);
    }
    // `recompute_bounds`' twin: `update` and `rebuild` are asserted equal, so a
    // guard on one is a divergence.
    let ink = inner
        // As drawn, a bound effect number its variable's.
        .map(|b| {
            let fx = doc.resolved_effects(id, node.effects());
            effect::escaped(b, effect::stack_escape(&fx), w)
        })
        .and_then(geometry::measurable);
    if let Some(b) = ink {
        c.ink_bounds.insert(id, b);
    }
    (bounds, ink)
}

/// The R-tree over every indexed node that has a box the tree can hold.
///
/// Filtered through [`geometry::measurable`]: `rstar`'s `AABB::from_corners` takes
/// corners component-wise, so an inverted rect (what [`geometry::transform_rect`]
/// answers for a `NaN` transform) becomes the maximal box and a candidate for
/// every query. Deliberately a second statement of the rule `recompute_bounds`
/// keeps, since the doors onto a non-finite box are not enumerable.
fn build_index(doc: &Document, world_bounds: &FxHashMap<NodeId, Rect>) -> RTree<BoundsEntry> {
    let entries: Vec<BoundsEntry> = world_bounds
        .iter()
        .filter(|(id, _)| doc.get(**id).is_some_and(|n| is_indexed(n.kind())))
        .filter_map(|(id, r)| geometry::measurable(*r).map(|r| entry(*id, r)))
        .collect();
    RTree::bulk_load(entries)
}

/// Every node in `id`'s subtree, with its depth, into `out`.
///
/// The depth comes free from the descent, where [`depth`] would walk to the root
/// per node. A node already in `out` short-circuits the recursion; the insert
/// rewrites its depth with the identical number.
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
/// answers, `used_geometry` answers, so tests can ask whether each consumer follows
/// the used geometry. Thread-local, so two tests cannot see each other's probe.
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

    /// Uninstalls the probe on drop, so a failing test cannot leave it installed.
    pub(super) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            PROBE.with(|p| *p.borrow_mut() = None);
        }
    }
}

/// The `measurable` guards, tested from inside the module because `build_index`
/// and `recompute_bounds` are private and the second one's state cannot be
/// reached through the public API.
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

    /// `build_index` refuses a box the R-tree would normalise to the whole plane.
    ///
    /// The map is written by hand. Both the inverted rect (a `NaN` transform) and
    /// the ordered infinite one (an overflowing product) are tried. The good
    /// sibling is the control: without it a `build_index` that dropped every entry
    /// would pass.
    ///
    /// flip: `build_index`'s `filter_map(… measurable …)` back to a `map` → fails
    /// at the first assertion (the inverted case).
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
    /// infinite box.
    ///
    /// The failure is forced through `boolean::poison_next`; `boolean_failed` is
    /// asserted first so a poisoned call that landed elsewhere cannot leave the
    /// test green. The overflow is a `scale(1e308)` operand, which
    /// `op_set_transform` accepts and `query::local_box` composes into an infinite
    /// box. The stderr line about a synthetic failure is expected.
    ///
    /// flip: remove `.and_then(geometry::measurable)` from the placeholder arm →
    /// fails at the bounds assertion, and independently at the container
    /// assertion (the group takes `0,0 .. inf,inf`).
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
        // The container's own measurable extent, so the group assertion is about
        // the boolean's contribution.
        create(
            &mut doc,
            sibling,
            group,
            1,
            rect_kind(10.0, 10.0),
            Affine::IDENTITY,
        );

        // One synthetic unwind, taken by the first `guarded` call: this boolean's.
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

    /// A blur under a very large scale does not give the document infinite ink.
    ///
    /// The square in `effect::escaped`'s `((a·rx)² + (c·ry)²).sqrt()` overflows
    /// near `1.3e154` though every guard passes its input; `f64::hypot` computes
    /// the same value without the overflowing intermediate.
    ///
    /// flip: restore `((a * rad.x).powi(2) + (c * rad.y).powi(2)).sqrt()` → fails
    /// at the first assertion (infinite box); the control stays green.
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
            effects: crate::keyed_by_position([crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius: 10.0 },
            )]),
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

        // The control: at an ordinary scale the blur reaches its full distance, so
        // the repair is not a clamp.
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
            effects: crate::keyed_by_position([crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius: 10.0 },
            )]),
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

    /// A blur radius whose square overflows does not report that it reaches
    /// nowhere, which would make `scene::paint_node` cull visible artwork; an
    /// infinite escape is an absence. Asked of `stack_escape` and `escaped`
    /// directly, since `Document::apply` refuses such a radius and a load clamps
    /// it; `escaped`'s floor stays as the renderers' own defence.
    ///
    /// flip: `deviation(radius).powi(2)` restored in `stack_escape` → fails at the
    /// `expect`; with `escaped`'s finiteness guard also removed → fails at the
    /// first assertion.
    #[test]
    fn a_blur_whose_square_overflows_still_reports_the_ink_it_covers() {
        let blur = |radius| {
            crate::keyed_by_position([crate::effect::Effect::new(
                crate::effect::EffectKind::LayerBlur { radius },
            )])
        };
        let box_ = Rect::new(0.0, 0.0, 100.0, 100.0);
        let ink_of = |radius| {
            crate::geometry::measurable(crate::effect::escaped(
                box_,
                crate::effect::stack_escape(&blur(radius)),
                Affine::IDENTITY,
            ))
        };
        // `deviation` halves it, so the square is about `1.8e308`, the last decade
        // before the old spelling overflowed.
        let ink = ink_of(4e154).expect("the blur has ink");
        assert!(
            ink.x0 < box_.x0 && ink.x1 > box_.x1,
            "the blur reported reaching nowhere: ink {ink:?} against box {box_:?}"
        );
        // The control: an escape that overflows `f64` is an absence.
        assert_eq!(ink_of(f64::MAX), None);
    }
}

/// Used geometry driven through the plumbing: a `probe` layout places and sizes
/// three nodes somewhere other than the document says, and each consumer inside
/// `Resolved` and `query` is asked whether it followed.
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
    /// is laid out, which is how a test makes entries leave the map.
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
                // alone, which a resize does not.
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
    /// Each assertion sits where the specified (x 10..110) and used (5..295) rect
    /// differ by a long way, so a consumer reading the document fails it.
    ///
    /// flip: `hit_test` handing `contains_local` the document's kind → fails at the
    /// x 250 click; `local_box` composing `child.transform()` → fails the group's
    /// box; `resolve_subtree` shaping at `node.kind()` → fails the wrapped-text
    /// assertion. Shaping at `node.kind()` in `reshape_text` does not bite here
    /// (this test only rebuilds); the next test covers it.
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
    /// The frame is the only node any commit names, so every changing used entry
    /// belongs to a node the commit did not name. A resize dirties the frame's whole
    /// subtree (`op_set_geometry`); only the fade and the move leave the children
    /// out of `existing`. The fade is the case the re-shape of `relaid` text exists
    /// for, and the move must not re-shape.
    ///
    /// flip: composing world transforms from `node.transform()` in `update` → fails
    /// at "frame widened to 400"; shaping at `node.kind()` in `reshape_text` → same
    /// comparison on the text's bounds; dropping the `relaid` re-shape, or
    /// recomputing used geometry over `existing` instead of `affected` → fails the
    /// fade's count (0 against 1).
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

        // Opacity dirties the frame alone, so the text reaches the re-shape only as
        // text whose used kind moved.
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
