# Core model: modules, identity, nodes, document

`ondin-core` holds the document model: ids, the node tree, the `Document` that owns it, and the non-node document state (canvas background, guides). This file covers §5.1 (module layout), §5.2 (identity), §5.3 (node model) and §5.5 (document). Operations, builders, history, `Resolved` and queries are in `core-ops.md`.

## 5. ondin-core

### 5.1 Module layout

```
ondin-core/src/
├─ lib.rs
├─ id.rs           # NodeId { actor, seq }, IdSource
├─ node.rs         # Node, NodeKind, Paint, Fill, Stroke, TextStyle, Pivot
├─ effect.rs       # Effect, EffectKind, Shadow, Filters — the effect stack and its escape (§5.3a)
├─ guide.rs        # Guide, GuideId, GuideAxis — ruler guides; document state, not nodes (§5.5)
├─ layout.rs       # LayoutGrid, GridAxis, GridAlign, tracks() — a frame's columns and rows;
│                  #   chrome over a node, never in the scene (§5.3b)
├─ document.rs     # Document, private tree ops, apply(), remap_subtree, reserve_existing_ids
├─ op.rs           # Operation, Transaction, OpError, ApplyOutcome, DirtySet
├─ build.rs        # composite + bulk-property builders, world-space projection facade (§5.7a)
├─ naming.rs       # the names the app generates for layers, and their numbering (§5.7b)
├─ history.rs      # History (undo/redo stacks)
├─ resolve.rs      # Resolved: world transforms, bounds, text layout cache, rstar index
├─ query.rs        # hit_test, nodes_in_view, bounds, group_chain, pivot_world
├─ geometry.rs     # local bounds per NodeKind; exact hit-testing (point-in-path, stroke distance);
│                  #   pivot_point / pivot_for — the one place a Pivot is resolved and chosen (§5.3)
├─ typography.rs   # the text attribute model (§5.4): Length, TextStyle, ParagraphStyle, BlockStyle,
│                  #   one generic Spans<A> over CharAttr/ParaAttr, and the boundary rule
├─ text.rs         # parley shaping/measurement + TextEdit (caret model); bundled fallback font;
│                  #   font metadata (fvar axes, named instances, GSUB/GPOS features) via skrifa
└─ io/
   ├─ mod.rs       # save/load entrypoints
   ├─ schema.rs    # versioned on-disk DTO + full tree-integrity verification
   └─ migrate.rs   # migrations old→current
```

Other modules are named in their own sections: `container` (layout engine, §5.3c), `component`/`propagate`/`variant`/`swap`/`reset` (§5.3d), `kit` (§5.3e), `variable`/`style`/`tokens` (§5.3f), `boolean`, `item` (`Keyed<T>`), `paint_box` (gradients following the box, §6.2).

### 5.2 Identity

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct NodeId { pub actor: u64, pub seq: u64 }

pub struct IdSource { actor: u64, next_seq: u64 } // one per editing session; actor = random u64
```

- Each editing session (GUI, `serve`, an MCP client acting through either) shares the session's `IdSource`. A random 64-bit actor makes cross-session collisions statistically impossible with no coordination — the property multiplayer needs.
- The document persists no allocator; validation only requires that a created id not already exist. Ids of deleted nodes are never re-minted (monotonic `seq` per actor).
- Wire form: `"a1b2c3:42"` (actor hex, `:`, seq decimal). `NodeId::from_wire` is wider than `to_wire`: it also accepts a leading `+`, uppercase hex and leading zeroes on either half (an empty half is refused). Each alias parses to the same id, so duplicate checks and cross-references still work. `from_wire_accepts_the_canonical_form_and_six_aliases_of_it` pins this; rejecting aliases would be a compatibility decision, not a repair.
- `GuideId(NodeId)` is a newtype over the same id, minted from the same `IdSource` (§5.5). One counter means guide and node ids never collide and `reserve_existing_ids` covers both in one walk; the wrapper stops them being passed to each other's functions. Same wire form.

### 5.3 Node model

```rust
pub struct Node {
    id: NodeId,
    parent: Option<NodeId>,
    children: Vec<NodeId>,     // ordered = z-order (last = topmost within parent)
    kind: NodeKind,
    transform: kurbo::Affine,  // LOCAL to parent; translate/rotate/skew/flip — never scale, see §5.6
    name: String,
    visible: bool,
    locked: bool,              // on a container it locks the contents too
    proportions_locked: bool,  // a resize holds the aspect ratio; NOT `locked`
    opacity: f32,              // 0.0..=1.0
    clip: bool,                // clip children to this node's own frame
    mask: bool,                // this layer paints nothing and clips the SIBLINGS above it
    mask_mode: MaskMode,       // HOW it masks; remembered while `mask` is off
    paint: Paint,              // shape/text kinds AND frames; empty for Root/Group
    effects: Vec<Keyed<Effect>>, // shadows, blur, colour filters — §5.3a; meaningful on containers too
    pivot: Option<Pivot>,      // transform origin; None = centre of own box
    exports: Vec<Keyed<ExportSpec>>, // files this layer produces — §7
    grids: Vec<Keyed<LayoutGrid>>,   // columns/rows drawn over this layer — §5.3b; frames only
    insets: Insets,            // CSS top/right/bottom/left + auto margins inside its frame — §5.3c
    display: Option<Display>,  // Flex(..) or Grid(..); None = no layout (NOT CSS display:none)
    item: LayoutItem,          // flex-grow/-shrink/-basis, align-self, justify-self, grid-column/-row,
                               // width/height and limits (and the limits' bindings, §5.3f) — one record for both layouts
    component: bool,           // a MAIN component — frames and groups only — §5.3d
    link: Option<NodeId>,      // the node this was copied from, ONE level up
    swap: Option<NodeId>,      // the main a NESTED COPY shows instead of its link's — §5.3d
    set: Option<VariantSet>,   // a COMPONENT SET's variant properties — a frame, never a main
    variant: Vec<String>,      // a variant's values, one per property of the set around it
    props: Vec<Keyed<Property>>, // component properties a main or set defines
    origin: Option<kit::Origin>, // where a VAULT node was vendored from — §5.3e; vault nodes only
    mode: Option<ModeId>,      // the mode a FRAME sets for itself and contents; None inherits — §5.3f
    corner_vars: [Option<VarId>; 4], // a Rect's corner bindings, RoundedRectRadii order — §5.3f
}

pub enum NodeKind {
    Root,
    Artboard { size: kurbo::Size },         // ground = `paint`'s fill list
    Group,
    Rect    { size: kurbo::Size, corner_radii: kurbo::RoundedRectRadii },
    Ellipse { size: kurbo::Size },
    Polygon { size: kurbo::Size, sides: u32 },                    // regular, first vertex up
    Star    { size: kurbo::Size, points: u32, inner_ratio: f64 },
    Line    { end: kurbo::Point },          // start is always the local origin
    Path    { path: kurbo::BezPath },
    Text    { content: String, style: TextStyle, sizing: TextSizing },
}

pub struct Paint { pub fills: Vec<Keyed<Fill>>, pub strokes: Vec<Keyed<Stroke>> } // list order = paint order

pub enum Pivot {
    Normalized(kurbo::Vec2),   // a fraction of the node's own box: (0.5, 0.5) is the centre
    Local(kurbo::Point),       // a point in the node's local space
}

pub enum MaskMode { Shape, Alpha }   // outline, or the mask's own alpha per pixel; Shape is default

pub struct Fill   { pub brush: BrushSource, pub visible: bool }
pub enum BrushSource { Literal(Brush), Bound { var: VarId, alpha: f32 } } // colour variable × fill's %;
                                     // no Deref — drawn through `Document::resolved_paint`
pub struct Stroke {
    pub brush: BrushSource,
    pub width: f64,            // the literal; 0 while `width_var` binds it
    pub width_var: Option<VarId>, // number variable, IN the item (so styles, reorders, copies carry it)
    pub join: kurbo::Join,
    pub cap: kurbo::Cap,
    pub dashes: Vec<f64>,      // on, off, on… empty = solid; the ONLY dash state stored
    pub dash_offset: f64,
    pub align: StrokeAlign,    // which side of the outline the width lies on (§6.3)
    pub sides: StrokeSides,    // which of a box's four sides; Rect/Artboard only
    pub miter_limit: f64,      // the RATIO kurbo and stroke-miterlimit take, not the angle shown
    pub dash_fit: bool,        // scale the pattern so whole periods span the outline (§6.3)
    pub visible: bool,
}

pub enum StrokeSides { All, Top, Right, Bottom, Left,
                       Custom { top: f64, right: f64, bottom: f64, left: f64 } } // 0 = no stroke there
pub enum Side { Top, Right, Bottom, Left }   // plus `Side::ALL`
pub enum DashStyle { Solid, Dotted, Dashed, Custom }  // presentation only: classified, never stored
```

Rules and reasons:

- All geometry is authored in the node's **local** space; `transform` positions it under the parent. `Line` stores only `end`; the start is the local origin, removing the "move endpoints vs. move transform" ambiguity.
- **`Path` holds a `BezPath`.** A vector network is a different model, not a richer one, and every reader (scene walk, `core::boolean`, SVG writer, save format) takes the `BezPath` as the whole geometry. No half-step to a network exists; vector networks are planned in `roadmap.md` as a model change.
- **Frame placement.** An `Artboard` hangs off `Root`, another `Artboard` or a `Group`, never under a `Boolean` or a mask. `build::can_parent` states the *parent* rule; `Document::frame_may_sit_under` the *ancestor* rule (walks to root for a boolean, or a node with `mask` set — `document::bars_frames`, since a group can carry a frame into an operand or mask at any depth). Enforced in create, insert, reparent, `SetMask` on a node whose subtree holds a frame, and the loader; violation is `OpError::ArtboardPlacement`. Frames nest (a card or component inside a page) and sit in groups (a flex row of cards is a group of frames). A frame as masked content (a sibling above a mask) is allowed.
- **`Artboard` is the type name; "frame" is the user's word.** Every string a designer reads says frame; the variant, serde tag and MCP snapshot's `"artboard"` keep the older word. (§15 D21)
- **Names** are written at creation or copy and never recomputed. Fresh nodes are numbered among new siblings; a copy increments a numbered name. Only the user and a boolean relabelled by an operation change (§9.4) rewrite it. Duplicate sibling names are legal (§5.7b).
- **`Root` may hold loose shapes.** A layer dragged out of a frame needs a destination, and "no frame" is honest. Creation agrees: a shape drawn outside every frame is created at the root. Frames follow the same rule as shapes in both directions, unless a group is above, when the node stays in that group (§9.4).
- **`clip` is node-level, not an `Artboard` field.** It is a property of the layer like `visible`; `NodeKind::clips_children()` spells which kinds have a frame to clip to. Defaults on for frames. The DTO projection and loader both normalise an inert `clip` so it neither reaches a document nor survives a round trip (otherwise a first open-and-save moves bytes). The flag *can* be set on a kind that ignores it (`op_set_clip` lets it sit). Readers (`scene::paint_node`, SVG writer, both `bounds_cover_subtree` copies, `menu.rs`) read `Node::clip` themselves and kind-gate with a `match`. Clip off also turns off subtree culling against the frame (§6.1).
- **`mask`** is node-level and not the same switch: a masking node paints nothing; its outline clips the **run of siblings above it** in the same parent, up to the next mask or end of list (a second mask begins a second run). `clip` cuts a container's children to its own frame; `mask` cuts its siblings to its own outline. `Document::governing_mask` states the run rule (nearest preceding sibling with the flag; `None` for a mask itself). It is read two ways that must agree: the hit test asks backwards, node by node; `scene::paint_node`, the SVG writer's child loop and the two bounds passes in `resolve` carry it forwards (`MaskRun`, `mask_box`). If they disagree, fix the forward reader. Both spellings are asserted against the same sibling arrangement (`ops::a_mask_governs_up_to_the_next_one_and_is_never_masked_itself`, `render::a_second_mask_ends_the_first_ones_run_instead_of_nesting`). Nothing masks inside a `Boolean` (children are operands), so the flag there is inert — and the hit test must honour that or a pasted mask makes part of a boolean unclickable. `NodeKind::can_mask()` = `!Artboard && (takes_paint() || Group)`; Root and Artboard are excluded in writing (a frame already has `clips_children`). Unlike `clip`, `SetMask` is **refused** on a kind that cannot honour it (`WrongKindForOp`), because the walk must decide whether the layer paints. (§15 D282)
- **`mask_mode`** is a field beside the flag so the mode survives the flag going off and on (an `Option` payload would lose *Alpha* on toggle). It is writable on a non-mask; `SetMaskMode` is its own op. `Shape` = hard-edged outline clip (default); `Alpha` = the mask's own alpha per pixel. **Luminance is absent on a dependency, not a decision**: vello's GPU `push_layer` takes no mask and no Porter-Duff compose moves colour into alpha, so it cannot be expressed in the compositor and a CPU-only version would make canvas and export disagree. (§15 D288)
- **`NodeKind::takes_paint()`** is the paint predicate beside `clips_children()`. `Root` (not a layer) and `Group` (no box) draw no fills; a frame does — its fills are `Node::paint`'s list, drawn behind children, strokes over them and outside the clip (§6.2). `Boolean` is a container that *is* a shape by the time it draws. The inspector's paint panels and the bulk colour edits both ask it. There is no `takes_stroke`: a frame's strokes are an ordinary stroke list, so `op_set_strokes` and the Stroke panel ask `takes_paint` too.
- **Predicate matrix.** `clips_children`, `can_mask`, `takes_paint`, `resolve::is_indexed`, `geometry::stroke_sides_apply` and `query::steps_into` each answer `false` for a variant nobody added, which is not a compile error. `node::kind_fixture` samples every variant behind an exhaustive wildcard-free `tag`; each predicate is asserted as the pair of name lists it divides the enum into (the *rejected* set does the work). `is_indexed` and `takes_paint` have identical membership today and are asserted side by side. The fixture's completeness is guarded only by the compile error in `tag`.
- **`proportions_locked`** shapes an edit already allowed (a resize holds aspect as if Shift were held, §9.4); `locked` refuses it. The two are independent. No kind gate — inert elsewhere. `remap_subtree` copies it. Set by `SetProportionsLocked`, saved.
- **`pivot`** is the transform origin: what rotation turns about and a mirror reflects across (not what Alt-resize holds still). `None` means the centre of the node's own box and is stored as an absence so the model can say "the user has not moved it" (the canvas marker draws only where it means something; files that never moved one omit it). `geometry::pivot_point` resolves it against a box; **nothing composes it into the world transform** — gestures bake it into the transform they produce. The two doors that can ask for the absence (marker dragged to the middle; Transform card's origin typed 50/50) go through `geometry::pivot_placed_at`, which answers `None` for the box centre, so both store the same thing. An axis with no extent is centred by itself.
- **`Pivot` variants:** normalized where the box is authored (a rect: a fraction stays a fraction under resize), absolute where it is emergent (group union, line, path, auto-sized text; becomes normalized once text is dragged to a fixed box). `pivot_for` makes the choice off the same question as `tools::resizable_size`. Both variants are refused when a coordinate is not finite (`Pivot::is_finite`, at `SetPivot` and `InsertSubtree`'s field loop; a `NaN` saves as `null` and the document never reopens — invariant 8).
- **`fills` and `strokes` are lists of any length; list order is paint order**: every visible fill then every visible stroke, each in `Vec` order, last painted last. `StrokeAlign::Outside` covers outlined text, so two stacks suffice. The inspector shows each list reversed (top row = last element).
- **`effects`** is a list of the same shape, not inside `Paint`: order is compositing order and two drop shadows at different distances is normal. It sits on the node because it is meaningful on containers (a shadow on a group, a blur over a frame), which is why `SetEffects` is gated only on `Root`. See §5.3a.
- **`sides`** needs four nameable sides: `Rect` and `Artboard` only (`geometry::stroke_sides_apply`). `geometry::effective_sides(kind, stroke)` is what consumers ask, so the stored value is never rewritten and an unsupported stroke draws the whole outline. `Custom`'s widths are independent of `width`; `0` switches a side off. The panel gates the control rather than clamping the value (same bargain as `stroke_align_applies`). (§15 D68)
- **`dashes`** is the only dash state; `Stroke::dash_style` classifies it from the *shape* (invariant under width), so no stored mode can drift. A dot is a zero-length dash `[0, s]`, making `s` the centre-to-centre spacing. Zero-length dashes drawing nothing under two of three caps and fitting a pattern to an outline length are resolved at the render boundary (§6.3). `dash_fit` is a flag: writing fitted numbers back would read "on" while true only until the shape resizes. (§15 D69)
- **`miter_limit` is the ratio** (1/sin(θ/2); default 4 ≈ 28.955°). The panel shows degrees; `node::miter_ratio_of_degrees`/`miter_degrees_of_ratio` convert at that one edge, and the panel converts back only when the angle moved (a non-bit-exact round trip would write a ratio a few ulps off past `strokes_tx`'s equality guard on every blur). `MIN_MITER_DEGREES` = 1°. `join` is kind-gated (`geometry::stroke_join_applies`); `miter_limit` is not.
- **Rect corners are always four radii**, never a scalar with an override. `GeometryPatch::CornerRadius(f64)` is a convenience over the same field; its inverse is a `CornerRadii`. Flattening is a real edit and must be asked for: the inspector's single field shows `largest(corners)` over a mixed set and writes only where the field differs from its seed. The per-corner disclosure is app view state. Radii over half the shorter side are clamped where the outline is built (`geometry::local_path`), not on the way in, so shrink-then-grow keeps the typed number.
- **`Polygon` and `Star` fill their box.** Vertices are placed on a circle then stretched so the outline's bounding box is exactly `0,0..w,h` (keeps `local_bounds`, selection, snaps and handles honest). One builder `geometry::star_path` (polygon = star with no inner ring), first vertex up. `sides`/`points` (3..=64) and `inner_ratio` are clamped where the outline is built.

### 5.5 Document

```rust
pub struct Document {
    schema_version: u32,
    nodes: FxHashMap<NodeId, Node>,
    root: NodeId,
    vault: Option<NodeId>,              // a SECOND parentless Root holding vendored kit mains — §5.3e
    canvas_background: peniko::Color,   // the ground behind and around the frames
    guides: Vec<Guide>,                 // ruler guides; unordered
    published: Option<Arc<kit::Published>>, // a kit's last publish, frozen; outside undo — §5.3e
}
impl Document {
    pub fn new(root_id: NodeId) -> Self;
    pub fn get(&self, id: NodeId) -> Option<&Node>;
    pub fn root(&self) -> NodeId;
    pub fn vault(&self) -> Option<NodeId>;
    pub fn in_vault(&self, id: NodeId) -> bool;   // the vault or under it — never resolved
    pub fn published(&self) -> Option<&kit::Published>;
    pub fn canvas_background(&self) -> peniko::Color;
    pub fn guides(&self) -> &[Guide];
    pub fn guide(&self, id: GuideId) -> Option<&Guide>;
    pub fn guides_owned_by(&self, owner: NodeId) -> impl Iterator<Item = &Guide>;
    pub fn apply(&mut self, tx: &Transaction) -> Result<ApplyOutcome, OpError>;
    // NO public mutable access to nodes or the tree.
}

pub struct Guide {                 // guide.rs
    pub id: GuideId,
    pub axis: GuideAxis,           // Horizontal | Vertical — the direction of the line itself
    pub position: f64,             // y for horizontal, x for vertical; owner-LOCAL if owned, else world
    pub color: Option<peniko::Color>,   // None = follow DEFAULT_GUIDE_COLOR
    pub owner: Option<NodeId>,     // the frame it is scoped to; None = the whole canvas
}
```

- **Two pieces of document state are not nodes:** `canvas_background` and `guides`. People set them for the artwork, not the sitting, so they are saved with the file; neither loosens the document/view-data line (§9.2, §9.3).
- `canvas_background` is the opaque ground behind the frames (`DEFAULT_CANVAS_BACKGROUND`), changed only through `SetCanvasBackground` (invariant 2). It is per document, not an `EditorSession` preference. (§15 D28)
- `guides` are the lines pulled from the rulers (§9.4). `color: None` follows `DEFAULT_GUIDE_COLOR` (`#a3332d`), so retuning the default carries every never-recoloured guide.
- **A guide is deliberately not a `Node`:** no parent, transform, paint or bounds; never rendered into the scene, never hit-tested as artwork, never in the layers tree or Select All. Making it a node would need all of that special-cased away.
- The list is **unordered** (no z, no operation names one by position). `apply` appends; IO sorts by id on save so bytes are stable (invariant 9).
- **Global or frame-scoped:** `owner: None` runs the full canvas with a world `position`; `Some(frame)` is drawn only within that frame and `position` is in the frame's local space. Chosen by where the pointer is when the drag ends (§9.4); a guide can move between modes. One flat list with an owner field (not per-node vectors) keeps `guides()`, `remove_all_guides` and the id-sorted save unchanged, and makes a scope change one undoable field edit (`SetGuideScope`).
- **The owner must be a live `Artboard`** (`Document::check_guide_owner` on `AddGuide` and `SetGuideScope` → `OpError::BadGuideOwner`; also the loader). A scope is a box to clip to and measure from; shapes, groups and root have none. **Deleting a frame must take its guides in the same transaction** (`build::guides_of`, nested frames included). `apply` also re-checks every guide's owner once after the last op (invariant 8), so a caller that never heard of `guides_of` gets a refused edit instead of an unopenable file; `guides_of` remains what makes the Delete verb succeed.
- **A scoped guide is stored frame-local**, so it rotates with its frame and stays clipped to it (nothing is destroyed; only snapping degrades, since a leaning guide has no single world x or y). A frame resize leaves the offset; the Scale tool multiplies it (§5.6). An offset outgrowing a shrunk frame is kept, not clamped; `guide_span` lets the clip hide it.

## Traps

- `Node::clip` is read directly by several readers, not via `clips_children()`; the flag is load-bearing on any kind the moment it is live.
- The mask run rule exists in forward and backward spellings; change both or the paired tests fail.
- A new `NodeKind` variant does not break any of the six `matches!` predicates at compile time; only `node::kind_fixture`'s `tag` does.
- `NodeId::from_wire` accepts aliases of the canonical form; duplicate detection still works because they parse equal.
- Guide owner must be a live `Artboard`; delete guides before the frame in a delete transaction or the undo (applied in reverse) fails.
- A bound stroke width stores `width = 0`; anything reading the literal needs `Document::resolved_paint`.
