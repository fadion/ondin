# Layout: layout grids and container layout

Covers §5.3b (layout grids, chrome drawn over a frame) and §5.3c (container layout: CSS flexbox, CSS grid and absolute insets, with used geometry derived from specified values). Components (§5.3d) and kits (§5.3e) are in their own files.

"Grid" names two unrelated things. §5.3b's *layout grids* are chrome (`ondin-core/src/layout.rs`). §5.3c's *grid* is the CSS grid container that places children (`ondin-core/src/container.rs`).

## 5.3b Layout grids (`layout.rs`)

Columns or rows drawn over a frame. Not CSS grid.

```rust
pub struct LayoutGrid {
    pub axis: GridAxis,      // Columns divide the width, Rows the height
    pub count: u32,          // 0 draws nothing; `tracks` clamps the top at MAX_TRACKS
    pub gutter: f64,         // between tracks
    pub margin: f64,         // edge to first track: both edges under Stretch, one under Start/End, unread under Center
    pub size: f64,           // track size; read unless Stretch, which derives it
    pub align: GridAlign,    // Stretch | Start | Center | End
    pub color: peniko::Color,
    pub visible: bool,       // this grid's own eye; saved document state
}
pub const MAX_TRACKS: u32 = 10_000;
pub fn tracks(grid: &LayoutGrid, size: Size) -> Vec<Rect>;   // bands in the frame's local space
pub fn next_grid_color(existing: &[LayoutGrid]) -> Color;    // first of GRID_COLORS nobody uses
```

- **Chrome, not ink.** Never a scene node, in no export, not in the snapshot. `Operation::SetLayoutGrids` answers false to `changes_ink` and `RenderOverrides::absorb` treats it as a no-op (the two lists are close but not the same; §5.7).
- **A field on the node, offered on a frame.** `Node::grids` sits beside `exports`: empty by default, skipped by the save format, ignored by every walk. A grid is measured against an authored size, which an `Artboard` has and a group's derived box does not. It is not normalized away on other kinds; the one panel and one overlay that read it ask the kind first.
- **Mode with a payload kept.** `GridAlign` decides which numbers are read, but every number stays a field, so switching to Stretch and back finds the typed track size. The panel dims unread fields.
- **Two guards.** `tracks` refuses a grid whose numbers do not fit (zero count, non-finite field, margins and gutters leaving no room) and clamps `count` to `MAX_TRACKS`. Under Stretch a track is `extent / n`, so the fit guard cannot fire however large `count` is; `u32::MAX` would ask the paint path for gigabytes. Only the panel clamps (to 1000); the model, file and loader do not. The cap is above the panel's so a file from a wider build is drawn, not truncated. Clamped, not refused: same bargain as `geometry::MAX_SIDES`.
- **Drawing.** `canvas::draw_layout_grids` intersects each band with the frame's box in local space, so a rotated frame's bands turn with it. Size and world transform are read through the preview (`EditorSession::display_node`, `preview_world_transform`), so bands follow an uncommitted move, rotation, skew or resize. Drawn over the pixel grid, under the frame labels.
- **Three visibility gates.** The workspace switch `View > Show layout grid` (`OndinApp::show_layout_grids`, an early return; present mode is not a switch), then `LayoutGrid::visible`, then `shown_visible` for the frame. The switch never writes the per-grid eyes; clearing them would spend saved state and land in undo history.
- **Inspector.** The *Layout grid* card is the third card a frame gets, under *Frame templates*, and acts on every selected frame (`frame_subjects`). Numeric fields are hand-wound (nothing to preview through `RenderOverrides`); the in-flight value is `app::InFlight::grid_scrub`, naming every frame written, read by the canvas too so all bands move under the hand. `cancel_gesture` drops it.
- **Colour is `PaintSlot::Grid(i)`.** The chip, hex field and picker are the app's own; the percentage is the alpha. The write goes to every selected frame, safe because the card only draws rows when the frames agree. The picker's arm asks `shared_grids(&frame_subjects())` (frames, not the selection) so it closes when agreement goes. This is the only slot whose paint is chrome, so it has its own preview path: `OndinApp::preview_grid` fills `grid_paint`, one op per frame in one buffer, applied after the scrub. `remap_grid_slots` walks an open picker down when a grid above it is removed.
- **New-grid colour.** `next_grid_color` returns the first entry of `GRID_COLORS` not used on the frame (the entry at the new index duplicates after a delete). The six are `DEFAULT_GRID_COLOR` rotated in 60-degree multiples, thirds first (red, green, blue), then olive, cyan, magenta; past six it repeats. Only the panel's `+` steps; `LayoutGrid::new` carries the plain default.

### Traps

- `count` is bounded by nothing but the panel; `tracks` must keep its own clamp.
- Taking the frame size or world transform from the committed resolve instead of the preview makes the grid slide out from under its frame mid-gesture.
- A picker over `PaintSlot::Grid` must close when the selected frames stop agreeing, or a hex typed recolours another frame's grid.

## 5.3c Container layout: flexbox, grid and insets

The model is CSS under CSS's names (`display: flex`, `display: grid`), explicitly not Figma auto layout. The chrome writes the names in sentence case ("Justify content", "Flex start"); the model, file and snapshot keep CSS spelling. Where CSS and design-tool convention disagree, CSS wins:

- Transforms do not affect layout: a rotated item keeps its unrotated slot and may overlap neighbours.
- Child order is flow order and paint order (`children` is z-order, §5.3), so in the layers panel a flex row reads bottom-up.
- Text wraps at the available width.
- What the user sets (specified) and what is drawn (used) are separate values.

Code export is designed, not built; it would write this model as CSS and report departures.

### Property set

| | |
|---|---|
| Container | `display` (unset / flex / grid), `flex-direction`, `flex-wrap`, `justify-content`, `align-items`, `justify-items` (grid), `align-content`, `gap` (row/column), `padding` (four sides), `grid-template-columns`/`-rows` (px, %, fr, auto, min-content, max-content, `minmax()`, `repeat(n)`), `grid-auto-flow` |
| Item | `width`/`height` (auto / px / %), `min-`/`max-` of both, `flex-grow`, `flex-shrink`, `flex-basis`, `align-self`, `justify-self`, `grid-column`/`grid-row` (auto / line / span), `position` (in flow / absolute) with `top`/`right`/`bottom`/`left` (px / % / auto) |
| Not built | `margin` other than `auto`, `order`, named grid areas, `auto-fill`/`auto-fit`, `calc`, `aspect-ratio`, block/inline/float layout (the engine supports each, so each is cheap later) |

### Who is a container

`display` applies to a `Group` and an `Artboard` (`container::is_container`). Setting it gives a group a box: its bounds become its layout box (padding included) instead of the union of its children, and `width: auto` means fit content. Resizing it writes `width`/`height` and reflows instead of scaling contents (`tools::resize_group`, §5.6); under the Scale tool its contents scale too. `tools::resize_layer` routes a laid group to `resize_geometry`, and `scale_geometry` writes its `LayoutItem` width/height in px through `build::sized_flex_item`, which also stops its growth. A group without `display` is a plain group. A `Boolean` ignores `display`; the root is not a container; a mask layer is out of flow as if absolute. Paint and clip stay frame-only (§5.7a). A laid group's box is `Resolved::used_frame`. The Container card sets `display`; a laid group's typed W and H take the handles' path to `sized_flex_item`.

A frame may sit inside any group (e.g. a flex row group of card frames); a frame under a boolean or mask, at any depth, is refused at the model level (§5.3). A card in a row is a rung of the row's `group_chain` (§5.10). The Scale tool on a group scales a nested frame's contents.

### Insets: constraints are `position: absolute`

In a box (a frame, or a group with `display`), a child's authored `top/right/bottom/left` (px or %) are its constraints: pin left is a left inset, pin right a right inset, stretch is both, scale is percentages. A child with no insets keeps its stored position, so old documents are unchanged; the Position card shows a held top and left outside a layout and writes nothing. A group without `display` has no edges, so insets are inert there. This is not §13's persistent constraints: it is a fixed algorithm re-run from specified values.

`container.rs` holds the arithmetic: `LengthPct` (`Px`, or `Percent` stored as typed, horizontal against width, vertical against height), `AutoMargins`, `Insets`, and `place`/`inverse` (CSS's absolute-positioning rule for one axis and its reverse). `Node::insets` is saved as an additive `NodeDto` field skipped when unset; `Operation::SetInsets` writes the whole set with no kind or parent gate.

Rulings:

- **Both insets on an axis stretch** a layer that has a size of its own, to the frame less both insets. Not CSS (which keeps a replaced element at intrinsic size): between two insets is where shapes-as-replaced-elements gives way. A kind that cannot stretch ignores its end inset when placing but stores it true (`AxisInsets::inverse`). A frame with a layout so pinned is laid at the stretched size.
- **Rotation, skew and flip turn about the box centre** (CSS default `transform-origin`).
- **Centring is `margin: auto`** between two insets; one auto side pushes. A box bigger than the space between its insets overflows both ways, centred (Figma's reading; CSS 2.1 puts it at the start inset). Recorded deviation.
- **The Position card** (§9.4) shows a pin diagram and the four fields in px or %. `container::with_edge` is the diagram's click; pinning and unpinning never move the layer, because the card writes the drawn placement into the stored transform and size in the same commit (`inspector::baked_placement`). Pinning an in-flow item takes it out of flow, so a hugging laid group or `fit-content` frame re-lays without it and shrinks; `inspector::kept_in_place` re-derives the insets against the box the container has after the edit (a scratch document resolved once; negative inset if need be). A typed inset that takes an in-flow item out of flow writes its drawn placement first so the unpinned axis stays where it is drawn.

**What stretches:** kinds whose size is a field: `Rect`, `Ellipse`, `Polygon`, `Star`, `Artboard`, and `Text` (stretched width is a wrap width, `AutoHeight(w)`, a `Fixed` box keeping its height; stretched height makes it `Fixed`). This differs deliberately from a container's stretch, which never wraps auto width: pinning both edges is the author choosing a width. `Path`, `Line`, `Boolean` are positioned and keep their size. `Group` and `Boolean` take no insets (box is their children's, measured after placement); the field is stored and inert.

**Edits to a pinned layer are edits of its insets, converted in one place.** Tools compute from where the layer is drawn and write ordinary `SetTransform`/`SetGeometry`; `build::keep_insets`, at the top of `EditorSession::commit_inner`, appends the `SetInsets` that draws each pinned layer there, each inset kept in its unit, judged against the frame the edit leaves; it returns the transaction untouched when nothing is pinned. Only a patch that can change the box counts as a resize (`GeometryPatch::resizes`, no wildcard): a corner radius, side count or inner ratio is applied to a kind still at its stored size. Never for a vault node (§5.3e): the vault is never resolved, so `used_local` has nothing and the stored transform would stand in for the drawn place. `keep_insets`, `flex_holds` and `kept_flow_translations` leave every vault node as written.

### Used geometry (specified vs used)

- Every leaf is measured and shapes are replaced elements. `Rect`, `Ellipse`, `Polygon`, `Star`, `Path`, `Line`, `Boolean` have intrinsic size equal to stored geometry (bounds for path, line, boolean), with `width`/`height` defaulting to `auto`. That stops flex's default `flex-shrink: 1` squeezing a 40-unit rect; two absolute insets still stretch a sized shape. A group without `display` placed as an item is atomic, its intrinsic size its children's union through specified geometry (`container::atomic_box`), except a child container with a layout, which is measured by that layout.
- **Text keeps each sizing mode's meaning.** Auto width never wraps; auto height wraps at the width given (stored width preferred, never below the widest word); fixed is fixed. `container::flexed_text` turns the size a container gives back into a kind, an auto-width box never narrower than its line (a smaller stretch overflows from the start). A text leaf reports its first baseline (`LayoutView::first_baseline`) so `baseline` aligns letters; shapes report none and align by bottom edge (CSS synthesized baseline). Min-content is `text::content_widths` (parley's widths plus the widest paragraph's start edge, positive first-line indent and end indent, since core places lines itself). All text measurement goes through `LayoutView::measured` and `content_widths`, answered from `Resolved`'s `text_memo` (keyed by node and width, kept across commits, dropped for a dirty node), so nudging a laid container does not re-shape text.
- Default `align-items: stretch` also stretches a replaced element's cross size. A grid's default is `normal`, which holds a replaced element at `start`.
- **Layout is derived, never stored.** Only specified properties are saved; used boxes live in `Resolved`'s sparse `used` map (read via `used_local`, `used_kind`; §5.9), never serialized (invariant 4). Order in `Resolved`: used geometry, then re-shape text at the used kind (the cached layout is shaped at it), then world transforms (composing used locals), bounds, reindex.
- Edits are ordinary operations: `SetInsets`, `SetDisplay`, `SetLayoutItem` (flex and grid alike, dirtying the parent too). Both layout operations return `OpError::BadLayout` where CSS refuses a value the node does not already hold. Undo and redo apply through `Document::apply_restoring`, which stands that check down so history can write back a file's refused value. Fields are additive `#[serde(default)]` with no schema bump (§5.11).
- **Every consumer of geometry must read used geometry**: `Resolved` (transforms, text, bounds, `local_path`, `mask_path`, boolean view), `query` (`hit_test`, `local_box`, `outline_at`), `scene::paint_node`, `scene::mask_geometry`, `RenderOverrides` (`transform_of`, `drawn_kind`, §6.2), the SVG writer and snapshot (§7), `DisplayNode::kind`, `preview_*` helpers, `canvas::crop_frame`, the layers frame-size badge, and the parent-frame box `image_tx`/`paste_text_as_layer` fit to. Editing tools (`tools/` resize, scale, rotate, skew, line-end; `inspector::multi_angle_tx`) compute from where a layer is drawn and `keep_insets` converts at the commit seam. Stays on the document: what an operation is applied to (`RenderOverrides::current_kind`), `EditorSession::committed_node`, pen and path point editing.
- **Preview.** `RenderOverrides` (§6.2) patches the walk without re-resolving, so it cannot express a reflow in general. Its `relayout` pass re-runs `container::place` for insets (laying a pinned layout root again first); for flex and grid it runs the engine itself in `flex_relayout` (`lay_out` at any layout root) before `relayout`.

### Engine: taffy 0.14.0

f32, declared `=0.14.0` with `std`, `flexbox`, `grid`, `detailed_layout_info`. Used values are quantized to 1/64 px (`container::quantize`) so f32 noise never reaches a field or file. No mirror tree: `container::FlexTree` implements taffy's low-level traits over Ondin nodes through `container::LayoutView` (`resolve::UsedView` for `Resolved`'s pass: `DocView` plus the used box a pinned root is stretched against and remembered text measurements; `RenderOverrides`' `PreviewView` for a preview). It is built per pass and dropped with its `taffy::Cache`s. The incremental part is `Resolved`'s choice of which layout roots to re-lay.

### Flex

- `place_node` runs `container::lay_out` at each **layout root** (a container with a layout that is not itself an in-flow item of one), as the parents-first walk reaches it, a whole chain of nested flex containers in one pass. Items take slot and size (`item_placed`); a root takes its size (`root_sized`: a frame asked to hug grows its kind, a group takes the box).
- **A root pinned by two insets on an axis is laid at the size they give** (CSS absolutely positioned container): `lay_out` lays at its own size, places its insets against `LayoutView::parent_box` as `resolve::root_used` will, and lays again at that size if they differ. Stretch wins over hug (a `fit-content` root so pinned is laid stretched): recorded deviation.
- `update` collects `affected` from each dirty node's `chain_root`, so a change to one item re-lays its siblings (the first hop counts for a child that just left the flow), and the climb passes through a group or boolean with no layout. `root` moves only on a hop into a layout. A laid group behind a plain group is a root of its own; the preview re-lays every root from a touched node up to its chain root.
- **Out of flow** (`container::in_flow`): a hidden layer (`display: none`'s reading), a mask, and a child with any inset.
- **An in-flow item turns about its box centre and its stored translation is ignored while in flow.** The preview re-lays a turn as the commit does.
- **`fit-content` is not stretched across its line**: `FlexTree::push` sets such a child of a stretching container to `flex-start` (`container::fit_content_across`), since taffy has no such size and is handed `auto`.
- **`Start`/`End` are CSS's `flex-start`/`flex-end`** in `justify-content`, `align-items`/`align-self`, `align-content`: the flow's own start (right under `row-reverse`, bottom of a row under `wrap-reverse`), as the cards draw.
- **A resize sticks.** `build::keep_flex_sizes`, beside `keep_insets` in `commit_inner`, writes `flex-grow`/`flex-shrink` 0 and `flex-basis` `auto` for a main-axis resize, and `align-self: start` for a cross resize of an item drawn stretched (an `auto` cross size on a kind a stretch resizes, never a frame: `build::stretched_across`), or `end` where the resize held the cross end and moved the start, judged on the flow's cross axis (under `wrap-reverse` a drag by the top writes `start`). Spelled once as `build::held`, also used by `sized_flex_item`. Nothing is held for a kind whose drawn size the layout never sets (path points, boolean operator, a line's end on a growing item); a px `min-`/`max-` is kept and clamps the resize (`container::can_stretch` in `build::flex_holds`). The gesture preview applies the same holds. The Item card names these flips with an Undo taking the whole step back.
- **A resize writes px over a size keyword**: `keep_flex_sizes` sets a `%` or `fit-content` width/height back to `auto` on each axis a resizing `SetGeometry` changed, whatever the parent (`build::sized_in_px`).
- **The stored translation is kept**: `build::kept_flow_translations` (commit door only, never in the preview where it would drop a drag's own translation) makes every `SetTransform` on an in-flow item keep its linear part and the stored translation; one left unchanged is dropped, except where the same edit takes the item out of flow. A resize builds its shift on the used transform, which for an in-flow item is its slot.
- Padding and gap are px only; the Scale tool scales them per axis (`tools::scaled_layout`, §5.6).

### Reorder by drag

An in-flow item's stored translation is drawn nowhere, so a move of one in-flow layer that stays in its parent commits a `Reorder`, not a `SetTransform`. `build::flex_reorder` reads the dragged box's centre against the **laid lines** (runs of other in-flow siblings in flow order, a new line where the main axis runs back in a wrapping container; each band the union of its items' cross extents), landing after every earlier line and, on the centre's line (the band holding it, else the nearest), by main-axis centre (reversed for `*-reverse`). Out-of-flow siblings keep their place; a drop leaving the item's flow place unchanged is no reorder. Z-order moves with it since child order is paint order. Under `wrap-reverse` the first line is the bottom one, which flow order says with no extra arm.

An in-flow item leaves its container only once the pointer is out of it (the area rule decides past that), because an overflowing row can put its box mostly outside a fixed frame. A frame inside the container covering most of the moved box takes it first.

Several in-flow items of one container, none leaving it, reorder as a block: `build::flex_reorder_many` reads the centre of their union by the same reading order (`build::flow_index`), moving them in child-list order; in a grid the block moves by tracks. Any other multi-move uses transforms, except an in-flow item, which keeps its slot in drag and release (`canvas::stays_in_flow`). The preview represents the reorder as a per-parent child order and outlines the landing slot (§6.2, §9.4).

### Inspector cards (§9.4)

*Container* sets `display` and, under flex, every container property above but grid's; *Item* sets an in-flow item's grow, shrink, basis, `align-self` and limits, or says why a layer is out of flow and puts it back. Setting `display: none` leaves every child where it is drawn and a hugging frame at its hugged size (`build::baked`: laid-group pinned children keep their insets; a child container with a layout keeps the size this one gave it) and a stretched label its sizing mode: a drawn `Fixed` box over a stored `Auto`/`AutoHeight` is baked as `AutoHeight` at the drawn width wherever its lines fill the box (`build::baked_ops`; every baking door, Position pin and Group included). The Transform card's W and H offer the sizing modes that differ for the layer; an in-flow item's X and Y are inert (the multi card's too while any member is in flow). A sizing field shows a number where it is a real size (W, H) and the keyword where the property is unset (basis, limits).

### Grid

**Model.** `Display::Grid(Grid)`: `columns`/`rows` as `Vec<Track>` (empty is `none`), `auto_flow` (`Row`, `Column`; no `dense`), `justify_content`/`align_content` both `AlignContent` (a grid's `normal` distribution is `stretch`, so `auto` tracks grow into free space), `justify_items`/`align_items` each `None` for CSS's `normal`, gaps and padding in world units. `Display` is not `Copy`; `LayoutView::display` returns a `Cow`. Track grammar is CSS's: `Track::{Size, Repeat}` (a repeat holds `TrackSize`s so cannot nest), `TrackSize::{Breadth, MinMax}`, `TrackBreadth::{Px, Percent, Fr, Auto, MinContent, MaxContent}`, untagged in the file. One item record serves both layouts: `LayoutItem` carries `justify_self`, `grid_column`, `grid_row` beside flex's fields, each read under the parent's `display`, so values written under flex take effect when the parent turns to grid. CSS text for the cards' typed fields is `tracks_css`/`parse_tracks`, `placement_css`/`parse_placement`; the parser reads and `Grid::is_valid` judges at the operation.

**Validity.** Operations refuse what CSS refuses (`OpError::BadLayout`): a negative track, an `fr` minimum, `repeat(0,...)` or empty repeat, line 0, `span 0`, more than `container::MAX_TEMPLATE_TRACKS` (1000) explicit tracks on an axis (not `layout::MAX_TRACKS`), and for flex and grid alike a negative padding, gap, grow, shrink, size, limit or basis. A file carrying one opens, the value kept as written and read around by `container::style_of` (template laid to the cap, negatives read as zero). An edit carries such a value forward: operations ask `is_valid_over` the value the node holds, so only a refused part the edit writes is refused.

**Engine.** `lay_out` is one `FlexTree` pass; `compute_child_layout` sends a grid container to taffy's grid algorithm. taffy's grid aligns `baseline` on its block axis and reads it as `start` across, so `align-items`/`align-self` offer and name it *Baseline* and the justify rows leave it out; a justify row holding it anyway (from flex or a file) reads *Baseline · Start* with Start's picture, on a lit row `glyph_combo` adds for a held value its list does not offer.

**`normal` is CSS's.** With neither the item's self-alignment nor the container's set, a replaced item keeps its size at the start of its area and a box (text, a container with a layout) stretches across it. taffy's own `normal` stretches anything `auto`, shapes included, so `FlexTree::push` writes `start` into a replaced child's taffy style where `container::grid_held` says to (`container::is_replaced` is the split `push` makes into `Leaf::Replaced`). `fit-content` is never stretched in a grid on either axis; an explicit `stretch` stretches a shape. Flex has no `normal`: it is `stretch` for every item.

**Gestures.**
- Resize holds: `flex_holds` and `sized_flex_item` have a grid arm, `build::grid_resize_held`. On each axis the resize changed, an item its resolved alignment stretches (own, else the container's; under `normal` a box, not a shape) takes `start`, or `end` where the resize moved the left or top edge and held the other. The size is the resize's own; cells are kept. `build::resized_edges` is the edge reading.
- A drop writes lines: `build::grid_drop` moves the item's area by the tracks its centre crossed on each axis, writing both axes, the span in the author's spelling, the start landing on a track the grid has (a start before the explicit grid written as the negative line that names it). Nothing is answered when the centre stays in its tracks (no undo step). The centre is used, not the pointer, matching flex's reading and avoiding a snap on release for off-centre grabs.
- `build::layout_drop` sends a drag to `grid_drop` or `flex_reorder` by the parent's `display`; `canvas::flex_reorder_of` asks it, so the preview adds the lines and siblings reflow round the landing.
- Several items move as a block: `build::grid_drop_many` moves every item's area by the tracks the centre of their union crossed, writing each explicit lines. A block pushed towards either end stops there whole (shift clamped to the leading item's room, or to where the trailing area's end meets the last laid track), so a drag makes no track past either end. `grid_drop` is its block of one; `build::layout_drop_many` picks `flex_reorder_many` or `grid_drop_many`; `canvas::flex_block_reorder_of` asks it.
- **Tracks are derived when asked and stored nowhere.** `container::laid_grid` (public as `build::laid_grid`) re-runs the pass from the grid's layout root under taffy's `detailed_layout_info`, at the size `lay_out` lays that root (a pinned root's stretched size included), and answers each axis's tracks in the container's space and every in-flow item's area as CSS line numbers. A stored map would be one more thing `update` must keep equal to `rebuild`.

**Track lines on the canvas.** `OndinApp::draw_grid_tracks` draws every track edge as a 1px `SELECT_DIM` line across the other axis's run of tracks (the container's box where that axis has none): dashed where a template track has the edge, dotted where only implicit tracks do (an edge shared by two tracks is drawn once, dashed if either is explicit). The space between neighbouring tracks (gap plus what `space-between` etc. hand out) is a band of `color::GRID_GAP`. Tracks come from the preview (`RenderOverrides::laid_grid`), except for a grid pinned both sides inside a parent drawn at another size than it stores (where `build::laid_grid` through `DocView` lays against the parent's stored box). `build::grid_drop_many` lays through `resolve::CommittedView` (`DocView`'s answers with the parent's box from used geometry) so it picks the cells the grid is drawn with.

Drawn for every selected grid container, and during a move for every selected layer's parent that is a grid; not for a hidden container. A moved item's unselected container is drawn as the drop reads it (`build::drop_grid`: `container::laid_grid` through `CommittedView`), not as the preview lays it, because the preview already contains the drop's `SetLayoutItem` and with content-sized tracks that moves the cells. A selected container keeps the preview's. Under an Alt-drag a selected container's lines ride the copy; an unselected one keeps its lines in place.

Hidden wherever `canvas::chrome_hidden` hides selection chrome, and by the pen or a path/image edited in place (`canvas::editing_in_place`); under the inspector's edit hold, only while the preview restyles the container without touching layout (fill, opacity), since the hold outlives the preview. A gap, padding or track scrubbed in the inspector keeps them. Z: over artwork, layout grids and frame labels; under hover outline and selection chrome.

Uncached: each drawn container's layout pass re-runs every frame. `OndinApp::dashed_in_view` cuts each dashed line to the view grown by one dash period (`egui::Shape::dashed_line` emits a segment per dash over the whole length), its start pulled back to a whole number of periods so the pattern holds still under a pan. Gap bands outside the view, and containers whose box misses it, are skipped before their layout pass. `track_lines` merges an edge with the last one only.

### Spacing handles on the canvas

A flex or grid container's gaps and padding are drawn as bands and dragged to write them, under a tool that selects. The subject is the container under the pointer (`spacing_subject`; `spacing_hover` = topmost layer there, in no group or instance, not locked), else the single selected one; one of several selected is never the subject, and a drag selects nothing. Unselected, a container shows and picks its gaps only (padding is where a marquee over its children starts).

The drag is one `SetDisplay` through the commit door, the Container card's own write, so `keep_spacing_bindings`, the style pass and override marks judge it as a typed value. For a bound slot the drag cuts the binding itself with a `SetSpacingVars` ahead of the `SetDisplay`; left to the pass, a drag to 0 was written as the slot's literal 0 and kept bound.

`spacing::bands` answers in the container's local space: four paddings edge to edge, a grid's column-gap and row-gap bands from its laid tracks across the other axis's run, a flex container's between-items bands across each line's cross band, and a wrapped one's line gaps across the content's whole main extent. A gap band names its axis, not its role: `column_gap` is the horizontal gap and `row_gap` the vertical in every direction, so a column's between-items gap is the row gap.

Nothing is drawn at rest. With the pointer inside the subject every band is drawn in `GRID_GAP`'s tint (a grid's gaps excepted, its track lines draw them) and the band under the pointer in `SPACING_STRONG` with a pill. A band under 6 screen px (`MIN_BAND_PX`) is dropped, except the kind being dragged. The whole band picks, at least 8 screen px across (`MIN_PICK_PX`; a zero gap is a strip on its seam); nothing picks within `HANDLE_PICK_PX` of the outline. A padding band yields to any child on the point except a mask; a gap band yields only to a child pinned by authored insets or, in a grid, an item spanning the gap, so a zero gap can be dragged open. The drag writes half the pointer's delta along the axis (`DRAG_RATE`), measured in the committed frame so the rate holds while the preview moves the band, in whole px, floored at 0, from the drawn value over the stored display, so an untouched bound slot stays bound. `Shift` steps by 10, `Alt` writes the opposite padding, `Alt+Shift` all four. Press, cursor and release are §9.4's.

### Traps

- **Two defences for flex translations:** `kept_flow_translations` runs at the commit door only; running it in the preview drops a drag's own translation.
- **Reading committed geometry** in an editing path (instead of used geometry) pins a laid group's child against the old size and snaps it back on release.
- `keep_insets` / `keep_flex_sizes` must never touch the vault; a vault node has no used geometry.
- A fix that adds a field to the model must be read by every used-geometry consumer listed above; the compiler does not find them.
- A grid drop must pick cells through `CommittedView`, not the preview, or the preview's own result feeds its hit-test.
- A justify row holding `baseline` is not a valid pick in a grid; show it, do not offer it.
- `layout::MAX_TRACKS` (layout grids, 10,000) and `container::MAX_TEMPLATE_TRACKS` (grid container, 1000) are different caps.
