# Tools and canvas interaction

The v1 tool set and how canvas gestures behave: the wheel, cancelling, context menus and their
chrome, selection chrome, multi-selection transforms, the Scale tool, skew, snapping during
resize, drop outlines and the transform origin (pivot).

## 9.4 v1 tools

- **Select** (click, shift-click, marquee, select-all) and **Hand** (pan).
- **Move and rotate** (`SetTransform`): Shift locks a move to one axis and a rotation to 15° steps.
- **Resize** by corners or sides (`SetGeometry`, §5.6): Shift locks the ratio, or releases it on a proportion-locked layer; Alt scales about the box centre. Group resize is recursive; a multi-selection gets one box for resize, rotate and skew.
- **Skew**: `Ctrl`-drag a side; Shift holds 15° steps. **Transform origin**: a draggable pivot marker with two numeric fields. **Scale** (`K`): the same gestures, but stroke widths, radii and type scale too.
- **Shapes**: frame, rect, ellipse, polygon, star, line. **Text**: edit on the canvas with a real caret.
- **Pen** (`Path`), straight and curved. Corner↔smooth conversion over a point selection goes through `tools::set_smoothness`; a smooth pair is exactly mirrored, because that is what `is_smooth` means.
- **Group/ungroup** and **frame selection** (`build::frame`, a new frame sized to the selection's union; a second builder beside `group`, not a flag on it).
- **Align** to each other, to the container (one layer selected), or to a **key layer** (`Alt`+`Shift`+click). **Distribute** (equal gaps, 3+ layers) and a **named gap** (2+ layers, packed outwards from the key).
- **Flip** H/V across the selection's centreline, or about a single layer's pivot where one is placed; **quarter turns** either way. **Z-order** one step or all the way (`Ctrl`+`[`/`]`).
- **Snapping**: alignment, equal-gap (gaps quoted on the canvas), baselines, the pixel grid. **Guides and view**: rulers and guides (`Shift`+`R`), View and Snap menus, focus mode.
- **Eyedropper** (`I`, or a picker's pick button): picks a colour off any pixel in the window, chrome included, onto the selection or into one picker slot (below).
- **Paint editing**: stacked fills and strokes (§6.3), solid or linear/radial gradient, in a detached picker; stroke options, per-corner radius, polygon sides, star points and ratio, opacity, artboard background. Bulk editing covers a multi-selection's transform, opacity, radius, fill and stroke list, Group Colors.
- **Clipboard** (cut/copy/paste, duplicate via `InsertSubtree`; Alt-drag drags out a copy), the **layers panel** (drag reorder/reparent of the whole selection, rename, `Shift`/`Ctrl` selection, visibility, lock, filter), nudge and the `Ctrl`+arrow size step, pan/zoom, and save/open with unsaved-change protection.

### The wheel

`canvas::wheel_input` runs while the canvas is hovered (or the pointer is over the origin fields or
the image-edit *Done* pill, whose `Area`s it takes the wheel back from). It reads raw `MouseWheel`
events, not egui's smoothed deltas (§15 D26).

| Input | Effect |
| --- | --- |
| scroll | pan vertically |
| `Shift`+scroll | pan horizontally |
| `Ctrl`/`Cmd`+scroll, `Alt`+scroll | zoom at the cursor (`Alt` is Adobe's spelling) |

### The right button cancels a gesture

A right-button press abandons whatever gesture is in flight: a move, resize or rotate, a shape being
drawn, a marquee, a layers-row drag, an inspector scrub, a component carried out of the insert
picker. The preview is dropped and nothing is committed, so history never records it (the CAD/3D
convention; the alternative is finishing a wrong edit and undoing it).

- The cancel shares a rung with Escape: `OndinApp::cancel_gesture` serves both. With nothing in flight it is a strict no-op, so right-click-to-remove-a-gradient-stop still works. Cancelling cannot end the press (§15 D31).
- "Is anything in flight" cannot be asked of egui's `dragged_id` (a fast right-click has already cleared it, §15 D168); `cancel_gesture` asks `InFlight::any`, exhaustive by construction.
- A cancel must shut every window the gesture opened: the two-digit opacity window's clock is a separate field, so `cancel_gesture` clears `opacity_entry`.
- An armed eyedropper is a gesture a right-click calls off: on the canvas the press runs `disarm_dropper`, setting `gesture_cancelled` if it disarmed (no menu opens, nothing is selected), then `cancel_gesture` as ever, so anything else in flight goes with the same press. Off the canvas egui never sees the press, and its release disarms (`gate_dropper_input`).

**The cancel happens on the press and the menu opens on the release** (`context-menus.md` §0 R1);
only one of the two pays out (a cancel on the release would let the gesture run on under the menu; a
menu on the press would take the release as a row pick):

- A press that cancels sets `gesture_cancelled`; `open_context_menu` refuses while it is up.
- `canvas::canvas_context_menu` checks the flag at the top, before any branch sets the selection, or a cancelling right-click would also select the layer under the pointer.
- `edit_valve` spends its engagement transition on the frame it bails, so a cancelled edit cannot commit a frame later.
- `OndinApp::secondary_press` records the press position (egui delivers `secondary_clicked` on the release, when it is gone), so jitter cannot shift the menu.

### Context menus (`menu.rs`)

`context-menus.md` is the spec for every row. This section covers the machinery.

- **One registry, six targets.** `menu::Item` is one enum of verbs; `Item::spec()` gives each its label, glyph, accelerator and `Group`. `menu::build` sorts requested rows into the canonical group order, so a menu function chooses *which* rows appear, never which group they go in. Emitted order within a group is kept: a `Group` buys a separator, not a position. `build` returns `Vec<Vec<Row>>`, so an empty group takes its separator with it.
- **Promotion.** `Row::promote()` is the one exception: a verb a kind lifts into its head is moved, not duplicated (*Ungroup* on a group, *Flatten* on a boolean).
- **Rows map to actions.** Most rows hand to an `input::Action`, so a row and its chord cannot mean different things. Rows with no chord (*Paste here*, *Add point here*, key-layer and base-operand designations, guide verbs, or anything `shortcuts.md` does not bind) call their canvas verb directly.
- **Exhaustive matches.** `Item::spec`'s accelerator match and `menu::ViewState::get` are exhaustive. A chord comes from `ViewSwitch::accel()`; `Row::checked` takes an `Option` so a caller with no answer can say so.
- `every_view_switch_is_on_exactly_one_menu` checks membership, not cardinality, across the hand-written `VIEW_MENU` and `SNAP_MENU`. The canvas menu's view rows derive from `VIEW_MENU` minus `context-menus.md` §6.1's excludes.

**A region opens only the menu it claimed** (R2). Three responses read `secondary_clicked()`: the
canvas, a layers row and the layers panel background (`picker::stop_bar` keeps its own right-click
for removing a gradient stop). The canvas resolves five of the six targets, in order:

1. a live text session;
2. a point selection;
3. a guide (`guide_at` answers `None` while guides are locked, so the canvas menu carries *Lock guides* itself);
4. `pick_at_pointer` → `pick_for_click`, the query a left-click runs, including `Ctrl` deep-select and a frame's name tag;
5. the page.

A hit inside the selection leaves it whole (right-click one of five layers and still reach
*Union*); a hit outside replaces it. Empty canvas does not deselect, because *Paste here* is why
that menu exists.

**Opening the next floating thing closes the last** (R4). `OndinApp::context_menu` is the one slot;
opening it clears `open_menu`, `picker`, `stroke_menu`, `type_menu`, `effect_menu`, `export_menu` and
`export_row_open`. The image-editing card is left out: it is a mode, not a popover. Three popover
lists exist:

| List | Members |
| --- | --- |
| `a_popover_is_open` (the lane test) | six popovers, including the image card |
| R4's clear | five of those six, plus the dropdown slot and the picker |
| `a_popover_owns_escape` | the lane's six minus the image card, because `Escape` is how image editing is left |

- `ContextMenu::just_opened` makes the opening frame skip the dismissal test, or the opening release would dismiss it.
- A click that dismisses a menu is spent on the dismissal: the canvas skips its mode input and a layers row its click. The library screen's ⋮ popup and project-filter dropdown follow the rule (§9.5).

**Escape and menus** (R3): an open menu is the first thing Escape unwinds, and the press is consumed.
`<OndinApp as eframe::App>::ui` handles the key and returns without calling `escape`;
`input::resolve` does not run while a menu is open. In focus mode the first Escape closes the menu
and the second leaves focus mode.

**A press that dismisses a floating thing is spent on it.** By door:

- the context menu's arm at the top of the frame returns;
- the top-bar dropdowns have a `dispatch` arm on `Action::Escape if open_menu != TopMenu::None`;
- the three modals are gated out of `input::resolve` and answer `should_close()`;
- the colour picker is a rung of `escape`'s `else if` chain, which pays out one rung (an armed pick button is the rung above it);
- the five inspector and Export popovers: an empty-bodied arm in `update`, gated by `a_popover_owns_escape`; they close themselves later in the frame.

That gate is for `Escape` only, and counts only popovers actually drawn: each open popover's handler
sets `OndinApp::popover_heard`, read by the arm at the top of the next frame; if nothing heard the
press, the flag is cleared and the press pays out its rung.

### Menu chrome

- **One row type.** `ui::MenuRow` serves top-bar dropdowns and context menus (accelerator column, checked state, destructive tint, dimmed-but-hoverable state); `menu_item` is the same row with the options off. Both allocate `ui::MENU_ITEM_H` (24). Dimmed rows sense `hover()`, not `click()`, so the tooltip saying why can show.
- **Separators.** `ui::menu_sep`: a nominal 1pt `text_a(20)` rule with `SEP_AIR` (3) either side, `ui::MENU_SEP_H` (7); the rule is `round(ppp).max(1.0)` device pixels for crispness.
- **Placement.** `ui::menu_place` flips, rounds, then clamps (last, because rounding a clamped value can push it out by a fraction of a point) against `content_rect()`. Height is computed from the row and separator constants, not measured, because placement precedes the `Area`. `ui::dropdown_place` (a list anchored on a field) puts the left edge on the field's (right if that overflows), above the field when there is no room below, then `menu_place` clamps.
- **Border and width.** A computed height includes `ui::MENU_BORDER` on every side (a `Frame` stroke grows its outer rect). `ui::menu_inner_w(card, pad)` subtracts padding and border before `set_width`: context menu 210, top-bar menus 150 and 160, `app::POPOVER_W` 272.
- **Red.** `theme::color::DANGER` (`#e0736b`) appears only on *Delete*, distinct from amber `WARN`. The save dot uses `color::UNSAVED`, the same value under its own name.

### Paste at a point

`OndinApp::paste_at` (*Paste here*) centres the pasted layers on a world point, measuring from the
source layers' bounds; cut-then-paste-here falls back to the fixed offset, the source being gone. It
asks `paste`'s three questions in order: a picture, the app's own subtrees, text.

### Selection chrome

**A single layer shows its own box**: an oriented quad with hollow handles on the shape's own
corners. Resizing a rotated layer changes its width and height and never leans it; a lean is a
different verb (`Ctrl` on a side).

- Sides resize one axis and draw nothing. Rotation is armed in a band just outside each corner.
- The `W × H` badge quotes that box: the inspector's numbers for one layer, the oriented box in its own axes for several. It hangs off the edge whose outward normal points most steeply down the screen, so on a rotated rectangle the text is never more than 45° off horizontal.
- **Hover** outlines exactly what a click would select, `Ctrl` and `Ctrl`+`Alt` included; a container lends the outline to every leaf it holds. The ring is 3px, against the selection box's 1.2px.
- `canvas::stroke_outlines` takes a `view` (`canvas::outline_view`, the viewport inflated by the ring width) and rejects subtrees missing it, copying `scene.rs`'s `bounds_cover_subtree`: a group or clipping frame can be rejected whole, a non-clipping frame cannot. Visibility is read through `display_node`, so a layers-panel eye preview counts.
- **Inspector edits hide the chrome** (box, handles, badge, pivot, hover outline, key-layer trace). `canvas::chrome_hidden` holds the list: `app::ChromeHold` is one term, beside `editing_in_place` (pen, path editing, image editing). A scrub's 1.5s count starts when the button comes up and each keystroke restarts it; the chrome returns on the timeout, on pointer movement over the canvas, or at once on `cancel_gesture`. An edit arms the hide if its transaction `changes_ink` (§5.7): a 90° turn does; the proportion lock and opening the per-corner dialog do not. Box trim moves the box without moving ink, so it commits through `commit_chrome_edit` and keeps its chrome. `app::edit_note` also requires that the control did something.
- Every inspector control commits through `commit_edit` or `edit_valve` (§9.3). `only_the_layers_panel_commits_without_the_committer` requires `self.session.commit` only in `panels/layers.rs`; the identity row's *Group*/*Ungroup* call the keyboard verbs declared in `app.rs`.

**A lone `Line` gets two end handles instead of a box.** `canvas::lone_line` is `Some` only when the
selection is exactly one `Line`; `canvas::line_ends` places the ends.

- Dragging an end (`Drag::LineEnd { id, which }`, `tools::move_line_end`) moves that point. Shift constrains to eighths of a turn about the other end (`axial_corner`); snapping applies to the unshifted case, on both axes. The two ends behave asymmetrically, because of the model (§15 D99).
- `canvas::selection_handles` returns `None` on `lone_line` (not on `line_ends`, so an unresolvable transform cannot fall through to the box), suppressing corners, side bands, skew, the rotate ring and their cursors. `draw_size_badge` returns early.
- The panel shows X/Y as the start point, rotation as the start→end angle, and `L` (length) in W's slot.

**A selected text node gets two boxes.** New text uses `BoxTrim::CapToBaseline` (§5.4); every gesture
and number reads the trimmed box. `canvas::draw_text_line_boxes` also outlines the untrimmed box
(`SELECT_DIM`, 1px, dashed 3/3, under the trimmed box; nothing when `untrimmed_bounds() ==
bounds()`), since cap-to-baseline leaves descenders outside (§15 D199).

**Baselines are drawn**, one hairline per line (`canvas::draw_text_baselines`): `SELECT_DIM`, 1px,
solid, the trimmed box's width, after the line box and before the outline. The *Snap to baselines*
switch controls drawing and snapping. A rotated text node shows baselines but cannot snap to them.

### Multi-selection: one box

Two or more layers get one box; dragging it transforms all of them.

- **Shape.** A transform (`IDENTITY` at rest) plus an extent (`canvas::selection_frame_box`): the union of the members' geometry quads in the box's own axes, so it does not creep under rotation and on stroked members is half a stroke tighter than ink. A member with no box (an empty `Group`) is skipped (`selection_frame_extent`).
- **Session orientation** lives on `Selection`, not in the document: composed on each commit, dropped when the selection changes, so the box does not snap upright the instant a turn ends.
- **Resize** runs on the oriented box. `canvas::selection_resize_box` is the committed-tree twin of `selection_frame_box`; `tools::resize_selection` takes `tools::SelectionBox { frame, extent }` and works in frame space (`frame · S · frame⁻¹ · world`); `canvas::resize_axes` maps the handle through the same frame. `Handles` stores no extent; everything recomputes it from the committed tree, because scaling against a previewed box compounds every frame. `snapped_resize` excludes the whole selection from the snap targets.
- **Typed doors** (`OndinApp::size_step`, `inspector::scale_tx`, the multi W/H fields) keep the upright union, which the panel reports.
- **Target.** `preview::Target { Layer(NodeId), Selection }` travels on `Drag::Resize`, `Drag::Rotate`, `Drag::Skew` and `Handles.target`; `resize_tx`, `rotate_tx`, `skew_tx`, `keep_ratio`, `snapped_resize` and `resize_axes` dispatch on it, so a drag cannot change target midway.
- **Style.** 1px `SELECT_DIM`, against each member's 1.2px `SELECT` outline; `COMPONENT_DIM` box and `COMPONENT` handles when every member is a main or instance (§5.3d).
- **Rotation** turns the arrangement about the union centre (`tools::rotate_selection`, `rotate_node` per member). §15 D50 covers the pivot, the proportion lock over a set, and where `build::outermost` applies.

**Resize mechanism.** A transform never carries scale (§5.6), so `tools::resize_selection` works like
`resize_group` with world space for the shared parent: each root is carried through the scale
(`scaled_about`), projected into its parent, then split by `split_geometry_scale` into the transform
it keeps and the scale its geometry takes. Shared pieces: `resize_factors` (factors and anchor,
including the `symmetric` and `keep_ratio` rules) and `scale_geometry` (a leaf's size, a line's end,
a path's points, or recursion into a container); `resize_group`, `scale_subtree` and
`resize_geometry` (§5.6) are built from these. An off-axis root under a non-uniform drag gets a skew
so it lands exactly. Options travel as `tools::Resize { keep_ratio, symmetric, scaling }`.

**Overflow guards** refuse per node: the member stays put and the rest move.

- `scaled_about` returns `Option<Affine>`; a product that overflows to `inf` is skipped.
- `split_geometry_scale` returns `Option<(Affine, f64, f64)>`, refusing a finite basis that cannot be written as `orientation × scale`, such as a rank-1 matrix (`build::Basis::exact`, §5.7a). All six call sites refuse per node: a missing edit is recoverable, a wrong one is not.

### The Scale tool

`Tool::Scale` (`K`, glyph `resize`, rail slot three beside Select and Hand) is Select with a
different verb on the handles.

- **Shared gates.** Click, marquee, move, rotate ring and pivot behave as under Select. `Tool::selects()` is the shared gate (click, drag start, hover outline, pivot marker, `rulers.rs` guide lines); the cursor shares one match arm.
- Only `Tool::scaling()` differs; `canvas::resize_opts` passes it into `tools::Resize`.
- **Proportional by default.** `canvas::keep_ratio` treats the tool as locked, so Shift releases it. Released, scalar properties take the geometric mean of the two factors.
- **Cursor.** `Bitmap::Scale`, from the rail button's Phosphor glyph; over a handle, the resize arrows win.
- **Numeric door.** The Transform card's Scale % row appears only while the tool is held (`inspector.md`).

**Adding a `Tool` variant is found by the compiler; adding a `NodeKind` is not.** Every predicate
over `Tool` is an exhaustive `match` (the cursor, `create_tx`), a `matches!` allow-list
(`drags_a_box`, `selects`), or a wildcard with the right default (`scaling` → `Scaling::Geometry`).
`NodeKind`'s `matches!` lists answer a missing kind with a confident "no" (§15 D87–D88).

**Arming a create tool clears the selection** (`OndinApp::choose_tool`). The set is `drags_a_box`:
Frame, Rect, Ellipse, Polygon, Star, Line, Text, Image. Left out: Select and Scale (operate on the
selection), Hand (a pan keeps it), Node and ImageEdit (entered *from* a selected layer), Pen (places
anchors, never drags a box), Eyedropper (picks onto the selection). `place_image_action` clears it too.

**A tool switch abandons the drag in flight.** `choose_tool` clears `self.drag` and `self.alt_clone`
and calls `clear_gesture_preview`. It does not call `clear_preview` (which would discard a live text
session's content) nor `cancel_gesture` (which needs an `&egui::Context` and covers states a tool
switch cannot be in). Unlike `cancel_gesture`'s `InFlight::any`, its three fields are a hand-kept list.

### The eyedropper (`eyedropper.rs`)

**It samples what is seen**, anywhere in the window, from two sources:

- **Over the canvas** (where egui sees the pointer): the rendered canvas texture's pixel under the
  hotspot, composited over the canvas ground and opaque, so a translucent layer gives the blended
  colour. egui paints the overlays (selection, handles, guides, rulers, pixel grid) over that
  texture, so they are never sampled. Read live, so a pan or zoom made while armed is followed.
- **Everywhere else**: the frozen frame (`OndinApp::dropper_frozen: eyedropper::Frozen`), a capture
  of the whole window, so the chrome's own colours are pickable. A live capture cannot work: the
  loupe sits under the pointer, so it would always read the loupe.

Both hold sRGB bytes: the texture is the document's space (invariant 7), and the window surface
(Rgba8Unorm or Bgra8Unorm) holds egui's gamma-space bytes. Nothing is converted.

**Two doors**, and `OndinApp::dropper_armed` says which is armed; a picker's pick button outranks
the rail tool, as the narrower ask.

| Door | Arms | Writes | Afterwards |
| --- | --- | --- | --- |
| Rail tool `Tool::Eyedropper` (`I`) | remembers the tool it was armed from (`dropper_return`); keeps the selection; disarms a picker's pick button | `ondin_core::set_top_paint_all`: every paint target's top **visible** fill (last in the list, which is paint order), or a new fill where none is visible; `Alt` the top stroke, adding a default one. Through `edited_brush`, so a binding survives an unchanged colour; opacity kept (`paint::alpha_of`, so a gradient's or picture's carries to the solid). Nothing selected: the hex goes to the clipboard and the status reads *Copied 6D8CD9 · nothing selected* | one shot: hands back to `dropper_return`; `Shift` keeps it armed |
| A picker's pick button (`Picker::picking`, at the start of `hex_row`) | the button; a picking picker takes no click, so the button cannot disarm it | that picker's slot only, through `write_slot_rgb`, the typed colour's door (§9.3) | picker stays open, disarmed; `Shift` keeps it armed |

- **One click, one transaction, one undo step.** A click never changes the selection.
- **No canvas preview.** Hovering writes nothing and installs no `RenderOverrides`: a preview would change the pixels the next sample reads. From a picker, the hovered colour (canvas sample or frozen pixel) shows in its plane, hue and value row (`Picker::preview`): a dim *was XXXXXX* beside a hex, or the channel fields tinted accent with the old colour as a chip in the format chooser. That preview is not in the frozen frame: picking over the picker reads it as captured.
- **The readback** (`CanvasRenderer::request_sample`): after the render, the `LOUPE_SIDE`² (11×11) device-pixel region round the hotspot is copied out of the canvas texture into a mapped buffer, without waiting, and collected a frame later. A `SampleKey` (hotspot, session revision, camera centre and zoom, canvas pixel size, `pick`) that matches the last sample is not read again, which keeps a still pointer from repainting forever. Headless, `cpu_region` draws the same region with `vello_cpu` and it lands at once; the tests use that arm.
- **A pick always reads afresh.** Uncommitted changes (typing in a text session, a preview, a decode landing, the previewed ground) move pixels without moving the revision, so a key can match pixels that have changed. Hover keys carry `pick` 0; each click mints its own number (`OndinApp::dropper_picks`, in `dropper_input`), so its key matches no earlier sample and the canvas is read again. A stale loupe is only a look; a stale pick is a wrong write. The accepted cost: under a still pointer the hover loupe can stay stale until the pointer moves. Pinned by `a_pick_reads_afresh_when_pixels_moved_under_a_still_pointer`.
- **Pick timing.** A click records a `PendingPick` whose `at` is a `PickAt`: `Canvas(key)` on the canvas, `Window(pos)` elsewhere. `OndinApp::dropper_frame` (run in `draw_canvas` after the render) lands a canvas pick when a sample with that key arrives, a window pick once the frozen frame is there. A canvas pick deferred behind an in-flight hover readback is copied off a later frame's render, so a pick whose camera or canvas size no longer matches is re-aimed at the pixel now showing the world point it was clicked on, or dropped when that point has left the canvas (`OndinApp::reaim_pick`).
- **The frozen frame** (`Frozen`: `None` → `Asked(pass, screen)` → `Ready(image, screen)`). Armed and `None`, `dropper_frame` sends one `ViewportCommand::Screenshot` tagged `FrozenShot(pass)`, and that frame draws no loupe, because eframe captures the frame the command is sent in. `gate_dropper_input` takes the `Event::Screenshot` only when its tag matches the pending ask, so a capture from an earlier arming is ignored. While `Asked`, `dropper_frame` keeps asking for frames for 30 passes, not forever: a surface format egui cannot capture (only Rgba8Unorm and Bgra8Unorm are) never answers. It is asked again after a pick that leaves the dropper armed (`land_pick` resets it, since the panels now show the write), and when the window's size in points or its pixels-per-point differ from the frozen frame's. Disarming drops it. `window_region` reads the `LOUPE_SIDE`² device-pixel region round `floor(p * ppp)`.
- **Input.** In `normal_mode_input` the dropper takes every press an armed pan does not (Space, Hand and middle-drag still pan), ahead of guides, handles and tools: they are chrome its sample never sees. It also routes in `TextInsert`, so a picker opened over a text session can pick. While a dropper is armed the canvas does not answer as Select (`OndinApp::selecting`): a picker's pick button leaves the tool on Select, and without this the hover outline, spacing bands and guide grabs kept answering.
- **The rest of the window is inert while armed** (`OndinApp::gate_dropper_input`, from `raw_input_hook`), which is what keeps the frozen frame true. egui gets the pointer only over the canvas; the picking picker too is pickable, not clickable. Elsewhere egui is told the pointer is gone and gets no press or wheel, so no panel hovers, takes a click or scrolls; `dropper_pointer` keeps the position for the loupe. A press there is the dropper's (`OndinApp::dropper_press`) and its release acts: primary records a `PickAt::Window` pick at the press position (`Alt` stroke, `Shift` stay), secondary disarms. A press egui already has finishes untouched, so a pan dragged off the canvas still ends; disarming hands the pointer back where it is. The canvas is told apart from the panels by `canvas_rect`, since all of them share egui's background layer; any area above that layer is chrome.
- **Ways out.** `Escape` is one rung (`disarm_dropper`, `shortcuts.md` §11): it disarms a picker's button and leaves the picker open, or hands the rail tool back. A right-click anywhere disarms and spends the click (above). A second `I` is `choose_tool`'s toggle arm. Leaving the editor for the library disarms (`go_to_dashboard`), since the gate runs on every screen and the library has neither a canvas nor the escape ladder. The picker's own pick button is not one: a picking picker takes no click.
- **The loupe is the cursor** everywhere (`CursorIcon::None`): it draws the region at 8pt a pixel, centred on the hotspot, the centre pixel ringed, with a hex chip below (above near the window's bottom). Its edge is two rims, dark outside light, under a soft shadow, so one of the pair contrasts with any ground. Off the canvas, until the frozen frame lands, the eyedropper bitmap (`cursor::Bitmap::Eyedropper`, the rail glyph mirrored to aim from its tip like the pen's) stands in, with no chip.
- **On the canvas the picker's preview is the latest sample**, not the one for the pixel under the pointer: each pixel of movement starts a readback that lands a frame later, and waiting for the matching one put the slot's own colour back for that frame.

### Skew

`Ctrl`-dragging a side skews: the dragged side follows the pointer along its own length, the
opposite side stays put (`canvas::skew_tx` → `tools::skew_to_handle` / `tools::skew_selection`).

- **Sides only.** Corners keep resize, with the rotate ring beyond.
- **Modifier.** `Handles::armed` takes a `skewing` flag and re-reads the side bands, so one statement of handle priority (D14) decides cursor, gesture and hover outline. Ctrl is read at the press.
- **Measured from the press.** `Drag::Skew` carries the press point, so the grabbed point follows the pointer and a second skew starts from zero.
- **No pointer snapping** (the pointer stands for an angle); Shift snaps to 15°. **Clamped** to ±89° (`tools::MAX_SKEW`); a wild drag pins at the limit.
- **Single node, shape or group:** the lean is in the node's own frame on its local box, no recursion. **Multi-selection:** a world-axis lean about the shared upright union; the selection frame is drawn from the same shear (`tools::side_shear`).
- **Cursor** points along the side (`Handles::screen_tangent`, the outward normal turned a quarter turn in the box's frame). §15 D52.

### Resize snapping and Alt

- **Snapped axes.** A resize snaps only the axes its handle travels along. `canvas::resize_axes` maps the handle's box-local outward direction through the box's world transform: a corner claims both axes, an upright side one, a rotated side both. The point box uses the same function.
- **Alt** keeps the box centre fixed: `tools::resized_box` and `resize_group` take `symmetric` next to `keep_ratio`; Shift and Alt compose. The centre is a fixed pivot, not the node's transform origin: a fixed pivot and a handle tracking the pointer cannot both be honoured (§15 D60), so `resized_box` and `resize_factors` take no pivot.
- **Alt+Shift.** Under `symmetric` the origin is centred on both axes, including the one the handle does not own (matters when `keep_ratio` drives the second axis, as with a side handle). Both copies of the rule, `resized_box`'s `edge` and `resize_factors`' `pin`, answer the centre before the anchor.
- **Past the anchor** the shape flips: both copies keep the sign of the scale factor.

### Drop outlines during a move

- **Frame drop.** `canvas::draw_frame_drop_outline` boxes any frame (never the root) that would adopt the selection on release: 3px `color::SELECT`, above the hover outline, below the selection chrome. The decision is `OndinApp::move_destination` (split out of `move_tx`), asked by outline and commit with the same snapped delta; for an in-flow item it reads the pointer from `Drag::Move`'s anchor, so `finish_drag` restores the drag around the transaction it builds.
- **Flex landing.** A flex item's move outlines its landing slot, dashed 1.5px `color::SELECT` (`canvas::draw_flex_landing`), from `RenderOverrides::landings` (§6.2) filtered to the item `flex_reorder_of` names. None for an item leaving its frame or a multi-selection. A grid item's move outlines the target cell and draws its container's track lines from `build::drop_grid`. When the reorder moves the container itself, the landing is composed through the container's previewed world transform.
- **Memoised frames.** The artboard list the drop rule uses is memoised (`OndinApp::with_frames` → `FrameIndex`), keyed on `EditorSession::revision`, which is bumped by `commit_inner`, `undo`, `redo` and `adopt_document` and nothing else: nothing may add, remove or move an artboard without one of those. A gesture in flight does not bump it, because the drop rule is defined against committed bounds.

### The transform origin (pivot)

**The toggle.** `OndinApp::show_pivot` is app-wide and sticky (someone rigging pivots will place
several): a `ui::HeadAction` toggle on the Transform header, dimmed with a tooltip for a
multi-selection. Escape turns it off, after `cancel_gesture` and before the `entered_group` unwind,
without undoing placed pivots.

**`canvas::draw_pivot` states:**

| State | When | Glyph |
| --- | --- | --- |
| Editable | toggle on, one layer selected | `icon::CROSSHAIR` |
| Read-only | hovered layer with a placed pivot, not selected, not mid-gesture, Select tool | `CROSSHAIR_SIMPLE` (also the toggle's glyph) |
| Absent | otherwise | none |

Both glyphs are `color::SELECT` at `PIVOT_GLYPH_PX` (22), nothing behind them, and must fit inside
`PIVOT_PICK_PX`.

**Dragging the marker:**

- **Nine-point snap** to the box's corners, side midpoints and centre within `PIVOT_SNAP_PX` (6 screen points). The comparison runs in local space, so the threshold is divided by the zoom and the node's basis column norms (`|col 0|` for x, `|col 1|` for y). A badge names the position.
- **Centre clears.** Snapping to the centre sets the pivot to `None`: the field leaves the file (§5.11), the read-only marker disappears, `flip` reverts to the union centreline. The rule is `geometry::pivot_placed_at`, shared with the numeric fields; a degenerate axis is centred by itself.
- **No overrides.** `Drag::Pivot { id }` has no `RenderOverrides` preview (the marker is chrome drawn from the pointer) and commits one `SetPivot` on release. `EditorSession::preview_pivot` resolves the pivot against a live gesture's box.
- **Priority.** While the toggle is on, the marker outranks resize handles in `begin_select_drag`.

**The origin fields** (`canvas::origin_fields`) replace the size badge in its slot while the toggle
is armed over one layer, sharing `badge_placement`; a shape being drawn takes the slot ahead of both.

- **Percentages of the layer's own box**, for every kind (0/50/100 = left, centre, right). `geometry::pivot_for` picks the stored form: a fraction for an authored box, an absolute point for an emergent one. A dragged marker lands on a whole percentage after the screen-space snap. Unclamped: a pivot may sit outside its box. Both at 50 clears the origin.
- **Degenerate axes** (no extent) are dimmed with a reason, per axis. A lone line is offered the fields.
- **Scrubbing** goes through `EditorSession`'s `pivot_preview` field, because the overrides cannot carry a pivot.
- **Widgets.** The first live egui widgets on the canvas: `ui::badge_field` is the badge pill with `value_field`'s plumbing (`Scrub`, `expr::eval`). They stay upright.
- **Shielding.** Their own `egui::Area` shields the canvas beneath (§15 D252, pinned by the `overlay_claim` test module in `canvas.rs`); `wheel_input` names their layer through `origin_fields_layer` and takes the wheel back.

**What reads the pivot.** `Handles::pivot` is the node's origin for one selection, the union centre
for several. Readers: `Drag::Rotate`; the inspector's skew field (rebuilds about it); the rotation
field (composes about it); quarter turns; `build::flip`, only when a pivot has been placed (D55).
Canvas skew and Alt-resize deliberately do not, each having its own fixed point (D55, D60).

### `Ctrl`+arrow size step

`Ctrl`+arrows step the selection's box by the nudge step (`input::NudgeStep`): 1, or 10 with
`Shift`. `→` wider, `←` narrower, `↓` taller, `↑` shorter (`shortcuts.md` §11).

- `OndinApp::size_step` uses `tools::resize_box_to` for one layer (grows along its own axes) and `tools::resize_selection` over the upright world union for several.
- The top-left corner is held, so the step does not `reveal_selection`. One arrow table serves nudge and resize; `normal_mode` picks the verb from the modifier.
- 1pt floor per axis. With every member proportion-locked, both axes move and the arrow names the driver (`tools::paired_size`). Guides are inert.
- Bursts coalesce under the undo verb `resize-layers`, separate from nudges.

## Traps

- **Same press, two consumers.** The cancel and the context menu share one press: `gesture_cancelled` must be read at the top of `canvas_context_menu`, before any branch selects.
- **Dropdown keyboard focus.** A dropdown opened by clicking its head leaves `focused() == None`, so `egui_wants_keyboard_input` does not cover it.
- **Popover flags outlive popovers.** Gate on `popover_heard`, not on the flags, or Escape is swallowed behind a popover nobody is drawing.
- **Three popover lists.** They differ deliberately, by the image card. A shared list would have to carry that exception.
- **Inert `TopMenu` row.** `TopMenu` needs its own Escape arm; without one, Escape closes the menu *and* drops the tool.
- **Two copies of one resize rule.** `resized_box` and `resize_factors` have diverged twice. Change both, and test both: the box can be identical while the path differs.
- **Unguarded `draw_drop_indicator`.** It does not check `shown_visible`, so a picture inside a hidden group is still ringed during a file drag.
- **Accelerators by hand.** Nothing makes `ViewSwitch::accel`/`Item::spec` accelerators agree with `input::resolve`.
- **Clear without `clear_preview`.** `choose_tool` must call `clear_gesture_preview`, not `clear_preview`, or a live text session's content is lost.
- **Predicate negatives.** `drags_a_box` decides whether arming a tool deletes the selection. When a predicate gains a second consumer, re-check its negative members.
- **Middle-button pan over the origin fields.** It is still swallowed by their `Area` (unverified).
- **Menu placement order.** `menu_place` must clamp after rounding; reversing the order cuts off a menu's last column.
- **The eyedropper's repaint guard is unpinned.** `request_sample`'s GPU arm skipping a key that matches the last sample is all that stops a still, armed pointer repainting every frame; headless the CPU arm answers `false` whatever the key, so `the_hover_sample_is_keyed_to_the_pointer` does not catch its removal.
- **The frozen frame's three guards.** The capture frame draws no loupe (`the_captured_frame_has_no_loupe`), a capture is taken only on its own `FrozenShot` tag (`off_the_canvas_a_pick_reads_the_frozen_window`), and `dropper_frame` stops asking for frames 30 passes after an unanswered ask, or a surface egui cannot capture repaints forever (unpinned).
