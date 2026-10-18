# Tools and canvas interaction

The v1 tool set and how canvas gestures behave: the wheel, cancelling, context menus and their
chrome, selection chrome, multi-selection transforms, the Scale tool, skew, snapping during
resize, drop outlines and the transform origin (pivot).

## 9.4 v1 tools

The v1 tools:

- **Select**: click, shift-click, marquee, select-all. **Hand** pans.
- **Move and rotate** (`SetTransform`). Shift locks a move to one axis and a rotation to 15° steps.
- **Resize** by any of four corners or four sides (`SetGeometry`, §5.6). Shift locks the ratio, or releases it on a layer whose proportions are locked. Alt scales about the box centre.
- **Transform origin**: a draggable pivot marker armed by the Transform panel's crosshair toggle, with two numeric fields in the size badge's slot while it is armed.
- **Skew**: `Ctrl`-drag a side. Shift holds it to 15° steps.
- **Group resize** (recursive, §5.6), and one box over a multi-selection for resize, rotate and skew.
- **Scale** (`K`): the same gestures, but stroke widths, radii and type scale too.
- **Shapes**: frame, rect, ellipse, polygon, star, line.
- **Pen** (`Path`), straight and curved. Corner↔smooth conversion over a point selection goes through `tools::set_smoothness`, and a smooth pair is exactly mirrored, because that is what `is_smooth` means.
- **Text**: create, then edit on the canvas with a real caret.
- **Group/ungroup** and **frame selection** (`build::frame`, a new frame sized to the selection's union; a second builder beside `group`, not a flag on it).
- **Align**: to each other, to the container with one layer selected, or to a **key layer** (`Alt`+`Shift`+click).
- **Distribute** (equal gaps, three or more layers) and a **named gap** (two or more layers, packed outwards from the key).
- **Flip** H/V: across the selection's centreline, or about a single layer's pivot where one has been placed. **Quarter turns** either way.
- **Z-order**: one step or all the way (`Ctrl`+`[`/`]`).
- **Snapping**: alignment snapping with transient snap lines; equal-gap snapping with its gaps quoted on the canvas; baseline snapping, with a selected node's baselines drawn; the pixel grid and pixel snapping.
- **Guides and view**: rulers and ruler guides (`Shift`+`R` toggles the bars), the top bar's View and Snap menus, present mode.
- **Paint editing**: stacked fills and strokes (§6.3), reorderable and individually hideable, each solid or a linear/radial gradient with any number of stops, edited in a detached picker. Stroke width/join/dash/cap/alignment, corner radius (all four or one at a time), polygon sides, star points and inner ratio, opacity, artboard background.
- **Bulk editing over a multi-selection**: transform, opacity, radius, the whole fill and stroke list, and Group Colors.
- **Clipboard**: cut/copy/paste and duplicate (`InsertSubtree`). Alt-drag drags out a copy.
- **Layers panel**: drag reorder/reparent carrying the whole selection, double-click rename, `Shift` range and `Ctrl` toggle selection, visibility, lock, name filter, collapse/expand all.
- **Keyboard**: nudge, and the `Ctrl`+arrow size step.
- **Navigation and files**: pan/zoom including zoom-to-fit; save, save-as and open, with unsaved-change protection and a save-state readout.

### The wheel

`canvas::wheel_input` handles the wheel, and only while the canvas is hovered:

| Input | Effect |
| --- | --- |
| scroll | pan vertically |
| `Shift`+scroll | pan horizontally |
| `Ctrl`/`Cmd`+scroll | zoom at the cursor |
| `Alt`+scroll | zoom at the cursor (Adobe's spelling of the gesture) |

It reads raw `MouseWheel` events, not egui's smoothed deltas (§15 D26).

### The right button cancels a gesture

A right-button press abandons whatever gesture is in flight: a move, resize or rotate, a shape
being drawn, a marquee, a layers-row drag, an inspector scrub, or a component carried out of the
insert picker. The preview is dropped and nothing is committed, so history never records the
gesture. This follows the CAD/3D convention. The alternative is finishing an edit you know is
wrong and then undoing it, which leaves a step in history.

- The cancel shares a rung with Escape. `OndinApp::cancel_gesture` serves both, so the two cannot drift.
- When nothing is in flight it is a strict no-op, so right-click-to-remove-a-gradient-stop still works.
- Cancelling cannot end the press (§15 D31).
- "Is anything in flight" cannot be asked of egui's `dragged_id`: a fast right-click has already cleared it by then (§15 D168). `cancel_gesture` asks `InFlight::any`, which is exhaustive by construction.
- A cancel must shut every window the gesture opened, not only drop its preview. The two-digit opacity window is one such window: its clock is a separate field from its preview, and `cancel_gesture` clears `opacity_entry`.

**The cancel happens on the press and the menu opens on the release** (`context-menus.md` §0 R1).
Only one of the two pays out:

- A press that cancels sets `gesture_cancelled`, and `open_context_menu` refuses while that flag is up.
- `canvas::canvas_context_menu` checks the flag at the top, before any branch sets the selection. Otherwise a cancelling right-click would also select the layer under the pointer.
- `edit_valve` spends its engagement transition on the frame it bails, so a cancelled edit cannot commit itself one frame later.
- `OndinApp::secondary_press` records where the press happened. egui delivers `secondary_clicked` on the release, by which time the press position is gone. Opening the menu at the press means a few pixels of jitter cannot shift it.

Neither half could move. With the cancel on the release, the gesture would run on underneath the
menu. With the menu on the press, the release would land inside the menu and pick a row.

### Context menus (`menu.rs`)

`context-menus.md` is the spec for every row. This section covers the machinery.

- **One registry, six targets.** `menu::Item` is one enum of verbs, and `Item::spec()` gives each verb its label, glyph, accelerator and `Group`. `menu::build` sorts the requested rows into the canonical group order, so a menu function chooses *which* rows appear and never which group they go in. Within a group, emitted order is kept, so a `Group` buys a separator, not a position.
- **Promotion.** `Row::promote()` is the one exception to the grouping. A verb that a kind lifts into its head is moved, not duplicated: *Ungroup* on a group, *Flatten* on a boolean.
- **Return shape.** `build` returns `Vec<Vec<Row>>`, so an empty group takes its separator with it by construction.
- **Rows map to actions.** Most rows hand straight to an `input::Action`, so a row and its chord cannot mean different things. A few rows have no chord, either because a keyboard cannot express them (*Paste here*, *Add point here*, key-layer and base-operand designations, the guide verbs) or because `shortcuts.md` does not bind them. Those call their canvas verb directly.
- **Exhaustive matches.** `Item::spec`'s accelerator match and `menu::ViewState::get` are exhaustive. A chord comes from `ViewSwitch::accel()`, and `Row::checked` takes an `Option` so a caller with no answer can say so.
- `every_view_switch_is_on_exactly_one_menu` checks membership, not cardinality: each switch must be on exactly one of the hand-written `VIEW_MENU` and `SNAP_MENU` arrays.
- The canvas menu's view rows are derived from `VIEW_MENU` minus `context-menus.md` §6.1's deliberate excludes.

**A region opens only the menu it claimed** (R2). Three responses read `secondary_clicked()`: the
canvas, a layers row and the layers panel background. `picker::stop_bar` keeps its own right-click
for removing a gradient stop. The canvas resolves five of the six targets itself, in this order:

1. a live text session;
2. a point selection;
3. a guide (`guide_at` answers `None` while guides are locked, so the canvas menu carries *Lock guides* itself);
4. `pick_at_pointer` → `pick_for_click`, the same query a left-click runs, including `Ctrl` deep-select and a frame's name tag;
5. the page.

A hit inside the selection leaves the selection whole, so you can right-click one of five layers
and still reach *Union*. A hit outside replaces the selection. Empty canvas does not deselect,
because *Paste here* is why that menu exists.

**Opening the next floating thing closes the last** (R4). `OndinApp::context_menu` is the one slot.
Opening it clears `open_menu`, `picker`, `stroke_menu`, `type_menu`, `effect_menu`, `export_menu` and
`export_row_open`. The image-editing card is deliberately left out: it is a mode, not a popover, and
clearing it would leave the mode half-entered. Three popover lists exist:

| List | Members |
| --- | --- |
| `a_popover_is_open` (the lane test) | six popovers, including the image card |
| R4's clear | five of those six, plus the dropdown slot and the picker |
| `a_popover_owns_escape` | the lane's six minus the image card, because `Escape` is how image editing is left |

Two consequences of R4:

- `ContextMenu::just_opened` makes the opening frame skip the dismissal test. Otherwise the release that opened the menu would read as a click that dismisses it.
- A click that dismisses a menu is spent on the dismissal. While a menu is open, the canvas skips its mode input and a layers row skips its click. The library screen's ⋮ popup and project-filter dropdown follow the same rule (§9.5).

**Escape and menus** (R3):

- An open menu is the first thing Escape unwinds, and the press is consumed. `<OndinApp as eframe::App>::ui` handles the key and returns without calling `escape`, and `input::resolve` does not run while a menu is open.
- In present mode a context menu can be open. The first Escape closes the menu and the second leaves present mode.

**A press that dismisses a floating thing is spent on it.** The mechanism differs by door:

- the context menu's arm at the top of the frame returns;
- the top-bar dropdowns have a `dispatch` arm on `Action::Escape if open_menu != TopMenu::None`;
- the three modals are gated out of `input::resolve` and answer `should_close()` at the bottom of the frame;
- the colour picker is `escape`'s second rung, inside an `else if` chain that pays out one rung;
- the five inspector and Export popovers are covered by an empty-bodied arm in `update`, gated by `a_popover_owns_escape`. The popovers still close themselves later in the frame.

That gate is for `Escape` only, not the whole keyboard. A modal hides the document, but a popover
sits over a canvas the user is still looking at. It also counts only popovers that are actually
drawn: each open popover's handler sets `OndinApp::popover_heard`, and the arm reads it at the top
of the next frame. If nothing heard the press, the flag is cleared and the press pays out its rung.

### Menu chrome

- **One row type.** `ui::MenuRow` serves the top-bar dropdowns and the context menus: an accelerator column, a checked state, a destructive tint, and a dimmed-but-hoverable state. `menu_item` is the same row with the options off. Both allocate `ui::MENU_ITEM_H` (24).
- **Dimmed rows** sense `hover()`, not `click()`, so the tooltip saying why can still show.
- **Separators.** `ui::menu_sep` is a nominal 1pt `text_a(20)` rule with `SEP_AIR` (3) either side, for a total of `ui::MENU_SEP_H` (7). The rule width is `round(ppp).max(1.0)` device pixels, so it is 1.333pt at 150% scaling, chosen for crispness. `menu_sep` allocates the constant and `menu::height` sums it.
- **Placement.** `ui::menu_place` flips, then rounds, then clamps. The clamp comes last because rounding a clamped value can push it out by a fraction of a point. It measures against `content_rect()`. Height is computed from the row and separator constants, not measured, because placement is needed before the `Area` is shown.
- **Anchored lists.** `ui::dropdown_place` is for a list anchored on a field, such as the `--` list. It puts the list's left edge on the field's (or its right edge, if that would overflow), places it above the field when there is no room below, then lets `menu_place` clamp.
- **Height includes the border.** A menu's computed height includes `ui::MENU_BORDER` on every side, because a `Frame` stroke grows its outer rect.
- **Width is the painted width.** `ui::menu_inner_w(card, pad)` subtracts padding and border before `set_width`. This applies to the context menu (210), the top-bar menus (150 and 160, wider where a label needs it) and `app::POPOVER_W` (272).
- **Red.** `theme::color::DANGER` (`#e0736b`) appears only on *Delete*, and is distinct from amber `WARN`. The save dot uses `color::UNSAVED`, the same value under its own name.

### Paste at a point

`OndinApp::paste_at` centres the pasted layers on a world point, which is what *Paste here* does.
It measures from the source layers' bounds. Cut-then-paste-here therefore falls back to the fixed
offset, since the source is gone. It asks the same three questions as `paste`, in the same order:
a picture, then the app's own subtrees, then text.

### Selection chrome

**A single layer shows its own box**: an oriented quad with hollow handles on the shape's own
corners. Resizing a rotated layer changes its width and height and never leans it. A lean is a
different verb, asked for with `Ctrl` on a side.

- Sides resize one axis and draw nothing. Rotation is armed in a band just outside each corner.
- The `W × H` badge quotes that box: the inspector's numbers for one layer, and the oriented box measured in its own axes for several. It hangs off the edge whose outward normal points most steeply down the screen, laid along that edge. On a rotated rectangle that edge is never more than 45° off horizontal, so the text stays readable.
- **Hover** outlines exactly what a click would select, `Ctrl` and `Ctrl`+`Alt` included. A container lends the outline to every leaf it holds. The ring is 3px, against the selection box's 1.2px.
- `canvas::stroke_outlines` takes a `view` (`canvas::outline_view`, the viewport inflated by the ring width) and rejects subtrees whose bounds miss it. The reject copies `scene.rs`'s `bounds_cover_subtree`: a group or a clipping frame can be rejected whole, but a non-clipping frame cannot. Visibility is read through `display_node`, so a layers-panel eye preview counts.
- **Inspector edits hide the chrome.** While an inspector value is being edited, the box, handles, size badge, pivot, hover outline and key-layer trace are all hidden. `canvas::chrome_hidden` holds that list, and `app::ChromeHold` is one term in it, beside a pen gesture and point editing.
  - A scrub's 1.5s count starts when the button comes up. Each keystroke restarts it.
  - The chrome comes back on the timeout, when the pointer moves over the canvas, or at once on `cancel_gesture`.
  - What counts is decided by the transaction: an edit arms the hide if it `changes_ink` (§5.7). Discrete edits such as a 90° turn count. The proportion lock and opening the per-corner dialog do not.
  - Box trim moves the box without moving ink, so it commits through `commit_chrome_edit` and keeps its chrome.
  - `app::edit_note` also requires that the control actually did something.
- Every inspector control commits through `commit_edit` or `edit_valve` (§9.3). `only_the_layers_panel_commits_without_the_committer` scans `panels/` recursively and requires that `self.session.commit` appears only in `layers.rs`. The identity row's *Group*/*Ungroup* buttons are the exception: they call the keyboard verbs, which are declared in `app.rs`.

**A lone `Line` gets two end handles instead of a box.** `canvas::lone_line` is `Some` only when the
selection is exactly one `Line`. `canvas::line_ends` places the two ends.

- Dragging an end (`Drag::LineEnd { id, which }`, `tools::move_line_end`) moves that point.
- Shift constrains the line to eighths of a turn about the other end, through `axial_corner`. Snapping applies to the unshifted case, on both axes.
- `canvas::selection_handles` returns `None` on `lone_line`, which suppresses corners, side bands, skew, the rotate ring and their cursors. It refuses on `lone_line`, not on `line_ends`, so an unresolvable transform cannot fall through to the box.
- `draw_size_badge` returns early for a lone line.
- The panel shows X/Y as the start point, rotation as the start→end angle, and `L` (length) in W's slot.
- The two ends behave asymmetrically, because of the model (§15 D99).

**A selected text node gets two boxes.** New text uses `BoxTrim::CapToBaseline` (§5.4). Every gesture
and number reads the trimmed box. `canvas::draw_text_line_boxes` also outlines the untrimmed box:
`SELECT_DIM`, 1px, dashed 3/3, drawn under the trimmed box. Nothing is drawn when
`untrimmed_bounds() == bounds()`. Cap-to-baseline leaves descenders outside the box (§15 D199).

**Baselines are drawn**, one hairline per line (`canvas::draw_text_baselines`): `SELECT_DIM`, 1px,
solid, the width of the trimmed box. They are drawn after the line box and before the outline. The
*Snap to baselines* switch controls both drawing and snapping. A rotated text node shows its
baselines but cannot be snapped to them.

### Multi-selection: one box

Two or more layers get one box, and dragging it transforms all of them.

- **Shape of the box.** It is a transform plus an extent, like a single layer's (`canvas::selection_frame_box`). The extent is the union of the members' geometry quads, measured in the box's own axes. The transform is `IDENTITY` at rest.
- **Empty members.** A member with no box, such as an empty `Group`, is skipped (`selection_frame_extent`). The box is absent only when no member has one.
- **Session orientation.** The orientation lives on `Selection`, not in the document. It is composed on each commit and dropped when the selection changes. A set has no shared document frame, but its box should not snap upright the instant a turn ends.
- **Quads, not ink.** The extent is measured from geometry quads, not world ink bounds, so it does not creep under rotation. As a result it is half a stroke tighter than ink on stroked members.
- **Resize** runs on the oriented box. `canvas::selection_resize_box` is the committed-tree twin of `selection_frame_box`. `tools::resize_selection` takes `tools::SelectionBox { frame, extent }` and works in frame space (`frame · S · frame⁻¹ · world`). `canvas::resize_axes` maps the handle direction through the same frame.
- **Typed doors** keep the upright union: `OndinApp::size_step`, `inspector::scale_tx` and the multi W/H fields. The panel reports the upright box for a set.
- **Target.** `preview::Target { Layer(NodeId), Selection }` travels on `Drag::Resize`, `Drag::Rotate` and `Drag::Skew`, and on `Handles.target`. `resize_tx`, `rotate_tx`, `skew_tx`, `keep_ratio`, `snapped_resize` and `resize_axes` all dispatch on it, so a drag cannot change its target midway.
- **Box style.** The box is drawn at 1px `SELECT_DIM`, against the 1.2px `SELECT` of each member's own outline. When every member is a main or instance, the box is `COMPONENT_DIM` and the handles `COMPONENT` (§5.3d). A single layer's box is not drawn twice.
- **Committed extents.** `Handles` does not store the box extent. Everything recomputes it from the committed tree, because scaling against a previewed box compounds every frame.
- **Snapping.** `snapped_resize` excludes the whole selection from the snap targets.
- **Rotation** turns the whole arrangement about the union centre (`tools::rotate_selection`, which calls `rotate_node` per member about that one point). §15 D50 covers that pivot, how the proportion lock reads over a set, and where `build::outermost` applies.

**Resize mechanism.** A transform never carries scale (§5.6), so `tools::resize_selection` works like
`resize_group` with world space in place of a shared parent. Each root is carried through the scale
(`scaled_about`), projected back into its parent, and then split by `split_geometry_scale` into the
transform it keeps and the scale its geometry takes. The shared pieces are:

- `resize_factors`: the factors and the anchor, including the `symmetric` and `keep_ratio` anchor rules;
- `scaled_about`;
- `split_geometry_scale`;
- `scale_geometry`: a leaf's size, a line's end, a path's points, or recursion into a container.

`resize_group`, `scale_subtree` and `resize_geometry` (§5.6) are built from these. An off-axis root
under a non-uniform drag gets a skew so it lands exactly. The drag's options travel together as
`tools::Resize { keep_ratio, symmetric, scaling }`.

**Overflow guards.** Each guard refuses per node: the member is left where it was and the rest move.

- `scaled_about` returns `Option<Affine>`. A member whose product overflows to `inf` is skipped.
- `split_geometry_scale` returns `Option<(Affine, f64, f64)>`. It refuses a finite basis that cannot be written as `orientation × scale`, such as a rank-1 matrix. The predicate is `build::Basis::exact` (§5.7a). All six call sites refuse per node, because a missing edit is recoverable and a wrong one is not.

### The Scale tool

`Tool::Scale` (`K`, glyph `resize`, rail slot three next to Select and Hand) is Select with a
different verb on the handles.

- **Shared gates.** Click, marquee, move, rotate ring and pivot all behave as under Select. `Tool::selects()` is the shared gate, used by the click, drag start, hover outline, pivot marker and `rulers.rs` guide lines. The cursor shares one match arm.
- **Only `Tool::scaling()` differs.** `canvas::resize_opts` passes it into `tools::Resize`.
- **Proportional by default.** `canvas::keep_ratio` treats the tool as locked, so Shift releases the lock. Released, scalar properties take the geometric mean of the two factors.
- **Cursor.** The rest cursor is `Bitmap::Scale`, drawn from the same Phosphor glyph as the rail button. Over a handle, the resize arrows win.
- **Numeric door.** The Transform card's Scale % row appears only while the tool is held (see `inspector.md`).

**Adding a `Tool` variant is found by the compiler; adding a `NodeKind` is not.** Every predicate
over `Tool` is one of three safe shapes:

- an exhaustive `match` (the cursor, `create_tx`);
- a `matches!` allow-list (`drags_a_box`, `selects`);
- a wildcard whose answer is the right default (`scaling` → `Scaling::Geometry`).

`NodeKind`'s `matches!` lists answer a missing kind with a confident "no" (§15 D87–D88).

**Arming a create tool clears the selection** (`OndinApp::choose_tool`). The set is `drags_a_box`:
Frame, Rect, Ellipse, Polygon, Star, Line, Text and Image. These are left out:

- Select and Scale, which operate on the selection;
- Hand, so a pan does not lose the selection;
- Node and ImageEdit, which are entered *from* a selected layer;
- Pen, which places anchors and never drags a box.

`place_image_action` clears the selection too.

**A tool switch abandons the drag in flight.** `choose_tool` clears `self.drag` and `self.alt_clone`,
and calls `clear_gesture_preview`. It does not call `clear_preview`, which would discard a live text
session's content. It does not route through `cancel_gesture`, which needs an `&egui::Context` and
covers states a tool switch cannot be in. Unlike `cancel_gesture`'s `InFlight::any`, its three
fields are a hand-kept list.

### Skew

`Ctrl`-dragging a side skews: the dragged side follows the pointer along its own length, and the
opposite side stays put (`canvas::skew_tx` → `tools::skew_to_handle` / `tools::skew_selection`).

- **Sides only.** Corners keep resize, with the rotate ring beyond them.
- **Modifier.** `Handles::armed` takes the modifier and re-reads the side bands, so one statement of handle priority (D14) decides the cursor, the gesture and the hover outline. Ctrl is read at the press.
- **Measured from the press.** `Drag::Skew` carries the press point, so the grabbed point follows the pointer and a second skew starts from zero.
- **No pointer snapping**, since the pointer stands for an angle. Shift snaps to 15°.
- **Clamped** to ±89° (`tools::MAX_SKEW`). A wild drag pins at the limit and does not snap back.
- **Single node, shape or group:** the lean is in the node's own frame, on its local box, with no recursion.
- **Multi-selection:** a world-axis lean about the shared upright union. The selection frame is drawn from the same shear (`tools::side_shear`).
- **Cursor.** It points along the side (`Handles::screen_tangent`, the outward normal turned a quarter turn in the box's own frame). §15 D52 has the design.

### Resize snapping and Alt

- **Snapped axes.** A resize snaps only the axes its handle travels along. `canvas::resize_axes` maps the handle's box-local outward direction through the box's world transform: a corner claims both axes, an upright side one, and a rotated side both. The point box uses the same function.
- **Alt.** Alt keeps the box centre fixed. `tools::resized_box` and `resize_group` take a `symmetric` flag next to `keep_ratio`. Shift and Alt compose. The centre is a fixed pivot, not the node's transform origin: a fixed pivot and a handle that tracks the pointer cannot both be honoured (§15 D60). So `resized_box` and `resize_factors` take no pivot.
- **Centring under Alt+Shift.** Under `symmetric`, the origin is centred on both axes, including the axis the handle does not own. That matters when `keep_ratio` drives the second axis, as with Alt+Shift on a side handle. Both copies of the rule, `resized_box`'s `edge` and `resize_factors`' `pin`, answer the centre before the anchor.
- **Past the anchor.** Dragging past the anchor flips the shape: the sign of the scale factor is kept in both copies.

### Drop outlines during a move

- **Frame drop.** `canvas::draw_frame_drop_outline` boxes any frame that would adopt the selection on release. It is 3px `color::SELECT`, above the hover outline and below the selection chrome.
  - The decision is `OndinApp::move_destination`, split out of `move_tx`, and both the outline and the commit ask it with the same snapped delta.
  - For an in-flow item it reads the pointer from `Drag::Move`'s anchor, so `finish_drag` restores the drag around the transaction it builds.
  - Only frames are outlined, never the root.
- **Flex landing.** A flex item's move outlines its landing slot, dashed, at 1.5px `color::SELECT` (`canvas::draw_flex_landing`). It reads `RenderOverrides::landings` (§6.2), filtered to the item `flex_reorder_of` names for the snapped delta.
  - No outline is drawn for an item leaving its frame, or for any multi-selection.
  - A grid item's move outlines the target cell.
  - While a grid item moves, its container's track lines are drawn from `build::drop_grid`, the committed grid in its committed place.
  - When the reorder moves the container itself, the landing is composed through the container's previewed world transform.
- **Memoised frames.** The artboard list used by the drop rule is memoised (`OndinApp::with_frames` → `FrameIndex`), keyed on `EditorSession::revision`.
  - `revision` is bumped by `commit_inner`, `undo`, `redo` and `adopt_document`, and nothing else.
  - Nothing may add, remove or move an artboard without one of those.
  - A gesture in flight does not bump it, because the drop rule is defined against committed bounds.

### The transform origin (pivot)

**The toggle.** `OndinApp::show_pivot` is app-wide and sticky: someone rigging pivots is about to
place several. It is a `ui::HeadAction` toggle on the Transform header, dimmed with a tooltip for a
multi-selection. Escape turns it off, after `cancel_gesture` and before the `entered_group` unwind,
without undoing any placed pivot.

**`canvas::draw_pivot` states:**

| State | When | Glyph |
| --- | --- | --- |
| Editable | toggle on, one layer selected | `icon::CROSSHAIR` |
| Read-only | hovered layer with a placed pivot, not selected, not mid-gesture, Select tool | `CROSSHAIR_SIMPLE` (also the toggle's glyph) |
| Absent | otherwise | none |

Both glyphs are `color::SELECT` at `PIVOT_GLYPH_PX` (22), with nothing behind them. The marker must
fit inside `PIVOT_PICK_PX`.

**Dragging the marker:**

- **Nine-point snap.** It snaps to the box's nine points (corners, side midpoints, centre) within 6 screen points. The comparison runs in local space, so the threshold is divided by the zoom and by the node's basis column norms, one per axis: `|col 0|` for x and `|col 1|` for y.
- **Badge.** A badge names the position while the marker is placed.
- **Centre clears.** Snapping to the centre sets the pivot to `None`: the field leaves the file (§5.11), the read-only marker disappears, and `flip` reverts to the union centreline. The rule belongs to `geometry::pivot_placed_at`, which the numeric fields share. A degenerate axis is centred by itself.
- **No overrides.** `Drag::Pivot { id }` has no `RenderOverrides` preview, because the marker is chrome drawn from the pointer. It commits one `SetPivot` on release. `EditorSession::preview_pivot` resolves the pivot against a live gesture's box.
- **Priority.** While the toggle is on, the marker outranks resize handles in `begin_select_drag`.

**The origin fields** (`canvas::origin_fields`). They replace the size badge in its slot while the
toggle is armed over one layer, and share `badge_placement` with it. A shape being drawn takes the
slot ahead of both.

- **Percentages of the layer's own box.** The display is a fraction for every kind. `pivot_for` picks the stored form: a fraction for an authored box, an absolute point for an emergent one. 0/50/100 are the left edge, centre and right edge.
- **Whole percentages.** A dragged marker lands on a whole percentage through the same functions, after the screen-space snap.
- **Unclamped.** A pivot may sit outside its box.
- **Degenerate axes.** An axis with no extent is dimmed with a reason, per axis. A lone line is offered the fields.
- **Both at 50 clears the origin.**
- **Scrubbing.** `EditorSession::pivot_preview` makes the fields scrubbable; the overrides cannot carry a pivot.
- **Widgets.** These are the first live egui widgets on the canvas. `ui::badge_field` is the badge pill with `value_field`'s plumbing (`Scrub`, `expr::eval`, a scrubbable label). The fields stay upright, taking the badge's position but not its angle.
- **Shielding.** They live in their own `egui::Area`, which shields the canvas beneath (`canvas::overlay_claim`). `wheel_input` names the fields' layer through `origin_fields_layer` and takes the wheel back.

**What reads the pivot.** `Handles::pivot` is the node's origin for a single selection and the union
centre for several. Readers:

- `Drag::Rotate`;
- the inspector's skew field, which rebuilds about it;
- the rotation field, which composes about it;
- the quarter turns;
- `build::flip`, but only when a pivot has been placed; otherwise flip uses the union centreline (D55).

The canvas skew and Alt-resize deliberately do not use the pivot, because each already has its own
fixed point (D55, D60).

### `Ctrl`+arrow size step

`Ctrl`+arrows step the selection's box by the nudge step (`input::NudgeStep`): 1, or 10 with
`Shift`. `→` makes it wider, `←` narrower, `↓` taller, `↑` shorter (`shortcuts.md` §11).

- **Delegation.** `OndinApp::size_step` uses `tools::resize_box_to` for one layer, which grows along its own axes, and `tools::resize_selection` over the upright world union for several.
- **Top-left held.** The top-left corner stays fixed, so the step does not `reveal_selection`. One arrow table serves nudge and resize: the arrow names the edge that moves either way. `normal_mode` resolves the pairs once and picks the verb from the modifier.
- **1pt floor** per axis on the ask.
- **Ratio.** With every member proportion-locked, both axes move and the arrow names the driver (`tools::paired_size`).
- **Guides** are inert.
- **Undo.** Bursts coalesce under the undo verb `resize-layers`, separate from nudges.

## Traps

- **Same press, two consumers.** The cancel and the context menu share one press: `gesture_cancelled` must be read at the top of `canvas_context_menu`, before any branch selects.
- **Dropdown keyboard focus.** A dropdown opened by clicking its head leaves `focused() == None`, so `egui_wants_keyboard_input` does not cover it.
- **Popover flags outlive popovers.** Gate on `popover_heard`, not on the flags, or Escape is swallowed behind a popover nobody is drawing.
- **Three popover lists.** They differ deliberately, by the image card. A shared list would have to carry that exception.
- **Inert `TopMenu` row.** `TopMenu` needs its own Escape arm; without one, Escape closes the menu *and* drops the tool.
- **Two copies of one resize rule.** `resized_box` and `resize_factors` are two copies of one rule and have diverged twice. Change both, and test both: the box can be identical while the path differs.
- **Unguarded `draw_drop_indicator`.** It does not check `shown_visible`, so a picture inside a hidden group is still ringed during a file drag.
- **Accelerators by hand.** Nothing makes `ViewSwitch::accel`/`Item::spec` accelerators agree with `input::resolve`; they are kept in step by hand.
- **Clear without `clear_preview`.** `choose_tool` must call `clear_gesture_preview`, not `clear_preview`, or a live text session's content is lost.
- **Predicate negatives.** `drags_a_box` now decides whether arming a tool deletes the selection. When a predicate gains a second consumer, re-check its negative members.
- **Middle-button pan over the origin fields.** It is still swallowed by their `Area`.
- **Menu placement order.** `menu_place` must clamp after rounding; reversing the order cuts off a menu's last column.
