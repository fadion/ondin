//! Document — the single source of truth (§5.5, invariants 2 & 4).
//!
//! The only way to mutate is `apply(Transaction)`. There is no public `&mut`
//! access to nodes or the tree.

use crate::build::{affine_is_finite, effect_is_finite};
use crate::effect::Effect;
use crate::export::ExportSpec;
use crate::guide::{Guide, GuideId};
use crate::id::{IdSource, NodeId};
use crate::image::{ImageEntry, ImageId};
use crate::io::{CURRENT_SCHEMA_VERSION, MAX_TREE_DEPTH};
use crate::item::Keyed;
use crate::meta::DocumentMeta;
use crate::node::{
    BlockStyle, CharSpans, Fill, FillRule, MaskMode, Node, NodeKind, Paint, ParaSpans,
    ParagraphStyle, Pivot, Stroke, TextStyle,
};
use crate::op::{
    ApplyOutcome, Described, DirtySet, GeometryPatch, OpError, Operation, Transaction,
};
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
    /// The **vault** — a second, parentless `Root` holding the kit mains this
    /// document has vendored (§5.3e, `crate::kit`).
    ///
    /// Its nodes are in [`Self::nodes`], so passes that ask by id see them, but
    /// outside the tree under [`Self::root`], so every walk from the root skips
    /// them. Opened and closed by [`Operation::OpenVault`] and
    /// [`Operation::CloseVault`], so undo takes it away with the first kit insert.
    vault: Option<NodeId>,
    /// The ground behind and around the frames.
    ///
    /// The one piece of document state that is not a node: it is set for the
    /// artwork, not the sitting, so it survives reopening the file. Opaque by
    /// construction — nothing behind the canvas could show through.
    canvas_background: Color,
    /// The ruler guides placed on this document (`crate::guide`).
    ///
    /// Unordered — guides have no z and no operation refers to one by position —
    /// so `apply` appends and IO sorts by id for byte-stable output (invariant 9).
    /// A `Vec` because every reader wants all of them and a document has a handful.
    guides: Vec<Guide>,
    /// The images this document carries, keyed by the id a fill refers to
    /// (`crate::image`).
    ///
    /// A map, where guides are a `Vec`, because every reader wants exactly one
    /// (the render walk resolves a fill's reference per frame). IO sorts by id.
    ///
    /// **Nothing collects it.** Entries are added and removed by explicit
    /// operations only: deleting the one layer showing a photograph and undoing
    /// has to bring the bytes back. A document can therefore carry an image
    /// nothing shows, which is a *Purge unused* command rather than a bug.
    images: FxHashMap<ImageId, ImageEntry>,
    /// The document's variables and its one mode list (§5.3f, `crate::variable`)
    /// — state that is not a node, checked after every transaction.
    variables: crate::variable::Variables,
    /// The document's styles (§5.3f, `crate::style`), checked after every
    /// transaction. Its values are copied onto the layers bound to it, so nothing
    /// that draws reads it.
    styles: crate::style::Styles,
    /// The document's library identity — typed name, project, creation stamp
    /// (`crate::meta`).
    ///
    /// **The one field here that no `Operation` can reach.** The library writes
    /// it, so it is outside undo on purpose: a rename typed into the dashboard,
    /// possibly while this document is not open, must not turn up in its editing
    /// history.
    meta: DocumentMeta,
    /// What this document last **published** as a kit (§5.3e) — a frozen copy of
    /// its mains and sets, which consumers vendor from instead of the working
    /// document.
    ///
    /// Outside undo for `meta`'s reason: an undo cannot take a publish back from
    /// documents that already read it. Behind an `Arc` because `apply` clones the
    /// document per transaction. Not derived state under invariant 4: it is the
    /// tree as it was at the publish.
    published: Option<std::sync::Arc<crate::kit::Published>>,
    /// **True only while [`Self::apply_restoring`] runs** — a history step putting
    /// back values this document already held, which the layout operations then
    /// take whether or not CSS accepts them. Never true between calls.
    restoring: bool,
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
            insets: Default::default(),
            display: None,
            item: Default::default(),
            component: false,
            link: None,
            swap: None,
            set: None,
            variant: Vec::new(),
            props: Vec::new(),
            description: String::new(),
            origin: None,
            mode: None,
            corner_vars: Default::default(),
            spacing_vars: Default::default(),
            visible_var: None,
            size_vars: Default::default(),
            text_vars: Default::default(),
            opacity_var: None,
            styles: Default::default(),
        };
        let mut nodes = FxHashMap::default();
        nodes.insert(root_id, root);
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            nodes,
            root: root_id,
            vault: None,
            canvas_background: DEFAULT_CANVAS_BACKGROUND,
            guides: Vec::new(),
            images: FxHashMap::default(),
            variables: Default::default(),
            styles: Default::default(),
            // Empty, not minted: a document that has never been through the
            // library has no id, and `Document::new` is also what every test
            // fixture and the starter document use. Minting here would put a
            // fresh identity on a document nobody has saved.
            meta: DocumentMeta::default(),
            published: None,
            restoring: false,
        }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// The mask `id` is clipped by, if any: the **nearest preceding sibling**
    /// carrying [`Node::mask`] (§5.3).
    ///
    /// "Nearest preceding" makes a second mask end the first one's run rather
    /// than nest inside it: the layers between two masks belong to the lower.
    ///
    /// **A mask is never masked** — it answers `None` for itself, since a mask
    /// begins a run and cannot begin inside one.
    ///
    /// **Nothing masks inside a boolean.** Its children are operands, not artwork,
    /// so the flag on one is inert. The walk never reaches them but the hit test
    /// does (through the spatial index), so this arm keeps a mask pasted into a
    /// boolean from making part of it unclickable while changing nothing it draws.
    ///
    /// The one production caller is [`crate::hit_test`], through `masked_away`.
    /// The other readers (`scene::paint_node`, the SVG writer's child loop, the
    /// bounds passes in `resolve`) walk the child list in order and carry the
    /// current mask forward; **fix a disagreement by correcting the forward
    /// reader, not by routing it through here.** Both spellings are asserted on
    /// the same sibling fixtures, in `ops` and `render`.
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
    /// **`except` is a list of names on their way out.** A node being renamed is
    /// its own sibling, so without it `free_name` sees the name it is replacing as
    /// taken and walks one past. A builder that creates a node and deletes others
    /// in the same transaction (`build::flatten_union` emits its `CreateNode`
    /// before the deletes) has the same problem several times over.
    ///
    /// Only the direct children: numbering is per-parent (§15 D127), and names
    /// repeating across parents is an accepted state.
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
    /// undone** — see [`crate::meta`]. It reports no `DirtySet` either: nothing
    /// drawn changed, so the caller's only obligation is to write the file.
    pub fn set_meta(&mut self, meta: DocumentMeta) {
        self.meta = meta;
    }

    /// What this document last published as a kit (§15 D1012), when it has.
    pub fn published(&self) -> Option<&crate::kit::Published> {
        self.published.as_deref()
    }

    /// Replace the published state — the publish itself, or *Stop sharing*'s
    /// nothing. Outside undo and reporting no `DirtySet`, as [`Self::set_meta`]
    /// is and for its reason.
    pub fn set_published(&mut self, published: Option<crate::kit::Published>) {
        self.published = published.map(std::sync::Arc::new);
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
    /// The reverse of [`Guide::owner`], so a caller cannot read a `None` owner as
    /// "owned by nothing in particular" when it means "owned by the canvas".
    ///
    /// The one caller is the Scale tool (§5.6). `build::guides_of` tests a whole
    /// doomed subtree, so it filters `guides()` directly; the canvas draws every
    /// guide in one pass rather than asking frame by frame.
    ///
    /// [`Guide::owner`]: crate::guide::Guide::owner
    pub fn guides_owned_by(&self, owner: NodeId) -> impl Iterator<Item = &Guide> {
        self.guides.iter().filter(move |g| g.owner == Some(owner))
    }

    /// One image's entry, or `None` for a reference to something this document
    /// does not carry.
    ///
    /// **A missing entry is a state to draw, not an error**: both backends and the
    /// SVG writer paint the grey cross [`crate::missing_placeholder`] describes,
    /// and the layers panel tints the row (§15 D179).
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

    /// The document's variables and mode list (§5.3f, `crate::variable`).
    pub fn variables(&self) -> &crate::variable::Variables {
        &self.variables
    }

    /// The document's styles (§5.3f, `crate::style`).
    pub fn styles(&self) -> &crate::style::Styles {
        &self.styles
    }

    /// The mode in force at `id` (§5.3f): the mode the nearest frame at or above
    /// it sets, else the document's default. `None` only in a document with no
    /// modes.
    ///
    /// **The node itself counts**: a frame's own fill reads in the mode it sets.
    ///
    /// Only the document's own modes count: a frame set to a kit's mode (§15 D1064)
    /// governs that kit's variables alone, which [`Self::resolve_var_in`] reads.
    pub fn mode_at(&self, id: NodeId) -> Option<crate::variable::ModeId> {
        self.mode_for_in(&self.variables, id, None)
    }

    /// The mode `id` reads a variable of `kit` in — `None` for one of the
    /// document's own: the nearest frame's at or above it that is a mode of the
    /// document or of that kit, else `None`, the default (§15 D1064). For the
    /// document's own, a default is filled in.
    fn mode_for_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        kit: Option<&str>,
    ) -> Option<crate::variable::ModeId> {
        let mut at = self.nodes.get(&id);
        while let Some(n) = at {
            if let Some(m) = n.mode {
                let ours = t.mode(m).is_some();
                let kits =
                    kit.is_some_and(|k| t.kits().get(k).is_some_and(|km| km.mode(m).is_some()));
                if ours || kits {
                    return Some(m);
                }
            }
            at = n.parent.and_then(|p| self.nodes.get(&p));
        }
        if kit.is_some() {
            return None;
        }
        t.default_mode()
    }

    /// `var`'s value as `id` reads it — in the mode in force there.
    pub fn resolve_var(
        &self,
        id: NodeId,
        var: crate::variable::VarId,
    ) -> Option<crate::variable::Value> {
        self.resolve_var_in(&self.variables, id, var)
    }

    /// [`Self::resolve_var`] against table `t` rather than the document's own —
    /// a previewed cell's (`Variables::with_cell`, §15 D1056). The `_in` doors
    /// below are the same rule with the table named, so the preview and the
    /// committed drawing cannot read a binding two ways.
    pub fn resolve_var_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        var: crate::variable::VarId,
    ) -> Option<crate::variable::Value> {
        match t.get(var)?.kit() {
            // A kit's variable, through the mapping (§15 D1064, D1038 (b)).
            Some(kit) => t.resolve_opt(var, self.mode_for_in(t, id, Some(kit))),
            None => t.resolve(var, self.mode_for_in(t, id, None)?),
        }
    }

    /// The brush `brush` draws at `id`: itself when literal, a solid of its
    /// variable's colour in the mode in force at `id` when bound (§5.3f).
    ///
    /// **The one door a bound paint is drawn through** — the scene walk, the SVG
    /// writer, the snapshot and the inspector. A binding that cannot resolve is
    /// refused at the commit (`crate::variable::check`), so the transparent
    /// fallback is for a document that broke the rules anyway. Borrowed when
    /// literal.
    pub fn resolve_brush<'a>(
        &self,
        id: NodeId,
        brush: &'a crate::variable::BrushSource,
    ) -> std::borrow::Cow<'a, crate::Brush> {
        self.resolve_brush_in(&self.variables, id, brush)
    }

    /// [`Self::resolve_brush`] against table `t`.
    pub fn resolve_brush_in<'a>(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        brush: &'a crate::variable::BrushSource,
    ) -> std::borrow::Cow<'a, crate::Brush> {
        use crate::variable::{BrushSource, Value};
        use std::borrow::Cow;
        match brush {
            BrushSource::Literal(b) => Cow::Borrowed(b),
            BrushSource::Bound { var, alpha } => {
                Cow::Owned(match self.resolve_var_in(t, id, *var) {
                    Some(Value::Color(c)) => crate::Brush::Solid(c.multiply_alpha(*alpha)),
                    _ => crate::Brush::Solid(Color::TRANSPARENT),
                })
            }
        }
    }

    /// `paint` as it draws at `id`: every bound brush replaced by
    /// [`Self::resolve_brush`]'s literal — borrowed untouched when nothing in it
    /// is bound, which is nearly always.
    ///
    /// **The scene walk's and the SVG writer's one call**: taken where a node's
    /// paint is read, so the code below it handles literal brushes only
    /// ([`crate::BrushSource::drawn`]). `id` is the node whose mode the paint
    /// reads in; a ghost not yet in the document has none, so it reads in the
    /// **default** mode (`scene::paint_ghost`).
    pub fn resolved_paint<'a>(&self, id: NodeId, paint: &'a Paint) -> std::borrow::Cow<'a, Paint> {
        self.resolved_paint_in(&self.variables, id, paint)
    }

    /// [`Self::resolved_paint`] against table `t`.
    pub fn resolved_paint_in<'a>(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        paint: &'a Paint,
    ) -> std::borrow::Cow<'a, Paint> {
        use std::borrow::Cow;
        let bound = paint.fills.iter().any(|f| f.brush.is_bound())
            || paint
                .strokes
                .iter()
                .any(|s| s.brush.is_bound() || s.width_var.is_some());
        if !bound {
            return Cow::Borrowed(paint);
        }
        let mut out = paint.clone();
        for f in out.fills.iter_mut() {
            if f.brush.is_bound() {
                f.brush = self.resolve_brush_in(t, id, &f.brush).into_owned().into();
            }
        }
        for s in out.strokes.iter_mut() {
            if s.brush.is_bound() {
                s.brush = self.resolve_brush_in(t, id, &s.brush).into_owned().into();
            }
            // A bound width resolves to its number (floored at 0; missing or
            // non-number is 0) and comes out **unbound**: the cell preview hands
            // this output to the scene as an override, which resolves again against
            // the committed table, so a surviving binding would draw the committed
            // number over the previewed one.
            if let Some(v) = s.width_var.take() {
                s.width = self.stroke_width_drawn_in(t, id, v);
            }
        }
        Cow::Owned(out)
    }

    /// `effects` as they draw at `id`: every bound number its variable's number in
    /// the mode in force (§15 D1074; a blur floored at 0, a missing or non-number
    /// value 0), **unbound in what comes out** as in `resolved_paint`. Borrowed
    /// when nothing is bound.
    ///
    /// **What every reader that draws, measures or exports asks** — never
    /// `Node::effects`, whose bound numbers are their literal 0.
    pub fn resolved_effects<'a>(
        &self,
        id: NodeId,
        effects: &'a [crate::Keyed<crate::Effect>],
    ) -> std::borrow::Cow<'a, [crate::Keyed<crate::Effect>]> {
        self.resolved_effects_in(&self.variables, id, effects)
    }

    /// [`Self::resolved_effects`] against table `t`.
    pub fn resolved_effects_in<'a>(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        effects: &'a [crate::Keyed<crate::Effect>],
    ) -> std::borrow::Cow<'a, [crate::Keyed<crate::Effect>]> {
        use std::borrow::Cow;
        if effects.iter().all(|e| e.vars.iter().all(Option::is_none)) {
            return Cow::Borrowed(effects);
        }
        let mut out = effects.to_vec();
        for e in out.iter_mut() {
            for slot in 0..4 {
                if let Some(v) = e.vars[slot].take() {
                    e.value
                        .set_number(slot, self.effect_number_drawn_in(t, id, v, slot));
                }
            }
        }
        Cow::Owned(out)
    }

    /// The number effect slot `slot` bound to `var` is drawn at, at `id` in table
    /// `t` (§15 D1074): the variable's number in the mode in force, read through
    /// `Effect::number_read` (a blur within its range); a missing or non-number
    /// value 0.
    pub fn effect_number_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        var: crate::variable::VarId,
        slot: usize,
    ) -> f64 {
        match self.resolve_var_in(t, id, var) {
            Some(crate::variable::Value::Number(x)) => crate::Effect::number_read(slot, x),
            _ => 0.0,
        }
    }

    /// The width a stroke whose width is bound to `var` is drawn at, at `id` in
    /// table `t` (§15 D1073): the variable's number in the mode in force, never
    /// below 0; a missing or non-number value 0.
    pub fn stroke_width_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        var: crate::variable::VarId,
    ) -> f64 {
        match self.resolve_var_in(t, id, var) {
            Some(crate::variable::Value::Number(x)) => x.max(0.0),
            _ => 0.0,
        }
    }

    /// `id`'s corner radii as drawn: each bound corner its variable's number in
    /// the mode in force, each other corner its literal. `None` when `id` binds no
    /// corner — the stored radii are the drawn ones.
    pub fn resolve_corners(&self, id: NodeId, radii: RoundedRectRadii) -> Option<RoundedRectRadii> {
        self.resolve_corners_in(&self.variables, id, radii)
    }

    /// [`Self::resolve_corners`] against table `t`.
    pub fn resolve_corners_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        radii: RoundedRectRadii,
    ) -> Option<RoundedRectRadii> {
        let n = self.nodes.get(&id)?;
        if n.corner_vars.iter().all(Option::is_none) {
            return None;
        }
        Some(self.corners_drawn_in(t, id, radii, n.corner_vars))
    }

    /// `radii` with the corners `vars` binds read from table `t` in the mode in
    /// force at `id` — [`Self::resolve_corners_in`] with the bindings named, so a
    /// preview can resolve the ones a write keeps (§15 D1058's sibling, the
    /// corners' *Fix*).
    pub fn corners_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        radii: RoundedRectRadii,
        vars: crate::variable::CornerVars,
    ) -> RoundedRectRadii {
        let read = |var: Option<crate::variable::VarId>, lit: f64| match var {
            Some(v) => match self.resolve_var_in(t, id, v) {
                Some(crate::variable::Value::Number(x)) => x.max(0.0),
                _ => 0.0,
            },
            None => lit,
        };
        let [tl, tr, br, bl] = vars;
        RoundedRectRadii {
            top_left: read(tl, radii.top_left),
            top_right: read(tr, radii.top_right),
            bottom_right: read(br, radii.bottom_right),
            bottom_left: read(bl, radii.bottom_left),
        }
    }

    /// `display` with the slots `vars` binds read from table `t`, in the mode in
    /// force at `id` — each a variable's number, never below 0 (the layout
    /// engine's floor on a literal), the other slots as they are. `vars` is named
    /// so a preview can resolve a binding it has not committed.
    pub fn spacing_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        display: &crate::container::Display,
        vars: crate::variable::SpacingVars,
    ) -> crate::container::Display {
        let mut out = display.clone();
        if vars.iter().all(Option::is_none) {
            return out;
        }
        let mut s = display.spacing();
        for (k, var) in vars.iter().enumerate() {
            if let Some(v) = var {
                s[k] = match self.resolve_var_in(t, id, *v) {
                    Some(crate::variable::Value::Number(x)) => x.max(0.0),
                    _ => 0.0,
                };
            }
        }
        out.set_spacing(s);
        out
    }

    /// `id`'s layout as it is laid out: its bound gaps and padding resolved
    /// ([`Self::spacing_drawn_in`] against this document's table), borrowed when
    /// nothing is bound. **What every layout view answers `display` with**, so a
    /// bound gap is laid at its variable's number and never at the 0 its literal
    /// holds (§15 D1058).
    pub fn resolved_display(
        &self,
        id: NodeId,
    ) -> Option<std::borrow::Cow<'_, crate::container::Display>> {
        let n = self.nodes.get(&id)?;
        let d = n.display.as_ref()?;
        if n.spacing_vars.iter().all(Option::is_none) {
            return Some(std::borrow::Cow::Borrowed(d));
        }
        Some(std::borrow::Cow::Owned(self.spacing_drawn_in(
            &self.variables,
            id,
            d,
            n.spacing_vars,
        )))
    }

    /// Whether `id` is shown — its own visibility, **its boolean variable's
    /// value in the mode in force there where it is bound**, else its literal. Not
    /// whether an ancestor hides it: this is the layer's own eye. False for a node
    /// not in the document.
    ///
    /// **What every reader that draws, picks, lays out or exports asks**, never
    /// `Node::visible`.
    pub fn shown(&self, id: NodeId) -> bool {
        self.shown_in(&self.variables, id)
    }

    /// [`Self::shown`] against table `t`.
    pub fn shown_in(&self, t: &crate::variable::Variables, id: NodeId) -> bool {
        let Some(n) = self.nodes.get(&id) else {
            return false;
        };
        match n.visible_var {
            Some(v) => match self.resolve_var_in(t, id, v) {
                Some(crate::variable::Value::Boolean(b)) => b,
                _ => n.visible,
            },
            None => n.visible,
        }
    }

    /// The opacity `id` is drawn at: **its bound variable's number in the mode in
    /// force there, read as a percentage** (a variable of 40 draws 0.4), clamped
    /// to 0–100, a missing or non-number value 0; unbound, its literal. 1 for a
    /// node not in the document.
    ///
    /// **What every reader that draws or exports asks**, never `Node::opacity`,
    /// which is 0 while bound, so a reader that skips this draws the layer
    /// invisible — chosen to be seen.
    pub fn drawn_opacity(&self, id: NodeId) -> f32 {
        match self.nodes.get(&id) {
            Some(n) => self.opacity_drawn_in(&self.variables, id, n.opacity, n.opacity_var),
            None => 1.0,
        }
    }

    /// `literal` with the binding `var` read from table `t`, in the mode in force
    /// at `id` — [`Self::drawn_opacity`] with the binding named, so a preview can
    /// resolve one it has not committed, as [`Self::size_drawn_in`]'s is.
    pub fn opacity_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        literal: f32,
        var: Option<crate::variable::VarId>,
    ) -> f32 {
        let Some(v) = var else { return literal };
        match self.resolve_var_in(t, id, v) {
            Some(crate::variable::Value::Number(x)) => (x.clamp(0.0, 100.0) / 100.0) as f32,
            _ => 0.0,
        }
    }

    /// `kind` with the axes `vars` binds read from table `t`, in the mode in
    /// force at `id` — each its variable's number, never below 0, the other axis
    /// as it is; `kind` unchanged when nothing is bound or it has no stored size.
    /// `vars` is named so a preview can resolve a binding it has not committed.
    pub fn size_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        kind: &NodeKind,
        vars: crate::variable::SizeVars,
    ) -> NodeKind {
        let mut out = kind.clone();
        if vars.iter().all(Option::is_none) {
            return out;
        }
        let Some(size) = out.box_size_mut() else {
            return out;
        };
        for (k, var) in vars.iter().enumerate() {
            if let Some(v) = var {
                let x = match self.resolve_var_in(t, id, *v) {
                    Some(crate::variable::Value::Number(x)) => x.max(0.0),
                    _ => 0.0,
                };
                if k == 0 {
                    size.width = x;
                } else {
                    size.height = x;
                }
            }
        }
        out
    }

    /// `id`'s kind at the size it is drawn at: its bound width and height
    /// resolved ([`Self::size_drawn_in`] against this document's table),
    /// borrowed when neither is bound. **What every layout view answers `kind`
    /// with, and what `Resolved`'s used kind starts from**, so a bound size is
    /// never read as the 0 its literal holds. A bound text's content and family
    /// resolve too ([`Self::text_drawn_in`]).
    pub fn resolved_kind(&self, id: NodeId) -> Option<std::borrow::Cow<'_, NodeKind>> {
        let n = self.nodes.get(&id)?;
        let sized = n.size_vars.iter().any(Option::is_some);
        let texted = n.text_vars.iter().any(Option::is_some);
        if !sized && !texted {
            return Some(std::borrow::Cow::Borrowed(&n.kind));
        }
        let mut kind = match sized {
            true => self.size_drawn_in(&self.variables, id, &n.kind, n.size_vars),
            false => n.kind.clone(),
        };
        if texted {
            kind = self.text_drawn_in(&self.variables, id, &kind, n.text_vars);
        }
        Some(std::borrow::Cow::Owned(kind))
    }

    /// `kind` with the content and family `vars` binds read from table `t`, in
    /// the mode in force at `id` — each its variable's string, the other field as
    /// it is; `kind` unchanged when nothing is bound or it is no `Text`. `vars` is
    /// named so a preview can resolve a binding it has not committed.
    ///
    /// A bound content draws with no spans: a byte range cannot index a string
    /// whose length is the mode's, and one inside a character panics the shaper.
    /// The binding verb and the commit's passes keep spans off a bound content;
    /// this is the second defence, for a door they miss.
    pub fn text_drawn_in(
        &self,
        t: &crate::variable::Variables,
        id: NodeId,
        kind: &NodeKind,
        vars: crate::variable::TextVars,
    ) -> NodeKind {
        let mut out = kind.clone();
        if vars.iter().all(Option::is_none) {
            return out;
        }
        let NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            ..
        } = &mut out
        else {
            return out;
        };
        let read = |v: Option<crate::variable::VarId>| {
            v.and_then(|v| match self.resolve_var_in(t, id, v) {
                Some(crate::variable::Value::String(s)) => Some(s),
                _ => None,
            })
        };
        if let Some(s) = read(vars[0]) {
            *content = s;
            *spans = Default::default();
            *para_spans = Default::default();
        }
        if let Some(s) = read(vars[1]) {
            style.font_family = s;
        }
        out
    }

    /// Every node whose fields read a variable — what an edit to the table
    /// dirties, since no node of it changed and every one of them may draw
    /// differently (§5.3f premise (ii)).
    pub fn bound_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .values()
            .filter(|n| n.reads_variables())
            .map(|n| n.id)
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The vault's root, when this document has vendored a kit main (§15 D1012).
    pub fn vault(&self) -> Option<NodeId> {
        self.vault
    }

    /// Whether `id` is the vault or under it — a vendored kit node, never drawn,
    /// picked or selected (§15 D1012). False for a node not in the document.
    pub fn in_vault(&self, id: NodeId) -> bool {
        let Some(v) = self.vault else {
            return false;
        };
        let mut cursor = Some(id);
        // Bounded by the node count, as every ancestor walk over stored parents
        // must be against a file the loader has not yet certified.
        for _ in 0..=self.nodes.len() {
            match cursor {
                Some(c) if c == v => return true,
                Some(c) => cursor = self.nodes.get(&c).and_then(|n| n.parent),
                None => return false,
            }
        }
        false
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
    ///
    /// One argument per field a file stores, which is why there are eleven: a
    /// struct of the same eleven fields would be this signature with a name.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        schema_version: u32,
        nodes: FxHashMap<NodeId, Node>,
        root: NodeId,
        vault: Option<NodeId>,
        canvas_background: Color,
        guides: Vec<Guide>,
        images: FxHashMap<ImageId, ImageEntry>,
        variables: crate::variable::Variables,
        styles: crate::style::Styles,
        meta: DocumentMeta,
        published: Option<crate::kit::Published>,
    ) -> Self {
        Self {
            schema_version,
            nodes,
            root,
            vault,
            canvas_background,
            guides,
            images,
            variables,
            styles,
            meta,
            published: published.map(std::sync::Arc::new),
            restoring: false,
        }
    }

    /// Iterate all nodes in unspecified order (used by IO on save, which sorts).
    pub(crate) fn nodes_iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// The node table itself, for `crate::component`'s rules and verbs, which ask
    /// by id across the whole document (who links to this, whose chain ends where).
    pub(crate) fn node_map(&self) -> &FxHashMap<NodeId, Node> {
        &self.nodes
    }

    /// [`Self::apply`] for a **history step** — an undo or a redo putting back
    /// values this document held before — with the layout operations' CSS
    /// validity check and the blur cap stood down.
    ///
    /// A file may carry a layout value CSS refuses, which the loader keeps as
    /// written. An edit replaces it, and that edit's inverse writes the refused
    /// value back, which `SetDisplay` and `SetLayoutItem` would refuse with
    /// `BadLayout`, so the undo would fail and the file's value could never be
    /// returned to. Every value a history step writes was in this document
    /// already, so it is not the operations' to refuse. Every other check stands.
    pub fn apply_restoring(&mut self, tx: &Transaction) -> Result<ApplyOutcome, OpError> {
        self.restoring = true;
        let outcome = self.apply(tx);
        self.restoring = false;
        outcome
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

        // Every guide's owner, once, after the whole transaction (§15 D491).
        // `check_guide_owner` guards the operations that name an owner, but
        // deleting the owning frame breaks the same rule and would leave a file
        // `io::load` refuses forever. It is a post-condition rather than an arm in
        // `op_delete` so the next op that breaks it is caught too; `build::guides_of`
        // is still how the Delete verb carries its guides and orders the inverse.
        //
        // It runs after the loop and before `*self = working`, so a transaction is
        // still all-or-nothing. A per-op check would refuse the legal order
        // frame-then-guides, which passes through an orphan on its way to a valid
        // document (`undoing_a_frame_delete_restores_its_guides` commits it).
        for guide in &working.guides {
            working.check_guide_owner(guide.owner)?;
        }

        // Item ids unique within each of a node's five lists (§15 D980), over the
        // nodes this transaction wrote: a duplicate would match two of an
        // instance's items to one of its main's. After the last op, for the guide
        // owners' reason; `dirty` covers every node an op wrote.
        for id in &dirty.0 {
            if let Some(n) = working.nodes.get(id) {
                n.check_item_ids()?;
            }
        }

        // The component rules (§15 D978): every link resolves, a main is a frame
        // or a group outside any main or instance, an instance's nodes belong to
        // it, no main holds an instance of itself. Over the whole document rather
        // than `dirty`, because a delete dirties the deleted node, not the
        // instance pointing at it (`component::check`).
        crate::component::check(&working.nodes).map_err(|(id, rule)| OpError::BadLink(id, rule))?;

        // The vault's rules (§15 D1012): an origin on every vault node and on
        // nothing else, a main or a set at each of its children. After the last op
        // for the guide owners' reason — vendoring opens the vault, then inserts.
        crate::kit::check(&working.nodes, working.root, working.vault)
            .map_err(|(id, rule)| OpError::Vault(id, rule))?;

        // The variables rules (§5.3f): every binding resolves to a variable of its
        // kind, every variable has a cell in every mode, no alias loops, a mode set
        // only on a frame. Over the whole document, for `component::check`'s reason.
        crate::variable::check(&working.variables, working.nodes.values(), |id| {
            working.in_vault(id)
        })
        .map_err(OpError::Variables)?;

        // The style rules (§15 D1062): every binding names a style of its slot,
        // names unique within a slot, a style's paints bound to colour variables,
        // and no vault node bound to one of the document's own — the same
        // whole-document reach.
        crate::style::check(
            &working.styles,
            &working.variables,
            working.nodes.values(),
            |id| working.in_vault(id),
        )
        .map_err(OpError::Styles)?;

        // All ops succeeded — commit the working copy.
        *self = working;

        // To undo [a, b, c] applied in order, apply [c⁻¹, b⁻¹, a⁻¹].
        inverse_ops.reverse();
        Ok(ApplyOutcome {
            inverse: Transaction(inverse_ops),
            dirty,
        })
    }

    /// Apply `tx` to `self` **without** the post-conditions — for a scratch copy
    /// a commit-time pass inspects to see the tree a transaction would leave
    /// (`component::settle_links`). Never the path an edit takes: [`Self::apply`]
    /// is, and it is what refuses a transaction this lets through.
    pub(crate) fn apply_unchecked(&mut self, tx: &Transaction) -> Result<(), OpError> {
        let mut dirty = DirtySet::default();
        for op in &tx.0 {
            self.apply_one(op, &mut dirty)?;
        }
        Ok(())
    }

    /// What `op` would replace — its inverse — with `self` untouched. `None` when
    /// `op` would be refused, or names no node ([`Operation::overwrites`]). The
    /// propagation pass's reader (`crate::propagate`): an operation's inverse
    /// carries the old value of exactly the field it writes, so this reads any
    /// field any operation can write without a reader per field.
    ///
    /// **Applied to a scratch document holding the one node `op` names**, because
    /// every operation `overwrites` writes that node alone. The scratch carries
    /// `root`, `schema_version` and `restoring` across — the last read by the
    /// history exemption of `op_set_display` and `op_set_layout_item`.
    /// **`op_set_mask` reads past its node** (`holds_a_frame`, §15 D876), so its
    /// scratch holds the subtree; with the node alone a peek would answer `Some`
    /// and `apply` would refuse the whole edit.
    ///
    /// - **Nothing is restored**, so nothing can fail to be: re-applying the
    ///   inverse would panic on an old value the loader admits and an op refuses
    ///   (a gradient's opacity of 5, §15 D767).
    /// - **The pass clones one node per read, not the document**; the live
    ///   preview calls it every frame.
    pub(crate) fn peek(&self, op: &Operation) -> Option<Operation> {
        let id = op.overwrites()?;
        let mut nodes = FxHashMap::default();
        nodes.insert(id, self.nodes.get(&id)?.clone());
        if matches!(op, Operation::SetMask { .. }) {
            for d in self.subtree_ids(id) {
                nodes.insert(d, self.nodes.get(&d)?.clone());
            }
        }
        let mut one = Document {
            schema_version: self.schema_version,
            nodes,
            root: self.root,
            vault: self.vault,
            canvas_background: self.canvas_background,
            guides: Vec::new(),
            images: FxHashMap::default(),
            variables: Default::default(),
            styles: Default::default(),
            meta: DocumentMeta::default(),
            published: None,
            restoring: self.restoring,
        };
        one.apply_one(op, &mut DirtySet::default()).ok()
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
            Operation::SetInsets { id, insets } => self.op_set_insets(*id, insets, dirty),
            Operation::SetDisplay { id, display } => self.op_set_display(*id, display, dirty),
            Operation::SetLayoutItem { id, item } => self.op_set_layout_item(*id, item, dirty),
            Operation::SetComponent { id, component } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.component, *component);
                dirty.0.insert(*id);
                Ok(Operation::SetComponent {
                    id: *id,
                    component: old,
                })
            }
            Operation::SetLink { id, link } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.link, *link);
                dirty.0.insert(*id);
                Ok(Operation::SetLink { id: *id, link: old })
            }
            Operation::SetSwap { id, swap } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.swap, *swap);
                dirty.0.insert(*id);
                Ok(Operation::SetSwap { id: *id, swap: old })
            }
            Operation::SetVariantSet { id, set } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.set, set.clone());
                dirty.0.insert(*id);
                Ok(Operation::SetVariantSet { id: *id, set: old })
            }
            Operation::SetVariant { id, values } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.variant, values.clone());
                dirty.0.insert(*id);
                Ok(Operation::SetVariant {
                    id: *id,
                    values: old,
                })
            }
            Operation::SetProperties { id, props } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.props, props.clone());
                dirty.0.insert(*id);
                Ok(Operation::SetProperties {
                    id: *id,
                    props: old,
                })
            }
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
            Operation::OpenVault { id } => self.op_open_vault(*id),
            Operation::CloseVault { id } => self.op_close_vault(*id),
            Operation::SetOrigin { id, origin } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.origin, origin.clone());
                dirty.0.insert(*id);
                Ok(Operation::SetOrigin {
                    id: *id,
                    origin: old,
                })
            }
            Operation::AddVariable { var } => self.op_add_variable(var, dirty),
            Operation::RemoveVariable { id } => self.op_remove_variable(*id, dirty),
            Operation::SetVariable { var } => {
                let v = self
                    .variables
                    .vars
                    .get_mut(&var.id)
                    .ok_or(OpError::NoSuchVariable(var.id))?;
                if v.kind != var.kind {
                    return Err(OpError::WrongKindForOp);
                }
                let old = std::mem::replace(v, var.clone());
                dirty.0.extend(self.bound_nodes());
                Ok(Operation::SetVariable { var: old })
            }
            Operation::SetKitModes { kit, modes } => {
                let old = match modes {
                    Some(m) => self.variables.kits.insert(kit.clone(), m.clone()),
                    None => self.variables.kits.remove(kit),
                };
                dirty.0.extend(self.bound_nodes());
                Ok(Operation::SetKitModes {
                    kit: kit.clone(),
                    modes: old,
                })
            }
            Operation::SetVariableName { id, name } => {
                let v = self
                    .variables
                    .vars
                    .get_mut(id)
                    .ok_or(OpError::NoSuchVariable(*id))?;
                let old = std::mem::replace(&mut v.name, name.clone());
                Ok(Operation::SetVariableName { id: *id, name: old })
            }
            Operation::SetVariableCell { id, mode, cell } => {
                self.op_set_variable_cell(*id, *mode, cell.clone(), dirty)
            }
            Operation::AddMode { mode, index, cells } => {
                self.op_add_mode(mode, *index, cells, dirty)
            }
            Operation::RemoveMode { id } => self.op_remove_mode(*id, dirty),
            Operation::SetModeName { id, name } => {
                let m = self
                    .variables
                    .modes
                    .iter_mut()
                    .find(|m| m.id == *id)
                    .ok_or(OpError::NoSuchMode(*id))?;
                let old = std::mem::replace(&mut m.name, name.clone());
                // A kit's column is matched to a document mode by name, so a
                // rename can move what every kit-bound layer reads.
                if !self.variables.kits.is_empty() {
                    dirty.0.extend(self.bound_nodes());
                }
                Ok(Operation::SetModeName { id: *id, name: old })
            }
            // Written on any node, so `reset::state_ops` can state every node's
            // mode; *setting* one anywhere but a frame is refused at the commit
            // (`variable::check`), where a transaction is judged by where it ends.
            Operation::SetMode { id, mode } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.mode, *mode);
                dirty.0.insert(*id);
                Ok(Operation::SetMode { id: *id, mode: old })
            }
            Operation::SetCornerVars { id, vars } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.corner_vars, *vars);
                dirty.0.insert(*id);
                Ok(Operation::SetCornerVars { id: *id, vars: old })
            }
            Operation::SetSpacingVars { id, vars } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.spacing_vars, *vars);
                dirty.0.insert(*id);
                Ok(Operation::SetSpacingVars { id: *id, vars: old })
            }
            Operation::SetVisibleVar { id, var } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.visible_var, *var);
                dirty.0.insert(*id);
                Ok(Operation::SetVisibleVar { id: *id, var: old })
            }
            Operation::SetSizeVars { id, vars } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.size_vars, *vars);
                dirty.0.insert(*id);
                Ok(Operation::SetSizeVars { id: *id, vars: old })
            }
            Operation::SetTextVars { id, vars } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.text_vars, *vars);
                dirty.0.insert(*id);
                Ok(Operation::SetTextVars { id: *id, vars: old })
            }
            Operation::SetOpacityVar { id, var } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.opacity_var, *var);
                dirty.0.insert(*id);
                Ok(Operation::SetOpacityVar { id: *id, var: old })
            }
            // ── The styles (§15 D1062) ── document-level, like the variables:
            // a style's values are written onto its users by ops of their own
            // (`style::settle`), so these dirty nothing.
            Operation::AddStyle { style } => {
                if !crate::style::survives_save(&style.value) {
                    return Err(OpError::NonFinite);
                }
                if self.styles.styles.contains_key(&style.id) {
                    return Err(OpError::DuplicateStyle(style.id));
                }
                self.styles.styles.insert(style.id, style.clone());
                Ok(Operation::RemoveStyle { id: style.id })
            }
            Operation::RemoveStyle { id } => {
                let style = self
                    .styles
                    .styles
                    .remove(id)
                    .ok_or(OpError::NoSuchStyle(*id))?;
                Ok(Operation::AddStyle { style })
            }
            Operation::SetStyleName { id, name } => {
                let s = self
                    .styles
                    .styles
                    .get_mut(id)
                    .ok_or(OpError::NoSuchStyle(*id))?;
                let old = std::mem::replace(&mut s.name, name.clone());
                Ok(Operation::SetStyleName { id: *id, name: old })
            }
            // Nothing is dirtied: a description is never drawn.
            Operation::SetDescription { of, text } => {
                let slot = match of {
                    Described::Node(id) => {
                        &mut self
                            .nodes
                            .get_mut(id)
                            .ok_or(OpError::NoSuchNode(*id))?
                            .description
                    }
                    Described::Style(id) => {
                        &mut self
                            .styles
                            .styles
                            .get_mut(id)
                            .ok_or(OpError::NoSuchStyle(*id))?
                            .description
                    }
                    Described::Variable(id) => {
                        &mut self
                            .variables
                            .vars
                            .get_mut(id)
                            .ok_or(OpError::NoSuchVariable(*id))?
                            .description
                    }
                };
                let old = std::mem::replace(slot, text.clone());
                Ok(Operation::SetDescription { of: *of, text: old })
            }
            Operation::SetStyleValue { id, value } => {
                if !crate::style::survives_save(value) {
                    return Err(OpError::NonFinite);
                }
                let s = self
                    .styles
                    .styles
                    .get_mut(id)
                    .ok_or(OpError::NoSuchStyle(*id))?;
                if s.slot() != value.slot() {
                    return Err(OpError::WrongKindForOp);
                }
                let old = std::mem::replace(&mut s.value, value.clone());
                Ok(Operation::SetStyleValue {
                    id: *id,
                    value: old,
                })
            }
            Operation::SetStyleOrigin { id, origin } => {
                let s = self
                    .styles
                    .styles
                    .get_mut(id)
                    .ok_or(OpError::NoSuchStyle(*id))?;
                let old = std::mem::replace(&mut s.origin, origin.clone());
                Ok(Operation::SetStyleOrigin {
                    id: *id,
                    origin: old,
                })
            }
            Operation::SetFillStyle { id, style } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.styles.fills, *style);
                dirty.0.insert(*id);
                Ok(Operation::SetFillStyle {
                    id: *id,
                    style: old,
                })
            }
            Operation::SetStrokeStyle { id, style } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.styles.strokes, *style);
                dirty.0.insert(*id);
                Ok(Operation::SetStrokeStyle {
                    id: *id,
                    style: old,
                })
            }
            Operation::SetEffectsStyle { id, style } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.styles.effects, *style);
                dirty.0.insert(*id);
                Ok(Operation::SetEffectsStyle {
                    id: *id,
                    style: old,
                })
            }
            Operation::SetTypeStyle { id, style } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.styles.text, *style);
                dirty.0.insert(*id);
                Ok(Operation::SetTypeStyle {
                    id: *id,
                    style: old,
                })
            }
            Operation::SetLayoutStyle { id, style } => {
                let node = self.nodes.get_mut(id).ok_or(OpError::NoSuchNode(*id))?;
                let old = std::mem::replace(&mut node.styles.layout, *style);
                dirty.0.insert(*id);
                Ok(Operation::SetLayoutStyle {
                    id: *id,
                    style: old,
                })
            }
        }
    }

    // ── The variables table (§5.3f, §15 D1039) ────────────────────────────────
    //
    // **Every edit that can change what a bound field reads dirties every node
    // that reads a variable** (`bound_nodes`): no node changed, and `Resolved`'s
    // `update` returns early on an empty set. A scan, not a reverse index: a
    // commit already clones the whole document. The table's rules are checked
    // after the last op (`variable::check`), so these handlers refuse only a
    // missing id or a duplicate one.

    fn op_add_variable(
        &mut self,
        var: &crate::variable::Variable,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if self.variables.vars.contains_key(&var.id) {
            return Err(OpError::DuplicateVariable(var.id));
        }
        self.variables.vars.insert(var.id, var.clone());
        dirty.0.extend(self.bound_nodes());
        Ok(Operation::RemoveVariable { id: var.id })
    }

    fn op_remove_variable(
        &mut self,
        id: crate::variable::VarId,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let var = self
            .variables
            .vars
            .remove(&id)
            .ok_or(OpError::NoSuchVariable(id))?;
        dirty.0.extend(self.bound_nodes());
        Ok(Operation::AddVariable { var })
    }

    fn op_set_variable_cell(
        &mut self,
        id: crate::variable::VarId,
        mode: crate::variable::ModeId,
        cell: crate::variable::Cell,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if !self.variables.has_mode(mode) {
            return Err(OpError::NoSuchMode(mode));
        }
        let v = self
            .variables
            .vars
            .get_mut(&id)
            .ok_or(OpError::NoSuchVariable(id))?;
        let old = v
            .cells
            .insert(mode, cell)
            .ok_or(OpError::NoSuchMode(mode))?;
        dirty.0.extend(self.bound_nodes());
        Ok(Operation::SetVariableCell {
            id,
            mode,
            cell: old,
        })
    }

    fn op_add_mode(
        &mut self,
        mode: &crate::variable::Mode,
        index: usize,
        cells: &[(crate::variable::VarId, crate::variable::Cell)],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if self.variables.mode(mode.id).is_some() {
            return Err(OpError::DuplicateMode(mode.id));
        }
        if index > self.variables.modes.len() {
            return Err(OpError::IndexOutOfRange);
        }
        for (var, cell) in cells {
            self.variables
                .vars
                .get_mut(var)
                .ok_or(OpError::NoSuchVariable(*var))?
                .cells
                .insert(mode.id, cell.clone());
        }
        self.variables.modes.insert(index, mode.clone());
        dirty.0.extend(self.bound_nodes());
        Ok(Operation::RemoveMode { id: mode.id })
    }

    /// **The column travels in the inverse**, `op_remove_image`'s reason: the
    /// mode's cells are held nowhere else.
    fn op_remove_mode(
        &mut self,
        id: crate::variable::ModeId,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let index = self
            .variables
            .mode_index(id)
            .ok_or(OpError::NoSuchMode(id))?;
        let mode = self.variables.modes.remove(index);
        let cells = self
            .variables
            .vars
            .values_mut()
            .filter_map(|v| v.cells.remove(&id).map(|c| (v.id, c)))
            .collect();
        dirty.0.extend(self.bound_nodes());
        Ok(Operation::AddMode { mode, index, cells })
    }

    /// Open the vault as an empty parentless `Root` (§15 D1012). Refused when one
    /// is open or the id is taken. Nothing is dirtied: nothing in it is drawn.
    fn op_open_vault(&mut self, id: NodeId) -> Result<Operation, OpError> {
        if self.vault.is_some() {
            return Err(OpError::Vault(id, crate::kit::VaultRule::State));
        }
        if self.nodes.contains_key(&id) {
            return Err(OpError::DuplicateId(id));
        }
        let mut vault = Document::new(id).nodes.remove(&id).expect("just made");
        vault.name = "Kits".to_string();
        self.nodes.insert(id, vault);
        self.vault = Some(id);
        Ok(Operation::CloseVault { id })
    }

    /// Close the vault — only when it is `id` and holds nothing, so the inverse of
    /// an open undoes exactly that.
    fn op_close_vault(&mut self, id: NodeId) -> Result<Operation, OpError> {
        let empty = self.nodes.get(&id).is_some_and(|n| n.children.is_empty());
        if self.vault != Some(id) || !empty {
            return Err(OpError::Vault(id, crate::kit::VaultRule::State));
        }
        self.nodes.remove(&id);
        self.vault = None;
        Ok(Operation::OpenVault { id })
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
        // The check has to be here and not only at the loader: past this line the
        // value is in the document and the next autosave writes a file that never
        // opens again. See [`OpError::NonFinite`] and `NodeKind::geometry_is_finite`.
        if !kind.geometry_is_finite() || transform.is_some_and(|t| !affine_is_finite(t)) {
            return Err(OpError::NonFinite);
        }
        let parent_node = self.nodes.get(&parent).ok_or(OpError::NoSuchNode(parent))?;
        if index > parent_node.children.len() {
            return Err(OpError::IndexOutOfRange);
        }
        check_child_kind(&parent_node.kind, kind)?;
        if matches!(kind, NodeKind::Artboard { .. }) && !self.frame_may_sit_under(parent) {
            return Err(OpError::ArtboardPlacement);
        }

        // **`None` means "number me among my new siblings"**, resolved here
        // (§15 D127; §5.7b names the sites that pass `Some`) because this is the
        // chokepoint: a tool added later is numbered without knowing numbering
        // exists, where a name computed per builder would be silent to forget.
        //
        // The op's result thus depends on the document it lands in. Invariant 3's
        // determinism is about ids, which still ride inside the op; a name is not
        // structural, and multiplayer (§12) must resolve rename conflicts anyway.
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
            // Nor insets: a new layer is placed by its transform until it is
            // pinned (`crate::container`).
            insets: Default::default(),
            display: None,
            item: Default::default(),
            component: false,
            link: None,
            swap: None,
            set: None,
            variant: Vec::new(),
            props: Vec::new(),
            description: String::new(),
            origin: None,
            mode: None,
            corner_vars: Default::default(),
            spacing_vars: Default::default(),
            visible_var: None,
            size_vars: Default::default(),
            text_vars: Default::default(),
            opacity_var: None,
            styles: Default::default(),
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
            // The same field invariant `op_create` upholds one door over. This is
            // the door an MCP write tool, an importer or a paste (`io::clip`, OS
            // clipboard text) comes through — traffic the model cannot vouch for.
            // Every coordinate a node carries (geometry, transform, pivot, effects,
            // paint) must be checked at every door that writes it (§15 D639, D831);
            // `op_set_fills` and `op_set_strokes` hold the paint half (§15 D451).
            if !n.kind.geometry_is_finite()
                || !affine_is_finite(n.transform)
                || n.pivot.is_some_and(|p| !p.is_finite())
                || !n.effects.iter().all(|e| effect_is_finite(e))
                || !n.paint.fills.iter().all(|f| f.brush.is_finite())
                || !n.paint.strokes.iter().all(|s| {
                    s.brush.is_finite()
                        && s.width.is_finite()
                        && s.dashes.iter().all(|d| d.is_finite())
                })
            {
                return Err(OpError::NonFinite);
            }
            // Not for a history step, as at `op_set_effects`.
            if !self.restoring && !n.effects.iter().all(|e| crate::effect::blur_within_cap(e)) {
                return Err(OpError::BlurTooLarge);
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

        // The converse of the loop above, which walks from each node to its listed
        // children: every non-root node must also **be** somebody's child, or it is
        // an orphan that panics `DeleteNode`, `Reparent` and `Reorder` and saves a
        // file whose load fails (§15 D423). Reachable from the public API alone
        // (two `capture_subtree` snapshots composed), and the loader's check (5)
        // already enforces it; the two doors into the tree must hold the same
        // standard (§5.11).
        //
        // A count is not a reachability check: a cycle among the nodes balances it
        // (§15 D832). So walk it, as the loader does. The depth rides along (the
        // loader's check (6)): a 300-deep payload arrives in one paste.
        //
        // The bound is absolute, so the parent's own depth is part of it. The
        // upward walk is bounded too: a cycle among existing nodes is an invariant
        // violation, but hanging on one is worse than refusing it.
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

        // No frame under a boolean or a mask, inside the subtree or above it
        // (§15 D876) — carried down the walk as one flag, seeded from the chain
        // the subtree lands in. The pasted text is a template from outside the
        // process (`io::clip`), so its own inside is not vouched for either.
        let mut stack = vec![(root_id, !self.frame_may_sit_under(parent))];
        while let Some((id, barred)) = stack.pop() {
            let n = by_id[&id];
            if barred && matches!(n.kind, NodeKind::Artboard { .. }) {
                return Err(OpError::ArtboardPlacement);
            }
            let barred = barred || bars_frames(n);
            stack.extend(n.children.iter().map(|c| (*c, barred)));
        }

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
        // `build::local_for_world` inverts the parent's world transform unguarded,
        // and `svg_in` accepts a singular parent (finite, not invertible), so a
        // `NaN` transform can arrive here. See [`OpError::NonFinite`].
        if !affine_is_finite(transform) {
            return Err(OpError::NonFinite);
        }
        let old = {
            let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
            let old = node.transform;
            node.transform = transform;
            old
        };
        // **This node alone**: `Resolved::update` expands the subtree itself
        // (`collect_subtree`, §15 D590). Listing descendants here would repeat that
        // and stop `update` telling why a node was dirty, so its text pass would
        // re-shape every text under a nudged group, though a text layout depends on
        // nothing outside its own node.
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
        // The subtree already obeys the frame rule inside itself; what can break
        // it is the chain it is moving under (§15 D876).
        if !self.frame_may_sit_under(new_parent) && self.holds_a_frame(id) {
            return Err(OpError::ArtboardPlacement);
        }

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
        fills: &[Keyed<Fill>],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's paint half (§15 D451): a `NaN` gradient stop offset or
        // colour channel is written to the file as `null` and kills the document
        // on the way back in, so it is refused here.
        if !fills.iter().all(|f| f.brush.is_finite()) {
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
        strokes: &[Keyed<Stroke>],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // The same refusal `op_set_fills` makes (§15 D451), plus a stroke's own
        // numbers: the width and dash lengths are written to the same file.
        if !strokes.iter().all(|s| {
            s.brush.is_finite() && s.width.is_finite() && s.dashes.iter().all(|d| d.is_finite())
        }) {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        // `is_paintable`, the same gate `op_set_fills` uses (§15 D400).
        if !is_paintable(&node.kind) {
            return Err(OpError::WrongKindForOp);
        }
        let old = std::mem::replace(&mut node.paint.strokes, strokes.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetStrokes { id, strokes: old })
    }

    /// The effect stack (§5.3a).
    ///
    /// **No `is_paintable` gate**: effects are carried by anything that
    /// composites, which is every kind but the root (the document, not a layer —
    /// the line `op_create` and `op_delete` draw). A blur over a group is ordinary.
    ///
    /// The stack is written whole, so this is also the reorder, the removal and
    /// the visibility toggle. See [`Operation::SetEffects`].
    fn op_set_effects(
        &mut self,
        id: NodeId,
        effects: &[Keyed<Effect>],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's third family (§15 D641). Checked before the node is looked
        // up so the reported error is the interesting one, as in
        // `op_insert_subtree`.
        if !effects.iter().all(|e| effect_is_finite(e)) {
            return Err(OpError::NonFinite);
        }
        // The loader clamps, so a history step should never carry a radius past
        // the cap; standing the check down for one anyway keeps an undo from
        // failing over a value the document already held.
        if !self.restoring && !effects.iter().all(|e| crate::effect::blur_within_cap(e)) {
            return Err(OpError::BlurTooLarge);
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
    /// **No kind gate**: the field is on the `Node`, and "can this be exported"
    /// is answered by whether it has bounds, not by what it is.
    ///
    /// The node is dirtied though nothing drawn changes, so the layers panel can
    /// mark an exportable layer without a second notification path.
    fn op_set_exports(
        &mut self,
        id: NodeId,
        exports: &[Keyed<ExportSpec>],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8's export half (§15 D492): a zero width or a negative
        // multiplier would export a junk file named for a size it does not have.
        // The app's own UI and the parser already refuse these, so the model was
        // the only door without the guard, where a hand-edited file or another
        // tool comes in.
        if !exports.iter().all(|k| export_spec_is_usable(k)) {
            return Err(OpError::BadExportSpec);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.exports, exports.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetExports { id, exports: old })
    }

    /// Replace a layer's layout grids (`crate::layout`), returning the inverse.
    ///
    /// **`op_set_exports`' twin**, down to the dirty mark: a grid is chrome over
    /// the frame, and the node is marked anyway so a panel sees the edit.
    /// **No kind gate**: the list is inert on a layer the app offers no grids for,
    /// and refusing would make the model's answer depend on which panel asked.
    fn op_set_grids(
        &mut self,
        id: NodeId,
        grids: &[Keyed<crate::layout::LayoutGrid>],
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.grids, grids.to_vec());
        dirty.0.insert(id);
        Ok(Operation::SetLayoutGrids { id, grids: old })
    }

    /// Replace a layer's insets (`crate::container`, §15 D871).
    ///
    /// **Refused when any length is not finite** ([`OpError::NonFinite`]): an
    /// inset is a position, and a `NaN` one would put the layer nowhere.
    ///
    /// **No kind or parent gate**, on `op_set_grids`' terms: the layout pass
    /// decides whether insets place a layer. The subtree follows the node through
    /// `Resolved::update`'s expansion.
    fn op_set_insets(
        &mut self,
        id: NodeId,
        insets: &crate::container::Insets,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if !insets.is_finite() {
            return Err(OpError::NonFinite);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.insets, *insets);
        dirty.0.insert(id);
        Ok(Operation::SetInsets { id, insets: old })
    }

    /// Replace a layer's layout (`crate::container::Display`, §15 D867).
    ///
    /// Refused when a number in it is not finite, `op_set_insets`' rule, and when
    /// it carries a value CSS refuses — a grid track (§15 D914), a negative
    /// padding or gap (§15 D937) — that the node does not already hold; a history
    /// step putting back what the node held is not checked at all
    /// ([`Self::apply_restoring`]). No kind gate: stored on any layer and read on
    /// a frame or a group. The subtree follows the node through
    /// `Resolved::update`'s expansion.
    fn op_set_display(
        &mut self,
        id: NodeId,
        display: &Option<crate::container::Display>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if display.as_ref().is_some_and(|d| !d.is_finite()) {
            return Err(OpError::NonFinite);
        }
        // A value CSS refuses is refused — unless this is a history step putting
        // back what the node held, or the refused part is one the node holds now
        // and the edit only carries it forward (§15 D937).
        let was = self
            .nodes
            .get(&id)
            .ok_or(OpError::NoSuchNode(id))?
            .display();
        if !self.restoring && display.as_ref().is_some_and(|d| !d.is_valid_over(was)) {
            return Err(OpError::BadLayout);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.display, display.clone());
        dirty.0.insert(id);
        Ok(Operation::SetDisplay { id, display: old })
    }

    /// Replace a layer's layout-item properties (`crate::container::LayoutItem`).
    ///
    /// Refused when a number is not finite, and when a value is one CSS refuses —
    /// line 0, `span 0` (`OpError::BadLayout`, §15 D914), a negative grow, shrink,
    /// size, limit or basis (§15 D937) — in a field that does not already hold it;
    /// a history step is not checked ([`Self::apply_restoring`]). The node is
    /// dirtied, and **its parent with it**: an item's properties move its siblings
    /// too, and `Resolved`'s expansion descends from the parent to reach them.
    fn op_set_layout_item(
        &mut self,
        id: NodeId,
        item: &crate::container::LayoutItem,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        if !item.is_finite() {
            return Err(OpError::NonFinite);
        }
        // `op_set_display`'s rule (§15 D937).
        let was = *self.nodes.get(&id).ok_or(OpError::NoSuchNode(id))?.item();
        if !self.restoring && !item.is_valid_over(&was) {
            return Err(OpError::BadLayout);
        }
        let node = self.nodes.get_mut(&id).ok_or(OpError::NoSuchNode(id))?;
        let old = std::mem::replace(&mut node.item, *item);
        let parent = node.parent;
        dirty.0.insert(id);
        if let Some(p) = parent {
            dirty.0.insert(p);
        }
        Ok(Operation::SetLayoutItem { id, item: old })
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
    // None of these touch `dirty`: a guide is egui chrome over the canvas, not
    // drawn by the renderer, so there is nothing cached to invalidate.

    fn op_add_guide(&mut self, guide: Guide) -> Result<Operation, OpError> {
        if self.guides.iter().any(|g| g.id == guide.id) {
            return Err(OpError::DuplicateGuide(guide.id));
        }
        // All three writers of a guide's position check it (this,
        // `op_set_guide_position`, `op_set_guide_scope`): the loader refuses a
        // non-finite one, so any door that does not saves a file that will not
        // reopen.
        if !guide.position.is_finite() {
            return Err(OpError::NonFinite);
        }
        self.check_guide_owner(guide.owner)?;
        self.guides.push(guide);
        Ok(Operation::RemoveGuide { id: guide.id })
    }

    /// A guide may only be scoped to a frame that is in the document.
    ///
    /// **The check makes op order inside a delete transaction load-bearing.**
    /// `apply` inverts `[a, b]` to `[b⁻¹, a⁻¹]`, so deleting a frame *before*
    /// removing its guides would invert to `[AddGuide, InsertSubtree]`, which this
    /// refuses and takes the whole undo with it. [`crate::build::guides_of`]
    /// establishes the order, and `undoing_a_frame_delete_restores_its_guides`
    /// holds it.
    fn check_guide_owner(&self, owner: Option<NodeId>) -> Result<(), OpError> {
        let Some(owner) = owner else { return Ok(()) };
        let node = self
            .nodes
            .get(&owner)
            .ok_or(OpError::BadGuideOwner(owner))?;
        // A frame, not any container: the scope is a box to clip the line to and
        // measure the offset from, and a group has neither. A guide on the canvas
        // is spelled `None`, so the root is excluded too.
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
    // Neither touches `dirty`: an image is drawn but changes nothing `Resolved`
    // caches, since bounds come from the shape and the scene is walked afresh
    // every frame.

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
        // The loader refuses a non-finite guide position (`io::schema`), so the
        // operation must too.
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
    /// read by the next rotate or mirror. Only the node is dirtied, so the canvas
    /// redraws the marker.
    fn op_set_pivot(
        &mut self,
        id: NodeId,
        pivot: Option<Pivot>,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        // Invariant 8 (§15 D639): `serde_json` writes a non-finite pivot as `null`
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
    /// whose flag sits inert: the walk has to decide whether a mask paints, so a
    /// state with no answer must not get into the tree.
    ///
    /// Only `id` and its subtree are dirtied. Being masked is a fact about where
    /// a sibling sits; the parent's box is recomputed by `Resolved::update`'s
    /// walk up from here.
    fn op_set_mask(
        &mut self,
        id: NodeId,
        mask: bool,
        dirty: &mut DirtySet,
    ) -> Result<Operation, OpError> {
        let kind = &self.nodes.get(&id).ok_or(OpError::NoSuchNode(id))?.kind;
        if mask && !kind.can_mask() {
            return Err(OpError::WrongKindForOp);
        }
        // A group holding a frame cannot become a mask: its outline would be a
        // union with a page in it (§15 D876). `can_mask` already keeps a frame
        // itself off, so this is only ever about what is inside.
        if mask && self.holds_a_frame(id) {
            return Err(OpError::ArtboardPlacement);
        }
        let node = self.nodes.get_mut(&id).expect("looked up above");
        let old = node.mask;
        node.mask = mask;
        for d in self.subtree_ids(id) {
            dirty.0.insert(d);
        }
        Ok(Operation::SetMask { id, mask: old })
    }

    /// **No kind gate and no mask gate**: the mode is inert and kept on a layer
    /// that is not a mask — see [`Node::mask_mode`]. The subtree is dirtied like
    /// `op_set_mask`'s, since the mode changes how everything above is drawn.
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
    /// **The node alone, not the subtree**, unlike `op_set_mask_mode`: a fill rule
    /// changes only the shape it is on and is not inherited.
    ///
    /// Accepts any kind, as `op_set_mask_mode` does: on a kind with no outline it
    /// is inert and remembered, as `clip` is on a kind with no frame.
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

    /// Whether a frame may sit anywhere under `parent` — nothing from `parent`
    /// up to the root [`bars_frames`] (§15 D876).
    ///
    /// Terminates on any tree `apply` or the loader produced, both of which refuse
    /// a cycle; the walk is upward, so it is at most the tree's depth.
    ///
    /// Public because every place that says in advance what `apply` will accept
    /// has to ask it too — a menu row, a layers-panel drop, a render preview — or
    /// it offers what the commit refuses.
    pub fn frame_may_sit_under(&self, parent: NodeId) -> bool {
        let mut up = Some(parent);
        while let Some(id) = up {
            let Some(node) = self.nodes.get(&id) else {
                return true;
            };
            if bars_frames(node) {
                return false;
            }
            up = node.parent;
        }
        true
    }

    /// Whether `id`'s subtree holds a frame, `id` itself included — what a move
    /// under [`Self::frame_may_sit_under`]'s `false` refuses (§15 D876).
    pub fn holds_a_frame(&self, id: NodeId) -> bool {
        self.subtree_ids(id).iter().any(|d| {
            self.nodes
                .get(d)
                .is_some_and(|n| matches!(n.kind, NodeKind::Artboard { .. }))
        })
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
    // are reserved past as well.
    //
    // Item ids too (§15 D980), from the same stream (`IdSource::mint_item`): the
    // positional ids a migration writes are actor 0, and `session::random_actor`
    // is an unguarded hash, so a session could draw 0.
    let items = doc.nodes.values().flat_map(|n| {
        let p = &n.paint;
        p.fills
            .iter()
            .map(|k| k.id.0)
            .chain(p.strokes.iter().map(|k| k.id.0))
            .chain(n.effects.iter().map(|k| k.id.0))
            .chain(n.exports.iter().map(|k| k.id.0))
            .chain(n.grids.iter().map(|k| k.id.0))
            // The component properties' too — the sixth keyed list.
            .chain(n.props.iter().map(|k| k.id.0))
    });
    let highest = doc
        .nodes
        .keys()
        .copied()
        .chain(doc.guides.iter().map(|g| g.id.0))
        .chain(items)
        // Variables and modes too (§15 D1039), minted from the same stream.
        .chain(doc.variables.ids())
        // And styles (§15 D1062).
        .chain(doc.styles.ids())
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
            // A copy of a mask is a mask; the duplicate starts a second run at the
            // index it was inserted at.
            mask: n.mask,
            mask_mode: n.mask_mode,
            fill_rule: n.fill_rule,
            paint: n.paint.clone(),
            pivot: n.pivot,
            // A copy exports the way its original did; the two then name the same
            // files, which `plan` does not overwrite silently.
            exports: n.exports.clone(),
            effects: n.effects.clone(),
            // The grid is measured against the copy's size (`crate::layout::tracks`
            // stores nothing), so a resized duplicate re-flows its own tracks.
            grids: n.grids.clone(),
            // A copy placed elsewhere is re-pinned by the `SetTransform` that
            // places it (`build::keep_insets`); what carries over is which edges
            // are pinned and in what unit.
            insets: n.insets,
            display: n.display.clone(),
            item: n.item,
            // A copy of a main is a main, and a copy of a linked node keeps its
            // link (§15 D978) — **remapped when it points inside this template**,
            // so a copied main and a copied instance stay linked to each other
            // (§5.3d). Whether a copy of a main should be an instance instead is
            // the verb's decision.
            component: n.component,
            link: n.link.map(|l| map.get(&l).copied().unwrap_or(l)),
            // A swap is remapped only when the main came along — the link's rule
            // (§15 D983).
            swap: n.swap.map(|s| map.get(&s).copied().unwrap_or(s)),
            // A copied set is a set and a copied variant keeps its values (§15
            // D982); a property's bound layers are remapped with the ids, and one
            // bound outside the template is kept for `variant::settle` to unbind
            // where it lands.
            set: n.set.clone(),
            variant: n.variant.clone(),
            props: n
                .props
                .iter()
                .map(|p| {
                    p.map(|p| crate::variant::Property {
                        bound: p
                            .bound
                            .iter()
                            .map(|b| map.get(b).copied().unwrap_or(*b))
                            .collect(),
                        ..p.clone()
                    })
                })
                .collect(),
            description: n.description.clone(),
            // **Never carried** (§15 D1012): a copy of a vendored node does not live
            // in the vault, and `kit::check` refuses an origin outside it. The kit
            // verbs write the origin after minting their copies here.
            origin: None,
            // Bindings are ids into the document's table, not the template, so
            // nothing is remapped (§5.3f).
            mode: n.mode,
            corner_vars: n.corner_vars,
            spacing_vars: n.spacing_vars,
            visible_var: n.visible_var,
            size_vars: n.size_vars,
            text_vars: n.text_vars,
            opacity_var: n.opacity_var,
            styles: n.styles,
        });
    }
    Some((out, new_root?))
}

/// Whether `node` bars a frame from sitting anywhere beneath it: a boolean or a
/// mask (§15 D876, the maintainer's ruling).
///
/// **An ancestor rule, which is why it is not [`crate::build::can_parent`]'s.**
/// A frame may sit in a group (§15 D870), which can sit in a boolean or under a
/// mask at any depth; a boolean's operands are outlines and a frame's box is not
/// its outline, and a page inside a mask's union would clip other pages. Every
/// door into the tree asks it: `op_create_node`, `op_insert_subtree`,
/// `op_reparent`, `op_set_mask` and the loader's `verify_integrity`.
pub(crate) fn bars_frames(node: &Node) -> bool {
    matches!(node.kind, NodeKind::Boolean { .. }) || node.mask
}

/// Enforce the Root/Artboard structural rules (§5.3): an Artboard hangs off the
/// root, another Artboard or a group, and nothing may parent a second Root.
///
/// A parent rule only: whether a frame may sit under a boolean or a mask further
/// up is [`bars_frames`]'s question, asked over the chain.
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
/// One list, so a new kind cannot be paintable to the panels and unpaintable to
/// the op layer (§15 D400).
fn is_paintable(kind: &NodeKind) -> bool {
    kind.takes_paint()
}

/// Whether an export spec carries values that can be a size and a quality
/// (§15 D492).
///
/// **Deliberately the same predicate `ExportScale::from_label` applies** (`> 0`
/// for a pixel count, finite and positive for a multiplier); it is restated
/// because `from_label` parses a string and this asks about a value.
///
/// `quality` is `1..=100`, what the JPEG encoder takes; `0` is out of range.
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
    // Before the `mem::replace` arms, none of which looks at its value:
    // `svg_in`'s `numbers()` and `BezPath::from_svg` do not filter for finite.
    // See [`OpError::NonFinite`].
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
    //! `InsertSubtree` validation. In-crate because the tests build a `Node` by
    //! struct literal (`pub(crate)` fields) to synthesize malformed subtrees that
    //! capture + remap can never produce but an MCP write tool, an importer or a
    //! paste could.

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
            insets: Default::default(),
            display: None,
            item: Default::default(),
            component: false,
            link: None,
            swap: None,
            set: None,
            variant: Vec::new(),
            props: Vec::new(),
            description: String::new(),
            origin: None,
            mode: None,
            corner_vars: Default::default(),
            spacing_vars: Default::default(),
            visible_var: None,
            size_vars: Default::default(),
            text_vars: Default::default(),
            opacity_var: None,
            styles: Default::default(),
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

    fn card(id: NodeId, parent: NodeId) -> Node {
        node(
            id,
            Some(parent),
            vec![],
            NodeKind::Artboard {
                size: Size::new(10.0, 10.0),
            },
        )
    }

    /// A frame may sit in a group, and never under a boolean or a mask, inside an
    /// inserted subtree or above it (§15 D870, D876). Each parent–child pair in
    /// the refused cases is legal alone, so only the chain says no.
    ///
    /// flip: the `barred` seed forced to `false` → fails at "a mask above the
    /// paste"; `bars_frames` not carried down the walk → fails at "a boolean
    /// inside the paste".
    #[test]
    fn a_frame_in_an_inserted_subtree_is_refused_under_a_boolean_or_a_mask_only() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xAA);

        let (g, inner) = (ids.mint(), ids.mint());
        insert(
            &mut doc,
            vec![group(g, None, vec![inner]), card(inner, g)],
            ab,
        )
        .expect("a card in a group is a row of cards");

        let (b, g, inner) = (ids.mint(), ids.mint(), ids.mint());
        let nodes = vec![
            node(
                b,
                None,
                vec![g],
                NodeKind::Boolean {
                    op: crate::node::BoolOp::Union,
                },
            ),
            group(g, Some(b), vec![inner]),
            card(inner, g),
        ];
        let got = insert(&mut doc, nodes, ab);
        assert!(
            matches!(got, Err(OpError::ArtboardPlacement)),
            "a boolean inside the paste: {got:?}"
        );

        let masked = ids.mint();
        let mut m = group(masked, None, vec![]);
        m.mask = true;
        insert(&mut doc, vec![m], ab).expect("an empty mask group");
        let (g, inner) = (ids.mint(), ids.mint());
        let got = insert(
            &mut doc,
            vec![group(g, None, vec![inner]), card(inner, g)],
            masked,
        );
        assert!(
            matches!(got, Err(OpError::ArtboardPlacement)),
            "a mask above the paste: {got:?}"
        );
    }

    /// The other three doors hold the same rule (§15 D876): creating a frame under
    /// a mask, reparenting a group that holds a frame into a boolean, and turning
    /// such a group into a mask. The controls beside each say the refusals are
    /// about the frame.
    ///
    /// flip: each door's check disabled alone → fails at its own assertion
    /// ("create under a mask", "reparent into a boolean", "mask a group with a
    /// card in it").
    #[test]
    fn create_reparent_and_set_mask_refuse_a_frame_under_a_boolean_or_a_mask() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xAB);
        let (plain, masked, boolean, a, b) =
            (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let create = |id, parent, kind| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        let rect = || NodeKind::Rect {
            size: Size::new(5.0, 5.0),
            corner_radii: Default::default(),
        };
        let frame = || NodeKind::Artboard {
            size: Size::new(5.0, 5.0),
        };
        doc.apply(&Transaction(vec![
            create(plain, ab, NodeKind::Group),
            create(masked, ab, NodeKind::Group),
            create(a, masked, rect()),
            Operation::SetMask {
                id: masked,
                mask: true,
            },
            create(
                boolean,
                ab,
                NodeKind::Boolean {
                    op: crate::node::BoolOp::Union,
                },
            ),
            create(b, boolean, rect()),
        ]))
        .expect("the fixture");

        let (in_plain, in_masked) = (ids.mint(), ids.mint());
        assert!(
            doc.apply(&Transaction(vec![create(in_plain, plain, frame())]))
                .is_ok(),
            "the control: a frame in a plain group"
        );
        let got = doc.apply(&Transaction(vec![create(in_masked, masked, frame())]));
        assert!(
            matches!(got, Err(OpError::ArtboardPlacement)),
            "create under a mask: {got:?}"
        );

        let bare = ids.mint();
        doc.apply(&Transaction(vec![create(bare, ab, NodeKind::Group)]))
            .unwrap();
        doc.apply(&Transaction(vec![create(ids.mint(), bare, rect())]))
            .unwrap();
        assert!(
            doc.apply(&Transaction(vec![Operation::Reparent {
                id: bare,
                new_parent: boolean,
                index: 0,
            }]))
            .is_ok(),
            "the control: a group with no frame becomes an operand"
        );
        let got = doc.apply(&Transaction(vec![Operation::Reparent {
            id: plain,
            new_parent: boolean,
            index: 0,
        }]));
        assert!(
            matches!(got, Err(OpError::ArtboardPlacement)),
            "reparent into a boolean: {got:?}"
        );

        let got = doc.apply(&Transaction(vec![Operation::SetMask {
            id: plain,
            mask: true,
        }]));
        assert!(
            matches!(got, Err(OpError::ArtboardPlacement)),
            "mask a group with a card in it: {got:?}"
        );
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

    /// This door refuses a non-finite pivot as it does the other coordinates
    /// (§15 D639); the value saves as `null` and kills the file on reload. The
    /// route is a paste (`io::clip`, OS-clipboard text no operation guard has
    /// seen), not only in-crate construction.
    ///
    /// Both variants, because `Normalized` holds a `Vec2` and `Local` a `Point`,
    /// and a guard written against one misses the other.
    ///
    /// flip: the clause in `op_insert_subtree`'s field loop → fails at
    /// "Normalized(…) was inserted"; only `Pivot::is_finite`'s `Local` arm →
    /// fails at "Local(…) was inserted".
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

    /// The pivot arm's twin for effects (§15 D641): an infinite blur radius makes
    /// `effect::escaped` report no escape at all, before any file is written.
    ///
    /// flip: the `effect_is_finite` clause in the field loop → fails at the
    /// `NonFinite` assertion; `op_set_effects`' own guard does not cover this door.
    #[test]
    fn insert_subtree_refuses_a_non_finite_effect() {
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xEF);
        let g = ids.mint();
        let mut n = group(g, None, vec![]);
        n.effects = crate::keyed_by_position([crate::Effect::new(crate::EffectKind::LayerBlur {
            radius: f64::INFINITY,
        })]);
        assert!(matches!(
            insert(&mut doc, vec![n], ab),
            Err(OpError::NonFinite)
        ));
        assert!(!doc.contains(g));
    }

    /// Both effect doors refuse a blur past `effect::MAX_BLUR` and take one at
    /// it, and take any in a history step. Flip: either `blur_within_cap` check
    /// removed → that door's `Err` assertion fails; its `!self.restoring` removed
    /// → that door's history-step `expect` fails.
    #[test]
    fn a_blur_past_the_cap_is_refused_at_both_doors() {
        let blur = |radius| {
            crate::keyed_by_position([crate::Effect::new(crate::EffectKind::LayerBlur { radius })])
        };
        let shadow = |b| {
            crate::keyed_by_position([crate::Effect::new(crate::EffectKind::DropShadow(
                crate::Shadow {
                    blur: b,
                    ..crate::Shadow::default()
                },
            ))])
        };
        let cap = crate::effect::MAX_BLUR;
        let (mut doc, _root, ab) = base();
        let mut ids = IdSource::new(0xF0);
        let g = ids.mint();
        insert(&mut doc, vec![group(g, None, vec![])], ab).expect("a group");
        let set = |doc: &mut Document, effects| {
            doc.apply(&Transaction(vec![Operation::SetEffects { id: g, effects }]))
        };
        set(&mut doc, blur(cap)).expect("a blur at the cap");
        set(&mut doc, shadow(cap)).expect("a shadow at the cap");
        assert!(matches!(
            set(&mut doc, blur(cap + 1.0)),
            Err(OpError::BlurTooLarge)
        ));
        assert!(matches!(
            set(&mut doc, shadow(1e30)),
            Err(OpError::BlurTooLarge)
        ));

        let h = ids.mint();
        let mut n = group(h, None, vec![]);
        n.effects = blur(1e30);
        assert!(matches!(
            insert(&mut doc, vec![n.clone()], ab),
            Err(OpError::BlurTooLarge)
        ));
        assert!(!doc.contains(h));

        // A history step puts back what the document held, so neither door
        // refuses one.
        let restore = |doc: &mut Document, op| doc.apply_restoring(&Transaction(vec![op]));
        restore(
            &mut doc,
            Operation::SetEffects {
                id: g,
                effects: blur(cap + 1.0),
            },
        )
        .expect("SetEffects in a history step");
        restore(
            &mut doc,
            Operation::InsertSubtree {
                parent: ab,
                index: 0,
                nodes: vec![n],
            },
        )
        .expect("InsertSubtree in a history step");
        assert!(doc.contains(h));
    }

    /// The pivot arm's twin for **paint** (§15 D831): `op_set_fills` and
    /// `op_set_strokes` ask `build::brush_is_finite` (§15 D451), and so must this
    /// door, which a paste reaches.
    ///
    /// Five values, one per clause: a fill's colour, a stroke's brush, a stroke's
    /// `width`, a dash length, and a gradient's `opacity` (finite for
    /// `valid_opacity`, so refused only here, §15 D767). The stroke clauses are
    /// why this arm restates `op_set_strokes`' predicate.
    ///
    /// flip: the fills clause → fails at "fill was inserted"; the strokes clause
    /// → fails at the stroke cases; `s.width.is_finite()` alone → fails at
    /// "stroke width was inserted" with both brush cases green.
    #[test]
    fn insert_subtree_refuses_non_finite_paint() {
        let bad = peniko::Color::new([f32::NAN, 0.0, 0.0, 1.0]);
        let cases: Vec<(&str, Paint)> = vec![
            (
                "fill",
                Paint {
                    fills: crate::keyed_by_position([Fill {
                        brush: crate::Brush::Solid(bad).into(),
                        visible: true,
                    }]),
                    ..Paint::default()
                },
            ),
            (
                "stroke brush",
                Paint {
                    strokes: crate::keyed_by_position([Stroke {
                        brush: crate::Brush::Solid(bad).into(),
                        ..Stroke::default()
                    }]),
                    ..Paint::default()
                },
            ),
            (
                "stroke width",
                Paint {
                    strokes: crate::keyed_by_position([Stroke {
                        width: f64::INFINITY,
                        ..Stroke::default()
                    }]),
                    ..Paint::default()
                },
            ),
            (
                "dash length",
                Paint {
                    strokes: crate::keyed_by_position([Stroke {
                        dashes: vec![4.0, f64::NAN],
                        ..Stroke::default()
                    }]),
                    ..Paint::default()
                },
            ),
            (
                // The only case a clipboard payload can actually carry: JSON cannot
                // produce a non-finite `f64`, so the `NaN` cases above die at
                // `from_str`. `1e30` is finite and readable and refused only here
                // (§15 D767). Not `1e39`: the field is an `f32`, and `inf` would
                // be caught by the finiteness half, not the range half pinned here.
                "gradient opacity",
                Paint {
                    fills: crate::keyed_by_position([Fill {
                        brush: crate::Brush::Gradient({
                            let mut g =
                                crate::image::GradientBrush::from(peniko::Gradient::default());
                            g.opacity = 1e30;
                            g
                        })
                        .into(),
                        visible: true,
                    }]),
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

    /// A cycle balances the child count (§15 D832): `[R, A, B]` with `A` and `B`
    /// each other's parent and child has a single root, claims each once, and
    /// `2 + 1 == 3`, yet is reachable from nothing.
    ///
    /// The fixture passes every other check in the function (single root, unique
    /// ids, children listed once, consistent back-pointers, legal kinds), so
    /// `MalformedSubtree` can only come from the reachability walk.
    ///
    /// flip: the walk replaced by `claimed.len() + 1 != nodes.len()` → fails at
    /// "a cycle was inserted"; no other `ondin-core` target covers it.
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

    /// The depth bound on an inserted subtree (§15 D832): a 300-deep chain
    /// arrives in one paste and would fail the loader's check (6) on reload.
    ///
    /// The second case is the one a subtree-relative check gets wrong: two chains
    /// each inside `MAX_TREE_DEPTH` compose to a document that is not, so the
    /// bound reads the parent's depth.
    ///
    /// flip: `parent_depth` pinned to `0` → fails at "two halves composed past
    /// the bound".
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
        // The seed is a letter pair because the D-number census reads a bare
        // hex `D` plus digits as a dangling citation.
        let mut ids = IdSource::new(0xDA);
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
