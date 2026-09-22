//! Document — the single source of truth (§5.5, invariants 2 & 4).
//!
//! The only way to mutate is `apply(Transaction)`. There is no public `&mut`
//! access to nodes or the tree.

use crate::build::{affine_is_finite, brush_is_finite, effect_is_finite};
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideId};
use crate::id::{IdSource, NodeId};
use crate::image::{ImageEntry, ImageId};
use crate::io::{CURRENT_SCHEMA_VERSION, MAX_TREE_DEPTH};
use crate::meta::DocumentMeta;
use crate::node::{
    BlockStyle, CharSpans, Fill, FillRule, MaskMode, Node, NodeKind, Paint, ParaSpans,
    ParagraphStyle, Pivot, Stroke, TextStyle,
};
use crate::op::{ApplyOutcome, DirtySet, GeometryPatch, OpError, Operation, Transaction};
use kurbo::{Affine, RoundedRectRadii};
use peniko::Color;
use rustc_hash::{FxHashMap, FxHashSet};

/// The ground a document is drawn on until someone changes it.
///
/// Near-black, from the design. A file written before the ground was part of
/// the format was drawn on exactly this, so it is also what such a file means
/// (`io::schema`, §5.11).
pub const DEFAULT_CANVAS_BACKGROUND: Color = Color::from_rgba8(23, 23, 24, 255);

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    schema_version: u32,
    nodes: FxHashMap<NodeId, Node>,
    root: NodeId,
    /// The ground behind and around the frames (§15 D18).
    ///
    /// The one piece of document state that is not a node. It sat on
    /// `EditorSession` at first, as a working-surface preference like the
    /// camera, and that turned out to be the wrong side of the line: people set
    /// it for the artwork, not for the sitting, so it has to survive reopening
    /// the file. It is opaque by construction — there is nothing behind the
    /// canvas for it to be transparent against.
    canvas_background: Color,
    /// The ruler guides placed on this document (`crate::guide`).
    ///
    /// Unordered — guides have no z, nothing stacks against them, and no
    /// operation refers to one by position — so `apply` appends and IO sorts by
    /// id for byte-stable output (invariant 9). A `Vec` rather than a map
    /// because every reader wants all of them (drawing, hit-testing, "remove
    /// all") and a document has a handful, not thousands.
    guides: Vec<Guide>,
    /// The images this document carries, keyed by the id a fill refers to
    /// (`crate::image`).
    ///
    /// **A map, where guides are a `Vec`, because the readers differ**: every
    /// reader of guides wants all of them, and every reader of this wants exactly
    /// one — the render walk resolves a fill's reference per frame. IO sorts by id
    /// for byte-stable output (invariant 9), as it does for nodes.
    ///
    /// **Nothing collects it.** An entry is added and removed by explicit
    /// operations only, never garbage-collected when its last fill goes: deleting
    /// the one layer showing a photograph and undoing has to bring the bytes back,
    /// which is only true if the bytes were never thrown away. The cost is a
    /// document that can carry an image nothing shows, which is a *Purge unused*
    /// command rather than a bug.
    images: FxHashMap<ImageId, ImageEntry>,
    /// The document's library identity — typed name, project, creation stamp
    /// (`crate::meta`).
    ///
    /// **The one field here that no `Operation` can reach**, and the only one
    /// that is not about drawing. It is written by the library rather than by
    /// the editor, so it is outside undo on purpose: a rename typed into the
    /// dashboard, possibly while this document is not open, must not turn up as
    /// a step in its editing history. `crate::meta` records the rest of that
    /// reasoning and the invariant-9 rule that constrains what may go in it.
    meta: DocumentMeta,
}

impl Document {
    /// Create a document with a single `Root` node using the given id.
    pub fn new(root_id: NodeId) -> Self {
        let root = Node {
            id: root_id,
            parent: None,
            children: Vec::new(),
            kind: NodeKind::Root,
            transform: Affine::IDENTITY,
            name: "Root".to_string(),
            visible: true,
            locked: false,
            proportions_locked: false,
            opacity: 1.0,
            clip: false,
            mask: false,
            mask_mode: MaskMode::default(),
            fill_rule: FillRule::default(),
            paint: Paint::default(),
            pivot: None,
            exports: Vec::new(),
            effects: Vec::new(),
            grids: Vec::new(),
        };
        let mut nodes = FxHashMap::default();
        nodes.insert(root_id, root);
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            nodes,
            root: root_id,
            canvas_background: DEFAULT_CANVAS_BACKGROUND,
            guides: Vec::new(),
            images: FxHashMap::default(),
            // Empty, not minted: a document that has never been through the
            // library has no id, and `Document::new` is also what every test
            // fixture and the starter document use. Minting here would put a
            // fresh identity on a document nobody has saved.
            meta: DocumentMeta::default(),
        }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// The mask `id` is clipped by, if any: the **nearest preceding sibling**
    /// carrying [`Node::mask`] (§5.3).
    ///
    /// "Nearest preceding" is the whole rule, and it is what makes a second mask
    /// end the first one's run rather than nest inside it: scanning backwards
    /// stops at the first one found, so the layers between two masks belong to
    /// the lower of the two and nothing belongs to both.
    ///
    /// **A mask is never masked.** It answers `None` for itself even with a mask
    /// below it, for the same reason — a mask *begins* a run, and a run cannot
    /// begin inside itself. Without this a stack of two masks would clip the
    /// second by the first and then everything above by both.
    ///
    /// **Nothing masks inside a boolean.** Its children are operands rather than
    /// artwork — the scene walk stops at the container and draws the result — so
    /// there is no run there for a mask to clip, and the flag on one of them is
    /// inert.
    ///
    /// Stated here rather than left to fall out, because it does not: the walk
    /// never reaches those children and so never notices, while the hit test
    /// reaches them through the spatial index (an operand is `is_indexed`, and a
    /// click on one selects the boolean above it). Without this arm a mask
    /// pasted or reordered into a boolean would make part of that boolean
    /// unclickable while changing nothing about how it draws.
    ///
    /// **One production caller — [`crate::hit_test`], through `masked_away` — and
    /// that is worth stating, because the run rule has four readers and only this
    /// one asks it backwards.**
    ///
    /// The other three are already walking the child list in order and carry the
    /// current mask forward as they go: `scene::paint_node`, the SVG writer's
    /// child loop, and the two bounds passes in `resolve`. That is the natural
    /// spelling when you are mid-iteration — the backwards scan would be one per
    /// child, of a list you are already holding — so unifying them onto this
    /// function would make the walk worse, not better. **Fix a disagreement by
    /// correcting the forward reader, not by routing it through here.**
    ///
    /// What keeps the two spellings honest is that they are asserted against the
    /// *same fixtures*: `ops::a_mask_governs_up_to_the_next_one_and_is_never_masked_itself`
    /// pins this one, and `render::a_second_mask_ends_the_first_ones_run_instead_of_nesting`
    /// pins the forward one on the same arrangement of siblings. A change to the
    /// rule that touches one and not the other fails the other.
    pub fn governing_mask(&self, id: NodeId) -> Option<NodeId> {
        let node = self.get(id)?;
        if node.mask() {
            return None;
        }
        let parent = self.get(node.parent()?)?;
        if matches!(parent.kind(), NodeKind::Boolean { .. }) {
            return None;
        }
        let siblings = parent.children();
        let index = siblings.iter().position(|c| *c == id)?;
        siblings[..index]
            .iter()
            .rev()
            .find(|c| self.get(**c).is_some_and(|n| n.mask()))
            .copied()
    }

    /// The names `parent`'s children carry, skipping `except` — everything a new
    /// or renamed child has to avoid to get a number of its own (`crate::naming`).
    ///
    /// **`except` is a list, and both of its uses are a name that is on its way
    /// out.** A node being *renamed* is its own sibling, so without it `free_name`
    /// sees the name it is replacing as taken and walks one past —
    /// `build::set_boolean_op` turning "Union 2" into "Subtract 3" for no reason
    /// anybody could name. And a builder that creates a node *and deletes others in
    /// the same transaction* has the same problem several times over:
    /// `build::flatten_union` emits its `CreateNode` before the deletes, so
    /// flattening a path called "Path" produced "Path 2" and then took the "Path"
    /// away, leaving a lone number-2 with nothing above it.
    ///
    /// Only the direct children. Numbering is per-parent (§15 D127): document-wide
    /// numbering yields "Rectangle 137" in a large file for no benefit, and names
    /// repeating across parents is already an accepted state — the layers panel
    /// shows ancestor rows for a search hit precisely so one can be located.
    pub fn child_names(&self, parent: NodeId, except: &[NodeId]) -> FxHashSet<&str> {
        self.nodes
            .get(&parent)
            .into_iter()
            .flat_map(|n| n.children.iter())
            .filter(|id| !except.contains(id))
            .filter_map(|id| self.nodes.get(id))
            .map(|n| n.name.as_str())
            .collect()
    }

    /// The ground behind and around the frames.
    pub fn canvas_background(&self) -> Color {
        self.canvas_background
    }

    /// The document's library identity (`crate::meta`).
    pub fn meta(&self) -> &DocumentMeta {
        &self.meta
    }

    /// Replace the library identity.
    ///
    /// **Not an `Operation`, so it does not go through `apply` and cannot be
    /// undone** — see [`crate::meta`] for why a rename belongs outside the
    /// document's history. It follows that this does not report a `DirtySet`
    /// either: nothing about the drawing changed, so no render invalidation is
    /// owed and the caller's only obligation is to write the file.
    pub fn set_meta(&mut self, meta: DocumentMeta) {
        self.meta = meta;
    }

    /// Every ruler guide on the document, in no particular order.
    pub fn guides(&self) -> &[Guide] {
        &self.guides
    }

    pub fn guide(&self, id: GuideId) -> Option<&Guide> {
        self.guides.iter().find(|g| g.id == id)
    }

    /// The guides scoped to `owner`, in no particular order.
    ///
    /// The reverse of [`Guide::owner`], spelled here so that a caller cannot
    /// accidentally read a `None` owner as "owned by nothing in particular" when it
    /// means "owned by the canvas".
    ///
    /// **One caller today** — the Scale tool, multiplying a frame's own guides
    /// (§5.6) — and the two that look as though they should are worth naming,
    /// because both were expected to and neither can. `build::guides_of` tests
    /// membership of a whole doomed *subtree* rather than one owner, so it filters
    /// `guides()` directly; and the canvas draws every guide in one pass rather than
    /// asking frame by frame, which is what keeps the draw one loop over a handful
    /// of guides instead of one per frame in the document.
    ///
    /// [`Guide::owner`]: crate::guide::Guide::owner
    pub fn guides_owned_by(&self, owner: NodeId) -> impl Iterator<Item = &Guide> {
        self.guides.iter().filter(move |g| g.owner == Some(owner))
    }

    /// One image's entry, or `None` for a reference to something this document
    /// does not carry.
    ///
    /// **A missing entry is a state to draw, not an error.** A linked image whose
    /// file has moved is the case v1 has to handle anyway, so a dangling reference
    /// takes the same path rather than a panic — and `None` here is what puts it
    /// on that path.
    ///
    /// The drawing at the end of it is the **placeholder**: both backends and the
    /// SVG writer paint the grey cross [`crate::missing_placeholder`] describes
    /// rather than leaving a blank, and the layers panel tints the row (§15 D179).
    pub fn image(&self, id: &ImageId) -> Option<&ImageEntry> {
        self.images.get(id)
    }

    pub fn has_image(&self, id: &ImageId) -> bool {
        self.images.contains_key(id)
    }

    /// Every entry, unordered. IO sorts; nothing else should depend on the order.
    pub fn images(&self) -> impl Iterator<Item = (&ImageId, &ImageEntry)> {
        self.images.iter()
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Number of nodes, including the root.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Build a document from already-validated parts (used by IO on load).
    /// Performs no validation itself — the caller (io::schema) checks integrity.
    pub(crate) fn from_parts(
        schema_version: u32,
        nodes: FxHashMap<NodeId, Node>,
        root: NodeId,
        canvas_background: Color,
        guides: Vec<Guide>,
        images: FxHashMap<ImageId, ImageEntry>,
        meta: DocumentMeta,
    ) -> Self {
        Self {
            schema_version,
            nodes,
            root,
            canvas_background,
            guides,
            images,
            meta,
        }
    }

    /// Iterate all nodes in unspecified order (used by IO on save, which sorts).
    pub(crate) fn nodes_iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// The one mutation path (§5.7, invariant 2).
    ///
    /// Atomic: every op is validated and applied against a working copy; if any
    /// op fails, the document is left untouched and the error is returned. On
    /// success returns the inverse transaction (apply it to undo) and the set of
    /// nodes whose world bounds/appearance changed.
    pub fn apply(&mut self, tx: &Transaction) -> Result<ApplyOutcome, OpError> {
        // Work on a clone so a mid-transaction failure cannot leave partial
        // state (§5.7 "validate all ops against a working copy"). apply() runs
        // once per committed edit (gestures commit on release — §9.3), never
        // per frame, so the clone cost is acceptable; a staged-diff optimization
        // can replace this later behind the same signature.
        let mut working = self.clone();
        let mut inverse_ops: Vec<Operation> = Vec::with_capacity(tx.0.len());
        let mut dirty = DirtySet::default();

        for op in &tx.0 {
            let inverse = working.apply_one(op, &mut dirty)?;
            inverse_ops.push(inverse);
        }

        // ⚠️ **Every guide's owner, once, after the whole transaction** (§15 D491,
        // `[S2.2-L2-05]`). `check_guide_owner` is enforced on `AddGuide` and
        // `SetGuideScope` — the operations that *name* an owner — and there is a
        // second way to break the same rule, which is to remove the frame. A
        // `DeleteNode` on a guide-owning frame was accepted, `doc.guides()` kept
        // the orphan, and `io::load(io::save(&doc))` then refused the file
        // **forever**: *"guide 51:4 is scoped to 51:2, which is not a frame in this
        // document"*. Autosave writes that file, and the in-memory session that
        // could still undo it is the only copy of the user's work.
        //
        // **A post-condition rather than a guide arm inside `op_delete`**, and
        // deliberately: the rule is about the document, not about one operation, so
        // checking it where the document becomes real catches the *next* op that
        // breaks it too. `build::guides_of` stays the thing that makes a delete
        // succeed — it is not an optimisation, it is how the Delete verb carries
        // its guides and how the inverse gets the order right — but a fourth delete
        // site that has not heard of it now gets a refused edit instead of an
        // unopenable document.
        //
        // ⚠️ **After the loop and before `*self = working`, so a transaction is
        // still all-or-nothing.** Checking per op would refuse a transaction that
        // ends up *correct*: `guides_of` puts the `RemoveGuide`s first and the
        // frame is still there when they run, so that order never holds an orphan
        // at all — but the **reverse** order, frame then guides, is legal, is what
        // a caller might write, and passes *through* a state with an orphan in it
        // on its way to a valid document.
        //
        // ⚠️ That is not hypothetical: `undoing_a_frame_delete_restores_its_guides`
        // commits exactly that transaction as its "wrong" order — wrong only for
        // the undo — so a per-op check would have turned an existing green test red
        // for a reason that has nothing to do with it. (This comment said "would
        // refuse the *correct* spelling", which is the sentence inverted; the
        // argument was always about the reverse one.)
        for guide in &working.guides {
            working.check_guide_owner(guide.owner)?;
        }

        // All ops succeeded — commit the working copy.
        *self = working;

        // To undo [a, b, c] applied in order, apply [c⁻¹, b⁻¹, a⁻¹].
        inverse_ops.reverse();
        Ok(ApplyOutcome {
            inverse: Transaction(inverse_ops),
            dirty,
        })
    }

    /// Apply a single op to `self` (already a working copy), returning the op
    /// that would undo it. Validates before mutating.
    fn apply_one(&mut self, op: &Operation, dirty: &mut DirtySet) -> Result<Operation, OpError> {
        match op {
            Operation::CreateNode {
                id,
                parent,
                index,
                kind,
                transform,
                name,
            } => self.op_create(*id, *parent, *index, kind, *transform, name, dirty),
            Operation::DeleteNode { id } => self.op_delete(*id, dirty),
            Operation::InsertSubtree {
                nodes,
                parent,
                index,
            } => self.op_insert_subtree(nodes, *parent, *index, dirty),
            Operation::SetTransform { id, transform } => {
                self.op_set_transform(*id, *transform, dirty)
            }
            Operation::Reparent {
                id,
                new_parent,
                index,
            } => self.op_reparent(*id, *new_parent, *index, dirty),
            Operation::Reorder { id, index } => self.op_reorder(*id, *index, dirty),
            Operation::SetGeometry { id, geometry } => self.op_set_geometry(*id, geometry, dirty),
            Operation::SetText {
                id,
                content,
                spans,
                para_spans,
            } => self.op_set_text(*id, content, spans, para_spans, dirty),
            Operation::SetTextStyle { id, style, spans } => {
                self.op_set_text_style(*id, style, spans.as_ref(), dirty)
            }
            Operation::SetTextSpans { id, spans } => self.op_set_text_spans(*id, spans, dirty),
            Operation::SetParagraphStyle {
                id,
                paragraph,
                spans,
            } => self.op_set_paragraph_style(*id, paragraph, spans.as_ref(), dirty),
            Operation::SetParagraphSpans { id, spans } => {
                self.op_set_paragraph_spans(*id, spans, dirty)
            }
            Operation::SetBlockStyle { id, block } => self.op_set_block_style(*id, *block, dirty),
            Operation::SetName { id, name } => self.op_set_name(*id, name, dirty),
            Operation::SetVisible { id, visible } => self.op_set_visible(*id, *visible, dirty),
            Operation::SetLocked { id, locked } => self.op_set_locked(*id, *locked, dirty),
            Operation::SetProportionsLocked { id, locked } => {
                self.op_set_proportions_locked(*id, *locked, dirty)
            }
            Operation::SetOpacity { id, opacity } => self.op_set_opacity(*id, *opacity, dirty),
            Operation::SetPivot { id, pivot } => self.op_set_pivot(*id, *pivot, dirty),
            Operation::SetClip { id, clip } => self.op_set_clip(*id, *clip, dirty),
            Operation::SetMask { id, mask } => self.op_set_mask(*id, *mask, dirty),
            Operation::SetMaskMode { id, mode } => self.op_set_mask_mode(*id, *mode, dirty),
            Operation::SetFillRule { id, rule } => self.op_set_fill_rule(*id, *rule, dirty),
            Operation::SetFills { id, fills } => self.op_set_fills(*id, fills, dirty),
            Operation::SetStrokes { id, strokes } => self.op_set_strokes(*id, strokes, dirty),
            Operation::SetEffects { id, effects } => self.op_set_effects(*id, effects, dirty),
            Operation::SetExports { id, exports } => self.op_set_exports(*id, exports, dirty),
            Operation::SetLayoutGrids { id, grids } => self.op_set_grids(*id, grids, dirty),
            Operation::SetCanvasBackground { background } => {
                Ok(self.op_set_canvas_background(*background))
            }
            Operation::AddGuide { guide } => self.op_add_guide(*guide),
            Operation::RemoveGuide { id } => self.op_remove_guide(*id),
            Operation::SetGuidePosition { id, position } => {
                self.op_set_guide_position(*id, *position)
            }
            Operation::SetGuideColor { id, color } => self.op_set_guide_color(*id, *color),
            Operation::SetGuideScope {
                id,
                owner,
                position,
            } => self.op_set_guide_scope(*id, *owner, *position),
            Operation::AddImage { id, entry } => self.op_add_image(id.clone(), entry.clone()),
            Operation::RemoveImage { id } => self.op_remove_image(id.clone()),
        }
    }

    // --- operation implementations ---

    #[allow(clippy::too_many_arguments)]
    fn op_create(
        &mut self,
        id: NodeId,
        parent: NodeId,
        index: usize,
        kind: &NodeKind,
        transform: Option<Affine>,
        name: &Option<String>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if self.nodes.contains_key(&id) {
            return Err(OpError::DuplicateId(id));
        }
        if matches!(kind, NodeKind::Root) {
            return Err(OpError::WrongKindForOp);
        }
        // ⚠️ **The check has to be here and not only at the loader**, which is
        // where invariant 8 puts it: past this line the value is in the document
        // and the next autosave writes a file that will never open again. See
        // [`OpError::NonFinite`] and `NodeKind::geometry_is_finite`.
        if !kind.geometry_is_finite() || transform.is_some_and(|t| !affine_is_finite(t)) {
            return Err(OpError::NonFinite);
        }
        let parent_node = self.nodes.get(&parent).ok_or(OpError::NoSuchNode(parent))?;
        if index > parent_node.children.len() {
            return Err(OpError::IndexOutOfRange);
        }
        check_child_kind(&parent_node.kind, kind)?;

        // **`None` means "number me among my new siblings"**, and it is resolved
        // here rather than at the eleven of fifteen production create sites that
        // pass it (§15 D127; §5.7b names the four that pass `Some`). Here because
        // this is the chokepoint: a tool added later passes `None` like every other
        // and is numbered without knowing that numbering exists, where a name
        // computed by each builder would be a rule every future create site has to
        // remember — and the failure of forgetting is silent, a layer called
        // "Rectangle" among three others.
        //
        // The cost is that the op's *result* depends on the document it lands in, so
        // two clients replaying it from diverged states would name the node
        // differently. Invariant 3's determinism is about **ids**, which still ride
        // inside the op; a name is not structural, and a rename conflict is
        // something multiplayer (§12) has to resolve for typed names anyway. That is
        // an inference about invariant 3's spirit rather than something §4 states,
        // and §15 D127 marks it as one.
        let name = name
            .clone()
            .unwrap_or_else(|| crate::naming::created_name(&self.child_names(parent, &[]), kind));

        let node = Node {
            id,
            parent: Some(parent),
            children: Vec::new(),
            kind: kind.clone(),
            transform: transform.unwrap_or(Affine::IDENTITY),
            name,
            visible: true,
            locked: false,
            proportions_locked: false,
            opacity: 1.0,
            // Frames clip by default (Figma's "clip content", on); nothing else
            // has a frame to clip to, so the flag is inert for other kinds.
            clip: kind.clips_children(),
            // Nothing is a mask until someone says so: a layer that started out
            // clipping its siblings would be a shape that draws nothing, which
            // reads as the tool having failed.
            mask: false,
            mask_mode: MaskMode::default(),
            fill_rule: FillRule::default(),
            paint: Paint::default(),
            // A fresh node pivots on its own centre, which is what `None` says.
            pivot: None,
            // Nothing is exportable until someone says it is (§7).
            exports: Vec::new(),
            // And nothing carries an effect until someone adds one (§5.3a).
            effects: Vec::new(),
            // Nor a layout grid, which a frame gets only when a designer asks
            // for one (`crate::layout`).
            grids: Vec::new(),
        };
        self.nodes.insert(id, node);
        self.nodes
            .get_mut(&parent)
            .expect("parent existence checked above")
            .children
            .insert(index, id);
        dirty.0.insert(id);
        dirty.0.insert(parent); // parent's union bounds grow
        Ok(Operation::DeleteNode { id })
    }

    fn op_delete(&mut self, id: NodeId, dirty: &mut DirtySet) -> Result<Operation, OpError> {
        if id == self.root {
            return Err(OpError::CannotModifyRoot);
        }
        let node = self.nodes.get(&id).ok_or(OpError::NoSuchNode(id))?;
        let parent_id = node.parent.ok_or(OpError::CannotModifyRoot)?;

        // Capture the subtree (root first) and its position before removal so
        // the inverse can restore identical ids at the identical place.
        let ids = self.subtree_ids(id);
        let captured: Vec<Node> = ids
            .iter()
            .map(|i| self.nodes[i].clone())
            .collect::<Vec<_>>();
        let index = self.nodes[&parent_id]
            .children
            .iter()
            .position(|c| *c == id)
            .expect("child must be listed in its parent");

        for i in &ids {
            self.nodes.remove(i);
            dirty.0.insert(*i);
        }
        self.nodes
            .get_mut(&parent_id)
            .expect("parent still present")
            .children
            .retain(|c| *c != id);
        dirty.0.insert(parent_id); // parent's union bounds shrink

        Ok(Operation::InsertSubtree {
            nodes: captured,
            parent: parent_id,
            index,
        })
    }

    fn op_insert_subtree(
        &mut self,
        nodes: &[Node],
        parent: NodeId,
        index: usize,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if nodes.is_empty() {
            return Err(OpError::MalformedSubtree);
        }
        let parent_kind = self
            .nodes
            .get(&parent)
            .ok_or(OpError::NoSuchNode(parent))?
            .kind
            .clone();
        if index > self.nodes[&parent].children.len() {
            return Err(OpError::IndexOutOfRange);
        }

        // Ids must be unique within the subtree and not already in the document.
        let set: FxHashSet<NodeId> = nodes.iter().map(|n| n.id).collect();
        if set.len() != nodes.len() {
            return Err(OpError::MalformedSubtree);
        }
        for n in nodes {
            if self.nodes.contains_key(&n.id) {
                return Err(OpError::DuplicateId(n.id));
            }
        }

        // Exactly one node is the subtree root: its parent is outside the set.
        let roots: Vec<&Node> = nodes
            .iter()
            .filter(|n| n.parent.is_none_or(|p| !set.contains(&p)))
            .collect();
        let [root] = roots.as_slice() else {
            return Err(OpError::MalformedSubtree);
        };
        let root_id = root.id;

        // Internal integrity: every referenced child exists in the set, names
        // this node as its parent, and is listed exactly once; every internal
        // parent/child pair obeys the same kind rules `CreateNode` enforces.
        let by_id: FxHashMap<NodeId, &Node> = nodes.iter().map(|n| (n.id, n)).collect();
        let mut claimed: FxHashSet<NodeId> = FxHashSet::default();

        // Per-node invariants first, in their own pass. A subtree may never
        // carry a second document root, and every node must satisfy the field
        // invariants `apply` upholds elsewhere. Checking these before the
        // structural pass is what makes the *reported* error the interesting
        // one: a stray `Root` would otherwise surface as its parent's
        // `InvalidParent`, which says nothing about the real problem.
        for n in nodes {
            if matches!(n.kind, NodeKind::Root) {
                return Err(OpError::WrongKindForOp);
            }
            if !(0.0..=1.0).contains(&n.opacity) {
                return Err(OpError::BadOpacity);
            }
            // The same field invariant `op_create` upholds one door over. This
            // is the door an MCP write tool or an importer would come through,
            // which is precisely the traffic the model cannot vouch for.
            // ⚠️ **The pivot is the third coordinate a node carries and it was
            // not in this list** (§15 D639). `[S2.2-L1-06]` names `SetPivot`;
            // this is the other door the same value arrives by, and the comment
            // above says which traffic that is. A guard at one of two doors
            // leaves the enforcement test's *"at every door that writes one"*
            // false, which is the defect rather than a tidiness.
            // ⚠️ **Paint was the fourth coordinate a node carries and it was not
            // in this list either** (§15 D831). The loop checked geometry, the
            // transform, the pivot and the effects, and never asked
            // `brush_is_finite` — which `op_set_fills` and `op_set_strokes` both
            // ask one door over, for invariant 8's paint half (§15 D451). The
            // comment above says which traffic this door carries, and since
            // `io::clip` that traffic is *OS-clipboard text*: a pasted gradient
            // stop with a `NaN` offset reached the document, and the next
            // autosave wrote a `.ondin` that never opened again.
            if !n.kind.geometry_is_finite()
                || !affine_is_finite(n.transform)
                || n.pivot.is_some_and(|p| !p.is_finite())
                || !n.effects.iter().all(effect_is_finite)
                || !n.paint.fills.iter().all(|f| brush_is_finite(&f.brush))
                || !n.paint.strokes.iter().all(|s| {
                    brush_is_finite(&s.brush)
                        && s.width.is_finite()
                        && s.dashes.iter().all(|d| d.is_finite())
                })
            {
                return Err(OpError::NonFinite);
            }
        }

        for n in nodes {
            for c in &n.children {
                let child = by_id.get(c).ok_or(OpError::MalformedSubtree)?;
                if child.parent != Some(n.id) {
                    return Err(OpError::MalformedSubtree);
                }
                if !claimed.insert(*c) {
                    return Err(OpError::MalformedSubtree);
                }
                check_child_kind(&n.kind, &child.kind)?;
            }
        }

        // ⚠️ **The converse of the loop above, and `claimed` was built and then
        // thrown away without it** (§15 D423). That loop walks *from each node to its listed
        // children*; nothing asked whether every non-root node **is** somebody's
        // child. A node whose `parent` is in the set is not a root by the test at
        // the top, passes the single-root check, gets inserted, and is listed by
        // nobody — an orphan.
        //
        // **Reachable from the public API alone**, which is what makes this worth
        // a line: two `capture_subtree` snapshots of the same node taken at
        // different moments — one before a child existed, one of the child —
        // compose into exactly that set. The result then panics `DeleteNode`,
        // `Reparent` and `Reorder` at their three
        // `expect("child must be listed in its parent")` sites, and saves a file
        // whose load fails with *"node(s) are not reachable from the root"*.
        //
        // The loader has enforced this since it was written — check (5), the
        // reachability walk — so the two doors into the tree were held to
        // different standards, which §5.11 says they must not be. ⚠️ **D423's
        // own closing sentence read *"Every production caller already satisfies
        // it: both feed `remap_subtree` output from a single fresh capture"*,
        // and `io::clip` made it false** — there are three `InsertSubtree` sites
        // now, and the third is OS-clipboard text, which is not a capture and
        // not this process's (§15 D832).
        //
        // 🚨 **And a count is not a reachability check: a cycle balances it**
        // (§15 D832). With `nodes = [R, A, B]`, `A.parent = B`, `B.parent = A`,
        // `A.children = [B]` and `B.children = [A]`, `R` is the single root, the
        // loop above claims `A` and `B` exactly once each, and `2 + 1 == 3`
        // balances — so an `A`/`B` component reachable from nothing was inserted,
        // committed and saved, and the file then failed `io::load` with *"2
        // node(s) are not reachable from the root"*. The arithmetic was standing
        // in for the walk, and the two agree on every input except the one that
        // matters.
        //
        // **So walk it, which is what the loader's check (5) does** — and the
        // depth rides along, which is check (6) and `[R1-L2-02]`: until
        // `io::clip` there was no way to nest without a user click per level, so
        // §15 D416's own conditional (*"fix if a builder, an importer or the MCP
        // write surface ever nests without one"*) had not fired. A 300-deep
        // payload now arrives in one paste.
        //
        // ⚠️ **The bound is absolute, so the parent's own depth is part of it.**
        // A 200-deep subtree pasted into a 200-deep parent reloads no better
        // than a 400-deep one pasted at the root, and a subtree-relative check
        // would have passed it. The upward walk is bounded too: a cycle among
        // the *existing* nodes is an invariant violation rather than an input,
        // but hanging on one would be a worse answer than refusing it.
        let mut parent_depth = 0usize;
        let mut up = self.nodes[&parent].parent;
        while let Some(id) = up {
            parent_depth += 1;
            if parent_depth > MAX_TREE_DEPTH {
                return Err(OpError::TooDeep);
            }
            up = self.nodes[&id].parent;
        }

        let mut visited: FxHashSet<NodeId> = FxHashSet::default();
        visited.insert(root_id);
        let mut stack = vec![(root_id, parent_depth + 1)];
        while let Some((id, depth)) = stack.pop() {
            if depth > MAX_TREE_DEPTH {
                return Err(OpError::TooDeep);
            }
            for c in &by_id[&id].children {
                if visited.insert(*c) {
                    stack.push((*c, depth + 1));
                }
            }
        }
        if visited.len() != nodes.len() {
            return Err(OpError::MalformedSubtree);
        }

        check_child_kind(&parent_kind, &root.kind)?;

        for n in nodes {
            self.nodes.insert(n.id, n.clone());
            dirty.0.insert(n.id);
        }
        // The root attaches to `parent` (a no-op for delete-undo, meaningful for
        // paste/duplicate where the caller retargets it).
        self.nodes.get_mut(&root_id).expect("just inserted").parent = Some(parent);
        self.nodes
            .get_mut(&parent)
            .expect("parent existence checked above")
            .children
            .insert(index, root_id);

        Ok(Operation::DeleteNode { id: root_id })
    }

    fn op_set_transform(
        &mut self,
        id: NodeId,
        transform: Affine,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // ⚠️ **The reachable route into this one is `build::local_for_world`**,
        // which is `parent_world.inverse() * world` with no guard: on a singular
        // parent — `<g transform="matrix(0,0,0,0,0,0)">`, which `svg_in` accepts
        // because it filters for *finite* and not for *invertible* — kurbo
        // divides by a zero determinant and every coefficient comes back `NaN`.
        // Moving the child then wrote that here, and the document was gone. See
        // [`OpError::NonFinite`].
        if !affine_is_finite(transform) {
            return Err(OpError::NonFinite);
        }
        let old = {
            let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
            let old = node.transform;
            node.transform = transform;
            old
        };
        // 🚨 **This node alone, and `Resolved::update` expands it** (§15 D590,
        // `[A4-L4-03]`). A transform change does move everything under this node,
        // and `update`'s `collect_subtree` says so — it walks the subtree of every
        // dirty id for exactly that reason, so listing the descendants here was
        // the same expansion done twice.
        //
        // What the duplication cost is that `update` could no longer tell *why* a
        // node was dirty. Its text pass runs over the dirty ids, and a text
        // layout depends on nothing outside its own node — so a group of twenty
        // 4,000-character blocks nudged with an arrow key re-shaped all twenty
        // per keypress, about 23 ms of parley for a change that cannot move a
        // glyph. `DirtySet` is `Resolved::update`'s only consumer, and the
        // expansion it needs it already performs.
        dirty.0.insert(id);
        Ok(Operation::SetTransform { id, transform: old })
    }

    fn op_reparent(
        &mut self,
        id: NodeId,
        new_parent: NodeId,
        index: usize,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if id == self.root {
            return Err(OpError::CannotModifyRoot);
        }
        let old_parent = self
            .nodes
            .get(&id)
            .ok_or(OpError::NoSuchNode(id))?
            .parent
            .ok_or(OpError::CannotModifyRoot)?;
        if !self.nodes.contains_key(&new_parent) {
            return Err(OpError::NoSuchNode(new_parent));
        }
        // Reparenting a node into itself or one of its descendants would form a
        // cycle. `subtree_ids` includes `id` itself, covering the self case.
        let subtree = self.subtree_ids(id);
        if subtree.contains(&new_parent) {
            return Err(OpError::WouldCycle);
        }
        let child_kind = self.nodes[&id].kind.clone();
        let new_parent_kind = self.nodes[&new_parent].kind.clone();
        check_child_kind(&new_parent_kind, &child_kind)?;

        let old_index = self.nodes[&old_parent]
            .children
            .iter()
            .position(|c| *c == id)
            .expect("child must be listed in its parent");
        self.nodes
            .get_mut(&old_parent)
            .expect("old parent present")
            .children
            .retain(|c| *c != id);

        // Index is validated against the target list as it stands at insertion
        // time (post-removal when reparenting within the same parent). Erroring
        // here is safe: the working copy is discarded on any error.
        let new_len = self.nodes[&new_parent].children.len();
        if index > new_len {
            return Err(OpError::IndexOutOfRange);
        }
        self.nodes.get_mut(&id).expect("node present").parent = Some(new_parent);
        self.nodes
            .get_mut(&new_parent)
            .expect("new parent present")
            .children
            .insert(index, id);

        for d in subtree {
            dirty.0.insert(d);
        }
        // Both parents' union bounds change; the old parent is not an ancestor
        // of the moved node anymore, so it must be listed explicitly.
        dirty.0.insert(old_parent);
        dirty.0.insert(new_parent);
        Ok(Operation::Reparent {
            id,
            new_parent: old_parent,
            index: old_index,
        })
    }

    fn op_reorder(
        &mut self,
        id: NodeId,
        index: usize,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if id == self.root {
            return Err(OpError::CannotModifyRoot);
        }
        let parent = self
            .nodes
            .get(&id)
            .ok_or(OpError::NoSuchNode(id))?
            .parent
            .ok_or(OpError::CannotModifyRoot)?;
        let old_index = self.nodes[&parent]
            .children
            .iter()
            .position(|c| *c == id)
            .expect("child must be listed in its parent");
        self.nodes
            .get_mut(&parent)
            .expect("parent present")
            .children
            .retain(|c| *c != id);
        let len = self.nodes[&parent].children.len();
        if index > len {
            return Err(OpError::IndexOutOfRange);
        }
        self.nodes
            .get_mut(&parent)
            .expect("parent present")
            .children
            .insert(index, id);
        dirty.0.insert(id);
        Ok(Operation::Reorder {
            id,
            index: old_index,
        })
    }

    fn op_set_geometry(
        &mut self,
        id: NodeId,
        patch: &GeometryPatch,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let old = {
            let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
            apply_geometry_patch(&mut node.kind, patch)?
        };
        for d in self.subtree_ids(id) {
            dirty.0.insert(d);
        }
        Ok(Operation::SetGeometry { id, geometry: old })
    }

    fn op_set_text(
        &mut self,
        id: NodeId,
        content: &str,
        spans: &CharSpans,
        para_spans: &ParaSpans,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text {
            content: c,
            spans: sp,
            para_spans: psp,
            style,
            paragraph,
            ..
        } = &mut node.kind
        else {
            return Err(OpError::WrongKindForOp);
        };
        let old_content = std::mem::replace(c, content.to_string());
        // Clamped to the new content, so an op assembled by hand — or replayed
        // out of order — cannot install a span pointing past the end of the
        // string it is meant to describe.
        let old_spans = std::mem::replace(sp, spans.clamped(content, style));
        let old_para_spans = std::mem::replace(psp, para_spans.clamped(content, paragraph));
        dirty.0.insert(id);
        Ok(Operation::SetText {
            id,
            content: old_content,
            spans: old_spans,
            para_spans: old_para_spans,
        })
    }

    fn op_set_text_style(
        &mut self,
        id: NodeId,
        style: &TextStyle,
        spans: Option<&CharSpans>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text {
            content,
            style: s,
            spans: sp,
            ..
        } = &mut node.kind
        else {
            return Err(OpError::WrongKindForOp);
        };
        let old = std::mem::replace(s, Box::new(style.clone()));
        let old_spans = match spans {
            // Re-stated against the new default — `Spans::restated` has why, and the
            // render preview of this op goes through the same function.
            None => {
                let restated = sp.restated(s);
                std::mem::replace(sp, restated)
            }
            // The list as it stood before an earlier re-statement dropped something
            // from it — see the op's own docs. Clamped exactly as `op_set_text_spans`
            // clamps, so an op assembled by hand cannot install a span pointing past
            // the end of the string.
            Some(given) => std::mem::replace(sp, given.clamped(content, s)),
        };
        dirty.0.insert(id);
        Ok(Operation::SetTextStyle {
            id,
            style: *old,
            spans: Some(old_spans),
        })
    }

    fn op_set_text_spans(
        &mut self,
        id: NodeId,
        spans: &CharSpans,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text {
            content,
            style,
            spans: sp,
            ..
        } = &mut node.kind
        else {
            return Err(OpError::WrongKindForOp);
        };
        let old = std::mem::replace(sp, spans.clamped(content, style));
        dirty.0.insert(id);
        Ok(Operation::SetTextSpans { id, spans: old })
    }

    fn op_set_paragraph_style(
        &mut self,
        id: NodeId,
        paragraph: &ParagraphStyle,
        spans: Option<&ParaSpans>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text {
            content,
            paragraph: p,
            para_spans: sp,
            ..
        } = &mut node.kind
        else {
            return Err(OpError::WrongKindForOp);
        };
        let old = std::mem::replace(p, paragraph.clone());
        let old_spans = match spans {
            // Re-stated against the new defaults, exactly as `op_set_text_style` does
            // for the character scope and through the same function.
            None => {
                let restated = sp.restated(p);
                std::mem::replace(sp, restated)
            }
            Some(given) => std::mem::replace(sp, given.clamped(content, p)),
        };
        dirty.0.insert(id);
        Ok(Operation::SetParagraphStyle {
            id,
            paragraph: old,
            spans: Some(old_spans),
        })
    }

    fn op_set_paragraph_spans(
        &mut self,
        id: NodeId,
        spans: &ParaSpans,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text {
            content,
            paragraph,
            para_spans: sp,
            ..
        } = &mut node.kind
        else {
            return Err(OpError::WrongKindForOp);
        };
        let old = std::mem::replace(sp, spans.clamped(content, paragraph));
        dirty.0.insert(id);
        Ok(Operation::SetParagraphSpans { id, spans: old })
    }

    fn op_set_block_style(
        &mut self,
        id: NodeId,
        block: BlockStyle,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let NodeKind::Text { block: b, .. } = &mut node.kind else {
            return Err(OpError::WrongKindForOp);
        };
        let old = std::mem::replace(b, block);
        dirty.0.insert(id);
        Ok(Operation::SetBlockStyle { id, block: old })
    }

    fn op_set_fills(
        &mut self,
        id: NodeId,
        fills: &[Fill],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's paint half (§15 D451). `OpError::NonFinite` used to cover
        // coordinates only, so a `NaN` gradient stop offset — or colour channel —
        // was stored here, written to the file as `null`, and killed the document
        // on the way back in. Refused where the user still has their work.
        if !fills.iter().all(|f| brush_is_finite(&f.brush)) {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        if !is_paintable(&node.kind) {
            return Err(OpError::WrongKindForOp);
        }
        let old = std::mem::replace(&mut node.paint.fills, fills.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetFills { id, fills: old })
    }

    fn op_set_strokes(
        &mut self,
        id: NodeId,
        strokes: &[Stroke],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // The same refusal `op_set_fills` makes, and for the same reason (§15
        // D451) — plus a stroke's own numbers, which a fill has no equivalent of:
        // `stroke-dasharray="1e999 5"` was one of `[S7.2-L5-01]`'s five sinks and
        // the width is written to the same file.
        if !strokes.iter().all(|s| {
            brush_is_finite(&s.brush)
                && s.width.is_finite()
                && s.dashes.iter().all(|d| d.is_finite())
        }) {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        // `is_paintable`, the same gate `op_set_fills` uses. This asked
        // `takes_stroke` while a frame could be stroked and not filled; giving a
        // frame a fill list closed that gap and the second predicate with it
        // (§15 D400).
        if !is_paintable(&node.kind) {
            return Err(OpError::WrongKindForOp);
        }
        let old = std::mem::replace(&mut node.paint.strokes, strokes.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetStrokes { id, strokes: old })
    }

    /// The effect stack (§5.3a).
    ///
    /// **No `is_paintable` gate, and that is the point of the field.** Every
    /// other `op_set_*` on paint guards a list the kind does not carry; effects
    /// are carried by anything that *composites*, which is every kind but the
    /// root — a group has no fill and a blur over one is still the ordinary
    /// thing to want. The root is excluded because it is the document rather
    /// than a layer in it, the same line `op_create` and `op_delete` draw.
    ///
    /// The stack is written whole rather than per entry, so this is also the
    /// reorder, the removal and the visibility toggle. See
    /// [`Operation::SetEffects`].
    fn op_set_effects(
        &mut self,
        id: NodeId,
        effects: &[Effect],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's third family (§15 D641, `[S10.1-L1-05]`). Checked before
        // the node is looked up so the *reported* error is the interesting one,
        // the same order `op_insert_subtree` uses.
        if !effects.iter().all(effect_is_finite) {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        if matches!(node.kind, NodeKind::Root) {
            return Err(OpError::WrongKindForOp);
        }
        let old = std::mem::replace(&mut node.effects, effects.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetEffects { id, effects: old })
    }

    /// The list of files this layer produces (§7).
    ///
    /// **No kind gate.** Every other `op_set_*` that has one is guarding a field
    /// the kind does not carry; this one is on the `Node`, and "can this be
    /// exported" is answered by whether it has bounds rather than by what it is
    /// — a group, a frame and a line are all perfectly ordinary subjects.
    ///
    /// **The node is dirtied even though nothing drawn changes**, which is the
    /// same call `op_set_pivot` makes and for a weaker reason: the canvas does
    /// not read this at all. It is here so the layers panel can mark an
    /// exportable layer without a second notification path, and it costs one
    /// entry in a set on an edit nobody makes in a loop.
    fn op_set_exports(
        &mut self,
        id: NodeId,
        exports: &[ExportSpec],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's export half (§15 D492, `[S8.2-L1-03]`). This was six lines
        // with no bound on anything, and a `.ondin` carrying
        // `"scale": {"Width": 0}` loaded and made `ondin export --all` write
        // **`Button@0w.png` at 1 × 1**: exit 0, no warning, `clamped == false`. The
        // filename asserts a 0-pixel width and the asset is junk a build script
        // will happily copy. `Times(-2.0)` is the same shape — `factor` floors it
        // to `1e-4` and the file is named `@-2x`.
        //
        // **Neither value can come from this app's own UI**, and that is the
        // argument for putting the check here rather than shrugging: both other
        // entry points already refuse exactly this set and say why. So the model
        // was the only door without the guard, which is where a hand-edited file
        // or another tool comes in.
        if !exports.iter().all(export_spec_is_usable) {
            return Err(OpError::BadExportSpec);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.exports, exports.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetExports { id, exports: old })
    }

    /// Replace a layer's layout grids (`crate::layout`), returning the inverse.
    ///
    /// **`op_set_exports`' twin**, down to the dirty mark: nothing drawn changes
    /// — a grid is chrome over the frame — and the node is marked anyway so a
    /// panel reading the tree sees the edit without a second notification path.
    /// ⚠️ **No kind gate.** The list is inert on a layer the app does not offer
    /// grids for, exactly as `exports` is inert on one nobody exports, and
    /// refusing here would make the model's answer depend on which panel asked.
    fn op_set_grids(
        &mut self,
        id: NodeId,
        grids: &[crate::layout::LayoutGrid],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.grids, grids.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetLayoutGrids { id, grids: old })
    }

    /// The ground is not a node, so this dirties nothing: `Resolved` holds
    /// transforms and bounds, and neither depends on what colour is behind the
    /// frames. Infallible for the same reason — there is no id to miss and no
    /// kind to be wrong.
    fn op_set_canvas_background(&mut self, background: Color) -> Operation {
        let old = std::mem::replace(&mut self.canvas_background, background);
        Operation::SetCanvasBackground { background: old }
    }

    // --- guides -----------------------------------------------------------
    //
    // None of the five touch `dirty`. A guide is not drawn by the renderer at
    // all — it is egui chrome over the canvas, like the selection box — so
    // there is no cached bound or scene fragment for one to invalidate.

    fn op_add_guide(&mut self, guide: Guide) -> Result<Operation, OpError> {
        if self.guides.iter().any(|g| g.id == guide.id) {
            return Err(OpError::DuplicateGuide(guide.id));
        }
        // All three writers of a guide's position, not just the one the finding
        // named: this, `op_set_guide_position` and `op_set_guide_scope`. The
        // loader refuses a non-finite one, so any door that does not is a file
        // that saves and will not reopen.
        if !guide.position.is_finite() {
            return Err(OpError::NonFinite);
        }
        self.check_guide_owner(guide.owner)?;
        self.guides.push(guide);
        Ok(Operation::RemoveGuide { id: guide.id })
    }

    /// A guide may only be scoped to a frame that is in the document.
    ///
    /// **The check is what makes op order inside a delete transaction
    /// load-bearing.** `apply` inverts `[a, b]` to `[b⁻¹, a⁻¹}`, so a transaction
    /// that deletes a frame *before* removing its guides would invert to
    /// `[AddGuide, InsertSubtree]` — the guide restored while its owner is still
    /// missing, which this refuses, taking the whole undo with it. Removing the
    /// guides first makes the inverse put the frame back first.
    /// [`crate::build::guides_of`] is where that order is established, and
    /// `undoing_a_frame_delete_restores_its_guides` is what holds it.
    fn check_guide_owner(&self, owner: Option<NodeId>) -> Result<(), OpError> {
        let Some(owner) = owner else { return Ok(()) };
        let node = self
            .nodes
            .get(&owner)
            .ok_or(OpError::BadGuideOwner(owner))?;
        // A frame, not any container: the scope is a *box* to clip the line to
        // and to measure the offset from, and a group has neither. The root is
        // excluded for the same reason it is not a frame — a guide on the canvas
        // is spelled `None`, which is the state this is distinguishing itself
        // from.
        match node.kind() {
            NodeKind::Artboard { .. } => Ok(()),
            _ => Err(OpError::BadGuideOwner(owner)),
        }
    }

    /// Removing a guide inverts to adding *the whole guide back*, not just its
    /// id — its position and colour go with it, so undoing a delete restores
    /// the guide the user had rather than a default one in the same place.
    fn op_remove_guide(&mut self, id: GuideId) -> Result<Operation, OpError> {
        let i = self
            .guides
            .iter()
            .position(|g| g.id == id)
            .ok_or(OpError::NoSuchGuide(id))?;
        Ok(Operation::AddGuide {
            guide: self.guides.remove(i),
        })
    }

    // --- images -------------------------------------------------------------
    //
    // Neither touches `dirty`, for the guides' reason turned around: a guide is
    // not drawn by the renderer, and an image is drawn but changes nothing
    // `Resolved` caches. Bounds come from the shape, not from the picture in it —
    // an image fill cannot move an edge — so there is no transform or box to
    // invalidate. What a new entry does change is what the scene walk *paints*,
    // and the scene is walked afresh every frame.

    fn op_add_image(&mut self, id: ImageId, entry: ImageEntry) -> Result<Operation, OpError> {
        if self.images.contains_key(&id) {
            return Err(OpError::DuplicateImage(id));
        }
        self.images.insert(id.clone(), entry);
        Ok(Operation::RemoveImage { id })
    }

    /// **The removed entry travels in the inverse**, which is the whole reason
    /// this returns one rather than just an id: undoing a purge has to put the
    /// bytes back, and nothing else in the document is holding them.
    fn op_remove_image(&mut self, id: ImageId) -> Result<Operation, OpError> {
        let entry = self
            .images
            .remove(&id)
            .ok_or_else(|| OpError::NoSuchImage(id.clone()))?;
        Ok(Operation::AddImage { id, entry })
    }

    fn op_set_guide_position(&mut self, id: GuideId, position: f64) -> Result<Operation, OpError> {
        // The loader has refused a non-finite guide position since it was
        // written (`io::schema`); the operation that writes one checked nothing,
        // so the asymmetry was a file that saves and will not reopen. §5.5 says
        // the *owner* is enforced "on both `AddGuide` and `SetGuideScope` … and
        // by the loader"; the position half was enforced by the loader alone.
        if !position.is_finite() {
            return Err(OpError::NonFinite);
        }
        let guide = self.guide_mut(id)?;
        let old = std::mem::replace(&mut guide.position, position);
        Ok(Operation::SetGuidePosition { id, position: old })
    }

    fn op_set_guide_color(
        &mut self,
        id: GuideId,
        color: Option<Color>,
    ) -> Result<Operation, OpError> {
        let guide = self.guide_mut(id)?;
        let old = std::mem::replace(&mut guide.color, color);
        Ok(Operation::SetGuideColor { id, color: old })
    }

    /// Rescope a guide, moving its owner and its position together — see
    /// [`Operation::SetGuideScope`] for why they cannot be two operations.
    ///
    /// Inverts to itself carrying the old pair, so a guide dragged into a frame
    /// and undone is back where it was in the space it was in.
    fn op_set_guide_scope(
        &mut self,
        id: GuideId,
        owner: Option<NodeId>,
        position: f64,
    ) -> Result<Operation, OpError> {
        // Validated before the guide is looked up mutably, so a rejected scope
        // leaves nothing half-changed even inside the working copy.
        if !position.is_finite() {
            return Err(OpError::NonFinite);
        }
        self.check_guide_owner(owner)?;
        let guide = self.guide_mut(id)?;
        let old = (
            std::mem::replace(&mut guide.owner, owner),
            std::mem::replace(&mut guide.position, position),
        );
        Ok(Operation::SetGuideScope {
            id,
            owner: old.0,
            position: old.1,
        })
    }

    fn guide_mut(&mut self, id: GuideId) -> Result<&mut Guide, OpError> {
        self.guides
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or(OpError::NoSuchGuide(id))
    }

    fn op_set_name(
        &mut self,
        id: NodeId,
        name: &str,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.name, name.to_string());
        dirty.0.insert(id);
        Ok(Operation::SetName { id, name: old })
    }

    fn op_set_visible(
        &mut self,
        id: NodeId,
        visible: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.visible;
        node.visible = visible;
        dirty.0.insert(id);
        Ok(Operation::SetVisible { id, visible: old })
    }

    fn op_set_locked(
        &mut self,
        id: NodeId,
        locked: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.locked;
        node.locked = locked;
        dirty.0.insert(id);
        Ok(Operation::SetLocked { id, locked: old })
    }

    /// Nothing derived depends on the flag — no bounds, no spatial index, no
    /// paint — so only the node itself is dirtied. It changes what the *next*
    /// resize does, not anything already on screen.
    fn op_set_proportions_locked(
        &mut self,
        id: NodeId,
        locked: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.proportions_locked;
        node.proportions_locked = locked;
        dirty.0.insert(id);
        Ok(Operation::SetProportionsLocked { id, locked: old })
    }

    /// Like the proportion lock, nothing already drawn depends on it: the pivot is
    /// read by the *next* rotate or mirror and baked into the transform that
    /// gesture produces. No bounds move, no paint changes, and the renderer has
    /// never heard of it — only the node itself is dirtied, and only so the canvas
    /// redraws the marker.
    fn op_set_pivot(
        &mut self,
        id: NodeId,
        pivot: Option<Pivot>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8, and the arm `[S2.2-L1-06]` found missing (§15 D639): a
        // pivot is a coordinate, `serde_json` writes a non-finite one as `null`,
        // and the file will not load. See [`Pivot::is_finite`] for why the
        // producers' own extent floors are not this check.
        if pivot.is_some_and(|p| !p.is_finite()) {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.pivot;
        node.pivot = pivot;
        dirty.0.insert(id);
        Ok(Operation::SetPivot { id, pivot: old })
    }

    fn op_set_opacity(
        &mut self,
        id: NodeId,
        opacity: f32,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(OpError::BadOpacity);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.opacity;
        node.opacity = opacity;
        dirty.0.insert(id);
        Ok(Operation::SetOpacity { id, opacity: old })
    }

    /// Turning a clip off lets descendants paint outside the frame, so the
    /// frame's own bounds are not the whole story any more — the subtree is
    /// dirtied, not just the node.
    fn op_set_clip(
        &mut self,
        id: NodeId,
        clip: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.clip;
        node.clip = clip;
        for d in self.subtree_ids(id) {
            dirty.0.insert(d);
        }
        Ok(Operation::SetClip { id, clip: old })
    }

    /// **Refused on a kind that cannot be one**, unlike [`Self::op_set_clip`],
    /// which lets the flag sit inert. The difference is what the flag does: an
    /// inert `clip` is a switch nothing reads, while a mask that "cannot be one"
    /// would still have to be *drawn* somehow — the walk has to decide whether
    /// the layer paints — so a state with no answer must not get into the tree
    /// in the first place.
    ///
    /// Only `id` and its subtree are dirtied. Being masked is a fact about where
    /// a sibling *sits*, so nothing is written to the layers above; what changes
    /// for them is the parent's box, and `Resolved::update` already walks up from
    /// here to recompute it.
    fn op_set_mask(
        &mut self,
        id: NodeId,
        mask: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        if mask && !node.kind.can_mask() {
            return Err(OpError::WrongKindForOp);
        }
        let old = node.mask;
        node.mask = mask;
        for d in self.subtree_ids(id) {
            dirty.0.insert(d);
        }
        Ok(Operation::SetMask { id, mask: old })
    }

    /// **No kind gate and no mask gate.** The mode is inert on a layer that is
    /// not a mask and is *kept* there on purpose — see [`Node::mask_mode`] — so
    /// there is no invalid state for this to refuse. The subtree is dirtied like
    /// `op_set_mask`'s, because switching between a clip and an alpha composite
    /// changes how everything above the mask is drawn.
    fn op_set_mask_mode(
        &mut self,
        id: NodeId,
        mode: MaskMode,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.mask_mode;
        node.mask_mode = mode;
        for d in self.subtree_ids(id) {
            dirty.0.insert(d);
        }
        Ok(Operation::SetMaskMode { id, mode: old })
    }

    /// Set how `id`'s geometry decides its interior (§15 D239).
    ///
    /// **The node alone, not the subtree**, which is where this parts company with
    /// `op_set_mask_mode` above: a mask changes how everything above it composites,
    /// while a fill rule changes only the shape it is on. A container's children are
    /// unaffected — the rule is not inherited, because a group has no geometry for it
    /// to mean anything about.
    ///
    /// Accepts any kind for the same reason `op_set_mask_mode` does: there is no
    /// invalid state to refuse. On a kind with no outline it is inert and remembered,
    /// exactly as `clip` is on a kind with no frame.
    fn op_set_fill_rule(
        &mut self,
        id: NodeId,
        rule: FillRule,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = node.fill_rule;
        node.fill_rule = rule;
        dirty.0.insert(id);
        Ok(Operation::SetFillRule { id, rule: old })
    }

    // --- helpers ---

    /// Deep-clone the subtree rooted at `id` (root first), for copy/duplicate.
    /// Returns `None` for the document root or an absent id. The captured nodes
    /// still carry their original ids/parent; feed them through [`remap_subtree`]
    /// before re-inserting so ids don't collide.
    pub fn capture_subtree(&self, id: NodeId) -> Option<Vec<Node>> {
        if id == self.root || !self.nodes.contains_key(&id) {
            return None;
        }
        Some(
            self.subtree_ids(id)
                .iter()
                .map(|i| self.nodes[i].clone())
                .collect(),
        )
    }

    /// Ids of the subtree rooted at `root`, root first (preorder). If `root` is
    /// absent, returns just `[root]`. Safe on any document `apply`/IO produced:
    /// both enforce that each node is listed as a child exactly once, so the
    /// walk cannot revisit a node.
    fn subtree_ids(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Some(node) = self.nodes.get(&id) {
                // Reverse so children are visited in their stored order.
                for child in node.children.iter().rev() {
                    stack.push(*child);
                }
            }
        }
        out
    }
}

/// Advance `ids` past every id in `doc` that shares its actor, so nothing it
/// mints afterwards can collide with a node already present.
///
/// A session that opens a document it did not create (or one saved by an
/// earlier session with the same actor) would otherwise re-mint ids the file
/// already uses, and every `CreateNode` would fail with `DuplicateId`. Call
/// this whenever a document is loaded into an existing session.
pub fn reserve_existing_ids(doc: &Document, ids: &mut IdSource) {
    let actor = ids.actor();
    // Guides are minted from the same stream (`crate::guide::GuideId`), so they
    // have to be reserved past as well — otherwise a file whose highest id
    // belongs to a guide hands the next `AddGuide` a duplicate.
    let highest = doc
        .nodes
        .keys()
        .copied()
        .chain(doc.guides.iter().map(|g| g.id.0))
        .filter(|id| id.actor == actor)
        .map(|id| id.seq)
        .max();
    if let Some(seq) = highest {
        ids.skip_to(seq + 1);
    }
}

/// Clone a captured subtree with fresh ids minted from `ids`, rewriting every
/// `id`/`parent`/`children` reference so the result is ready for
/// [`Operation::InsertSubtree`]. Returns `(nodes, new_root_id)`, or `None` if the
/// template is empty, malformed (a child id missing from the set), or lacks a
/// single root. The root keeps its original external `parent` link; the insert
/// op retargets it to the destination the caller chooses.
pub fn remap_subtree(template: &[Node], ids: &mut IdSource) -> Option<(Vec<Node>, NodeId)> {
    if template.is_empty() {
        return None;
    }
    let set: FxHashSet<NodeId> = template.iter().map(|n| n.id).collect();
    let map: FxHashMap<NodeId, NodeId> = template.iter().map(|n| (n.id, ids.mint())).collect();

    let mut new_root = None;
    let mut out = Vec::with_capacity(template.len());
    for n in template {
        let new_id = map[&n.id];
        let is_root = n.parent.is_none_or(|p| !set.contains(&p));
        let new_parent = if is_root {
            if new_root.is_some() {
                return None; // more than one root — malformed
            }
            new_root = Some(new_id);
            n.parent // external parent; InsertSubtree overwrites it
        } else {
            Some(map[&n.parent.unwrap()])
        };
        let mut new_children = Vec::with_capacity(n.children.len());
        for c in &n.children {
            new_children.push(*map.get(c)?);
        }
        out.push(Node {
            id: new_id,
            parent: new_parent,
            children: new_children,
            kind: n.kind.clone(),
            transform: n.transform,
            name: n.name.clone(),
            visible: n.visible,
            locked: n.locked,
            proportions_locked: n.proportions_locked,
            opacity: n.opacity,
            clip: n.clip,
            // A copy of a mask is a mask. It lands beside its original in the
            // same parent, so the duplicate starts a second run at the index it
            // was inserted at — which is the rule doing exactly what it says
            // rather than a case needing special handling.
            mask: n.mask,
            mask_mode: n.mask_mode,
            fill_rule: n.fill_rule,
            paint: n.paint.clone(),
            pivot: n.pivot,
            // A copy exports the way its original did — duplicating an icon that
            // is set up to write 1× and 2× PNGs and getting one that writes
            // nothing would be the copy quietly dropping the part that made it
            // worth duplicating. The two then name the same files, which is
            // `plan`'s problem and is why nothing there overwrites silently.
            exports: n.exports.clone(),
            // Same reasoning one field up, and with none of its caveat: a copy
            // keeps the shadow it was drawn with, and two layers carrying the
            // same effect stack collide over nothing.
            effects: n.effects.clone(),
            // And a copy of a frame is drawn against the same columns. The grid
            // is measured against the *copy's* size, so a duplicate that is then
            // resized re-flows its own tracks and does not inherit the original's
            // (`crate::layout::tracks` takes the size and stores nothing).
            grids: n.grids.clone(),
        });
    }
    Some((out, new_root?))
}

/// Enforce the Root/Artboard structural rules (§5.3): an Artboard hangs off the
/// root or off another Artboard, and nothing may parent a second Root.
///
/// Shared with `io::schema` so a loaded file is held to exactly the same
/// structural rules as one built through operations.
pub(crate) fn check_child_kind(
    parent_kind: &NodeKind,
    child_kind: &NodeKind,
) -> Result<(), OpError> {
    if crate::build::can_parent(parent_kind, child_kind) {
        return Ok(());
    }
    // Same rule, two error names, because the two ways of breaking it read
    // very differently to whoever hit it.
    if matches!(child_kind, NodeKind::Artboard { .. }) {
        Err(OpError::ArtboardPlacement)
    } else {
        Err(OpError::InvalidParent)
    }
}

/// Whether a paint op may target this kind — [`NodeKind::takes_paint`], and
/// nothing else.
///
/// ⚠️ **A second summary line used to sit above this one**, left behind by an
/// earlier rewrite and describing the opposite of what the function does now:
/// "Root/Group/Artboard are not (§5.3 — Artboard uses its own background)". Both
/// halves of that had gone stale — a frame is paintable (§15 D400) and there is
/// no background field to use — and rustdoc rendered it as the summary, so the
/// item's own first line was never what a reader saw.
///
/// **It used to be a second copy of that list, and the two drifted the first time
/// a kind was added** — which is what `takes_paint`'s own doc comment predicted
/// would happen. Adding `Boolean` to one of them left `SetFills` rejecting the very
/// transaction `build::boolean` emits, with `WrongKindForOp` and nothing to say
/// where the disagreement was. One list now, so a new kind cannot be paintable to
/// the panels and unpaintable to the op layer (§15).
fn is_paintable(kind: &NodeKind) -> bool {
    kind.takes_paint()
}

/// Whether an export spec carries values that can be a size and a quality
/// (§15 D492).
///
/// **Deliberately the same predicate `ExportScale::from_label` applies**, which is
/// the parser the panel's own field goes through: `> 0` for a pixel count, finite
/// and positive for a multiplier. Written here rather than reached for because
/// `from_label` answers `Option<ExportScale>` from a *string* and this asks about a
/// value that is already one — the same rule, two questions — and duplicating three
/// comparisons is cheaper than a conversion that would have to invent a label.
///
/// `quality` is `1..=100` because that is what its own doc says it is and what the
/// JPEG encoder takes. `0` is not "worst quality", it is out of range.
pub(crate) fn export_spec_is_usable(spec: &ExportSpec) -> bool {
    let scale = match spec.scale {
        crate::export::ExportScale::Times(n) => n.is_finite() && n > 0.0,
        crate::export::ExportScale::Width(px) | crate::export::ExportScale::Height(px) => px > 0,
    };
    scale && (1..=100).contains(&spec.quality)
}

/// Apply a geometry patch to a node kind in place, returning the patch that
/// reverses it. Errors if the patch does not match the node kind (§5.6).
pub(crate) fn apply_geometry_patch(
    kind: &mut NodeKind,
    patch: &GeometryPatch,
) -> Result<GeometryPatch, OpError> {
    use GeometryPatch as G;
    // ⚠️ **Before the fourteen `mem::replace` arms, not inside them.** Not one of
    // those arms looked at the value it was writing, so `SetGeometry` was the
    // widest door of the three onto the unopenable-document failure — and the
    // widest because `svg_in`'s `numbers()` and `BezPath::from_svg` do not filter
    // for finite the way `parse_len` does. See [`OpError::NonFinite`].
    if !patch.is_finite() {
        return Err(OpError::NonFinite);
    }
    match (kind, patch) {
        (NodeKind::Rect { size, .. }, G::Size(new))
        | (NodeKind::Ellipse { size }, G::Size(new))
        | (NodeKind::Polygon { size, .. }, G::Size(new))
        | (NodeKind::Star { size, .. }, G::Size(new))
        | (NodeKind::Artboard { size, .. }, G::Size(new)) => {
            Ok(G::Size(std::mem::replace(size, *new)))
        }
        (NodeKind::Polygon { sides, .. }, G::Sides(new))
        | (NodeKind::Star { points: sides, .. }, G::Sides(new)) => {
            Ok(G::Sides(std::mem::replace(sides, *new)))
        }
        (NodeKind::Star { inner_ratio, .. }, G::InnerRatio(new)) => {
            Ok(G::InnerRatio(std::mem::replace(inner_ratio, *new)))
        }
        // Both corner patches invert to `CornerRadii`: a uniform set flattens
        // four values into one, so only the per-corner form can put them back.
        (NodeKind::Rect { corner_radii, .. }, G::CornerRadius(new)) => Ok(G::CornerRadii(
            std::mem::replace(corner_radii, RoundedRectRadii::from_single_radius(*new)),
        )),
        (NodeKind::Rect { corner_radii, .. }, G::CornerRadii(new)) => {
            Ok(G::CornerRadii(std::mem::replace(corner_radii, *new)))
        }
        (NodeKind::Line { end }, G::LineEnd(new)) => Ok(G::LineEnd(std::mem::replace(end, *new))),
        (
            NodeKind::Path {
                path,
                corner_radii: radii,
            },
            G::Path {
                path: new,
                corner_radii: new_radii,
            },
        ) => Ok(G::Path {
            path: std::mem::replace(path, new.clone()),
            corner_radii: std::mem::replace(radii, new_radii.clone()),
        }),
        (NodeKind::Text { sizing, .. }, G::TextSizing(new)) => {
            Ok(G::TextSizing(std::mem::replace(sizing, *new)))
        }
        // The rail is boxed in the model and not in the patch: a patch is built
        // once per edit where the field is read on every layout, so the
        // indirection is worth paying there and not here.
        (NodeKind::Text { on_path, .. }, G::TextPath(new)) => Ok(G::TextPath(
            std::mem::replace(on_path, new.clone().map(Box::new)).map(|b| *b),
        )),
        (NodeKind::Text { on_path_flip, .. }, G::TextPathFlip(new)) => {
            Ok(G::TextPathFlip(std::mem::replace(on_path_flip, *new)))
        }
        (NodeKind::Text { on_path_offset, .. }, G::TextPathOffset(new)) => {
            Ok(G::TextPathOffset(std::mem::replace(on_path_offset, *new)))
        }
        // Switching a boolean's operation is a geometry edit like any other, and
        // inverts to the operation it replaced — so undo after a switch puts the
        // previous one back rather than dissolving the container.
        (NodeKind::Boolean { op }, G::BoolOp(new)) => Ok(G::BoolOp(std::mem::replace(op, *new))),
        _ => Err(OpError::WrongKindForOp),
    }
}

#[cfg(test)]
mod tests {
    //! `InsertSubtree` validation. These live in-crate because they build a
    //! `Node` by struct literal — `pub(crate)` fields, no constructor — to
    //! synthesize the malformed subtrees that the public API (capture + remap)
    //! can never produce, which is exactly the shape an MCP write tool or an
    //! importer could hand us.

    use super::*;
    use kurbo::{Point, Size, Vec2};

    fn base() -> (Document, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        }]))
        .expect("seed");
        (doc, root, ab)
    }

    fn node(id: NodeId, parent: Option<NodeId>, children: Vec<NodeId>, kind: NodeKind) -> Node {
        Node {
            id,
            parent,
            children,
            kind,
            transform: Affine::IDENTITY,
            name: "n".into(),
            visible: true,
            locked: false,
            proportions_locked: false,
            opacity: 1.0,
            clip: false,
            mask: false,
            mask_mode: MaskMode::default(),
            fill_rule: FillRule::default(),
            paint: Paint::default(),
            pivot: None,
            exports: Vec::new(),
            effects: Vec::new(),
            grids: Vec::new(),
        }
    }

    fn group(id: NodeId, parent: Option<NodeId>, children: Vec<NodeId>) -> Node {
        node(id, parent, children, NodeKind::Group)
    }

    fn insert(doc: &mut Document, nodes: Vec<Node>, parent: NodeId) -> Result<(), OpError> {
        let index = doc.get(parent).unwrap().children().len();
        doc.apply(&Transaction(vec![Operation::InsertSubtree {
            nodes,
            parent,
            index,
        }]))
        .map(|_| ())
    }

    /// A frame may sit in a frame, but never in a group — and the check has to reach
    /// *inside* an inserted subtree to see it, not just look at the root being
    /// attached (which here is a perfectly legal group).
    #[test]
    fn rejects_an_artboard_under_a_group_inside_the_subtree() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xAA);
        let (g, inner) = (ids.mint(), ids.mint());
        let nodes = vec![
            group(g, None, vec![inner]),
            node(
                inner,
                Some(g),
                vec![],
                NodeKind::Artboard {
                    size: Size::new(10.0, 10.0),
                },
            ),
        ];
        assert!(matches!(
            insert(&mut doc, nodes, ab),
            Err(OpError::ArtboardPlacement)
        ));
    }

    #[test]
    fn rejects_a_root_kind_anywhere_in_the_subtree() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xBB);
        let (g, inner) = (ids.mint(), ids.mint());
        let nodes = vec![
            group(g, None, vec![inner]),
            node(inner, Some(g), vec![], NodeKind::Root),
        ];
        assert!(matches!(
            insert(&mut doc, nodes, ab),
            Err(OpError::WrongKindForOp)
        ));
    }

    #[test]
    fn rejects_a_child_listed_twice() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xCC);
        let (g, inner) = (ids.mint(), ids.mint());
        let nodes = vec![
            group(g, None, vec![inner, inner]),
            group(inner, Some(g), vec![]),
        ];
        assert!(matches!(
            insert(&mut doc, nodes, ab),
            Err(OpError::MalformedSubtree)
        ));
    }

    #[test]
    fn rejects_out_of_range_opacity() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xDD);
        let g = ids.mint();
        let mut n = group(g, None, vec![]);
        n.opacity = 2.0;
        assert!(matches!(
            insert(&mut doc, vec![n], ab),
            Err(OpError::BadOpacity)
        ));
    }

    /// The pivot is a coordinate and this door checked the other two (§15 D639).
    ///
    /// `[S2.2-L1-06]` names `SetPivot`, where the same value saves as `null` and
    /// kills the file on reload. This is the second door it arrives by, and the
    /// reason the arm is here rather than in `tests/io.rs` is the module doc
    /// above: outside the crate a `Vec<Node>` can only come from
    /// `capture_subtree`, i.e. from a document the operation guards have already
    /// vetted.
    ///
    /// 🚨 **This doc used to end *"so no external route reaches this check"*, and
    /// §15 D639 carried the same sentence. `io::clip` is that route** (§15 D832)
    /// — a paste is OS-clipboard text, which no operation guard has seen and
    /// which any web page's *Copy* button can write. The guard is no longer
    /// *"kept for the traffic the model cannot vouch for"*; it is load-bearing on
    /// a route that exists, and the three arms beside it (paint, reachability,
    /// depth) are there because the same premise had excused their absence.
    ///
    /// Both variants, because they are different types: `Normalized` holds a
    /// `Vec2` and `Local` a `Point`, and a guard written against one compiles
    /// happily while missing the other. That is why the predicate lives on
    /// `Pivot` and not at the call site.
    ///
    /// Two flips, both run. Blinding the clause in `op_insert_subtree`'s field
    /// loop fails at *"Normalized(Vec2 { x: 0.5, y: NaN }) was inserted"*, the
    /// predicted site. And blinding only `Pivot::is_finite`'s `Local` arm —
    /// the predicate somebody writes if they read the enum as one shape — leaves
    /// that first assertion **green** and fails at *"Local((inf, 0.0)) was
    /// inserted"*, which is what says the second half of this loop is carrying
    /// its own weight rather than restating the first.
    #[test]
    fn insert_subtree_refuses_a_non_finite_pivot() {
        for bad in [
            Pivot::Normalized(Vec2::new(0.5, f64::NAN)),
            Pivot::Local(Point::new(f64::INFINITY, 0.0)),
        ] {
            let (mut doc, _root, ab) = base();
            let mut ids = IdSource::new(0xDE);
            let g = ids.mint();
            let mut n = group(g, None, vec![]);
            n.pivot = Some(bad);
            assert!(
                matches!(insert(&mut doc, vec![n], ab), Err(OpError::NonFinite)),
                "{bad:?} was inserted"
            );
            assert!(!doc.contains(g), "{bad:?} left the node behind");
        }
    }

    /// The pivot arm's twin for effects (§15 D641, `[S10.1-L1-05]`).
    ///
    /// Same door, same reason it is in-crate, and the value is the one that does
    /// damage before the file is ever written: an infinite blur radius makes
    /// `effect::escaped` report **no** escape at all.
    ///
    /// Flip: the `|| !n.effects.iter().all(effect_is_finite)` clause removed.
    /// Red here, and **green across the rest of the workspace** — which is what
    /// says `op_set_effects`' own guard does not cover this door.
    #[test]
    fn insert_subtree_refuses_a_non_finite_effect() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xEF);
        let g = ids.mint();
        let mut n = group(g, None, vec![]);
        n.effects = vec![crate::Effect::new(crate::EffectKind::LayerBlur {
            radius: f64::INFINITY,
        })];
        assert!(matches!(
            insert(&mut doc, vec![n], ab),
            Err(OpError::NonFinite)
        ));
        assert!(!doc.contains(g));
    }

    /// The pivot arm's twin for **paint**, the fourth family (§15 D831).
    ///
    /// `op_set_fills` and `op_set_strokes` have asked `build::brush_is_finite`
    /// since §15 D451; this door never did, so the one field family the loader
    /// writes as `null` could still arrive through it. Until `io::clip` that was
    /// a guard against traffic nobody could produce; a paste is OS-clipboard
    /// text, and any web page's *Copy* button can write it.
    ///
    /// **Four values, because they are four different clauses**, and a guard
    /// written by reading the word "colour" covers only the first: a fill's
    /// solid colour, a stroke's brush, a stroke's `width`, and a dash length.
    /// The stroke trio is why this arm restates `op_set_strokes`' predicate
    /// rather than calling `brush_is_finite` alone — a stroke carries numbers a
    /// fill has no equivalent of.
    ///
    /// Flips, all four run against the clause that catches them. Removing the
    /// fills clause fails at *"fill was inserted"* and leaves the other three
    /// green; removing the strokes clause fails at the remaining three. ⚠️ The
    /// interesting one is `width`: deleting `s.width.is_finite()` alone leaves
    /// *both* brush assertions green, which is what says the three stroke
    /// sub-clauses are carrying their own weight rather than restating each
    /// other.
    #[test]
    fn insert_subtree_refuses_non_finite_paint() {
        let bad = peniko::Color::new([f32::NAN, 0.0, 0.0, 1.0]);
        let cases: Vec<(&str, Paint)> = vec![
            (
                "fill",
                Paint {
                    fills: vec![Fill {
                        brush: crate::Brush::Solid(bad),
                        visible: true,
                    }],
                    ..Paint::default()
                },
            ),
            (
                "stroke brush",
                Paint {
                    strokes: vec![Stroke {
                        brush: crate::Brush::Solid(bad),
                        ..Stroke::default()
                    }],
                    ..Paint::default()
                },
            ),
            (
                "stroke width",
                Paint {
                    strokes: vec![Stroke {
                        width: f64::INFINITY,
                        ..Stroke::default()
                    }],
                    ..Paint::default()
                },
            ),
            (
                "dash length",
                Paint {
                    strokes: vec![Stroke {
                        dashes: vec![4.0, f64::NAN],
                        ..Stroke::default()
                    }],
                    ..Paint::default()
                },
            ),
            (
                // 🚨 **The only one of the five a clipboard payload can actually
                // carry**, and the reason it is here rather than left to the
                // four above. `io::schema`'s own note says no JSON input can
                // produce a non-finite `f64` — `serde_json` writes `NaN` as
                // `null`, refuses `null` where an `f64` is wanted and refuses
                // `1e999` outright — so every `NaN` case above dies at
                // `from_str` one function before this door. A gradient's
                // `opacity` is checked with `valid_opacity` and not
                // `is_finite` (§15 D767), which makes `1e30` finite, writable,
                // readable, and refused only here.
                //
                // ⚠️ **`1e30` and not the `1e39` the finding's headline used**:
                // the field is an `f32`, whose maximum is ~`3.4e38`, so `1e39`
                // is a compile error here and would arrive from JSON as `inf` —
                // caught, but by the finiteness half rather than by the range
                // half, which is the clause this case exists to pin.
                "gradient opacity",
                Paint {
                    fills: vec![Fill {
                        brush: crate::Brush::Gradient({
                            let mut g =
                                crate::image::GradientBrush::from(peniko::Gradient::default());
                            g.opacity = 1e30;
                            g
                        }),
                        visible: true,
                    }],
                    ..Paint::default()
                },
            ),
        ];
        for (name, paint) in cases {
            let (mut doc, _root, ab) = base();
            let mut ids = IdSource::new(0xFA);
            let g = ids.mint();
            let mut n = node(
                g,
                None,
                vec![],
                NodeKind::Rect {
                    size: Size::new(5.0, 5.0),
                    corner_radii: RoundedRectRadii::default(),
                },
            );
            n.paint = paint;
            assert!(
                matches!(insert(&mut doc, vec![n], ab), Err(OpError::NonFinite)),
                "{name} was inserted"
            );
            assert!(!doc.contains(g), "{name} left the node behind");
        }
    }

    /// A cycle balances the count this door used to make (§15 D832).
    ///
    /// `[R, A, B]` with `A.parent = B`, `B.parent = A`, `A.children = [B]` and
    /// `B.children = [A]`: `R` is the single root, the child loop claims `A` and
    /// `B` exactly once each, and `claimed.len() + 1 == nodes.len()` — `2 + 1 ==
    /// 3` — balanced. So the `A`/`B` component went in reachable from nothing,
    /// committed, and saved a file that `io::load` refused with *"2 node(s) are
    /// not reachable from the root"*. The arithmetic agreed with the walk on
    /// every input except this one.
    ///
    /// ⚠️ **The fixture is built to pass every other check in the function**,
    /// which is the point: single root, unique ids, every child listed once,
    /// every parent back-pointer consistent, every kind legal. Weaken any of
    /// those and the test passes for the wrong reason — it would be measuring
    /// `MalformedSubtree` arriving from a different arm.
    ///
    /// Flip: the walk replaced by the old `claimed.len() + 1 != nodes.len()`.
    /// Red here at the predicted site, and — measured with `--no-fail-fast`
    /// rather than assumed — **green across every other target in
    /// `ondin-core`**: 459 lib tests, `build`, `bulk`, `io`, `ops`, `resolve`
    /// and `spine` all pass while a cycle goes into the document. That is what
    /// says nothing else was covering this, and it is the reason the arm is
    /// here rather than left to the loader, which catches it one save too late.
    #[test]
    fn insert_subtree_refuses_a_cycle_that_balances_the_count() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xC1);
        let (r, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let nodes = vec![
            group(r, None, vec![]),
            group(a, Some(b), vec![b]),
            group(b, Some(a), vec![a]),
        ];
        assert!(
            matches!(insert(&mut doc, nodes, ab), Err(OpError::MalformedSubtree)),
            "a cycle was inserted"
        );
        for id in [r, a, b] {
            assert!(!doc.contains(id), "{id:?} was left behind");
        }
    }

    /// The depth bound §15 D416's own conditional asked for (§15 D832).
    ///
    /// D416 left this door unbounded on the argument that *"nothing nests
    /// without a user click per level"*, and named the condition that would end
    /// it: *"fix if a builder, an importer or the MCP write surface ever nests
    /// without one"*. `io::clip` is that importer — a 300-deep chain arrives in
    /// one paste — and the saved document then failed the loader's check (6)
    /// forever, which is the shape where the user's in-memory session is the
    /// only copy of their work.
    ///
    /// 🚨 **The second case is the one a subtree-relative check gets wrong**, and
    /// it is why the bound reads the parent's depth. Two chains, each
    /// comfortably inside `MAX_TREE_DEPTH` on its own, compose to a document
    /// that is not: the first goes in and the second is refused *because of
    /// where it lands*. A guard measuring only the incoming subtree passes both
    /// and writes the same unreloadable file.
    ///
    /// Flip: `parent_depth` pinned to `0`. The first two assertions stay green
    /// and the third fails — the predicted site, and the reason the third case
    /// is here at all.
    #[test]
    fn insert_subtree_refuses_a_subtree_that_would_nest_too_deep() {
        // A flat chain of `len` groups: the first is the subtree root.
        fn chain(ids: &mut IdSource, len: usize) -> Vec<Node> {
            let minted: Vec<NodeId> = (0..len).map(|_| ids.mint()).collect();
            minted
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    group(
                        *id,
                        i.checked_sub(1).map(|p| minted[p]),
                        minted.get(i + 1).copied().into_iter().collect(),
                    )
                })
                .collect()
        }

        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xD0);
        assert!(
            matches!(
                insert(&mut doc, chain(&mut ids, MAX_TREE_DEPTH + 8), ab),
                Err(OpError::TooDeep)
            ),
            "a chain past the bound was inserted"
        );

        // Half the bound fits under an artboard at depth 1, twice over it does not.
        let half = MAX_TREE_DEPTH / 2;
        let first = chain(&mut ids, half);
        let tail = first.last().expect("half is non-empty").id;
        insert(&mut doc, first, ab).expect("half the bound fits");
        assert!(
            matches!(
                insert(&mut doc, chain(&mut ids, half), tail),
                Err(OpError::TooDeep)
            ),
            "two halves composed past the bound"
        );
    }

    #[test]
    fn a_valid_subtree_still_inserts() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xEE);
        let (g, inner) = (ids.mint(), ids.mint());
        let nodes = vec![
            group(g, None, vec![inner]),
            node(
                inner,
                Some(g),
                vec![],
                NodeKind::Rect {
                    size: Size::new(5.0, 5.0),
                    corner_radii: RoundedRectRadii::default(),
                },
            ),
        ];
        insert(&mut doc, nodes, ab).expect("well-formed subtree inserts");
        assert!(doc.contains(g) && doc.contains(inner));
    }
}
