# Booleans, flatten, outline and masks in the identity row

Non-destructive boolean containers (`core::boolean`, `NodeKind::Boolean`), the destructive verbs
beside them (flatten, outline shape, outline stroke, convert text to path), the identity card's boolean row with its
Mask/Group/Ungroup buttons, the Mask card, and how a failed boolean draws. Part of §9.4.

## 9.4 Booleans

### Model

- `NodeKind::Boolean { op }` is a container. Children keep their transforms and geometry and stay
  selectable and editable; the combined outline is **derived**, cached in `Resolved` beside text
  layouts, invalidated children-first (booleans nest).
- `geometry::local_path` returns `None` for a boolean on purpose. Anything needing "this shape's own
  path" goes through a wrapper: `Resolved::local_path` (canvas; callers `mask_path` and
  `canvas::stroke_outlines`) and `svg::outline` (exporter). `stroke_outlines` has a `Boolean` arm for
  an empty or abandoned outline so it is not drawn as its operands.
- **Stroke alignment is granted by kind**: `geometry::stroke_align_applies` treats a boolean as closed,
  because `boolean::from_flo` closes every subpath and normalizes winding.

### Arithmetic

- `flo_curves` (kurbo has no path booleans), which keeps curves as curves; polygon clippers would
  flatten a union of two circles into a thousand-segment polyline in the file.
- **Coordinates are scaled by a constant into flo_curves and back** — `boolean::c` multiplies,
  `boolean::k` divides; they are the whole seam. flo_curves has absolute tolerances (e.g. a
  `CLOSE_DISTANCE` of 0.01) that would otherwise erase thin results. Scale and accuracy are
  **constants**, never zoom- or size-dependent, or the same shapes would combine differently by camera.
- `boolean::MAX_BOOL_COORD` (10¹⁵⁰) is refused at `evaluate`: past ~10³⁰⁴ (scaled) flo_curves hangs.
  Nothing upstream bounds magnitude (`geometry_is_finite` only tests finiteness).
- **Winding**: flo_curves is even-odd, Ondin fills non-zero. `from_flo` alternates each subpath's
  direction with its nesting depth, so the result means the same under either rule (and is safe for
  SVG). Nesting is counted from `representative_point`: the bbox centre if inside, else a horizontal
  scan line through all segments, taking the midpoint of the first confirmed span. A subpath whose
  point cannot be found is **kept**, not dropped.
- **Shared edges are spliced** (`boolean::spliced`, in `from_flo`): flo_curves leaves operands that
  share an edge as separate contours either side of it, which fill right and stroke a line through
  the result, so two contours walking one edge in opposite directions become one. Ends must match
  within `SEAM_EPS` (10⁻⁶), well under the thinnest sliver a boolean keeps, so a sliver's two sides
  are never taken for one edge; handles within `SEAM_HANDLE_EPS`, since flo_curves hands a split edge
  back with handles off by around 10⁻⁶. A contour of no area (two touching circles' intersection)
  is dropped; a thin one is a shape and is kept.
- `Exclude` is `exclude_of`: all operand outlines concatenated and read **even-odd** (via
  `Node::fill_rule` deriving even-odd for an `Exclude`). It is not `sub(add(a,b), intersect(a,b))`,
  which returns the operands unchanged. Each contour is turned one way first, so two that share a
  whole edge walk it oppositely and are spliced, and a contour that appears twice is dropped, as the
  even-odd read cancels it. An edge shared only in part stays, since no arithmetic splits it: the
  fill is right and a stroke draws along it.
- `ondin-core`'s `tests/boolean_suite.rs` checks shared, partly shared and nearly shared edges,
  touching corners, containment, identical operands and the shapes of `tests/common` under every
  operation against the definition, and for specks and stray edges; its `KNOWN` list holds what still
  fails, each entry required to keep failing.
- `boolean::fold_operands`: `Union` and `Intersect` fold as a **balanced tree** (N log N on disjoint
  operands, which is what icon masks are); `Subtract` keeps the left fold — it is not associative.

### Operations and operand order

- Children are stored bottom-first, so operand order is layer order. `Subtract` = bottom child minus
  everything above it.
- `build::boolean` refuses fewer than two operands and refuses text (no outline until converted), and
  **gives the result the bottom operand's paint** (otherwise it draws nothing).
- **The key layer** (`key: Option<NodeId>`) moves to index 0, others keep relative order — meaningful
  only for `Subtract`; inert for the commutative three. `build::mask` takes the same argument with the
  same reorder. Once the container exists the key is not stored: re-designating is an
  `Operation::Reorder` to index 0 (`canvas::designate_key`).
- `layers::subtract_base` marks `children[0]` of a `Subtract` with a faint `stack-simple` glyph in the
  row's badge slot (shared `RowBadge`, `BADGE_ICON`), derived every frame. Only `Subtract`.
- Badge slot priority: mask (`ph-circle-half`) > frame size badge > subtract base mark. No row can want
  two (a frame cannot be a mask; operands are not painted).
- **Switching the operation is a `GeometryPatch`**: children and paint stay, undo restores the previous
  op, re-picking is an empty transaction. Travels the normal preview→commit valve.
- The dropdown is **disabled, never hidden**, with one layer selected. Its enabled test is a dry run of
  `build::boolean`. `Ctrl+Alt+U/S/I/X` are the four commands.

### Drawing and preview

- **Operands never reach the canvas**: `scene::paint_node` and `scene::paint_ghost` stop at a boolean.
  The boolean's own fill and stroke govern the result; operands' paint is inert until released.
  The inspector offers Fill/Stroke via `NodeKind::takes_paint()`.
- Gestures re-derive the outline in the preview: `RenderOverrides` keeps its own boolean cache, filled
  once per preview build, **deepest from the root first**.
- Committed and previewed answers share `boolean::evaluate_node` over `boolean::Operands` — five
  lookups the caller supplies (`children_of`, `visible_of`, `kind_of`, transforms, nested outlines).
  Three views use it: `Resolved` (committed), `RenderOverrides` (pending gesture), and ghosts (captured
  subtrees the document never saw). `Resolved::operand_path(doc, id)` is the committed "what this
  subtree contributes as an operand, in its parent's space", over a `with_committed_view` callback.
- `kind_of` matters: a resize changes geometry, so `RenderOverrides` answers with `drawn_kind` (ghost,
  override, then the **used** kind).
- `RenderOverrides::reevaluate_booleans` seeds from: a moved node's parent chain; a ghost's parent
  itself (an Alt-drag copy inside a boolean joins it); and a parent the preview reorders.
  `operand_children` inserts ghosts **at their `Ghost::index`** (the fold is order-sensitive beyond the
  first operand), starting from `RenderOverrides::children_of` where the preview reorders.
- `render::GhostNode::path` is evaluated once when the ghost subtree is built (`build_ghost`,
  children-first), never in the scene walk.
- **Everything that measures a boolean measures its result**: `query::local_box` and
  `session::preview_local_box` read the (previewed) outline first, so selection box, badge, handles and
  transform fields describe the visible shape.

### Behaviour in the editor

- **Name follows the operation** while it is still one of the four labels, with or without a number
  (`naming::is_boolean_label`); the number carries across. A user-typed name is never overwritten.
- **Selects like a group**: in the spatial index, hit-tested against its outline (`query::hit_test`),
  treated as a group by `group_chain`; double-click steps inside.
- Layers row draws `layers::op_glyph` (shared with the dropdown) and takes drops like a group
  (`layer_row`'s `is_container`), which is how a third operand is added.
- **Grouping inside a boolean re-parenthesises**: `boolean::operand_of` gives a `Group` the union of
  its contents, so `a ⊻ b ⊻ c` becomes `a ⊻ (b ∪ c)`. Intended.
- **SVG** writes the derived outline only, never the operands.
- **Release is `build::ungroup`**: the splice a group needs, folding the used transform into each child.
  Operands' own paint comes back into view; `Ctrl+Shift+G` and the identity row's Ungroup reach it.

## Flatten, Outline shape, Outline stroke, Convert to path

All four share `build::replace_with_path` (*Outline stroke* only when it replaces the layer), which carries what survives being redrawn: transform,
z-slot, paint, name, opacity, visibility, effects, **moved pivot**, **mask flag**, the **effective fill rule**
(derived even-odd for an `Exclude`, else stored), and the layer's place in a layout (insets when
authored, item properties when non-default). `display` is not carried (a path has no children).
Lock and the proportion lock are dropped deliberately. None of the four has a chord.

| Verb | Admits | Result |
| --- | --- | --- |
| `build::flatten` (`Ctrl+E`) | one boolean, or two or more layers | one boolean → `Path` of its outline in place; a set → union in the parent's space at the topmost member's slot with the bottom member's paint (`build::flatten_union`, carries paint only) |
| `build::outline` (*Outline shape*) | one Rect/Ellipse/Polygon/Star/Line/Path | `Path` of its own outline, per-anchor radii baked in; cut at the **drawn** (used) size, via a `Resolved` lookup |
| `build::outline_stroke` (*Outline stroke*) | one Shape/Path/Boolean with a visible stroke | each visible stroke a `Path` filled with that stroke's brush (a bound brush stays bound), below |
| `build::outline_text` (*Convert to path*) | one Text | `Path` from `ondin_core::text::outline` (glyph coords already node-local) |

- Flatten refuses a single non-boolean (that is *Outline shape*'s verb — the two rows never appear
  together), text, `Artboard`, `Root`. Groups contribute their union, nested booleans their result,
  via `Resolved::operand_path`; `build::carried_paint` shared by all arms.
- Flatten's two refusals of an admissible selection: `OpError::EmptyGeometry` (operands cancel out —
  move a shape) and `OpError::BooleanAbandoned` (flo_curves unwound — moving won't help).
  `flatten_union` brackets `boolean::failures` across its own `evaluate`; `flatten_boolean` reads the
  cache, so asks `Resolved::boolean_failed`.
- The destruction is one `DeleteNode` over a node that still has children, so one undo restores it.
- `build::can_outline`: on a `Path`, live only if **some radius is `> 0`** (not "not every radius is
  `<= 0`" — `NaN` passes that). Row present and dimmed otherwise. Groups, booleans, artboards, text,
  root refused.
- `build::can_outline_text` asks `text::outline` itself (all-space text has glyphs but no contours).
  ~1µs per glyph, so the menu computes it once at open (`ContextMenu::text_outlineable`).

### Outline stroke

- **The geometry is the ink as drawn** (`build::stroke_ink`, mirroring `scene::paint_strokes`): the
  drawn width (`Document::resolved_paint`), per-side strokes, dash patterns
  (`geometry::dash_fit_pieces`, `resolved_dashes`, cut with `kurbo::dash`), caps, joins and mitre
  limit. An inside or outside stroke is laid at twice the width, then intersected with or subtracted
  from the shape.
- **It is polygon arithmetic, as *Offset path* is** (`build::stroke_polygons`, then `refit`): each
  laid path flattened finer the wider the stroke, and finer for a thin one, stroked by i_overlay with
  bevels at the flattening's vertices, round joins and caps as asked, and each real corner's mitre
  added as its own wedge while it is within the limit. i_overlay's own mitre clips past the limit
  where a rendered stroke bevels, and flo_curves, which an earlier version united band pieces with,
  drops or inverts pieces that share an edge (a 50-radius circle stroked 110 wide lost a quarter).
  The union and the inside or outside cut are i_overlay's, read non-zero. The refit keeps every piece
  of ink, each dot of a half-unit dotted stroke included, down to a hundredth of the half-width
  squared.
- **A stroke that leaves no ink is refused, not removed.** If any visible stroke with a width yields
  nothing (a butt-capped stroke of a dot), the whole operation is `EmptyGeometry`, rather than the
  layer losing that stroke with nothing drawn in its place.
  `ondin-core`'s `tests/stroke_suite.rs` checks the closed shapes and open paths of
  `tests/common` at three widths with each join, cap and alignment, dashed (centred, inside and
  outside), dotted and thin on the catalogue shrunk, and per side on a rectangle, against an
  independent oracle.
- **Result shape.** A layer with no visible fill and one stroke is replaced by the path
  (`replace_with_path`, then the stroke's brush as its one fill, no strokes, non-zero). Otherwise the
  layer, its strokes removed, and the stroke paths above it (each named "‹layer› stroke") go into a new
  group in its slot, named as the layer; the group takes the layer's effects, visibility and opacity,
  each binding moved with its literal, which composite over both: a hidden layer's strokes stay hidden,
  and a bound opacity survives a layer with no visible fill being deleted inside that group.
- Refused for frames (their children would be lost), groups and text, and for a mask the result would
  group: it would mask its own stroke paths and stop masking the siblings above it
  (`build::can_outline_stroke`). The
  layer-menu row dims without a visible stroke (`LayerState::stroke_outlineable`);
  `inspector::outline_stroke` commits and selects the result.

## The identity card's boolean row

Four controls: the op dropdown plus three 28×28 icon buttons, `ACTION_GLYPH` 16.

- **Dropdown**: four ops, then under a hairline a fifth row, *Flatten* (`arrows-merge` glyph — an
  action, not a state). `ComboBox::height(BOOL_MENU_H)` = 220 so the five rows never get a scroll bar.
  `inspector::flatten_selection` gates on the dropdown's own predicate so the keyboard path reports a
  useful sentence. `inspector::popup_rule` draws the hairline via `ui::hairline_in_column`, sized from
  `available_width`, with gaps `ABOVE` 5pt / `BELOW` 4pt and the layout's `item_spacing.y` subtracted
  back out with `add_space`.
- **Mask** (`ph-circle-half`) is a state: lit while the target is a mask, click again releases.
  `Inspector::mask_action` → `Make { members, key }` or `Release(id)`, behind both look and action.
  - Make goes through `build::mask` (wraps two or more in a group; a lone layer is flagged in place).
    Which layer becomes the mask is `build::mask_target`'s answer only: the key if designated, else the
    bottom-most non-picture member, else the bottom-most, among members `build::can_be_mask` accepts.
  - `mask_action` dry-runs `build::mask` on a throwaway `IdSource`, so builder refusals dim the control.
  - Release reads either end: one selected mask, or one selected container holding one.
  - Requires one parent across the selection. Returns a `Result` because each refusal has its own
    tooltip — including refusing layers inside a `Boolean` (operands are not artwork), which is
    redundant for correctness (the dry run declines too) but load-bearing for the message.
  - Same verb on `Ctrl+Alt+M` and the context menu's *Use as mask*, which dims with the same sentence
    via `OndinApp::mask_refusal` → `menu::Context::mask_refused`.
- **Group / Ungroup** (`ph-selection-plus`, `ph-selection-slash`), gated by `inspector::group_actions`:
  - Ungroup live when something selected is a `Group` or `Boolean`; both buttons call
    `app::ungroup_selection`, which builds through `build::ungroup_all` — one transaction however many
    containers, freed layers selected.
  - Group is a dry run of `build::group`, plus: a selection that is exactly one container is refused.
- Phosphor codepoints are confirmed by an atlas dump: the font's `post` table is version 3.0 (no glyph
  names), so a wrong codepoint lays out and renders nothing.

## The Mask card

`inspector::inspector_mask`: directly under the identity card while the selection is a mask **or a
container holding one**. One dropdown: *Shape* or *Alpha*. Targets come from
`inspector::mask_card_targets` (free function): the selected layer if it is a mask, else its **direct**
mask children. A list — two runs report *Mixed* and a pick writes every target that would change, in
one transaction. There is no "mask group" flag; it is derived from the children every time.

## When a boolean fails

- flo_curves can panic (intransitive comparator in `GraphPath::exterior_paths`) on any op. The unwind
  is caught at `boolean::evaluate`, which answers `None`; the boolean draws a placeholder, the
  document survives. A `#[cfg(panic = "abort")] compile_error!` sits beside the guard.
- `boolean::failures()` is a thread-local **count** read either side of a call (an empty intersection
  also answers `None`; a count composes across nested inner `evaluate`s). `Resolved` keeps the answer
  per node (`Resolved::boolean_failed`); the layers row takes the warning colour and the commit says so
  in the status bar.
- It draws §5.5a's missing-picture placeholder over `query::boolean_placeholder` (the operands'
  union); scene walk, hit test, `Resolved::recompute_bounds` and SVG all ask that one function, giving
  the node world bounds so it stays in the R-tree. Committed-only: during a gesture
  `RenderOverrides::boolean_path` always has an entry.
- Text cannot be an operand; a mask can be text (`Resolved::mask_path` takes glyph outlines).

Performance: re-running the whole boolean per operand edit and per preview build is cheap enough with
the balanced fold; per-operand caching is not to be built. Pixel tests live in
`ondin-export/tests/boolean.rs` (a subtracted hole and an excluded overlap are each `alpha == 0`).

## Traps

- Asking `geometry::local_path` for a boolean gets `None` silently — wrong ring, centred aligned stroke,
  unmeasured dashes. Go through the wrappers.
- A preview evaluator that overrides only transforms moves operands without resizing them; `kind_of`
  must answer the drawn kind.
- `MAX_BOOL_COORD` guards a hang the `catch_unwind` cannot see; the panic guard is inert under
  `panic = "abort"`.
- Group's lone-container refusal looks redundant in front of a builder that says yes, and nothing
  tests the row's gating.
- `replace_with_path` must carry the effective fill rule, or a flattened `Exclude` becomes its union.
- *Outline stroke*'s ink must not be `kurbo::stroke`'s expansion, though that looks like the same
  thing in one call: read even-odd it double-counts the inner corners (a 100 square with a 10 stroke
  comes out 4100 instead of 4000). `outline_stroke_traces_the_ink_each_alignment_draws` and
  `stroke_ink_covers_what_a_non_zero_expansion_fills` (`ondin-core/tests/build.rs`) pin it.
