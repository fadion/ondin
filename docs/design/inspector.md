# Inspector cards

The inspector's cards for v1 tools: Transform (single and multi), Position, Item and Container,
the frame cards, Effects, Export, Appearance, Fill and Stroke with their mixed states, and Group
Colors. The Type card is in `type-panel.md`.

## 9.4 v1 tools — inspector

### Card order and composition

- **Single layer:** identity, then *Frame templates* (frames only), Align, Transform, Position, Item, Container, *Layout grid*, Appearance, Type (text), Fill, Stroke, Effects, Export.
- **Multi-selection:** the whole inspector, not just a count. Transform, Appearance, Fill, Stroke, Group Colors and Export, all backed by §5.7a's bulk builders, so the app, MCP and the CLI agree on what "set the fill of these" means.
  - Text layers in the selection add the Type card. With one text layer it is the single card; with two or more it is the several-layer card, which writes per layer (§15 D1077).
  - Frames in the selection add *Frame templates* (under identity) and *Layout grid* (under Container).
  - Pinnable layers add Position. Frames, groups or children of laid containers add Container and Item.
- **"Mixed" is a panel, not a label** (§15 D53). Every control that has room says the word *Mixed*. Only controls too small for the word show a dash (§15 D130): `ui::Swatch::Mixed`, a 16px chip, and `ui::segment_mixed`.

### Transform (single layer)

**W and H for kinds with no authored size.** `Path`, `Boolean` and `Group` show W/H, with
`inspector_single`'s `size0` falling back to `preview_local_box`, the box the canvas handles use.
An empty group shows no row.

- **The write.** An authored `size` is replaced. Anything else goes through `tools::resize_box_to`, which puts a pointer at the corner the new box wants and calls `tools::resize_layer`, the bottom-right drag's path. A typed number and a dragged handle therefore mean the same thing.
- **Geometry only.** The write asks for geometry only, with no modifiers. The proportion lock is applied to the target size before the call; passing `keep_ratio` too would apply it twice.
- **Committed values.** The box and transform come from the committed tree, not the preview, so a scrub does not compound.
- **`Line`.** No W/H. Its W slot shows **`L`** (length), which writes through `tools::move_line_end` and anchors the start point.

**The proportion lock** is a 28×28 `link-simple` button after W and H. W is exactly X's width and H
absorbs the difference, so the two rows share an edge down the middle of the card.

- **Locked editing.** Editing either field scales the other by the aspect measured before either was touched.
- **Stored on the node.** It lives at `Node::proportions_locked` (§5.3) and is saved, because it is a constraint on gestures. The per-corner radius disclosure is view state in `OndinApp`.
- **Commit path.** It reads `DisplayNode::proportions_locked()` and commits `SetProportionsLocked` straight through `session.commit`, as a toggle with no valve.
- **Glyph.** Broken chain unlocked, whole chain locked.
- **Scope.** `canvas::keep_ratio` is the lock XOR Shift, so Shift on a locked layer releases it. Snapping always applies, and the aspect constraint is applied to the snapped pointer: the constraint wins over the snap.
- **Over a set,** the lock reads as all-locked, through `tools::proportions_locked`, asked of the whole selection. An empty selection is not locked. Its users: handles, multi W/H, `Ctrl`+arrows, and the multi chain button.
- **Paired size.** The numeric doors pair through `tools::paired_size`. They do not use the handles' `Resize` flag, which takes the larger factor and would refuse a one-axis shrink.

**Sizing modes on W/H** (`layout::size_modes`). The unit is a menu:

- `px` always;
- `fit-content` for a frame with a layout;
- `auto` for a group with one;
- `%` for an in-flow kind that layout can resize.

How it behaves:

- **Keyword display.** A keyword side shows the size it resolved to, with the keyword where the unit goes. The menu opens under the unit (`ui::value_field_unit`).
- **Picking a mode** (`layout::size_mode_tx`) moves nothing. It converts what the axis *specifies*: a number as it is, `auto` as the stored size, and the drawn size only under a keyword.
- **`px`** writes the picked axis only. For text it goes through `baked_ops` and keeps the stored mode: W gives an auto-width label `AutoHeight`, and H fixes the box.
- **Picking the lit row** does nothing. This stops a re-pick from rewriting `33.33%` as `33.3%`.
- **Typing and clicks.** Typing writes px, or `%` while in `%` mode. A click in and out writes nothing. A typed size keeps a text's stored sizing mode.
- **Lock disabled.** The proportion lock is disabled while either side is a keyword or `%`, with the reason on hover.
- **Single card only.** Multi W/H are plain fields.

**X/Y of an in-flow item are disabled**, with the reason on hover. Its container places it, and
`keep_flex_sizes` drops a typed translation. On the multi card they are disabled while any outermost
member is in flow. The hover suggests dragging on the canvas only when every member is an in-flow
item of one container (`inspector::drags_as_a_block`). `inspector::laid_out_xy` answers both the gate
and the hover.

**The Scale row** appears only while the Scale tool is held: a percentage in W's slot, with `0.5x`
and `2x` buttons filling the row.

- **Photographic scaling.** It passes `tools::Scaling::Photographic`, so strokes, radii and font size scale with the layer. Every W/H path passes `Resize::geometry`.
- **No stored scale.** The field is a verb with a live readout. It applies against the committed box every frame and returns to 100 when the valve closes, or when the row disappears.
- **The write.** It goes through `inspector::scale_tx`, which resizes the union from `Handle::BottomRight` so the top-left holds, through the ordinary edit valve.
- **Live readout.** During a canvas Scale drag it reads out `canvas::live_scale_pct`, the geometric mean of the previewed and committed factors, and commits nothing.
- **Halve and double.** `0.5x`/`2x` are `ui::label_button` at `FieldButton::Off`, straight to `commit_edit`. They are never shown as selected.
- **Named rows.** Because this row can come and go, every row from it downwards is drawn through `inspector::transform_row` and identified by name, not position. Otherwise egui's positional ids would renumber and lose half-typed `DragValue` state.

**Rotation and skew** share a row at X/Y widths. Below them, four equal buttons (`ui::field_button_sized`):
turn 90° counter-clockwise, turn 90° clockwise, flip H, flip V.

- **Skew** is clamped at `tools::MAX_SKEW` (±89°) and uses the `ANGLE` glyph. It reads `build::Basis::of`, and rebuilds by replacing `skew` alone, keeping `angle` and `mirror` together and multiplying the residual world-basis scale back on.
- **Rotation** shows `Basis::of`'s angle at rest, and a latched number during a gesture (§15 D206). It writes by composition (`inspector::turned_about`, `rotate_about · old_world`), never by rebuilding, which would spin a mirrored layer past ±90°. Both fields turn about the pivot.
- **A `Line`'s rotation** is the world-space start→end angle, not its basis angle. Writes apply the delta (typed minus shown) about the pivot.
- **No `-0°`.** `inspector::shown_degrees` snaps magnitudes under 1e-9° to +0. Tests must assert `is_sign_positive`, because `-0.0 == 0.0`.
- **Quarter turns** go through `tools::rotate_node` about the pivot read from the committed tree. Adding 90 to the field would be wrong for mirrored and sheared layers. Mirrors use `build::flip`. Both cards draw the row through `inspector::quarters_row(TurnScope)`.

### Transform (multi-selection)

**Position and size describe the union box; rotation and skew describe the layers.**

- **X/Y/W/H** move and scale the union, and every member follows. Scale % is on the union side too: the arrangement scales about the union's top-left.
- **Rotation/skew** show the shared value or `Mixed`. Dragging nudges every layer by the same amount; typing sets each one absolutely. Both act about each layer's own origin. Sweeping about the union centre is only reachable from the canvas ring. Skew clamps to `MAX_SKEW`.
- **Row layout** matches the single card (X/Y; W/H plus chain; Scale when armed; rotation plus skew; four discrete buttons), with named rows.
- **Quarter turns** (`inspector::rotate_selection_quarter`) turn each layer about its own origin in one transaction. They stay live over a mixed rotation.
- **Mirrors** use `build::flip` about the union centreline, so the layers swap sides as one arrangement.
- **The chain button** is live. It shows whole only when every member is locked, and a press drives the whole set to `!all_locked`.
- **The pivot toggle** is `FieldButton::Disabled`: a set has no stored origin.
- **Cost guard.** The six fields build a transaction only while mid-edit, because `resize_selection` recurses through every descendant. X/Y/W/H also return an empty transaction when nothing changed, and W/H compare after `paired_size`.

### Position (§5.3c)

Offered for layers whose parent is an `Artboard`, or a `Group` with `display`, and whose kind takes
insets (`inset_subjects`). A laid group's child is measured against the group's laid box
(`inset_placement`, through `preview_frame`). Each edit applies to every subject at its own distances.

- **Pin diagram** (`INSET_DIAGRAM_W` = 74): a well with the layer at its centre and four struts, and no container outline.
  - A pinned edge is solid accent. An unpinned edge is dashed and faint.
  - An edge pinned between two auto margins is solid `color::MEASURE`.
  - Clicking anywhere in the band between the layer edge and the well pins or unpins that edge where it is (`container::with_edge`).
- **Four fields** (L/R over T/B).
  - A pinned inset shows in its own unit with an accent edge, and a clickable `px`/`%` suffix that converts without moving anything. A unit switch converts existing pins and never makes one (`inset_flip_ops`).
  - An unpinned edge reads `auto`, with its current distance underneath as the value. Typing or dragging pins it in px through the valve, after writing the item's drawn placement so the other axis holds (`typed_inset_ops`).
  - A field only clicked through pins nothing.
- **Default hold.** Outside a layout, top and left are held by default: an axis with neither inset keeps its offset from the container's top-left. It is shown as held (solid strut, distance with no unit) without writing an inset. Clicking a held strut pins it; pinning the opposite edge ends the default.
- **Centre buttons**, one per axis: both insets at their current distances plus `margin: auto`. Off unpins the end edge and the margins.
- **Badge.** The *Absolute* header badge shows only inside a container with a layout (`ui::section_head_badged`).
- **Baked placement.** Every pin, unpin and centre commit also writes where the layer is drawn into its stored transform, and for a stretched layer its stored size (`baked_placement`). Unpinning therefore never jumps.
- **Kept in place.** Taking an in-flow item out of a hugging container measures against that container's post-edit box (`kept_in_place`).
- **Reads** `DisplayNode::insets`.

### Item and Container (§5.3c)

The order is Transform, Position, Item, Container, Layout grid: from the layer in its parent to its
children. Subjects come from `item_subjects` and `container_subjects`, on `frame_subjects`' rule.
Labels use CSS names in sentence case (*Justify content*, *Flex start*), with lower-case units.

**Container.** `display` is a segmented row: `none` / `flex` / `grid`.

- **Under `flex`:**
  - direction and wrap, with per-cell tooltips (`ui::segmented_tipped`);
  - three alignment dropdowns whose pictures turn with the container (`layout::Orient`, a transpose and a mirror, never a rotation);
  - two gaps;
  - padding as two paired fields that open to four.
- **Under `grid`** (`panels/layout/grid.rs`):
  - `grid-auto-flow` (*Row*/*Column*);
  - column and row track lists, each with a count, a `+`, and rows of grip, kind menu, fields and cross, plus the whole template as an editable CSS line. The line applies on a committing defocus, never on `Escape`, and shows parse errors in red;
  - `justify-content`, `justify-items`, `align-items` and `align-content`, with *Start*/*End* names. The items rows lead with *Normal*. `align-items` offers *Baseline* and `justify-items` does not; a held baseline reads *Baseline · Start* on its own lit row;
  - gaps, columns first;
  - padding.
- Switching between flex and grid keeps padding and gaps.

**Item.**

- **In flex:** grow, shrink, basis, `align-self`.
- **In grid:** `grid-column`/`grid-row` as CSS, start/end, `justify-self`/`align-self` (the latter with *Baseline*).
- An unset self-alignment reads *Auto · \<inherited\>*.
- **Both:** four limits behind a *Min / max* disclosure. Its closed summary is read per item, and reads *Mixed* when items differ.
- A basis or limit left at a keyword shows the keyword in the digits' place, with no unit; typing a number leaves it.
- **Skipped layers.** For a layer the flow skips, the card says why in words (*Absolutely positioned* and which edges hold it, *Hidden*, or a mask), with one button that restores every subject skipped for that reason.

**`display: none`** from the card writes every child's drawn transform and size into its stored
values, and a hugging frame's hugged size into its own (`build::baked`, `build::baked_ops`), so
nothing moves.

- This covers every child, pinned ones included, whose insets are kept.
- A child container keeps the size it was given.
- *Unpin and return to the layout* does the same.

**Receipts.** When a resize flips grow, shrink, basis, `align-self`, or a grid item's
`justify-self`/`align-self` on the user's behalf, the inspector names it and outlines the changed
rows (all but basis). An Undo takes back the whole step. Each property is described from the items
that changed, with every distinct value once.

Fields read `DisplayNode::display` and `item` and commit through `layout::edited`'s latch (§9.3). A
number field over a disagreeing selection reads *Mixed* (`layout::mixed_if`). A mixed segmented row
draws `segment_mixed`'s dash in its first cell. A sizing field's unit reads `–` beside *Mixed*.
Neither card has a header badge.

### Frame cards (§5.3b)

**Frame templates.** Second card, above Align and Transform, collapsed by default. It is not shown
when any selected frame is a main, variant, set or instance root.

- **Layout.** Five groups (Phone/Tablet/Desktop/Print/Social) in a scroll box capped at 184pt, two cells across. Each cell draws the template at its proportions in a 20×28 slot, floored at 5pt a side, beside its name and size.
- **Cells.** Cells are hand-painted, with no resting ground. Lit and hovered inks are `button_face`'s.
- **It reads as well as writes.** The size every selected frame agrees on, from the committed document, lights a cell. A match must be exact and orientation-sensitive. Two entries may not share a box, because `preset_named` returns the first match.
- **Print sizes** are points at 72/inch (A4 = 595×842).
- **The write.** Picking one writes `GeometryPatch::Size`, the same patch W/H write. Clicking the lit cell writes nothing.
- **Over a multi-selection** it writes each frame's own size and leaves the children alone. That differs from multi W/H, which resizes the union as a rigid body. Disagreement just lights nothing.

**A frame's paint** uses the shared Fill and Stroke panels (`PaintScope::Node`). There is no separate
background card, and an empty fill reads *No fill* as on every layer.

**Layout grid.** Under Container. One block per grid:

- an axis switch (`columns`/`rows`), an eye and an ✕;
- the grid's colour on a Fill row;
- `N`/`G` and `M`/`W`;
- a four-cell alignment strip whose glyphs follow the axis.

Details:

- Fields an alignment does not read are dimmed, not hidden, so their values survive a switch.
- **`+`** adds columns to a frame with none, and rows to a frame that already has columns. It takes the first `layout::GRID_COLORS` entry the frame is not using, and opens the card.
- **Open and closed.** The card rests closed while there is no grid, and closes again when the last is deleted (`sync_paint_collapse`, through its own `synced_selection` entry).
- **Hidden grid.** Its controls are `disable`d, except the eye and ✕. The eye is `FieldButton::Off` in both states, with `EYE`/`EYE_SLASH`.
- **Spacing.** Rows sit at `ui::CARD_ROW_GAP`, and grids twice that apart.
- **View switch.** *View ▸ Show layout grid* sits above the eye. With it off, nothing draws, and the card does not say so: workspace state and document state never write to each other.

**Subjects.** Both frame cards find their own subjects (`frame_subjects`), filtered out of the
selection. Over several frames the grid card has three states:

| State | Card |
| --- | --- |
| none has a grid | rests closed; `+` writes one grid to all |
| all agree | shows the list; edits reach every frame in one undo step |
| any differ | shown open with *Mixed layout grids. Add to override.* and no rows; `+` replaces every list |

Rows draw only when the frames agree, which is what makes the first frame a safe anchor for
`PaintSlot::Grid(i)`. The detached picker closes when `shared_grids` answers `None`. The agreed list
comes from the committed document.

### Effects (§5.3a)

- **Rows.** One row per effect: the kind glyph, name, one headline number and ✕, with the eye outside the field. The headline is a shadow's alpha %, a blur's radius, or the count of off-neutral channels for Filters. `+` adds a drop shadow.
- **Order.** Model order, first at the top: effects are a pipeline (§6.4). Fill draws upside down because paints stack in space.
- **Popover.** The design's card with its title replaced by a kind combo. Retyping carries shared fields over (`EffectKind::retyped`). It sits in the shared popover lane and dismisses through `panels::dismissed_by_click`.
- **Shadow colour** is a Fill row (chip, hex, %) on `PaintSlot::Effect(i)`. The % *is* the alpha.
- **A Filters row** uses `ui::value_field_metered`, a bar growing from the middle, plus a per-channel reset. The ranges are 0…200%, symmetric about the 100% neutral, so the meter reads as `factor − 1.0`.
- **Open and closed.** The card opens on a layer with effects and shuts on one without, through its own `synced_selection` *Effects* entry.
- **Multi-selection.** Export's shape: one agreed stack, or *"These layers have different effects"*, where `+` replaces every stack. The `Effect(i)` write reads each subject's own list rather than cloning the anchor's, because the detached picker can outlive an agreeing state.
- **Not built:** reordering the stack is a decided non-goal. It changes pixels only between `Filters` and `LayerBlur`, and between two shadows on the same side.

### Export (§7)

- **Rows.** One row per `ExportSpec` on a `74px 1fr 28px 28px` grid: scale dropdown, format dropdown, settings button, ✕. Scale is dimmed, not hidden, for SVG.
- **Size line.** Under each row a faint line gives the pixel size the row will produce.
  - Row and line are one unit 3 apart; units are 6 apart. `item_spacing` is zero inside the region, and both gaps are allocated.
  - A row with no size line (every row over a multi-selection, and a single layer's SVG row) spends 6 + 3. The last row allocates nothing.
  - The line zeroes `interact_size.y`, so 13pt ink is not placed in a 24pt slot.
- **Open and closed.** Closed on a layer that exports nothing. The header `+` opens it, through its own `synced_selection` *Export* entry.
- **Settings popover** (272 wide, `menu_frame` padding, `popover_anchor` lane, `ui::POPOVER_SECTION_GAP`): prefix, suffix, JPEG quality, background, and two switches. Over a multi-selection the *Frame's own* background chip is muted, with a plural tooltip. `Solid`/`Transparent` show normally.
- **Panel menu popover:** presets, copy/paste a whole list, folder rule, re-export-on-save. It and the settings popover are mutually exclusive.
- **Popover sets.** Both popovers are in all three lane sets. One `key_pressed(Escape)` block at the foot of `export_buttons` clears both, and reports to the router only while one is open.
- **`+` over a multi-selection** stays live. Over a disagreeing selection it replaces every list with the one seeded export. Its tooltip is three-way.
- **Seeding.** `ladder_scale` climbs 1×, 2×, 3×, then 4× forever, by row count. The format is inherited from the last row, PNG otherwise. Every rung is in `ExportScale::COMMON`. Duplicate names are de-collided on disk.
- **Preview** section at the foot, collapsed by default. Opening it renders and encodes once per edit, keyed on `EditorSession::revision`.
- **JPEG quality** is the one scrubbable export setting. It holds its own in-flight value in `export_quality_scrub` and bypasses both committers: `commit_run` would make it uncancellable, and `RenderOverrides` treats `SetExports` as a no-op. The decision is `quality_step`.

### Appearance (multi-selection)

- **Fields.** Opacity, and corner radius when any member is a rect. Both are absolute and show *Mixed* on disagreement.
- **Scope.** Opacity is set on outermost members, radius on the whole subtree (§5.7a).
- **Corner button.** The per-corner button is present and inert, and the fields keep single-card widths.
- **Tolerance.** "Disagree" is a tolerance, `build::OPACITY_AGREEMENT`/`RADIUS_AGREEMENT`, read by `inspector::shared_opacity_shown`/`shared_radius_shown`.

### Fill and Stroke

**One implementation over a `PaintScope`** (`Node(NodeId)` or `Selection`).

- **Shared lists.** Over a selection the shown list is `build::shared_fills`/`shared_strokes`, giving a `PaintShown` that is either a `List` all targets agree on by value (item ids ignored) or `Mixed`.
- **Writes** go through `build::retarget_fills_all`/`retarget_strokes_all` (`inspector::write_fill_list`, `write_strokes`). Every target gets the edited values under its own item ids: the anchor's mapped by position, and one fresh id per added row on every target (§5.3d).
- **Hidden panels.** Both panels are hidden when nothing in scope takes paint (`build::any_paint_in`).
- **Slots.** Rows ride `PaintSlot::Selection(PaintTarget, usize)`.
- **List writes.** `write_paint_list` is generic and carries whole `Fill`/`Stroke` values, so visibility, order and stroke properties survive.

**A disagreeing selection draws one stand-in row**, for any disagreement, including one unpainted
layer. The empty state is only for nothing painted at all.

- **Per-property values.** Values are decided property by property (`build::shared_over_fills`/`shared_over_strokes`, through `OndinApp::shown_over_fills`/`shown_over_strokes`, one walk `shown_over` over `display_node`). What the paints agree on shows as itself.
- **Display vs operation.** `inspector::Spread<T>` separates what the row says (`agreed()`) from what controls operate from (`base()`, the first paint in scope). So the chip shows the dash and the hex says *Mixed*, while the picker opens on a real colour and the weight scrubs from a real number. `shown_brushes` is the single entry for chip, hex and picker. A mixed row always uses the hex field, even when the base is a gradient.
- **Seeded fields** write only where the value differs from the seed. This covers every field seeded from one member of a disagreeing set, including the single-layer radius field showing `largest(corners)` (guard `shared.is_some() || r != seed`).
- **Hex field** (agreeing row): it compares bytes, `parse_hex(&text)` against `parse_hex(&hex_of(current))`, before writing. Defocus uses `ui::defocus_commits`, so `Escape` does not commit. It is buffered, not valved, because partial hex like `6D8` is a valid, different colour.
- **One property per control.** Each control edits one property of every paint in scope (`build::edit_fills_all`/`edit_strokes_all`). Chip and picker set the whole brush. Hex sets RGB and keeps each alpha. % sets alpha and keeps each colour or gradient. The picker's own hex and swatch grid go through `edit_paints_all`. Its plane and strips set the whole brush.
- **Eye.** Reads *any* paint on.
- **`+`.** Inert (`ui::HeadAction::inert`, with the reason). The ✕ clears every target.
- **Slot after agreement.** The stand-in rides `PaintSlot::SelectionAll(PaintTarget)`. When a write brings the targets into agreement, `write_slot`/`valve_slot` call `retarget_mixed_paint` to move the picker and hex buffer to `Selection(target, 0)`.

**Stroke properties over a scope.** `stroke_properties` works over a selection. Three controls are
offered only when every target qualifies, through `inspector::stroke_rule(scope, applies)`:

| Control | Shown when every target | Predicate |
| --- | --- | --- |
| alignment | has a closed outline | `geometry::stroke_align_applies` |
| sides | is a rect | `stroke_sides_apply` |
| join | has a corner | `stroke_join_applies` |

**Mixed stroke unit** (`mixed_stroke_properties`), laid out by the same `inspector::paint_unit`:

- **Position dropdown** (`inspector::align_combo`, shared): shows *Mixed* as its text but never lists it, and lights no cell. Both behaviours fall out of `Option<StrokeAlign>`.
- **Weight:** *Mixed*, absolute, starting from 0. It writes through `set_stroke_weight`.
- **Dash/cap/join popup and sides selector:** present and `FieldButton::Disabled`, with tooltips.

**Paint rows** carry a grip and an eye. `RowCtx::grip: Option<(PaintList, usize)>`: canvas ground,
guide colour, decorations and Group Colors pass `None`; a frame's fill is a real list entry.

- **Hidden rows** draw at `HIDDEN_OPACITY` (60%). The swatch is painted at full opacity inside a nested `set_opacity(1.0)`, because `mask_corners` paints ground-coloured slivers that would turn translucent.
- **Grip.** It is drawn on a list of one too, but `RowCtx::reorderable = false` stops it sensing and drops its tooltip (`ui::grip(ui, enabled, row_h)`). The grab band is the field's whole left end up to the glyph's right edge, not the 9pt gap before the chip.
- **Stroke unit.** A stroke is a multi-row unit: a colour row, then position + weight + options + sides (`1.2fr 1fr 28px 28px`), then sides rows when open. `paint_unit` sets `item_spacing.y` to 0 and allocates both gaps: rows one `CARD_ROW_GAP` apart, strokes two apart, nothing after the last. The sides rows add explicit `add_space` and their own scope.
- **Drop targets.** `paint::Slots` records the spans actually laid out. Drops are resolved from those spans, never by dividing by a pitch.
- **Live reorder.** `OndinApp::paint_order` runs before drawing and returns the indices in dropped order, so there is no insertion line. `inspector::carried_unit` lifts the carried entry into an `Order::Tooltip` layer and translates its shapes, with the layout untouched (§15 D59, D61). The stroke options popup closes when a drag starts. The helpers are `paint::PaintDrag`, `display_order`, `landing`, `reordered`, `reordered_index` and `removed_index`. The layers tree deliberately does not reuse them (§15 D306).
- **Weight** scrubs in whole units (`ui::Scrub::whole(0.5)`) and types any number. The cap `ui::MAX_STROKE_WIDTH` (10 000) applies to weight, per-side widths and dash numbers, as a field cap only.
- **Weight is the master.** `inspector::set_stroke_weight` re-broadcasts to all four sides of a `Custom` set, zeroes included. Named single sides are left alone.
- **Valve.** Every scrubbable stroke number goes through `inspector::stroke_valve`. `strokes_tx` fans the write out, and returns empty when nothing changed.
- **Position dropdown height** is set through `interact_size.y`/`button_padding.y`, at the full 28.

**Sides selector:** rows in the card, not a popover.

- **Why rows.** It carries values beside the weight, two strokes would otherwise occlude each other, and the rows travel with a reorder.
- **Visibility.** Shown when `sides_apply && stroke_sides_open.contains(&index)`. The button opens and closes it without touching the stroke, and shows `FieldButton::Set` while the sides hold a value.
- **"All sides"** is in the dropdown.
- **Index-keyed chrome.** `forget_stroke_chrome` drops it (popup, open sides, dash buffer) on a subject change only. `remap_paint_indices(anchor, list, map)` renumbers it on reorder or removal: popup, sides, dash buffer, detached picker and hex buffer.

**Dash/cap/join popover** (`app::POPOVER_W`, 272): four modes, each showing only its own fields.

- **Modes and numbers.** The mode comes from `Stroke::dash_style()`. `pattern_for` carries numbers across switches. A mode with nothing to inherit falls back to multiples of the width.
- **Cap and join** are common to all modes. Cap is a named combo, *Straight / Rounded / Extended* (`cap_label`).
- **Custom list.** A buffered comma-separated field applied on `ui::defocus_commits`. Unparseable, all-zero or single-number input leaves the model alone. It writes only when the text differs from what `dash_list_text` would print.
- **Mitre angle.** Shown only when `joins && join == Miter`.
- **Join row.** `Ui::disable`d where the kind has no corner. Dimmed, not reset.
- **Dismissal.** The popover stays open when a cap is chosen: the dismiss test asks whether the click landed on a layer floating above.

### Group Colors

**Rows.** Lists every distinct solid colour in scope as an editable row. Editing one recolours every
fill and stroke using it, in one transaction.

- **Scope.** The selection, except that a single selected container contributes its children rather than itself (`group_color_scope`). The listing, both edits and `group_color_shown` all use this one function.
- **Colour slot.** `PaintSlot::GroupColor([u8; 4])` is keyed by the committed colour. After a commit, row and picker are retargeted to the new colour, from both `write_slot` and `valve_slot` through `inspector::retarget_after`. `group_color_shown` finds uses in the committed document and shows the override's value.
- **Row ending.** A row ends in a layer count (`ColorUse::nodes.len()`) and a reticle that selects those layers. `uses` counts paint slots instead, so the two differ.
- **Locked layers.** The reticle goes through `OndinApp::may_select_in_bulk` and leaves locked layers out, saying so (*"Selected 2 of 3 layers with this colour — 1 locked"*). The count is not narrowed. If every user is locked, nothing changes.
- **Gradients** are listed under the solids (`build::gradients_in`, §5.7a) on `PaintSlot::GroupGradient(build::PaintAt)`, keyed by first location, which is stable because the census orders its roots. Edits read the brush from the committed document and `build::repaint` every use. A location key needs no retargeting (§15 D135).

**Group vs frame.**

- **Group:** Fill and Stroke in `PaintScope::Selection` over its contents, plus Group Colors.
- **Frame:** its own `PaintScope::Node` Fill and Stroke, plus Group Colors for its contents.

### Clip content

"Clip content" is a `switch_row`, the same control the OpenType list uses, not an `egui::checkbox`.

## Traps

- **Three doors on a click-through.** A bare click through a multi X/Y/W/H field is stopped by the panel's empty-transaction arms, by the valve's engagement latch, and by `changes_nothing` at `commit_inner`. A click-through test stays green with any of them removed, so each door needs its own test.
- **Seeded from one member.** Any field seeded from one member of a disagreeing set must write only on a real change, or a bare focus flattens the set.
- **Hex quantisation.** The hex field quantises to 8 bits. Compare bytes before writing, or a click-through rewrites the colour and spends an undo step.
- **Value keys.** Value-keyed slots (`GroupColor`, `SelectionAll`) name something that stops existing after the commit. Retarget from every slot verb, or the picker closes in the user's hand.
- **Synced-selection edges.** `paint_subject_changed` consumes its edge. Each card that opens and closes on the subject (Fill, Stroke, Effects, Export, Layout grid) needs its own `synced_selection` entry.
- **Inherited spacing.** A scope's `item_spacing` does not apply before its first item. Spacing inside `paint_unit` must be explicit.
- **Committed-tree reads.** Template lighting, grid agreement and Group Colors keys read the committed document. Reading the preview mid-drag gives the wrong answer.
- **Effect write from the picker.** `PaintSlot::Effect(i)` must read each subject's own list, never clone the anchor's. The picker outlives the card's agreement.
