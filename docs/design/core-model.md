# Core model: modules, identity, nodes, document

`ondin-core` holds the document model: ids, the node tree, the `Document` that owns it, its pages, and the non-node document state (each page's ground and draft flag, guides). This file covers §5.1 (module layout), §5.2 (identity), §5.3 (node model) and §5.5 (document). Operations, builders, history, `Resolved` and queries are in `core-ops.md`.

## 5. ondin-core

### 5.1 Module layout

```
ondin-core/src/
├─ lib.rs
├─ id.rs           # NodeId { actor, seq }, IdSource
├─ node.rs         # Node, NodeKind, Paint, Fill, Stroke, TextStyle, Pivot
├─ effect.rs       # Effect, EffectKind, Shadow, Filters — the effect stack and its escape (§5.3a)
├─ page.rs         # Page { id, ground, draft }, free_name; add, duplicate, delete — the page edits (§5.5)
├─ guide.rs        # Guide, GuideId, GuideAxis — ruler guides; document state, not nodes (§5.5)
├─ layout.rs       # LayoutGrid, GridAxis, GridAlign, tracks() — a frame's columns and rows;
│                  #   chrome over a node, never in the scene (§5.3b)
├─ document.rs     # Document, private tree ops, apply(), remap_subtree, reserve_existing_ids
├─ op.rs           # Operation, Transaction, OpError, ApplyOutcome, DirtySet
├─ build.rs        # composite + bulk-property builders, world-space projection facade (§5.7a)
├─ naming.rs       # the names the app generates for layers, and their numbering (§5.7b)
├─ history.rs      # History (undo/redo stacks of noted steps)
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

Other modules are named in their own sections: `container` (layout engine, §5.3c), `component`/`propagate`/`variant`/`swap`/`reset` (§5.3d), `kit` (§5.3e), `variable`/`style`/`tokens` (§5.3f), `boolean`, `edit` (the headless editing arithmetic the canvas's gestures call: resize, rotate, skew, the create builders, the pen's anchors and point edits, `tools.md`), `verbs` (the verb registry MCP, scripts and `ondin run` call, `core-ops.md`), `item` (`Keyed<T>`), `paint_box` (gradients following the box, §6.2), `fit` (curves fitted to a polygon, for *Simplify*, *Offset* and *Outline stroke*, `app-pen-and-nodes.md`), `symmetry` (a mirrored path's axes and whether they still hold, §5.3), `rename` (batch rename's text rule, `app-canvas-chrome.md`), `find` (find and replace in text layers' content, on `rename`'s matcher, `app-canvas-chrome.md`), `diff` and `merge` (`ondin diff`, `ondin merge`, `export.md`), `export` (a layer's export settings, §7), `svg_in` (SVG import, §7.1), `foreign` (what the readers of other tools' files share: the `Archive` trait the zip's entries come through, the `Unmapped` report rows and their `Tally`, `library.md`), `sketch_in`, `penpot_in` and `figma_in` (a `.sketch` or a `.penpot` file, or a Figma file fetched through the REST API, read into a new document, entries through `foreign::Archive` and pictures through `svg_in::ImageSink`, so core holds no zip reader, decoder or network client, `library.md`), `take` (the library import's census of a document and its trim to the chosen pages and parts, through `apply` with the editor's own builders, `library.md`), `image` (the image table, §5.5a), `families` (the font families a document names), `fallback_fonts` (the font subsets a text falls in and the family stack it is shaped against, §5.4a), `meta` (a document's library entry, §5.11a), and under `io/`, `clip` (the clipboard's wire form) and `probe` (the meta read from a file's first bytes).

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
    blend: BlendMode,          // how the layer mixes with what is beneath it; PassThrough on frames/groups only
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
    credit: Option<Box<credit::Credit>>, // an inserted icon's or photo's provenance and attribution; never drawn
    modes: Vec<ModeId>,        // the modes a FRAME or page sets for itself and contents, at most one per
                               // collection, id order; a collection with none inherits — §5.3f
    corner_vars: [Option<VarId>; 4], // corner bindings (frame, rect, polygon, star), RoundedRectRadii order — §5.3f
}

pub enum NodeKind {
    Root,
    Artboard { size: kurbo::Size, corner_radii: kurbo::RoundedRectRadii, corner_smoothing: f64 }, // ground = `paint`'s fill list
    Group,
    Rect    { size: kurbo::Size, corner_radii: kurbo::RoundedRectRadii, corner_smoothing: f64 },
    Ellipse { size: kurbo::Size, arc: EllipseArc },  // EllipseArc { start, sweep, ratio }
    Polygon { size: kurbo::Size, sides: u32, corner_radius: f64, corner_smoothing: f64 },  // regular, first vertex up
    Star    { size: kurbo::Size, points: u32, inner_ratio: f64, corner_radius: f64, corner_smoothing: f64 },
    Line    { end: kurbo::Point },          // start is always the local origin
    Spiral  { size: kurbo::Size, turns: f64 },   // open; fills its box as a polygon does
    Arc     { end: kurbo::Point, sweep: f64 },   // open; from the origin to `end`, as a line
    Path    { path: kurbo::BezPath, corner_radii: Vec<f64>, corner_smoothing: f64,
              mirror: symmetry::PathMirror }, // a radius per anchor; PathMirror { vertical, horizontal }
    Text    { content: String, style: TextStyle, sizing: TextSizing },
    Slice   { size: kurbo::Size },          // a region to export; draws nothing
}

pub struct Paint { pub fills: Vec<Keyed<Fill>>, pub strokes: Vec<Keyed<Stroke>> } // list order = paint order

pub enum Pivot {
    Normalized(kurbo::Vec2),   // a fraction of the node's own box: (0.5, 0.5) is the centre
    Local(kurbo::Point),       // a point in the node's local space
}

pub enum MaskMode { Shape, Alpha, Luminance } // outline, own alpha, or brightness × alpha per pixel; Shape is default

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
- **`Path` holds a `BezPath`.** A vector network is a different model, not a richer one, and every reader (scene walk, `core::boolean`, SVG writer, save format) takes the `BezPath` as the whole geometry. No half-step to a network exists, and networks are a non-goal (`non-goals.md`).
- **`Path::mirror`** names which of the path's own axes it is symmetric about, so point editing keeps it so (`app-pen-and-nodes.md`, *Mirrored paths*). The axes' positions are not stored: each runs through the centre of the path's control box (`symmetry::centre`), exact for any symmetric point set, so an axis cannot drift from the points. A flag is kept only while it holds (`symmetry::kept`: every segment's reflection is in the path, either way round, and every anchor's radius matches its image's, within 1e-6 of the path's extent). `op_create` and `GeometryPatch::Path` both apply it. The patch's `mirror: None` keeps the axes that still hold, `Some` sets them (also only where they hold), and its inverse always carries `Some(old)`, so undo restores an axis an edit dropped. A writer that knows nothing of mirrors (a resize bake, *Simplify*, *Offset*, a join) therefore keeps a correct axis, as a baked move or scale does, or drops it, never leaving a false one. Reset (`reset::state_ops`) writes the source's mirror back with `Some`.
- **Frame placement.** An `Artboard` hangs off `Root`, another `Artboard` or a `Group`, never under a `Boolean` or a mask. `build::can_parent` states the *parent* rule; `Document::frame_may_sit_under` the *ancestor* rule (walks to root for a boolean, or a node with `mask` set — `document::bars_frames`, since a group can carry a frame into an operand or mask at any depth). Enforced in create, insert, reparent, `SetMask` on a node whose subtree holds a frame, and the loader; violation is `OpError::ArtboardPlacement`. Frames nest (a card or component inside a page) and sit in groups (a flex row of cards is a group of frames). A frame as masked content (a sibling above a mask) is allowed.
- **`Artboard` is the type name; "frame" is the user's word.** Every string a designer reads says frame; the variant, serde tag and MCP snapshot's `"artboard"` keep the older word. (§15 D21)
- **Names** are written at creation or copy and never recomputed. Fresh nodes are numbered among new siblings; a copy increments a numbered name. Only the user and a boolean relabelled by an operation change (§9.4) rewrite it. Duplicate sibling names are legal (§5.7b).
- **`Root` may hold loose shapes.** A layer dragged out of a frame needs a destination, and "no frame" is honest. Creation agrees: a shape drawn outside every frame is created at the current page's root. Frames follow the same rule as shapes in both directions, unless a group is above, when the node stays in that group (§9.4).
- **`clip` is node-level, not an `Artboard` field.** It is a property of the layer like `visible`; `NodeKind::clips_children()` spells which kinds have a frame to clip to. Defaults on for frames. The DTO projection and loader both normalise an inert `clip` so it neither reaches a document nor survives a round trip (otherwise a first open-and-save moves bytes). The flag *can* be set on a kind that ignores it (`op_set_clip` lets it sit). Readers (`scene::paint_node`, SVG writer, both `bounds_cover_subtree` copies, `menu.rs`) read `Node::clip` themselves and kind-gate with a `match`. Clip off also turns off subtree culling against the frame (§6.1).
- **`mask`** is node-level and not the same switch: a masking node paints nothing; its outline clips the **run of siblings above it** in the same parent, up to the next mask or end of list (a second mask begins a second run). `clip` cuts a container's children to its own frame; `mask` cuts its siblings to its own outline. `Document::governing_mask` states the run rule (nearest preceding sibling with the flag; `None` for a mask itself). It is read two ways that must agree: the hit test asks backwards, node by node; `scene::paint_node`, the SVG writer's child loop and the two bounds passes in `resolve` carry it forwards (`MaskRun`, `mask_box`). If they disagree, fix the forward reader. Both spellings are asserted against the same sibling arrangement (`ops::a_mask_governs_up_to_the_next_one_and_is_never_masked_itself`, `render::a_second_mask_ends_the_first_ones_run_instead_of_nesting`). Nothing masks inside a `Boolean` (children are operands), so the flag there is inert — and the hit test must honour that or a pasted mask makes part of a boolean unclickable. `NodeKind::can_mask()` = `!Artboard && (takes_paint() || Group)`; Root and Artboard are excluded in writing (a frame already has `clips_children`). Unlike `clip`, `SetMask` is **refused** on a kind that cannot honour it (`WrongKindForOp`), because the walk must decide whether the layer paints. (§15 D282)
- **`mask_mode`** is a field beside the flag so the mode survives the flag going off and on (an `Option` payload would lose *Alpha* on toggle). It is writable on a non-mask; `SetMaskMode` is its own op. `Shape` = hard-edged outline clip (default); `Alpha` = the mask's own alpha per pixel; `Luminance` = the mask's brightness times its alpha per pixel, SVG's `<mask>`, with sRGB weights 0.2125/0.7154/0.0721 (vello's) on sRGB values, as browsers weigh them. `MaskMode::ALL` is what the Mask card offers. Both backends and the SVG writer draw all three (§6.2, §7).
- **`NodeKind::takes_paint()`** is the paint predicate beside `clips_children()`. `Root` (not a layer) and `Group` (no box) draw no fills; a frame does — its fills are `Node::paint`'s list, drawn behind children, strokes over them and outside the clip (§6.2). `Boolean` is a container that *is* a shape by the time it draws. The inspector's paint panels and the bulk colour edits both ask it. There is no `takes_stroke`: a frame's strokes are an ordinary stroke list, so `op_set_strokes` and the Stroke panel ask `takes_paint` too.
- **Predicate matrix.** `clips_children`, `can_mask`, `takes_paint`, `resolve::is_indexed`, `geometry::stroke_sides_apply` and `query::steps_into` each answer `false` for a variant nobody added, which is not a compile error. `node::kind_fixture` samples every variant behind an exhaustive wildcard-free `tag`; each predicate is asserted as the pair of name lists it divides the enum into (the *rejected* set does the work). `is_indexed` is `takes_paint`'s set plus `Slice` (it paints nothing but is picked by its edge); the two are asserted side by side. The fixture's completeness is guarded only by the compile error in `tag`.
- **`credit`** (`credit.rs`) is where an asset the assets browser inserted came from and what its licence asks: kind (Icon or Photo), the attribution sentence, the author's, provider's, page's and licence's names and addresses, `required` (whether the licence asks for the credit to be shown, which *Credits…* lists by) and the day inserted, `YYYY-MM-DD`. Every address is empty or in `web_link`'s form. Only `SetCredit` writes it, which refuses any other address (`BadCredit`); the user never edits it. It is no ink (`changes_ink` false). It follows a main to its instances as a whole value (`Mode::Whole`), since it travels with the picture it credits: a photo filled into a main, its picture replaced, an icon ungrouped inside it reach every copy, and a copy's own is an override that *Reset* puts back to the main's. A load drops a credit that will not read (a missing field, a kind from a later build) rather than refuse the document. Every copy keeps it (`remap_subtree`); `build::ungroup` copies a group's credit onto each child without one of its own, so an ungrouped icon's parts stay credited; a layer flattened or outlined into a path (`replace_with_path`, `flatten_union`), outlined into a group of stroke paths, traced without keeping the picture, or cut by *Divide* hands what replaced it the first credit among what it replaced (`build::carried_credit`; a *Divide* face takes the nearest one on a source over it or on a group going with that source), and *Convert to frame* and back carry the container's own (onto the background rectangle when a frame's picture goes there); `build::replace_image_fill`, the app's *Replace…* and an avatar *Fill with content…* draws over a shape's fills drop a photo's credit (`credit::dropped_by_replace`) when no other fill or stroke of the layer still shows a picture, since the picture it credited is gone, and keep an icon's; a copy's *Reset fill* takes a photo credit back with the picture. `credit::credits_in(doc, roots)` lists the credits under some roots in tree order, folding layers whose sentence, licence and source address agree into one line and skipping a layer inside one carrying the same credit; hidden layers count, as a credit is owed for what the file holds.
- **`proportions_locked`** shapes an edit already allowed (a resize holds aspect as if Shift were held, §9.4); `locked` refuses it. The two are independent. No kind gate — inert elsewhere. `remap_subtree` copies it. Set by `SetProportionsLocked`, saved.
- **`pivot`** is the transform origin: what rotation turns about and a mirror reflects across (not what Alt-resize holds still). `None` means the centre of the node's own box and is stored as an absence so the model can say "the user has not moved it" (the canvas marker draws only where it means something; files that never moved one omit it). `geometry::pivot_point` resolves it against a box; **nothing composes it into the world transform** — gestures bake it into the transform they produce. The two doors that can ask for the absence (marker dragged to the middle; Transform card's origin typed 50/50) go through `geometry::pivot_placed_at`, which answers `None` for the box centre, so both store the same thing. An axis with no extent is centred by itself.
- **`Pivot` variants:** normalized where the box is authored (a rect: a fraction stays a fraction under resize), absolute where it is emergent (group union, line, arc, path, auto-sized text; becomes normalized once text is dragged to a fixed box). `pivot_for` makes the choice off the same question as `edit::resizable_size`. Both variants are refused when a coordinate is not finite (`Pivot::is_finite`, at `SetPivot` and `InsertSubtree`'s field loop; a `NaN` saves as `null` and the document never reopens — invariant 8).
- **`fills` and `strokes` are lists of any length; list order is paint order**: every visible fill then every visible stroke, each in `Vec` order, last painted last. `StrokeAlign::Outside` covers outlined text, so two stacks suffice. The inspector shows each list reversed (top row = last element).
- **`effects`** is a list of the same shape, not inside `Paint`: order is compositing order and two drop shadows at different distances is normal. It sits on the node because it is meaningful on containers (a shadow on a group, a blur over a frame), which is why `SetEffects` is gated only on `Root`. See §5.3a.
- **`blend`** (`BlendMode`) is Pass through plus the sixteen modes CSS, SVG and PDF share; Plus darker, Plus lighter and other modes CSS lacks are left out so the canvas and every export agree. Frames and groups default to Pass through (their children blend with what lies under the container); every other kind, `Boolean` included (it draws its operands as one shape), defaults to Normal and may not hold Pass through (`BlendMode::passes_through`, `default_for`, `allowed_on`). A blended layer mixes with what is already drawn inside its nearest isolated ancestor (`BlendMode::isolates`), failing that its top-level frame's content (§6.2). Not bindable to a variable and in no style slot.
- **`sides`** needs four nameable sides: `Rect` and `Artboard` only (`geometry::stroke_sides_apply`). `geometry::effective_sides(kind, stroke)` is what consumers ask, so the stored value is never rewritten and an unsupported stroke draws the whole outline. `Custom`'s widths are independent of `width`; `0` switches a side off. The panel gates the control rather than clamping the value (same bargain as `stroke_align_applies`). (§15 D68)
- **`dashes`** is the only dash state; `Stroke::dash_style` classifies it from the *shape* (invariant under width), so no stored mode can drift. A dot is a zero-length dash `[0, s]`, making `s` the centre-to-centre spacing. Zero-length dashes drawing nothing under two of three caps and fitting a pattern to an outline length are resolved at the render boundary (§6.3). `dash_fit` is a flag: writing fitted numbers back would read "on" while true only until the shape resizes. (§15 D69)
- **`miter_limit` is the ratio** (1/sin(θ/2); default 4 ≈ 28.955°). The panel shows degrees; `node::miter_ratio_of_degrees`/`miter_degrees_of_ratio` convert at that one edge, and the panel converts back only when the angle moved (a non-bit-exact round trip would write a ratio a few ulps off past `strokes_tx`'s equality guard on every blur). `MIN_MITER_DEGREES` = 1°. `join` is kind-gated (`geometry::stroke_join_applies`); `miter_limit` is not.
- **Frame and rect corners are always four radii**, never a scalar with an override. A frame has everything a rect has (radius, per-corner, smoothing, bindings), absent from the file while square; its rounded outline (`geometry::rect_path` through `local_path`) is its fill, stroke, per-side stroke and the clip of its children. Hit-testing still uses the box for both. `NodeKind::corner_radii`/`set_corner_radii`/`has_four_corners`/`corner_smoothing` are the kind-blind accessors; a polygon's or star's one radius reads as four equal corners and is written from the top left. `GeometryPatch::CornerRadius(f64)` is a convenience over the same field; its inverse is a `CornerRadii`. Flattening is a real edit and must be asked for: the inspector's single field shows `largest(corners)` over a mixed set and writes only where the field differs from its seed. The per-corner disclosure is app view state. Radii over half the shorter side are clamped where the outline is built (`geometry::local_path`), not on the way in, so shrink-then-grow keeps the typed number.
- **`corner_smoothing`** (`0..=1`, absent from the file at 0, clamped by `GeometryPatch::CornerSmoothing`) eases each rounded corner into its sides, on a frame, rect, polygon, star or path — one value per shape (a path's is not per anchor). `geometry::smoothed_rect` follows Figma's published construction. Each corner is a cubic, then a circular arc of `90° × (1 − s)`, then a cubic, and it reaches `(1 + s) × r` along each side; that reach is capped at half the shorter side, with the smoothing giving way first. At 0 the outline is `RoundedRect`'s. `geometry::round_corners_smoothed` generalises it to any angle for polygons, stars and paths: the arc shrinks to `turn × (1 − s)` and reaches `(1 + s)` times the circular fillet's reach; on a short leg the smoothing yields before the radius; a fillet against a curved leg keeps its tangent join, unsmoothed. A test pins that on a square it reproduces `smoothed_rect`. A smoothed frame or rect leaves the SVG writer as a `<path>`; the code export writes circular `border-radius` and reports it; a per-side stroke (`side_path`) keeps circular corners. The inspector's field (a percentage, full width) opens from its own button beside the per-corner one; the two disclosures are app view state and never open together.
- **`Polygon` and `Star` fill their box.** Vertices are placed on a circle then stretched so the outline's bounding box is exactly `0,0..w,h` (keeps `local_bounds`, selection, snaps and handles honest). One builder `geometry::star_path` (polygon = star with no inner ring), first vertex up. `sides`/`points` (3..=64) and `inner_ratio` are clamped where the outline is built. `corner_radius` rounds every vertex, inner and outer, through the path fillet (`geometry::round_corners`), so it is clamped to half of each side as a path's anchor radius is; the rounded outline pulls in from the box, which stays the bounds. It is one scalar (`GeometryPatch::CornerRadius`, its own inverse), absent from the file at 0, scaled by the Scale tool, and written to SVG as a `<path>` rather than a `<polygon>`. It binds to a variable as four corners (§5.3f) and the multi-selection radius field reaches it.
- **`Spiral` and `Arc` are kinds, not flattened paths**, for D12's reason: a polygon or star stays editable only as its own kind. Both are open: drawn with a stroke and no fill, and hit by the stroke alone, as an open path is (a fill added later still paints, unlike a `Line`'s); `stroke_align_applies` and `stroke_join_applies` are false (no inside; their cubics meet tangent). A spiral's stroke is laid from where the curve first bends no tighter than half the width (`geometry::stroked_path`, which render, Outline stroke and SVG export share; Penpot and Sketch write the cut path for one stroke and no fill, and report *Spiral strokes, drawn from the centre* otherwise): inward of there the stroke's inner edge has a cusp, so a thick stroke leaves the innermost fraction of a turn without ink and ends in a clean cap. Their limits are clamped where the outline is built, not on the way in, as a polygon's sides are.
  - `Spiral { size, turns }`: an Archimedean spiral (`geometry::unit_spiral`: four cubics a half turn, clockwise on screen from its middle, ending at three o'clock) stretched to fill `size` exactly (`geometry::spiral_path`), so `local_bounds` is the box. Turns clamp to `geometry::MIN_TURNS..=MAX_TURNS` (0.5..=50). An authored box: it binds W/H, stretches in a layout (`container::can_stretch`) and takes a normalized pivot. `GeometryPatch::Turns` sets the count.
  - `Arc { end, sweep }`: from the local origin to `end` like a `Line`; `sweep` is signed degrees of a circle, positive clockwise on screen (bulging left of the way to `end`), 0 the straight line, clamped to ±`geometry::MAX_SWEEP` (359) (`geometry::arc_path`). An emergent box (its path's bounds) and an absolute pivot, as a line's. `GeometryPatch::LineEnd` moves the end and `GeometryPatch::Sweep` sets the sweep.
- **`Ellipse::arc`** (`EllipseArc { start, sweep, ratio }`) keeps part of the ellipse. Angles are degrees clockwise from three o'clock, taken on the circle before it is stretched to the box, so 90° is the bottom of the box whatever its proportions; `ratio` is the hole as a fraction of the radius. The default 0 · 360 · 0 is the whole ellipse and is not written (`EllipseArc::is_default`). `GeometryPatch::Arc` normalises on apply: start wraps at 360, sweep clamps to `0..=360`, ratio to `EllipseArc::MAX_RATIO` (0.99, so a ring keeps some width). `geometry::ellipse_path` draws a sweep under 360 with no hole as a wedge closed to the centre, a full sweep with a hole as a ring, and both as a ring sector; the stroke follows every edge. A ring's inner outline runs the other way, so the hole is a hole under either fill rule, in fill, stroke and hit-testing (`contains_local` tests a non-whole ellipse against its outline). `stroke_join_applies` is true for a sweep under 360. Booleans and *Outline shape* take the arc's outline through `local_path`.
- **`Slice` is a box and nothing else**, for exporting a region of its page (§7). It is a sized kind wherever size is written or measured (`GeometryPatch::Size`, `local_bounds`, `pivot_for`'s authored half, `container::resized`, `build::baked_ops`, `reset`), but `geometry::local_path` is `None`: no fill, stroke, mask, clip or outline (`takes_paint`, `can_mask`, `clips_children`, `build::can_outline` all false). It is indexed and hit only along its edge (§5.10), lends nothing to its container's box or ink (`resolve::lends_bounds`, §5.9), is never in a flow (§5.3c), and draws nothing in the canvas, SVG, PDF, raster or code (§6.2, §7). Default name "Slice".

### 5.5 Document

```rust
pub struct Document {
    schema_version: u32,
    nodes: FxHashMap<NodeId, Node>,
    pages: Vec<Page>,                   // in order; never empty
    vault: Option<NodeId>,              // another parentless Root, never a page: vendored kit mains — §5.3e
    guides: Vec<Guide>,                 // ruler guides; unordered
    published: Option<Arc<kit::Published>>, // a kit's last publish, frozen; outside undo — §5.3e
}
pub struct Page {                  // page.rs — what a page has that is not a field of its root
    pub id: NodeId,                // the page's parentless Root node
    pub ground: Option<peniko::Color>, // opaque; behind and around the page's frames; None = the viewer's default
    pub draft: bool,               // left out of everything gathered from every page
}
impl Document {
    pub fn new(root_id: NodeId) -> Self;          // one empty page, "Page 1"
    pub fn get(&self, id: NodeId) -> Option<&Node>;
    pub fn pages(&self) -> &[Page];
    pub fn page(&self, id: NodeId) -> Option<&Page>;
    pub fn is_page(&self, id: NodeId) -> bool;
    pub fn page_index(&self, id: NodeId) -> Option<usize>;
    pub fn first_page(&self) -> NodeId;
    pub fn page_ids(&self) -> impl Iterator<Item = NodeId>;
    pub fn gathered_pages(&self) -> impl Iterator<Item = NodeId>; // the non-draft pages, in order
    pub fn page_of(&self, id: NodeId) -> Option<NodeId>;          // None in the vault
    pub fn ground(&self, page: NodeId) -> peniko::Color;              // own, else DEFAULT_CANVAS_BACKGROUND
    pub fn own_ground(&self, page: NodeId) -> Option<peniko::Color>;  // the stored colour, if any
    pub fn vault(&self) -> Option<NodeId>;
    pub fn in_vault(&self, id: NodeId) -> bool;   // the vault or under it — never resolved
    pub fn published(&self) -> Option<&kit::Published>;
    pub fn guides(&self) -> &[Guide];
    pub fn guide(&self, id: GuideId) -> Option<&Guide>;
    pub fn guides_owned_by(&self, owner: NodeId) -> impl Iterator<Item = &Guide>;
    pub fn apply(&mut self, tx: &Transaction) -> Result<ApplyOutcome, OpError>;
    // NO public mutable access to nodes or the tree.
}

pub struct Guide {                 // guide.rs
    pub id: GuideId,
    pub axis: GuideAxis,           // Horizontal | Vertical — the direction of the line itself
    pub position: f64,             // y for horizontal, x for vertical; frame-LOCAL, or world for a page
    pub color: Option<peniko::Color>,   // None = follow DEFAULT_GUIDE_COLOR
    pub owner: NodeId,             // the frame it is scoped to, or the page it runs across
}
```

- **A page is a parentless `NodeKind::Root` listed in `pages`.** Its layers are that root's subtree; its name is the root's name and its modes the root's `Node::modes` (§5.3f); the ground and draft flag live in `Page`. Every page shares one origin, so a layer moved between pages keeps its coordinates. Components, styles, variables, images and the vault belong to the document and serve every page. Pages are roots in a table rather than nodes under one root so that the structural operations, the loader's tree rules and the clipboard's rules hold unchanged: Delete, Reparent and Reorder refuse a page as they refuse the vault (`CannotModifyRoot`), and `capture_subtree` answers `None` for one (it still captures the vault whole, which the kit verbs read).
- `Document::new` makes a one-page document named `Page 1` (`page::PAGE_STEM`); new pages take the lowest free `Page N` (`page::free_name`). The vault is never a page.
- The page edits are core builders the pages list and the page verbs share: `page::add`,
  `page::duplicate` (below the page, `… copy`, its layers copied as *Duplicate* copies them and its
  guides moved onto the copies) and `page::delete` (the page's guides, its layers as *Delete* takes
  them, then `RemovePage`; never the last page). Renaming, moving and the draft flag are one op each.
- A **draft** page is left out of everything that gathers from every page — kit publishing (§5.3e), *Export all* and *Export frames, all pages…* (§7), presenting past the current page, batch rename's *All pages* past the current page, the library cover — and kept by everything aimed at it directly. `gathered_pages` is what those read.
- **Document state that is not a node:** each page's `ground` and `draft`, and `guides`. People set them for the artwork, not the sitting, so they are saved with the file; neither loosens the document/view-data line (§9.2, §9.3). Which page is current is view state and never saved (`app-chrome.md`, *Pages*).
- A page's `ground` is a colour of its own, opaque, or `None`: no colour of its own, drawn on the viewer's default — the app's theme canvas (`theme::color::canvas()`), `DEFAULT_CANVAS_BACKGROUND` for everything headless (CLI, SVG/PNG, the MCP snapshot). `Document::ground` answers own-or-`DEFAULT_CANVAS_BACKGROUND`, so headless output is the same whichever; `own_ground` answers the stored colour, and the app resolves through `session::page_ground` instead. A new document, a new page made from a page without a colour, and an imported page without a colour of its own start at `None`. It is changed only through `SetPageGround` (invariant 2) and is per page, not an `EditorSession` preference. (§15 D28)
- `guides` are the lines pulled from the rulers (§9.4). `color: None` follows `DEFAULT_GUIDE_COLOR` (`#a3332d`), so retuning the default carries every never-recoloured guide.
- **A guide is deliberately not a `Node`:** no parent, transform, paint or bounds; never rendered into the scene, never hit-tested as artwork, never in the layers tree or Select All. Making it a node would need all of that special-cased away.
- The list is **unordered** (no z, no operation names one by position). `apply` appends; IO sorts by id on save so bytes are stable (invariant 9).
- **Page-wide or frame-scoped:** a guide owned by a page runs that page's whole canvas with a world `position`; one owned by a frame is drawn only within that frame and `position` is in the frame's local space. Chosen by where the pointer is when the drag ends (§9.4); a guide can move between modes. One flat list with an owner field (not per-node vectors) keeps `guides()`, `remove_all_guides` and the id-sorted save unchanged, and makes a scope change one undoable field edit (`SetGuideScope`).
- **The owner must be a live `Artboard` or a page** (`Document::check_guide_owner` on `AddGuide` and `SetGuideScope` → `OpError::BadGuideOwner`; also the loader). A frame scope is a box to clip to and measure from; shapes and groups have none. **Deleting a frame must take its guides in the same transaction** (`build::guides_of`, nested frames included). `apply` also re-checks every guide's owner once after the last op (invariant 8), so a caller that never heard of `guides_of` gets a refused edit instead of an unopenable file; `guides_of` remains what makes the Delete verb succeed.
- **A scoped guide is stored frame-local**, so it rotates with its frame and stays clipped to it (nothing is destroyed; only snapping degrades, since a leaning guide has no single world x or y). A frame resize leaves the offset; the Scale tool multiplies it (§5.6). An offset outgrowing a shrunk frame is kept, not clamped; `guide_span` lets the clip hide it.

## Traps

- `Node::clip` is read directly by several readers, not via `clips_children()`; the flag is load-bearing on any kind the moment it is live.
- The mask run rule exists in forward and backward spellings; change both or the paired tests fail.
- A new `NodeKind` variant does not break any of the six `matches!` predicates at compile time; only `node::kind_fixture`'s `tag` does.
- `NodeId::from_wire` accepts aliases of the canonical form; duplicate detection still works because they parse equal.
- Guide owner must be a live `Artboard` or a page; delete guides before the frame — or the page — in a delete transaction or the undo (applied in reverse) fails.
- A bound stroke width stores `width = 0`; anything reading the literal needs `Document::resolved_paint`.
